//! Finalized NEAR reads and explicit, durable relay transaction preparation.

use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use near_crypto::{InMemorySigner, Signer};
use near_primitives::{
    action::{Action, FunctionCallAction},
    gas::Gas,
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{AccountId, Balance},
    views::{
        ExecutionStatusView, FinalExecutionOutcomeView, FinalExecutionStatus, TxExecutionStatus,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::rpc::{Rpc, RpcError};

#[derive(Clone)]
pub struct NearClient {
    rpc: Rpc,
}

/// These immutable fields also identify the local sandbox deployment manifest.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GenesisIdentity {
    pub chain_id: String,
    pub genesis_time: String,
    pub genesis_height: u64,
    pub validators: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NearBlock {
    pub height: u64,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CoordinatorState {
    pub owner_id: String,
    pub authority_id: String,
    pub project_id: String,
    pub mpc_id: String,
    pub sign_key: String,
    pub read_key: String,
    pub sign_path: String,
    pub queue_head: u32,
    pub queue_length: u32,
    pub next_plan: u64,
    pub next_command: u64,
    pub next_invoice: u64,
    pub retry_limit: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CoordinatorDomain {
    pub domain: String,
    pub chain: String,
    pub vault: String,
    pub asset: String,
    pub program: Option<String>,
    pub owner_identity: String,
    pub owner_recipient: String,
    pub sequence: u64,
    pub free: u64,
    pub held: u64,
    pub pending_reserves: u64,
    pub available: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CoordinatorCommand {
    pub operation: String,
    pub domain: String,
    pub command: String,
    pub digest: String,
    pub action: u64,
    pub plan: String,
    pub signature: Option<Value>,
    pub signing: bool,
    pub signature_error: Option<String>,
    pub state: u64,
    pub result: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RouteQuote {
    pub route: String,
    pub source: String,
    pub input: u64,
    pub min_input: u64,
    pub output: u64,
    pub deposit: String,
    pub refund: String,
    pub recipient: String,
    pub quote_hash: String,
    pub expires: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlanRoute {
    pub quote: RouteQuote,
    pub delivery: Option<String>,
    pub refund: Option<String>,
    pub dispatch: Option<String>,
    pub received: u64,
    pub returned: u64,
    pub dispatched: u64,
    pub unsettled_input: u64,
    pub confirmed_transport_expense: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Reservation {
    pub domain: String,
    pub operation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CoordinatorPlan {
    pub plan: String,
    pub destination: String,
    pub principal: u64,
    pub local_amount: u64,
    pub expires: u64,
    pub status: String,
    pub reserves: Vec<Reservation>,
    pub routes: Vec<PlanRoute>,
    pub allocation: Option<String>,
    pub invalidations: Vec<String>,
    pub closures: Vec<String>,
    pub attempts: u32,
    pub requires_attention: bool,
    pub error: Option<String>,
    pub queue_released: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalProject {
    pub owner_id: String,
    pub authority_id: String,
    pub token_id: String,
    #[serde(with = "decimal")]
    pub free: u128,
    #[serde(with = "decimal")]
    pub pending_owner_outflow: u128,
    pub next_task: u64,
    pub next_request: u64,
    pub next_invoice: u64,
    pub next_operation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalTask {
    pub agent: String,
    pub state: String,
    pub paused: bool,
    pub started: bool,
    #[serde(with = "decimal")]
    pub issued: u128,
    #[serde(with = "decimal")]
    pub recovered: u128,
    #[serde(with = "decimal")]
    pub available: u128,
    #[serde(with = "decimal")]
    pub pending_outflow: u128,
    pub live_request: Option<u64>,
    pub report: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalRequest {
    pub task_id: u64,
    #[serde(with = "decimal")]
    pub amount: u128,
    pub purpose: [u8; 32],
    pub state: String,
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalBridge {
    pub domain: Option<String>,
    pub key: Option<String>,
    #[serde(with = "decimal")]
    pub held: u128,
    #[serde(with = "decimal")]
    pub accounted_custody: u128,
    pub sequence: u64,
    pub next_funding: u64,
}

/// A wallet may sign this intent; it conveys no owner or authority permission itself.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FunctionCallIntent {
    pub receiver_id: String,
    pub method_name: String,
    #[serde(rename = "args_base64", with = "argument_bytes")]
    pub args: Value,
    pub gas: u64,
    #[serde(with = "decimal")]
    pub deposit: u128,
}

/// Only permissionless operations can be signed using the server relay credential.
pub struct RelayCall(FunctionCallIntent);

impl RelayCall {
    pub fn coordinator(
        coordinator: &str,
        method: &str,
        args: Value,
        deposit: u128,
    ) -> Result<Self> {
        ensure!(
            matches!(
                method,
                "advance"
                    | "advance_queue"
                    | "request_signature"
                    | "observe_near"
                    | "observe_foreign"
                    | "observe_funding_near"
                    | "observe_funding_foreign"
            ),
            "coordinator action requires an owner or authority signature"
        );
        ensure!(
            method != "advance_queue" || deposit == 0,
            "advance_queue is not payable"
        );
        Self::new(coordinator, method, args, deposit)
    }

    pub fn execute(vault: &str, args: Value, deposit: u128) -> Result<Self> {
        Self::new(vault, "execute_command", args, deposit)
    }

    pub fn checkpoint(vault: &str, selectors: Value, deposit: u128) -> Result<Self> {
        Self::new(vault, "checkpoint", selectors, deposit)
    }

    pub fn collect(receiver: &str) -> Result<Self> {
        Self::new(receiver, "collect", json!({}), 1)
    }

    fn new(receiver: &str, method: &str, args: Value, deposit: u128) -> Result<Self> {
        receiver
            .parse::<AccountId>()
            .context("invalid NEAR receiver")?;
        ensure!(
            args.is_object(),
            "NEAR function arguments must be an object"
        );
        Ok(Self(FunctionCallIntent {
            receiver_id: receiver.to_owned(),
            method_name: method.to_owned(),
            args,
            gas: 300_000_000_000_000,
            deposit,
        }))
    }
}

pub struct RelaySigner(Signer);

impl RelaySigner {
    pub fn from_file(path: &Path, expected_account: &str) -> Result<Self> {
        // Parsing errors must not expose the credential's contents.
        let signer = InMemorySigner::from_file(path)
            .map_err(|_| anyhow!("cannot load NEAR relay credential"))?;
        ensure!(
            signer.get_account_id().as_str() == expected_account,
            "NEAR relay account mismatch"
        );
        Ok(Self(signer))
    }

    pub fn account_id(&self) -> String {
        self.0.get_account_id().to_string()
    }

    pub fn public_key(&self) -> String {
        self.0.public_key().to_string()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreparedTransaction {
    pub signed_tx_base64: String,
    pub hash: String,
    pub sender: String,
    pub nonce: u64,
}

pub enum TransactionOutcome {
    Unknown,
    Pending,
    Final(Box<FinalExecution>),
}

pub struct FinalExecution {
    pub outcome: FinalExecutionOutcomeView,
    pub failures: Vec<ExecutionFailure>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionFailure {
    pub receipt_id: String,
    pub executor_id: String,
    pub error: Value,
}

impl FinalExecution {
    /// A callback may succeed after catching a failed child receipt. Preserve both facts.
    pub fn json_result(&self) -> Result<Value> {
        ensure!(
            self.failures.is_empty(),
            "NEAR execution contains failed receipts"
        );
        let FinalExecutionStatus::SuccessValue(bytes) = &self.outcome.status else {
            return Err(anyhow!("NEAR transaction did not succeed"));
        };
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_slice(bytes).context("NEAR return value is not valid JSON")
        }
    }
}

impl NearClient {
    pub async fn connect(rpc: Rpc, expected: &GenesisIdentity) -> Result<Self> {
        let status: Value = rpc.call("status", &json!({})).await?;
        ensure!(
            status["chain_id"] == expected.chain_id,
            "NEAR chain identity mismatch"
        );
        let genesis: GenesisIdentity = rpc.call("genesis_config", &json!({})).await?;
        ensure!(&genesis == expected, "NEAR genesis identity mismatch");
        Ok(Self { rpc })
    }

    pub async fn finalized_block(&self) -> Result<NearBlock> {
        #[derive(Deserialize)]
        struct BlockResponse {
            header: NearBlock,
        }
        let response: BlockResponse = self
            .rpc
            .call("block", &json!({"finality": "final"}))
            .await?;
        response
            .header
            .hash
            .parse::<CryptoHash>()
            .map_err(|_| anyhow!("invalid NEAR block hash"))?;
        Ok(response.header)
    }

    pub async fn view<T: DeserializeOwned>(
        &self,
        account: &str,
        method: &str,
        args: &Value,
        block: &NearBlock,
    ) -> Result<T> {
        account
            .parse::<AccountId>()
            .context("invalid NEAR contract account")?;
        let response: ViewResponse = self
            .rpc
            .call(
                "query",
                &json!({
                    "request_type": "call_function",
                    "block_id": block.hash,
                    "account_id": account,
                    "method_name": method,
                    "args_base64": STANDARD.encode(serde_json::to_vec(args)?),
                }),
            )
            .await?;
        response
            .decode(block)
            .with_context(|| format!("NEAR view {account}.{method}"))
    }

    /// Account ownership for login requires a currently installed full-access key,
    /// not merely a valid signature by any key associated with that account.
    pub async fn has_full_access_key(
        &self,
        account: &str,
        public_key: &str,
        block: &NearBlock,
    ) -> Result<bool> {
        account
            .parse::<AccountId>()
            .context("invalid NEAR account")?;
        public_key
            .parse::<near_crypto::PublicKey>()
            .context("invalid NEAR public key")?;
        let key: Value = self
            .rpc
            .call(
                "query",
                &json!({
                    "request_type": "view_access_key",
                    "block_id": block.hash,
                    "account_id": account,
                    "public_key": public_key,
                }),
            )
            .await?;
        ensure!(
            key["block_hash"] == block.hash && key["block_height"] == block.height,
            "NEAR access key read returned a different block"
        );
        Ok(key["permission"] == "FullAccess")
    }

    pub async fn coordinator_state(
        &self,
        coordinator: &str,
        block: &NearBlock,
    ) -> Result<CoordinatorState> {
        self.view(coordinator, "get_state", &json!({}), block).await
    }

    pub async fn coordinator_domain(
        &self,
        coordinator: &str,
        domain: &str,
        block: &NearBlock,
    ) -> Result<CoordinatorDomain> {
        self.view(coordinator, "get_domain", &json!({"domain": domain}), block)
            .await
    }

    pub async fn coordinator_commands(
        &self,
        coordinator: &str,
        from: u64,
        limit: u64,
        block: &NearBlock,
    ) -> Result<Vec<CoordinatorCommand>> {
        ensure!(
            (1..=100).contains(&limit),
            "NEAR command page limit must be 1..100"
        );
        self.view(
            coordinator,
            "list_commands",
            &json!({"from": from, "limit": limit}),
            block,
        )
        .await
    }

    pub async fn coordinator_plan(
        &self,
        coordinator: &str,
        plan: &str,
        block: &NearBlock,
    ) -> Result<Option<CoordinatorPlan>> {
        self.view(coordinator, "get_plan", &json!({"plan": plan}), block)
            .await
    }

    pub async fn local_project(&self, vault: &str, block: &NearBlock) -> Result<LocalProject> {
        self.view(vault, "get_project", &json!({}), block).await
    }

    pub async fn local_bridge(&self, vault: &str, block: &NearBlock) -> Result<LocalBridge> {
        self.view(vault, "get_bridge", &json!({}), block).await
    }

    pub async fn local_task(
        &self,
        vault: &str,
        task: u64,
        block: &NearBlock,
    ) -> Result<Option<LocalTask>> {
        self.view(vault, "get_task", &json!({"task_id": task}), block)
            .await
    }

    pub async fn local_request(
        &self,
        vault: &str,
        request: u64,
        block: &NearBlock,
    ) -> Result<Option<LocalRequest>> {
        self.view(vault, "get_request", &json!({"request_id": request}), block)
            .await
    }

    pub async fn route_receiver(
        &self,
        vault: &str,
        route: &str,
        block: &NearBlock,
    ) -> Result<String> {
        let receiver: String = self
            .view(vault, "route_receiver", &json!({"route": route}), block)
            .await?;
        receiver
            .parse::<AccountId>()
            .context("invalid NEAR route receiver")?;
        Ok(receiver)
    }

    /// The caller serializes preparation for this access key and persists the returned
    /// bytes before broadcasting. An unresolved submission must be reconciled first.
    pub async fn prepare_relay_call(
        &self,
        call: &RelayCall,
        signer: &RelaySigner,
        minimum_nonce: u64,
    ) -> Result<PreparedTransaction> {
        #[derive(Deserialize)]
        struct AccessKey {
            nonce: u64,
            block_hash: CryptoHash,
        }
        let access: AccessKey = self
            .rpc
            .call(
                "query",
                &json!({
                    "request_type": "view_access_key",
                    "finality": "optimistic",
                    "account_id": signer.account_id(),
                    "public_key": signer.public_key(),
                }),
            )
            .await?;
        let nonce = access
            .nonce
            .checked_add(1)
            .context("NEAR access key nonce exhausted")?
            .max(minimum_nonce);
        sign_call(&call.0, signer, nonce, access.block_hash)
    }

    /// This method makes one RPC attempt. Errors leave the signed identity unresolved.
    pub async fn broadcast(&self, prepared: &PreparedTransaction) -> Result<()> {
        validate_prepared(prepared)?;
        let _: Value = self
            .rpc
            .call(
                "send_tx",
                &json!({
                    "signed_tx_base64": prepared.signed_tx_base64,
                    "wait_until": "NONE",
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn transaction_outcome(
        &self,
        hash: &str,
        sender: &str,
    ) -> Result<TransactionOutcome> {
        hash.parse::<CryptoHash>()
            .map_err(|_| anyhow!("invalid NEAR transaction hash"))?;
        sender.parse::<AccountId>().context("invalid NEAR sender")?;
        let response: Result<TransactionResponse> = self
            .rpc
            .call(
                "tx",
                &json!({
                    "tx_hash": hash,
                    "sender_account_id": sender,
                    "wait_until": "NONE",
                }),
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error)
                if error
                    .downcast_ref::<RpcError>()
                    .and_then(|rpc| rpc.cause.as_ref())
                    .is_some_and(|cause| cause["name"] == "UNKNOWN_TRANSACTION") =>
            {
                return Ok(TransactionOutcome::Unknown);
            }
            Err(error) => return Err(error),
        };
        if response.final_execution_status != TxExecutionStatus::Final {
            return Ok(TransactionOutcome::Pending);
        }
        let outcome = response
            .outcome
            .context("final NEAR transaction has no outcome")?;
        ensure!(
            outcome.transaction.hash.to_string() == hash,
            "NEAR transaction identity mismatch"
        );
        ensure!(
            outcome.transaction.signer_id.as_str() == sender,
            "NEAR transaction sender mismatch"
        );
        Ok(TransactionOutcome::Final(Box::new(classify_execution(
            outcome,
        )?)))
    }
}

#[derive(Deserialize)]
struct ViewResponse {
    result: Vec<u8>,
    block_hash: String,
    block_height: u64,
}

impl ViewResponse {
    fn decode<T: DeserializeOwned>(self, expected: &NearBlock) -> Result<T> {
        ensure!(
            self.block_hash == expected.hash && self.block_height == expected.height,
            "NEAR view returned a different block than requested"
        );
        serde_json::from_slice(&self.result).context("invalid NEAR contract view result")
    }
}

#[derive(Deserialize)]
struct TransactionResponse {
    final_execution_status: TxExecutionStatus,
    #[serde(flatten)]
    outcome: Option<FinalExecutionOutcomeView>,
}

fn sign_call(
    call: &FunctionCallIntent,
    signer: &RelaySigner,
    nonce: u64,
    block_hash: CryptoHash,
) -> Result<PreparedTransaction> {
    let transaction = Transaction::V0(TransactionV0 {
        signer_id: signer.0.get_account_id(),
        public_key: signer.0.public_key(),
        nonce,
        receiver_id: call.receiver_id.parse().context("invalid NEAR receiver")?,
        block_hash,
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: call.method_name.clone(),
            args: serde_json::to_vec(&call.args)?,
            gas: Gas::from_gas(call.gas),
            deposit: Balance::from_yoctonear(call.deposit),
        }))],
    });
    let (hash, _) = transaction.get_hash_and_size();
    let signed = SignedTransaction::new(signer.0.sign(hash.as_ref()), transaction);
    // near-primitives serializes the canonical Borsh transaction to base64.
    let signed_tx_base64 = serde_json::to_value(&signed)?
        .as_str()
        .context("NEAR signed transaction serialization failed")?
        .to_owned();
    Ok(PreparedTransaction {
        signed_tx_base64,
        hash: hash.to_string(),
        sender: signer.account_id(),
        nonce,
    })
}

fn validate_prepared(prepared: &PreparedTransaction) -> Result<()> {
    let signed: SignedTransaction = serde_json::from_value(json!(prepared.signed_tx_base64))
        .context("invalid prepared NEAR transaction")?;
    ensure!(
        signed.get_hash().to_string() == prepared.hash
            && signed.transaction.signer_id().as_str() == prepared.sender
            && signed.transaction.nonce().nonce() == prepared.nonce,
        "prepared NEAR transaction identity differs from signed bytes"
    );
    ensure!(
        signed
            .signature
            .verify(signed.get_hash().as_ref(), signed.transaction.public_key()),
        "prepared NEAR transaction signature is invalid"
    );
    Ok(())
}

fn classify_execution(outcome: FinalExecutionOutcomeView) -> Result<FinalExecution> {
    ensure!(
        matches!(
            outcome.status,
            FinalExecutionStatus::SuccessValue(_) | FinalExecutionStatus::Failure(_)
        ),
        "final NEAR transaction contains an incomplete execution status"
    );
    let mut failures = Vec::new();
    for receipt in std::iter::once(&outcome.transaction_outcome).chain(&outcome.receipts_outcome) {
        ensure!(
            !matches!(receipt.outcome.status, ExecutionStatusView::Unknown),
            "final NEAR transaction contains an unresolved receipt"
        );
        if let ExecutionStatusView::Failure(error) = &receipt.outcome.status {
            failures.push(ExecutionFailure {
                receipt_id: receipt.id.to_string(),
                executor_id: receipt.outcome.executor_id.to_string(),
                error: serde_json::to_value(error)?,
            });
        }
    }
    Ok(FinalExecution { outcome, failures })
}

/// Send these decoded bytes directly to the wallet's FunctionCall action. JSON
/// parse/stringify in JavaScript would round coordinator u64 amounts above 2^53.
mod argument_bytes {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _, ser::Error as _};
    use serde_json::Value;

    pub fn serialize<S: Serializer>(args: &Value, serializer: S) -> Result<S::Ok, S::Error> {
        let bytes = serde_json::to_vec(args).map_err(S::Error::custom)?;
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let bytes = STANDARD.decode(encoded).map_err(D::Error::custom)?;
        serde_json::from_slice(&bytes).map_err(D::Error::custom)
    }
}

mod decimal {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(amount: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&amount.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(D::Error::custom("expected an unsigned decimal amount"));
        }
        value.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_view_rejects_a_different_source_block() {
        let block = NearBlock {
            height: 42,
            hash: "expected".into(),
        };
        let response = ViewResponse {
            result: b"7".to_vec(),
            block_hash: "other".into(),
            block_height: 42,
        };
        assert!(response.decode::<u64>(&block).is_err());
    }

    #[test]
    fn relay_cannot_sign_financial_decisions_and_signed_identity_is_immutable() {
        for method in [
            "approve",
            "apply",
            "control",
            "retry",
            "withdraw_free",
            "set_authority",
        ] {
            assert!(RelayCall::coordinator("coordinator.near", method, json!({}), 0).is_err());
        }
        let key: near_crypto::SecretKey = "ed25519:3D4YudUahN1nawWogh8pAKSj92sUNMdbZGjn7kERKzYoTy8tnFQuwoGUC51DowKqorvkr2pytJSnwuSbsNVfqygr".parse().unwrap();
        let signer = RelaySigner(InMemorySigner::from_secret_key(
            "relay.near".parse().unwrap(),
            key,
        ));
        let call = RelayCall::coordinator(
            "coordinator.near",
            "advance",
            json!({"plan": "00".repeat(32)}),
            1,
        )
        .unwrap();
        let mut prepared = sign_call(&call.0, &signer, 7, CryptoHash::default()).unwrap();
        validate_prepared(&prepared).unwrap();
        prepared.nonce = 8;
        assert!(validate_prepared(&prepared).is_err());
    }

    #[test]
    fn successful_callback_does_not_hide_a_failed_receipt() {
        let hash = CryptoHash::default().to_string();
        let receipt = |status: Value| {
            json!({
                "proof": [], "block_hash": hash, "id": hash,
                "outcome": {
                    "logs": [], "receipt_ids": [], "gas_burnt": 0,
                    "tokens_burnt": "0", "executor_id": "vault.near", "status": status,
                }
            })
        };
        let failure = json!({"Failure": {"ActionError": {
            "index": 0, "kind": {"FunctionCallError": {"ExecutionError": "transfer rejected"}}
        }}});
        let response = json!({
            "final_execution_status": "FINAL",
            "status": {"SuccessValue": STANDARD.encode(b"false")},
            "transaction": {
                "signer_id": "relay.near", "public_key": "ed25519:11111111111111111111111111111111",
                "nonce": 1, "receiver_id": "vault.near", "actions": [],
                "signature": format!("ed25519:{}", "1".repeat(64)), "hash": hash,
            },
            "transaction_outcome": receipt(json!({"SuccessReceiptId": hash})),
            "receipts_outcome": [receipt(failure), receipt(json!({"SuccessValue": STANDARD.encode(b"false")}))],
        });
        let parsed: TransactionResponse = serde_json::from_value(response).unwrap();
        let execution = classify_execution(parsed.outcome.unwrap()).unwrap();
        assert_eq!(execution.failures.len(), 1);
        assert!(execution.json_result().is_err());
    }
}
