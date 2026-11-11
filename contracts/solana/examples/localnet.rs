use std::fs;
use std::path::PathBuf;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::{instruction::Instruction, program_pack::Pack};
use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token;
use apodect_solana::state::{Project, Task, Workspace};
use apodect_solana::{accounts, instruction};
use serde_json::{json, Value};
use solana_keypair::Keypair;
use solana_signer::Signer;

#[path = "localnet/bridge.rs"]
mod bridge;

#[path = "localnet/client.rs"]
mod client;

use client::{address, ensure, Localnet, Result};

const SUPPLY: u64 = 1_000_000_000_000;
const FUNDING: u64 = 100_000_000_000;

fn ix(data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
    Instruction {
        program_id: apodect_solana::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

impl Localnet {
    fn bootstrap(&self) -> Result<()> {
        if self.dir.join("manifest.json").exists() {
            return self.check();
        }
        ensure(
            !self.dir.join("bootstrap-started.json").exists(),
            "interrupted Solana bootstrap detected; use local-reset before trying again",
        )?;
        self.verify_program()?;
        self.write_json("bootstrap-started.json", &json!({"status":"bootstrapping"}))?;
        let owner = self.key("owner", true)?;
        let authority = self.key("authority", true)?;
        let agent = self.key("agent", true)?;
        for key in [&owner, &authority, &agent] {
            let signature = self.rpc(
                "requestAirdrop",
                json!([key.pubkey().to_string(), 20_000_000_000u64]),
            )?;
            self.confirm(signature.as_str().ok_or("missing airdrop signature")?)?;
        }

        let mint = Keypair::new();
        let rent = self
            .rpc(
                "getMinimumBalanceForRentExemption",
                json!([spl_token::state::Mint::LEN]),
            )?
            .as_u64()
            .ok_or("missing mint rent")?;
        self.send(
            &owner,
            &[
                solana_system_interface::instruction::create_account(
                    &owner.pubkey(),
                    &mint.pubkey(),
                    rent,
                    spl_token::state::Mint::LEN as u64,
                    &spl_token::ID,
                ),
                spl_token::instruction::initialize_mint2(
                    &spl_token::ID,
                    &mint.pubkey(),
                    &owner.pubkey(),
                    Some(&owner.pubkey()),
                    6,
                )?,
            ],
            &[&mint],
        )?;
        let owner_token = self.create_token(&owner, mint.pubkey(), owner.pubkey())?;
        let agent_token = self.create_token(&owner, mint.pubkey(), agent.pubkey())?;
        self.send(
            &owner,
            &[spl_token::instruction::mint_to(
                &spl_token::ID,
                &mint.pubkey(),
                &owner_token,
                &owner.pubkey(),
                &[],
                SUPPLY,
            )?],
            &[],
        )?;

        let workspace = Pubkey::find_program_address(
            &[b"workspace", owner.pubkey().as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        self.send(
            &owner,
            &[ix(
                instruction::InitializeWorkspace { nonce: 0 },
                accounts::InitializeWorkspace {
                    workspace,
                    owner: owner.pubkey(),
                    system_program: solana_system_interface::program::ID,
                },
            )],
            &[],
        )?;
        let project = Pubkey::find_program_address(
            &[b"project", workspace.as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        let vault =
            Pubkey::find_program_address(&[b"vault", project.as_ref()], &apodect_solana::ID).0;
        self.send(
            &owner,
            &[ix(
                instruction::CreateProject {
                    authority: authority.pubkey(),
                },
                accounts::CreateProject {
                    workspace,
                    project,
                    mint: mint.pubkey(),
                    vault,
                    owner: owner.pubkey(),
                    token_program: spl_token::ID,
                    system_program: solana_system_interface::program::ID,
                },
            )],
            &[],
        )?;
        self.send(
            &owner,
            &[ix(
                instruction::FundProject { amount: FUNDING },
                accounts::FundProject {
                    workspace,
                    project,
                    mint: mint.pubkey(),
                    vault,
                    owner_token,
                    owner: owner.pubkey(),
                    token_program: spl_token::ID,
                    invoice: None,
                },
            )],
            &[],
        )?;

        self.write_json("manifest.json", &json!({
            "schema_version":1,"chain":"solana","rpc_url":self.url,
            "contracts":{
                "program":apodect_solana::ID.to_string(),
                "workspace":workspace.to_string(),
                "project":project.to_string(),
                "vault":vault.to_string()
            },
            "accounts":{
                "owner":{"address":owner.pubkey().to_string(),"key_file":self.dir.join("keys/owner.json")},
                "authority":{"address":authority.pubkey().to_string(),"key_file":self.dir.join("keys/authority.json")},
                "agent":{"address":agent.pubkey().to_string(),"key_file":self.dir.join("keys/agent.json")}
            },
            "asset":{
                "address":mint.pubkey().to_string(),
                "decimals":6,
                "owner_token":owner_token.to_string(),
                "agent_token":agent_token.to_string()
            },
            "initial_project_funding":FUNDING,"token_program":spl_token::ID.to_string()
        }))?;
        self.check()?;
        fs::remove_file(self.dir.join("bootstrap-started.json"))?;
        eprintln!("Solana workspace, Project and funded token vault bootstrapped");
        Ok(())
    }

    /// Create the finite local provider wallet; this is simulator setup, not custody logic.
    fn bootstrap_liquidity_fixture(&self) -> Result<Value> {
        self.bootstrap()?;
        let mut manifest = self.manifest()?;
        if manifest["asset"]["liquidity_token"].is_null() {
            ensure(
                !self.dir.join("bridge-bootstrap-started.json").exists(),
                "interrupted LP bootstrap requires explicit local reset",
            )?;
            self.write_json(
                "bridge-bootstrap-started.json",
                &json!({"status":"bootstrapping liquidity"}),
            )?;
            let owner = self.key("owner", false)?;
            let lp = self.key("liquidity", true)?;
            let signature = self.rpc(
                "requestAirdrop",
                json!([lp.pubkey().to_string(), 20_000_000_000u64]),
            )?;
            self.confirm(signature.as_str().ok_or("missing LP airdrop")?)?;
            let mint = address(&manifest["asset"]["address"])?;
            let token = apodect_solana::bridge::associated_account(lp.pubkey(), mint);
            self.send(
                &owner,
                &[apodect_solana::bridge::create_associated_account(
                    owner.pubkey(),
                    lp.pubkey(),
                    mint,
                )],
                &[],
            )?;
            let tx = self.send(
                &owner,
                &[spl_token::instruction::transfer_checked(
                    &spl_token::ID,
                    &address(&manifest["asset"]["owner_token"])?,
                    &mint,
                    &token,
                    &owner.pubkey(),
                    &[],
                    500_000_000_000,
                    6,
                )?],
                &[],
            )?;
            self.finalized_proof(&tx)?;
            manifest["asset"]["liquidity_token"] = json!(token.to_string());
            manifest["accounts"]["liquidity"] = json!({
                "address": lp.pubkey().to_string(),
                "key_file": self.dir.join("keys/liquidity.json")
            });
            self.write_json("manifest.json", &manifest)?;
            fs::remove_file(self.dir.join("bridge-bootstrap-started.json"))?;
        }
        Ok(manifest)
    }

    fn manifest(&self) -> Result<Value> {
        let value: Value = serde_json::from_slice(&fs::read(self.dir.join("manifest.json"))?)?;
        ensure(
            value["schema_version"] == 1
                && value["chain"] == "solana"
                && value["rpc_url"] == self.url,
            "Solana manifest identity mismatch",
        )?;
        Ok(value)
    }

    fn check(&self) -> Result<()> {
        let manifest = self.manifest()?;
        self.verify_program()?;
        ensure(
            address(&manifest["contracts"]["program"])? == apodect_solana::ID,
            "manifest program ID mismatch",
        )?;
        let owner = self.key("owner", false)?;
        let authority = self.key("authority", false)?;
        let agent = self.key("agent", false)?;
        ensure(
            owner.pubkey() != authority.pubkey()
                && owner.pubkey() != agent.pubkey()
                && authority.pubkey() != agent.pubkey(),
            "local roles must be distinct",
        )?;
        for (role, key) in [
            ("owner", &owner),
            ("authority", &authority),
            ("agent", &agent),
        ] {
            ensure(
                address(&manifest["accounts"][role]["address"])? == key.pubkey(),
                "manifest role key mismatch",
            )?;
            ensure(
                manifest["accounts"][role]["key_file"]
                    == self
                        .dir
                        .join(format!("keys/{role}.json"))
                        .to_string_lossy()
                        .as_ref(),
                "manifest key path mismatch",
            )?;
            ensure(
                self.rpc(
                    "getBalance",
                    json!([key.pubkey().to_string(), {"commitment":"confirmed"}]),
                )?["value"]
                    .as_u64()
                    .ok_or("missing role balance")?
                    > 0,
                "local role has no SOL",
            )?;
        }
        let workspace_key = address(&manifest["contracts"]["workspace"])?;
        let workspace: Workspace = self.read_confirmed(workspace_key)?;
        ensure(
            workspace.owner == owner.pubkey(),
            "Workspace owner mismatch",
        )?;
        let project_key = address(&manifest["contracts"]["project"])?;
        let project: Project = self.read_confirmed(project_key)?;
        let (expected_project, bump) = Pubkey::find_program_address(
            &[
                b"project",
                workspace_key.as_ref(),
                &project.index.to_le_bytes(),
            ],
            &apodect_solana::ID,
        );
        ensure(
            project_key == expected_project
                && project.bump == bump
                && project.workspace == workspace_key
                && workspace.project_count > project.index,
            "Project identity mismatch",
        )?;
        ensure(
            project.authority == authority.pubkey(),
            "Project authority mismatch",
        )?;
        let mint = address(&manifest["asset"]["address"])?;
        let mint_account = self.account_confirmed(mint)?;
        ensure(
            address(&mint_account["owner"])? == spl_token::ID,
            "mint token program mismatch",
        )?;
        let mint_state = spl_token::state::Mint::unpack(&self.bytes(&mint_account)?)?;
        ensure(
            project.mint == mint && mint_state.decimals == 6 && manifest["asset"]["decimals"] == 6,
            "Project mint/decimal mismatch",
        )?;
        let vault_key = address(&manifest["contracts"]["vault"])?;
        ensure(
            vault_key
                == Pubkey::find_program_address(
                    &[b"vault", project_key.as_ref()],
                    &apodect_solana::ID,
                )
                .0,
            "vault PDA mismatch",
        )?;
        let vault = self.token_confirmed(vault_key)?;
        ensure(
            vault.mint == mint && vault.owner == project_key,
            "vault custody mismatch",
        )?;
        for (field, holder) in [
            ("owner_token", owner.pubkey()),
            ("agent_token", agent.pubkey()),
        ] {
            let token = self.token_confirmed(address(&manifest["asset"][field])?)?;
            ensure(
                token.mint == mint && token.owner == holder,
                "role token account mismatch",
            )?;
        }
        let program_accounts = self.rpc(
            "getProgramAccounts",
            json!([apodect_solana::ID.to_string(), {"encoding":"base64","commitment":"confirmed"}]),
        )?;
        let mut controlled = u128::from(project.free) + u128::from(project.held);
        for item in program_accounts
            .as_array()
            .ok_or("missing program accounts")?
        {
            let bytes = self.bytes(&item["account"])?;
            if bytes.starts_with(Task::DISCRIMINATOR) {
                let task = Task::try_deserialize(&mut bytes.as_slice())?;
                if task.project == project_key {
                    task.external_outstanding()?;
                    controlled += u128::from(task.available);
                }
            }
        }
        ensure(
            u128::from(vault.amount) >= controlled,
            "vault cannot cover recorded free and controlled task principal",
        )?;
        eprintln!("Solana RPC program, roles, mint, account identities and custody checked");
        Ok(())
    }
}

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::var("APODECT_LOCALNET_DIR")?);
    ensure(dir.is_absolute(), "APODECT_LOCALNET_DIR must be absolute")?;
    let localnet = Localnet {
        dir,
        url: std::env::var("APODECT_RPC_URL")?,
    };
    let port = localnet
        .url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|port| port.parse::<u16>().ok());
    ensure(
        port.is_some_and(|port| (1024..=65534).contains(&port)),
        "only a numeric loopback RPC port is allowed",
    )?;
    match std::env::args().nth(1).as_deref() {
        Some("prepare") => localnet.prepare(),
        Some("bootstrap") => localnet.bootstrap(),
        Some("check") => localnet.check(),
        Some("bridge") => bridge::run(&localnet),
        _ => Err("usage: localnet {prepare|bootstrap|check|bridge}".into()),
    }
}
