//! Native Solana reads, gas-payer transactions and immutable receipt evidence.
//!
//! Preparing a transaction has no network effect. The caller must persist its
//! signed bytes and signature before broadcasting, and reconcile that signature
//! after an interrupted submission instead of preparing another payment.

use std::path::Path;

use anchor_lang::{
    AccountDeserialize, Discriminator, InstructionData, ToAccountMetas,
    prelude::Pubkey,
    solana_program::{instruction::Instruction, program_pack::Pack},
    system_program,
};
use anchor_spl::token::spl_token;
use anyhow::{Context, Result, bail, ensure};
use apodect_solana::{
    accounts,
    bridge::{self, BridgeRoute, Checkpoint, Command, CommandReceipt, FundingReceipt, action},
    instruction,
    state::{BudgetRequest, Project, Task, Workspace},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solana_keypair::{Keypair, read_keypair_file};
use solana_message::Message;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;

use crate::rpc::Rpc;

const ZERO: [u8; 32] = [0; 32];

pub struct Binding {
    pub genesis_hash: String,
    pub project: Pubkey,
    pub mint: Pubkey,
    pub domain: [u8; 32],
}

pub struct Client {
    rpc: Rpc,
    binding: Binding,
    payer: Option<Keypair>,
}

pub struct Finalized<T> {
    pub slot: u64,
    pub value: T,
}

pub struct FinalizedBlock {
    pub slot: u64,
    pub hash: String,
}

pub struct TaskRecord {
    pub address: Pubkey,
    pub task: Task,
    pub request: Option<RequestRecord>,
}

pub struct RequestRecord {
    pub address: Pubkey,
    pub request: BudgetRequest,
}

pub struct TaskPage {
    pub project: Finalized<Project>,
    pub tasks: Vec<TaskRecord>,
    pub next_task: u64,
    pub slot: u64,
}

pub struct RequestPage {
    pub requests: Vec<RequestRecord>,
    pub next_request: u64,
    pub total: u64,
    pub slot: u64,
}

pub struct FundingRecord {
    pub id: [u8; 32],
    pub receipt: Pubkey,
    pub payload: Vec<u8>,
}

pub struct RouteState {
    pub record: BridgeRoute,
    pub escrow: Pubkey,
    pub escrow_balance: u64,
    pub slot: u64,
}

#[derive(Default)]
pub struct CheckpointSelection {
    pub task: Option<Pubkey>,
    pub request: Option<Pubkey>,
    pub plan: Option<[u8; 32]>,
    pub route: Option<[u8; 32]>,
    pub operation: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedTransaction {
    pub signature: String,
    pub bytes_base64: String,
    pub last_valid_block_height: u64,
}

#[derive(Debug, PartialEq)]
pub enum TransactionOutcome {
    Unknown,
    Pending,
    Finalized { slot: u64 },
    Failed { slot: u64, error: Value },
}

/// The values are ready for the coordinator's Read MPC request. The immutable
/// account payload is verified independently of any delivery-provider status.
pub struct ReceiptEvidence {
    pub payload: Vec<u8>,
    pub receipt: Pubkey,
    pub slot: u64,
    pub request: Value,
    pub values: Value,
}

#[derive(Deserialize)]
struct RpcContext {
    slot: u64,
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    context: RpcContext,
    value: T,
}

#[derive(Deserialize)]
struct RpcAccount {
    owner: String,
    executable: bool,
    data: (String, String),
}

impl RpcAccount {
    fn bytes(&self) -> Result<Vec<u8>> {
        ensure!(
            self.data.1 == "base64",
            "Unexpected Solana account encoding"
        );
        STANDARD
            .decode(&self.data.0)
            .context("Invalid Solana account data")
    }

    fn decode<T: AccountDeserialize>(&self) -> Result<Option<T>> {
        let owner: Pubkey = self.owner.parse().context("Invalid Solana account owner")?;
        let data = self.bytes()?;
        if owner == system_program::ID && data.is_empty() && !self.executable {
            // Sending lamports to a PDA does not initialize its program state.
            return Ok(None);
        }
        ensure!(
            owner == apodect_solana::ID && !self.executable,
            "Foreign Solana state account"
        );
        T::try_deserialize(&mut data.as_slice())
            .map(Some)
            .context("Invalid Solana state account")
    }
}

impl Client {
    pub fn new(rpc: Rpc, binding: Binding, payer_file: Option<&Path>) -> Result<Self> {
        ensure!(
            binding.domain != ZERO,
            "Solana coordinator domain is required"
        );
        let payer = payer_file
            .map(|path| {
                read_keypair_file(path)
                    .map_err(|_| anyhow::anyhow!("Cannot read Solana gas-payer keypair"))
            })
            .transpose()?;
        Ok(Self {
            rpc,
            binding,
            payer,
        })
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    pub fn payer(&self) -> Option<Pubkey> {
        self.payer.as_ref().map(Signer::pubkey)
    }

    pub async fn validate_deployment(&self) -> Result<()> {
        let genesis: String = self.rpc.call("getGenesisHash", &json!([])).await?;
        ensure!(
            genesis == self.binding.genesis_hash,
            "Solana genesis does not match deployment"
        );
        self.read_project().await?;

        let mint = self.token_account_data(self.binding.mint, 0).await?;
        let mint =
            spl_token::state::Mint::unpack(&mint.value).context("Invalid Solana token mint")?;
        ensure!(mint.decimals == 6, "Project token must have six decimals");
        self.read_token(self.vault_address(), 0).await?;
        Ok(())
    }

    pub async fn finalized_block(&self) -> Result<FinalizedBlock> {
        let slot: u64 = self
            .rpc
            .call("getSlot", &json!([{"commitment":"finalized"}]))
            .await?;
        #[derive(Deserialize)]
        struct Block {
            blockhash: String,
        }
        let block: Option<Block> = self
            .rpc
            .call(
                "getBlock",
                &json!([
                    slot, {"commitment":"finalized", "transactionDetails":"none", "rewards":false,
                        "maxSupportedTransactionVersion":0}
                ]),
            )
            .await?;
        Ok(FinalizedBlock {
            slot,
            hash: block
                .context("Finalized Solana block is unavailable")?
                .blockhash,
        })
    }

    pub async fn read_project(&self) -> Result<Finalized<Project>> {
        let project = self
            .read_required::<Project>(self.binding.project, 0)
            .await?;
        ensure!(
            project.value.domain == self.binding.domain,
            "Wrong Solana Project domain"
        );
        ensure!(
            project.value.mint == self.binding.mint,
            "Wrong Solana Project token"
        );
        let (expected, bump) = Pubkey::find_program_address(
            &[
                b"project",
                project.value.workspace.as_ref(),
                &project.value.index.to_le_bytes(),
            ],
            &apodect_solana::ID,
        );
        ensure!(
            expected == self.binding.project && bump == project.value.bump,
            "Invalid Solana Project PDA"
        );
        Ok(project)
    }

    pub async fn read_task(&self, address: Pubkey) -> Result<Option<Finalized<Task>>> {
        let task = self.read_optional::<Task>(address, 0).await?;
        if let Some(task) = &task {
            ensure!(
                task.value.project == self.binding.project,
                "Task belongs to another Project"
            );
        }
        Ok(task)
    }

    pub async fn read_workspace(&self, address: Pubkey) -> Result<Finalized<Workspace>> {
        self.read_required(address, 0).await
    }

    pub async fn token_balance(&self, address: Pubkey, owner: Pubkey) -> Result<Finalized<u64>> {
        let data = self.token_account_data(address, 0).await?;
        let token = spl_token::state::Account::unpack(&data.value)
            .context("Invalid Solana token account")?;
        ensure!(
            token.mint == self.binding.mint && token.owner == owner,
            "Solana token account owner or mint mismatch"
        );
        Ok(Finalized {
            slot: data.slot,
            value: token.amount,
        })
    }

    pub async fn read_request(
        &self,
        address: Pubkey,
        task: Pubkey,
    ) -> Result<Option<Finalized<BudgetRequest>>> {
        let request = self.read_optional::<BudgetRequest>(address, 0).await?;
        if let Some(request) = &request {
            ensure!(
                request.value.project == self.binding.project && request.value.task == task,
                "Request belongs to another Project or task"
            );
        }
        Ok(request)
    }

    /// Task indices begin at zero. A missing record within task_count is an RPC
    /// consistency error, not a reason to advance the persistent scan cursor.
    pub async fn scan_tasks(&self, start: u64, limit: u64) -> Result<TaskPage> {
        ensure!(
            (1..=100).contains(&limit),
            "Solana scan limit must be between 1 and 100"
        );
        let project = self.read_project().await?;
        ensure!(
            start <= project.value.task_count,
            "Solana task cursor exceeds finalized history"
        );
        let end = start.saturating_add(limit).min(project.value.task_count);
        let keys: Vec<_> = (start..end).map(|index| self.task_address(index)).collect();
        let tasks = self.read_batch::<Task>(&keys, project.slot).await?;
        let request_keys: Vec<_> = tasks.value.iter().filter_map(|task| task.pending).collect();
        let requests = self
            .read_batch::<BudgetRequest>(&request_keys, tasks.slot)
            .await?;
        let slot = requests.slot;
        let mut requests = request_keys.into_iter().zip(requests.value);
        let mut records = Vec::with_capacity(tasks.value.len());

        for (address, task) in keys.into_iter().zip(tasks.value) {
            ensure!(
                task.project == self.binding.project,
                "Foreign task in Solana scan"
            );
            let request = if let Some(pending) = task.pending {
                let (key, request) = requests.next().context("Missing pending Solana request")?;
                ensure!(
                    key == pending
                        && request.project == self.binding.project
                        && request.task == address,
                    "Foreign request in Solana scan"
                );
                Some(RequestRecord {
                    address: key,
                    request,
                })
            } else {
                None
            };
            records.push(TaskRecord {
                address,
                task,
                request,
            });
        }

        Ok(TaskPage {
            project,
            tasks: records,
            next_task: end,
            slot,
        })
    }

    pub fn task_address(&self, index: u64) -> Pubkey {
        bridge::pda(&[b"task", self.binding.project.as_ref(), &index.to_le_bytes()])
    }

    pub async fn scan_requests(&self, task: Pubkey, start: u64, limit: u64) -> Result<RequestPage> {
        ensure!(
            (1..=100).contains(&limit),
            "Solana scan limit must be between 1 and 100"
        );
        let state = self
            .read_task(task)
            .await?
            .context("Task is not finalized")?;
        ensure!(
            start <= state.value.request_count,
            "Request cursor exceeds finalized history"
        );
        let end = start.saturating_add(limit).min(state.value.request_count);
        let keys: Vec<_> = (start..end)
            .map(|index| bridge::pda(&[b"request", task.as_ref(), &index.to_le_bytes()]))
            .collect();
        let records = self.read_batch::<BudgetRequest>(&keys, state.slot).await?;
        let mut requests = Vec::with_capacity(records.value.len());
        for (address, request) in keys.into_iter().zip(records.value) {
            ensure!(
                request.project == self.binding.project && request.task == task,
                "Foreign request in Solana history"
            );
            requests.push(RequestRecord { address, request });
        }
        Ok(RequestPage {
            requests,
            next_request: end,
            total: state.value.request_count,
            slot: records.slot,
        })
    }

    /// Funding IDs are caller-supplied, so there is no numeric cursor. Filter the
    /// immutable program accounts and deduplicate their IDs in the indexer.
    pub async fn funding_receipts(&self, payer: Pubkey) -> Result<Finalized<Vec<FundingRecord>>> {
        #[derive(Deserialize)]
        struct Entry {
            pubkey: String,
            account: RpcAccount,
        }
        let response: RpcResponse<Vec<Entry>> = self.rpc.call("getProgramAccounts", &json!([
            apodect_solana::ID.to_string(),
            {
                "commitment":"finalized", "encoding":"base64", "withContext":true,
                "filters":[
                    {"dataSize":168},
                    {"memcmp":{"offset":0,"bytes":bs58::encode(FundingReceipt::DISCRIMINATOR).into_string()}},
                    {"memcmp":{"offset":8,"bytes":bs58::encode(self.binding.domain).into_string()}}
                ]
            }
        ])).await?;
        let mut records = Vec::with_capacity(response.value.len());
        for entry in response.value {
            let data = entry.account.bytes()?;
            ensure!(data.len() == 168, "Invalid funding account length");
            let id = data[40..72]
                .try_into()
                .expect("Checked funding receipt length");
            let receipt: Pubkey = entry
                .pubkey
                .parse()
                .context("Invalid funding receipt address")?;
            // Another Project can copy a domain when binding. Only this
            // Project's derived receipt addresses belong in its projection.
            if receipt != self.record_address(b"funding", id) {
                continue;
            }
            validate_receipt(
                &entry.account,
                &data,
                self.binding.domain,
                id,
                true,
                Some(payer),
                response.context.slot,
            )?;
            records.push(FundingRecord {
                id,
                receipt,
                payload: data[8..].to_vec(),
            });
        }
        Ok(Finalized {
            slot: response.context.slot,
            value: records,
        })
    }

    pub fn route_address(&self, route: [u8; 32]) -> Pubkey {
        self.record_address(b"bridge-escrow", route)
    }

    pub fn vault_address(&self) -> Pubkey {
        bridge::pda(&[b"vault", self.binding.project.as_ref()])
    }

    pub async fn route_state(&self, route: [u8; 32]) -> Result<Option<RouteState>> {
        let Some(record) = self
            .read_optional::<BridgeRoute>(self.record_address(b"bridge-route", route), 0)
            .await?
        else {
            return Ok(None);
        };
        ensure!(
            record.value.project == self.binding.project && record.value.id == route,
            "Foreign Solana route"
        );
        ensure!(
            matches!(record.value.kind, bridge::DELIVERY | bridge::REFUND),
            "Invalid Solana route kind"
        );
        let escrow = self.route_address(route);
        let balance = self.read_token(escrow, record.slot).await?.value.amount;
        Ok(Some(RouteState {
            record: record.value,
            escrow,
            escrow_balance: balance,
            slot: record.slot,
        }))
    }

    pub async fn read_command(
        &self,
        operation: [u8; 32],
    ) -> Result<Option<Finalized<CommandReceipt>>> {
        let receipt = self
            .read_optional::<CommandReceipt>(self.record_address(b"bridge-command", operation), 0)
            .await?;
        if let Some(receipt) = &receipt {
            ensure!(
                receipt.value.project == self.binding.project
                    && receipt.value.operation == operation,
                "Foreign Solana command receipt"
            );
        }
        Ok(receipt)
    }

    pub async fn prepare_execute(
        &self,
        bytes: [u8; 384],
        signature: [u8; 64],
        recovery_id: u8,
    ) -> Result<PreparedTransaction> {
        let project = self.read_project().await?;
        let command = Command::parse(&bytes).context("Invalid coordinated Solana command")?;
        ensure!(
            command.domain == self.binding.domain,
            "Command targets another Solana domain"
        );
        command
            .authenticate(&bytes, &signature, recovery_id, &project.value.command_key)
            .context("Invalid coordinator signature")?;

        let task = (command.task != ZERO).then(|| Pubkey::new_from_array(command.task));
        let request = if matches!(
            command.action,
            action::OUT_OF_BUDGET | action::CANCEL | action::RESTORE_REQUEST
        ) {
            let task = self
                .read_task(task.context("Command task is required")?)
                .await?
                .context("Command task is not finalized")?;
            if command.action == action::RESTORE_REQUEST {
                Some(bridge::pda(&[
                    b"request",
                    &command.task,
                    &task.value.request_count.to_le_bytes(),
                ]))
            } else {
                task.value.pending
            }
        } else {
            (command.request != ZERO).then(|| Pubkey::new_from_array(command.request))
        };
        let accounts = accounts::ExecuteCommand {
            project: self.binding.project,
            workspace: project.value.workspace,
            mint: self.binding.mint,
            vault: self.vault_address(),
            receipt: self.record_address(b"bridge-command", command.operation),
            payer: self.gas_payer()?.pubkey(),
            system_program: system_program::ID,
            token_program: spl_token::ID,
            task,
            request,
            plan: (action::RESERVE..=action::CLOSE_PLAN)
                .contains(&command.action)
                .then(|| self.record_address(b"bridge-plan", command.related)),
            route: matches!(
                command.action,
                action::EXPECT_DELIVERY | action::EXPECT_REFUND | action::DISPATCH
            )
            .then(|| self.record_address(b"bridge-route", command.reference)),
            escrow: matches!(
                command.action,
                action::EXPECT_DELIVERY | action::EXPECT_REFUND
            )
            .then(|| self.route_address(command.reference)),
            recipient: matches!(command.action, action::DISPATCH | action::OWNER_WITHDRAW)
                .then(|| Pubkey::new_from_array(command.recipient)),
            target: (command.action == action::INVALIDATE)
                .then(|| self.record_address(b"bridge-command", command.reference)),
        };
        self.prepare(
            make_instruction(
                instruction::ExecuteCommand {
                    bytes,
                    signature,
                    recovery_id,
                },
                accounts,
            ),
            command.operation,
        )
        .await
    }

    pub async fn prepare_checkpoint(
        &self,
        observation: [u8; 32],
        selection: CheckpointSelection,
    ) -> Result<PreparedTransaction> {
        ensure!(observation != ZERO, "Checkpoint observation is required");
        let instruction = make_instruction(
            instruction::RecordCheckpoint {
                observation,
                task: selection.task.map(|key| key.to_bytes()).unwrap_or(ZERO),
                request: selection.request.map(|key| key.to_bytes()).unwrap_or(ZERO),
                plan: selection.plan.unwrap_or(ZERO),
                route: selection.route.unwrap_or(ZERO),
                operation: selection.operation.unwrap_or(ZERO),
            },
            accounts::RecordCheckpoint {
                project: self.binding.project,
                checkpoint: self.record_address(b"checkpoint", observation),
                payer: self.gas_payer()?.pubkey(),
                system_program: system_program::ID,
                task: selection.task,
                request: selection.request,
                plan: selection
                    .plan
                    .map(|id| self.record_address(b"bridge-plan", id)),
                route: selection
                    .route
                    .map(|id| self.record_address(b"bridge-route", id)),
                command: selection
                    .operation
                    .map(|id| self.record_address(b"bridge-command", id)),
            },
        );
        self.prepare(instruction, observation).await
    }

    pub async fn prepare_collect(
        &self,
        route: [u8; 32],
        intent: [u8; 32],
    ) -> Result<PreparedTransaction> {
        ensure!(intent != ZERO, "Collection intent is required");
        let route_state = self
            .route_state(route)
            .await?
            .context("Route is not finalized")?;
        self.prepare(
            make_instruction(
                instruction::CollectRoute {},
                accounts::CollectRoute {
                    project: self.binding.project,
                    mint: self.binding.mint,
                    vault: self.vault_address(),
                    route: self.record_address(b"bridge-route", route),
                    plan: self.record_address(b"bridge-plan", route_state.record.plan),
                    escrow: route_state.escrow,
                    token_program: spl_token::ID,
                },
            ),
            intent,
        )
        .await
    }

    /// Submit precisely the persisted bytes. A transport error leaves the
    /// outcome unknown and never causes this method to sign a replacement.
    pub async fn broadcast(&self, prepared: &PreparedTransaction) -> Result<()> {
        let bytes = STANDARD
            .decode(&prepared.bytes_base64)
            .context("Invalid prepared Solana bytes")?;
        let transaction: Transaction =
            wincode::deserialize(&bytes).context("Invalid prepared Solana transaction")?;
        ensure!(
            transaction
                .signatures
                .first()
                .map(ToString::to_string)
                .as_deref()
                == Some(&prepared.signature),
            "Prepared Solana signature does not match bytes"
        );
        let signature: String = self.rpc.call("sendTransaction", &json!([
            prepared.bytes_base64,
            {"encoding":"base64", "preflightCommitment":"finalized", "skipPreflight":false, "maxRetries":0}
        ])).await.with_context(|| format!("Solana submission outcome is unknown for {}", prepared.signature))?;
        ensure!(
            signature == prepared.signature,
            "Solana RPC returned a different signature; reconcile the persisted transaction"
        );
        Ok(())
    }

    pub async fn transaction_expired(&self, prepared: &PreparedTransaction) -> Result<bool> {
        let height: u64 = self
            .rpc
            .call("getBlockHeight", &json!([{"commitment":"finalized"}]))
            .await?;
        Ok(height > prepared.last_valid_block_height)
    }

    pub async fn transaction_outcome(&self, signature: &str) -> Result<TransactionOutcome> {
        signature
            .parse::<Signature>()
            .context("Invalid Solana transaction signature")?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Status {
            slot: u64,
            err: Option<Value>,
            confirmation_status: Option<String>,
        }
        let response: RpcResponse<Vec<Option<Status>>> = self
            .rpc
            .call(
                "getSignatureStatuses",
                &json!([[signature], {"searchTransactionHistory":true}]),
            )
            .await?;
        ensure!(response.value.len() == 1, "Invalid Solana status response");
        let Some(status) = response.value.into_iter().next().flatten() else {
            return Ok(TransactionOutcome::Unknown);
        };
        ensure!(
            status.slot <= response.context.slot,
            "Solana signature status exceeds its context slot"
        );
        if status.confirmation_status.as_deref() != Some("finalized") {
            return Ok(TransactionOutcome::Pending);
        }
        Ok(match status.err {
            Some(error) => TransactionOutcome::Failed {
                slot: status.slot,
                error,
            },
            None => TransactionOutcome::Finalized { slot: status.slot },
        })
    }

    pub async fn checkpoint_evidence(
        &self,
        signature: &str,
        observation: [u8; 32],
    ) -> Result<ReceiptEvidence> {
        self.receipt_evidence(signature, observation, false, None)
            .await
    }

    pub async fn funding_evidence(
        &self,
        signature: &str,
        funding_id: [u8; 32],
        payer: Pubkey,
    ) -> Result<ReceiptEvidence> {
        self.receipt_evidence(signature, funding_id, true, Some(payer))
            .await
    }

    /// Recover a finalized transaction anchor for an immutable receipt when its
    /// submitter did not use this backend. This does not initiate another transfer.
    pub async fn receipt_signature(&self, id: [u8; 32], funding: bool) -> Result<Option<String>> {
        let address = self.record_address(if funding { b"funding" } else { b"checkpoint" }, id);
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Entry {
            signature: String,
            err: Option<Value>,
            confirmation_status: Option<String>,
        }
        let mut before = None;
        loop {
            let mut config = json!({"commitment":"finalized", "limit":1000});
            if let Some(cursor) = &before {
                config["before"] = json!(cursor);
            }
            let entries: Vec<Entry> = self
                .rpc
                .call(
                    "getSignaturesForAddress",
                    &json!([address.to_string(), config]),
                )
                .await?;
            if let Some(entry) = entries.iter().find(|entry| {
                entry.err.is_none() && entry.confirmation_status.as_deref() == Some("finalized")
            }) {
                return Ok(Some(entry.signature.clone()));
            }
            let Some(last) = entries.last() else {
                return Ok(None);
            };
            ensure!(
                before.as_ref() != Some(&last.signature),
                "Solana signature history did not advance"
            );
            before = Some(last.signature.clone());
        }
    }

    async fn receipt_evidence(
        &self,
        signature: &str,
        id: [u8; 32],
        funding: bool,
        payer: Option<Pubkey>,
    ) -> Result<ReceiptEvidence> {
        let slot = match self.transaction_outcome(signature).await? {
            TransactionOutcome::Finalized { slot } => slot,
            TransactionOutcome::Failed { .. } => {
                bail!("Failed Solana transaction is not receipt evidence")
            }
            _ => bail!("Solana receipt transaction is not finalized"),
        };
        let receipt = self.record_address(if funding { b"funding" } else { b"checkpoint" }, id);
        let response = self.account(receipt, slot).await?;
        let account = response
            .value
            .context("Immutable Solana receipt is not finalized")?;
        let data = account.bytes()?;
        validate_receipt(
            &account,
            &data,
            self.binding.domain,
            id,
            funding,
            payer,
            response.context.slot,
        )?;

        let signature: Signature = signature.parse().context("Invalid Solana signature")?;
        let request = json!({"Solana": {
            "tx_id": encode_hex(signature.as_ref()),
            "finality":"Finalized",
            "extractors":[{"AccountState":{"pubkey":encode_hex(receipt.as_ref())}}]
        }});
        let values = json!([{"SvmExtractedValue":{"AccountState":{
            "owner":encode_hex(apodect_solana::ID.as_ref()),
            "data":encode_hex(&data)
        }}}]);
        Ok(ReceiptEvidence {
            payload: data[8..].to_vec(),
            receipt,
            slot,
            request,
            values,
        })
    }

    fn record_address(&self, seed: &[u8], id: [u8; 32]) -> Pubkey {
        bridge::pda(&[seed, self.binding.project.as_ref(), &id])
    }

    fn gas_payer(&self) -> Result<&Keypair> {
        self.payer
            .as_ref()
            .context("Solana gas-payer key is not configured")
    }

    async fn prepare(
        &self,
        instruction: Instruction,
        intent: [u8; 32],
    ) -> Result<PreparedTransaction> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Blockhash {
            blockhash: String,
            last_valid_block_height: u64,
        }
        let response: RpcResponse<Blockhash> = self
            .rpc
            .call("getLatestBlockhash", &json!([{"commitment":"finalized"}]))
            .await?;
        let payer = self.gas_payer()?;
        let memo = Instruction {
            program_id: "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr".parse()?,
            accounts: vec![],
            data: encode_hex(&intent).into_bytes(),
        };
        let message = Message::new(&[instruction, memo], Some(&payer.pubkey()));
        let transaction = Transaction::new(&[payer], message, response.value.blockhash.parse()?);
        let bytes = wincode::serialize(&transaction).context("Cannot encode Solana transaction")?;
        ensure!(
            bytes.len() <= 1232,
            "Solana transaction exceeds packet size"
        );
        Ok(PreparedTransaction {
            signature: transaction.signatures[0].to_string(),
            bytes_base64: STANDARD.encode(bytes),
            last_valid_block_height: response.value.last_valid_block_height,
        })
    }

    async fn account(
        &self,
        address: Pubkey,
        min_slot: u64,
    ) -> Result<RpcResponse<Option<RpcAccount>>> {
        let response: RpcResponse<Option<RpcAccount>> = self.rpc.call("getAccountInfo", &json!([
            address.to_string(), {"encoding":"base64", "commitment":"finalized", "minContextSlot":min_slot}
        ])).await?;
        ensure!(
            response.context.slot >= min_slot,
            "Solana RPC returned an older account context"
        );
        Ok(response)
    }

    async fn read_optional<T: AccountDeserialize>(
        &self,
        address: Pubkey,
        min_slot: u64,
    ) -> Result<Option<Finalized<T>>> {
        let response = self.account(address, min_slot).await?;
        let value = response
            .value
            .as_ref()
            .map(RpcAccount::decode)
            .transpose()?
            .flatten();
        Ok(value.map(|value| Finalized {
            slot: response.context.slot,
            value,
        }))
    }

    async fn read_required<T: AccountDeserialize>(
        &self,
        address: Pubkey,
        min_slot: u64,
    ) -> Result<Finalized<T>> {
        self.read_optional(address, min_slot)
            .await?
            .context("Required Solana account is not finalized")
    }

    async fn read_batch<T: AccountDeserialize>(
        &self,
        addresses: &[Pubkey],
        min_slot: u64,
    ) -> Result<Finalized<Vec<T>>> {
        if addresses.is_empty() {
            return Ok(Finalized {
                slot: min_slot,
                value: vec![],
            });
        }
        let addresses: Vec<_> = addresses.iter().map(ToString::to_string).collect();
        let response: RpcResponse<Vec<Option<RpcAccount>>> = self.rpc.call("getMultipleAccounts", &json!([
            addresses, {"encoding":"base64", "commitment":"finalized", "minContextSlot":min_slot}
        ])).await?;
        ensure!(
            response.context.slot >= min_slot && response.value.len() == addresses.len(),
            "Inconsistent Solana account batch"
        );
        let value = response
            .value
            .into_iter()
            .map(|account| {
                account
                    .context("Missing Solana scan account")?
                    .decode()?
                    .context("Uninitialized Solana scan account")
            })
            .collect::<Result<_>>()?;
        Ok(Finalized {
            slot: response.context.slot,
            value,
        })
    }

    async fn token_account_data(
        &self,
        address: Pubkey,
        min_slot: u64,
    ) -> Result<Finalized<Vec<u8>>> {
        let response = self.account(address, min_slot).await?;
        let account = response.value.context("Solana token account is missing")?;
        ensure!(
            account.owner == spl_token::ID.to_string() && !account.executable,
            "Wrong Solana token program"
        );
        Ok(Finalized {
            slot: response.context.slot,
            value: account.bytes()?,
        })
    }

    async fn read_token(
        &self,
        address: Pubkey,
        min_slot: u64,
    ) -> Result<Finalized<spl_token::state::Account>> {
        let data = self.token_account_data(address, min_slot).await?;
        let token = spl_token::state::Account::unpack(&data.value)
            .context("Invalid Solana token account")?;
        ensure!(
            token.mint == self.binding.mint && token.owner == self.binding.project,
            "Foreign Solana Project token account"
        );
        Ok(Finalized {
            slot: data.slot,
            value: token,
        })
    }
}

fn make_instruction(data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
    Instruction {
        program_id: apodect_solana::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}").expect("Writing into String cannot fail");
    }
    output
}

fn validate_receipt(
    account: &RpcAccount,
    data: &[u8],
    domain: [u8; 32],
    id: [u8; 32],
    funding: bool,
    payer: Option<Pubkey>,
    context_slot: u64,
) -> Result<()> {
    ensure!(
        account.owner == apodect_solana::ID.to_string() && !account.executable,
        "Foreign Solana receipt owner"
    );
    let (length, discriminator) = if funding {
        (168, FundingReceipt::DISCRIMINATOR)
    } else {
        (1256, Checkpoint::DISCRIMINATOR)
    };
    ensure!(
        data.len() == length && &data[..8] == discriminator,
        "Invalid immutable Solana receipt layout"
    );
    let payload = &data[8..];
    ensure!(
        payload[..32] == domain && payload[32..64] == id,
        "Solana receipt identity mismatch"
    );
    if funding {
        ensure!(
            payload[128..160] == payer.context("Funding payer is required")?.to_bytes(),
            "Wrong Solana funding payer"
        );
        bridge::integer(&payload[96..128]).context("Invalid Solana funding amount")?;
    } else {
        let slot = bridge::integer(&payload[96..128]).context("Invalid checkpoint slot")?;
        ensure!(
            slot <= context_slot,
            "Solana checkpoint is newer than finalized context"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "solana_tests.rs"]
mod tests;
