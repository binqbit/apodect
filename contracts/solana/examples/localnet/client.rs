use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::{instruction::Instruction, program_pack::Pack};
use anchor_lang::AccountDeserialize;
use anchor_spl::token::spl_token;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use solana_keypair::{read_keypair_file, Keypair};
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;

pub(super) type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub(super) struct Localnet {
    pub(super) dir: PathBuf,
    pub(super) url: String,
}

pub(super) fn ensure(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub(super) fn address(value: &Value) -> Result<Pubkey> {
    Ok(value.as_str().ok_or("missing manifest address")?.parse()?)
}

fn unique_message(payer: Pubkey, instructions: &[Instruction]) -> Message {
    // Separate explicit calls can otherwise have identical signatures while they share
    // a recent blockhash. A memo distinguishes intentions without changing contract data.
    // This is called once per send; retries must retain the original signed transaction.
    let mut instructions = instructions.to_vec();
    instructions.push(Instruction {
        program_id: "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"
            .parse()
            .expect("standard SPL Memo program ID"),
        accounts: vec![],
        data: Keypair::new().pubkey().to_string().into_bytes(),
    });
    Message::new(&instructions, Some(&payer))
}

impl Localnet {
    pub(super) fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let mut child = Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--fail-with-body",
                "--noproxy",
                "*",
                "--connect-timeout",
                "2",
                "--max-time",
                "20",
                "-H",
                "Content-Type: application/json",
                "--data-binary",
                "@-",
                &self.url,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("missing curl stdin")?
            .write_all(&serde_json::to_vec(
                &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            )?)?;
        let output = child.wait_with_output()?;
        ensure(
            output.status.success(),
            &format!(
                "RPC transport failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )?;
        let response: Value = serde_json::from_slice(&output.stdout)?;
        ensure(
            response.get("error").is_none(),
            &format!("{method}: {}", response["error"]),
        )?;
        response
            .get("result")
            .cloned()
            .ok_or_else(|| "missing RPC result".into())
    }

    pub(super) fn confirm(&self, signature: &str) -> Result<()> {
        let start = Instant::now();
        loop {
            let result = self.rpc(
                "getSignatureStatuses",
                json!([[signature], {"searchTransactionHistory":true}]),
            )?;
            let status = &result["value"][0];
            ensure(
                status["err"].is_null(),
                &format!("transaction {signature} failed: {}", status["err"]),
            )?;
            if matches!(
                status["confirmationStatus"].as_str(),
                Some("confirmed" | "finalized")
            ) {
                return Ok(());
            }
            ensure(
                start.elapsed() < Duration::from_secs(60),
                &format!("transaction {signature} remains unconfirmed; inspect it before retrying"),
            )?;
            sleep(Duration::from_millis(200));
        }
    }

    pub(super) fn send(
        &self,
        payer: &Keypair,
        instructions: &[Instruction],
        extra: &[&Keypair],
    ) -> Result<String> {
        let blockhash = self.rpc("getLatestBlockhash", json!([{"commitment":"confirmed"}]))?;
        let blockhash = blockhash["value"]["blockhash"]
            .as_str()
            .ok_or("missing blockhash")?
            .parse()?;
        let mut signers = vec![payer];
        for signer in extra {
            if signer.pubkey() != payer.pubkey() {
                signers.push(*signer);
            }
        }
        let tx = Transaction::new(
            &signers,
            unique_message(payer.pubkey(), instructions),
            blockhash,
        );
        let signature = tx.signatures[0].to_string();
        let serialized = wincode::serialize(&tx)?;
        ensure(
            serialized.len() <= 1232,
            "transaction exceeds Solana's legacy packet size limit",
        )?;
        let result = self
            .rpc(
                "sendTransaction",
                json!([
                    STANDARD.encode(serialized),
                    {
                        "encoding":"base64",
                        "preflightCommitment":"confirmed",
                        "skipPreflight":false,
                        "maxRetries":5
                    }
                ]),
            )
            .map_err(|error| {
                format!("transaction {signature} submission is unconfirmed: {error}; inspect it before retrying")
            })?;
        ensure(
            result.as_str() == Some(signature.as_str()),
            &format!("RPC returned a different transaction signature; inspect {signature} before retrying"),
        )?;
        self.confirm(&signature)?;
        Ok(signature)
    }

    pub(super) fn account_confirmed(&self, key: Pubkey) -> Result<Value> {
        let result = self.rpc(
            "getAccountInfo",
            json!([key.to_string(), {"encoding":"base64","commitment":"confirmed"}]),
        )?;
        let value = result["value"].clone();
        ensure(!value.is_null(), &format!("missing account {key}"))?;
        Ok(value)
    }

    pub(super) fn bytes(&self, account: &Value) -> Result<Vec<u8>> {
        Ok(STANDARD.decode(account["data"][0].as_str().ok_or("missing account data")?)?)
    }

    pub(super) fn read_confirmed<T: AccountDeserialize>(&self, key: Pubkey) -> Result<T> {
        let account = self.account_confirmed(key)?;
        ensure(
            address(&account["owner"])? == apodect_solana::ID,
            "state account owner mismatch",
        )?;
        Ok(T::try_deserialize(&mut self.bytes(&account)?.as_slice())?)
    }

    pub(super) fn token_confirmed(&self, key: Pubkey) -> Result<spl_token::state::Account> {
        let account = self.account_confirmed(key)?;
        ensure(
            address(&account["owner"])? == spl_token::ID,
            "token program mismatch",
        )?;
        Ok(spl_token::state::Account::unpack(&self.bytes(&account)?)?)
    }

    pub(super) fn finalized_proof(&self, signature: &str) -> Result<Value> {
        let start = Instant::now();
        loop {
            let tx = self.rpc("getTransaction", json!([
                signature,
                {"commitment":"finalized", "encoding":"json", "maxSupportedTransactionVersion":0}
            ]))?;
            if !tx.is_null() {
                ensure(tx["meta"]["err"].is_null(), "finalized transaction failed")?;
                let slot = tx["slot"].as_u64().ok_or("missing finalized slot")?;
                let block = self.rpc(
                    "getBlock",
                    json!([
                        slot,
                        {"commitment":"finalized", "transactionDetails":"none", "rewards":false,
                            "maxSupportedTransactionVersion":0}
                    ]),
                )?;
                return Ok(json!({
                    "tx_hash": signature,
                    "block_height": slot.to_string(),
                    "block_hash": block["blockhash"],
                    "timestamp": tx["blockTime"].as_u64().unwrap_or(0).to_string(),
                    "finalized": true
                }));
            }
            ensure(
                start.elapsed() < Duration::from_secs(90),
                "transaction is not finalized; inspect its signature before retrying",
            )?;
            sleep(Duration::from_millis(200));
        }
    }

    pub(super) fn read_finalized_optional<T: AccountDeserialize>(
        &self,
        key: Pubkey,
    ) -> Result<Option<T>> {
        let response = self.rpc(
            "getAccountInfo",
            json!([key.to_string(),{"encoding":"base64","commitment":"finalized"}]),
        )?;
        let value = &response["value"];
        if value.is_null() {
            return Ok(None);
        }
        let owner = address(&value["owner"])?;
        let data = self.bytes(value)?;
        if owner == solana_system_interface::program::ID && data.is_empty() {
            // Prefunding a PDA with lamports does not create a typed receipt.
            return Ok(None);
        }
        ensure(owner == apodect_solana::ID, "foreign account owner")?;
        Ok(Some(T::try_deserialize(&mut data.as_slice())?))
    }

    pub(super) fn finalized_token(&self, key: Pubkey) -> Result<spl_token::state::Account> {
        let response = self.rpc(
            "getAccountInfo",
            json!([
                key.to_string(), {"encoding":"base64", "commitment":"finalized"}
            ]),
        )?;
        let value = &response["value"];
        ensure(
            !value.is_null() && address(&value["owner"])? == spl_token::ID,
            "missing legacy token account",
        )?;
        Ok(spl_token::state::Account::unpack(&self.bytes(value)?)?)
    }

    pub(super) fn read_finalized_batch<T: AccountDeserialize>(
        &self,
        keys: &[Pubkey],
    ) -> Result<Vec<T>> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let addresses: Vec<_> = keys.iter().map(|key| key.to_string()).collect();
        let response = self.rpc(
            "getMultipleAccounts",
            json!([
                addresses, {"encoding":"base64", "commitment":"finalized"}
            ]),
        )?;
        let accounts = response["value"]
            .as_array()
            .ok_or("missing batch account values")?;
        ensure(
            accounts.len() == keys.len(),
            "incomplete batch account response",
        )?;
        accounts
            .iter()
            .map(|account| {
                ensure(
                    !account.is_null() && address(&account["owner"])? == apodect_solana::ID,
                    "missing program-owned scan account",
                )?;
                Ok(T::try_deserialize(&mut self.bytes(account)?.as_slice())?)
            })
            .collect()
    }

    pub(super) fn write_json(&self, name: &str, value: &Value) -> Result<()> {
        let temp = self.dir.join(format!("{name}.tmp"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(temp, self.dir.join(name))?;
        Ok(())
    }

    pub(super) fn key(&self, name: &str, create: bool) -> Result<Keypair> {
        let path = self.dir.join("keys").join(format!("{name}.json"));
        if !path.exists() && create {
            let key = Keypair::new();
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(&serde_json::to_vec(&key.to_bytes().as_slice())?)?;
            file.sync_all()?;
        }
        ensure(
            fs::metadata(&path)?.permissions().mode() & 0o777 == 0o600,
            "local key files must have mode 0600",
        )?;
        read_keypair_file(&path)
    }

    pub(super) fn prepare(&self) -> Result<()> {
        ensure(
            self.dir
                .join("rootlesskit/api.sock")
                .as_os_str()
                .as_encoded_bytes()
                .len()
                < 100,
            "Solana namespace socket path is too long; use a shorter checkout/data path",
        )?;
        fs::create_dir_all(self.dir.join("keys"))?;
        fs::set_permissions(self.dir.join("keys"), fs::Permissions::from_mode(0o700))?;
        self.key("validator", true)?;
        let config = format!(
            "json_rpc_url: {}\nwebsocket_url: ''\nkeypair_path: {}\ncommitment: confirmed\n",
            self.url,
            serde_json::to_string(&self.dir.join("keys/validator.json").to_string_lossy())?
        );
        fs::write(self.dir.join("cli.yml"), config)?;
        println!("Solana local artifacts and configuration prepared");
        Ok(())
    }

    pub(super) fn verify_program(&self) -> Result<()> {
        let loader: Pubkey = "BPFLoaderUpgradeab1e11111111111111111111111".parse()?;
        let account = self.rpc(
            "getAccountInfo",
            json!([
                apodect_solana::ID.to_string(),
                {"encoding":"jsonParsed","commitment":"confirmed"}
            ]),
        )?["value"]
            .clone();
        ensure(
            account["executable"] == true && address(&account["owner"])? == loader,
            "Apodect program account loader/executable mismatch",
        )?;
        let expected_data = Pubkey::find_program_address(&[apodect_solana::ID.as_ref()], &loader).0;
        ensure(
            account["data"]["parsed"]["type"] == "program"
                && address(&account["data"]["parsed"]["info"]["programData"])? == expected_data,
            "program-data identity mismatch",
        )?;
        let program_data = self.rpc(
            "getAccountInfo",
            json!([expected_data.to_string(), {"encoding":"jsonParsed","commitment":"confirmed"}]),
        )?["value"]
            .clone();
        let parsed = &program_data["data"]["parsed"];
        ensure(
            address(&program_data["owner"])? == loader
                && parsed["type"] == "programData"
                && parsed["info"]["authority"] == solana_system_interface::program::ID.to_string(),
            "local genesis program-data loader/authority mismatch",
        )?;
        let artifact = fs::read(std::env::var("APODECT_SOLANA_PROGRAM")?)?;
        ensure(
            STANDARD.decode(
                parsed["info"]["data"][0]
                    .as_str()
                    .ok_or("missing program bytecode")?,
            )? == artifact,
            "validator program differs from the built SBF artifact; reset the local ledger after code changes",
        )
    }

    pub(super) fn create_token(
        &self,
        owner: &Keypair,
        mint: Pubkey,
        holder: Pubkey,
    ) -> Result<Pubkey> {
        let token = Keypair::new();
        let rent = self
            .rpc(
                "getMinimumBalanceForRentExemption",
                json!([spl_token::state::Account::LEN]),
            )?
            .as_u64()
            .ok_or("missing rent")?;
        self.send(
            owner,
            &[
                solana_system_interface::instruction::create_account(
                    &owner.pubkey(),
                    &token.pubkey(),
                    rent,
                    spl_token::state::Account::LEN as u64,
                    &spl_token::ID,
                ),
                spl_token::instruction::initialize_account3(
                    &spl_token::ID,
                    &token.pubkey(),
                    &mint,
                    &holder,
                )?,
            ],
            &[&token],
        )?;
        Ok(token.pubkey())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apodect_solana::{accounts, instruction};
    use litesvm::LiteSVM;

    #[test]
    fn equal_explicit_transfers_in_one_block_execute_independently() {
        let mut svm = LiteSVM::new();
        let payer = Keypair::new();
        let recipient = Keypair::new().pubkey();
        svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
        svm.airdrop(&recipient, 1_000_000).unwrap();
        let instructions = [solana_system_interface::instruction::transfer(
            &payer.pubkey(),
            &recipient,
            100,
        )];
        let blockhash = svm.latest_blockhash();

        // Before the fix, equal explicit calls had the same signature and one payment.
        let plain = || {
            Transaction::new(
                &[&payer],
                Message::new(&instructions, Some(&payer.pubkey())),
                blockhash,
            )
        };
        assert_eq!(plain().signatures, plain().signatures);

        let transaction = || {
            Transaction::new(
                &[&payer],
                unique_message(payer.pubkey(), &instructions),
                blockhash,
            )
        };
        let first = transaction();
        let second = transaction();
        assert_ne!(first.signatures, second.signatures);
        svm.send_transaction(first.clone()).unwrap();
        svm.send_transaction(second).unwrap();
        assert_eq!(svm.get_balance(&recipient), Some(1_000_200));

        // Replaying existing signed bytes remains one effect, regardless of the memo.
        assert!(svm.send_transaction(first).is_err());
        assert_eq!(svm.get_balance(&recipient), Some(1_000_200));
    }

    #[test]
    fn bounded_command_with_all_optional_accounts_and_memo_fits_packet() {
        let payer = Keypair::new();
        let command = super::super::ix(
            instruction::ExecuteCommand {
                bytes: [0; 384],
                signature: [0; 64],
                recovery_id: 0,
            },
            accounts::ExecuteCommand {
                project: Pubkey::new_unique(),
                workspace: Pubkey::new_unique(),
                mint: Pubkey::new_unique(),
                vault: Pubkey::new_unique(),
                receipt: Pubkey::new_unique(),
                payer: payer.pubkey(),
                system_program: solana_system_interface::program::ID,
                token_program: spl_token::ID,
                associated_token_program: apodect_solana::bridge::ASSOCIATED_TOKEN_PROGRAM,
                task: Some(Pubkey::new_unique()),
                request: Some(Pubkey::new_unique()),
                plan: Some(Pubkey::new_unique()),
                route: Some(Pubkey::new_unique()),
                escrow: Some(Pubkey::new_unique()),
                recipient: Some(Pubkey::new_unique()),
                target: Some(Pubkey::new_unique()),
            },
        );
        let tx = Transaction::new(
            &[&payer],
            unique_message(payer.pubkey(), &[command]),
            Default::default(),
        );
        let size = wincode::serialize(&tx).unwrap().len();
        assert!(size <= 1232, "command and memo use {size} bytes");
    }
}
