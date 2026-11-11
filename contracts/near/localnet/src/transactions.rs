//! Single-broadcast transport. SDK transaction retries may sign a fresh nonce on an
//! RPC error, so mutations use the native RPC client and durable inclusion barriers.
use crate::{Localnet, read_json, write_json};
use anyhow::{Context, Result, bail};
use near_crypto::InMemorySigner;
use near_jsonrpc_client::{JsonRpcClient, methods};
use near_primitives::action::{Action, FunctionCallAction};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{SignedTransaction, Transaction, TransactionV0};
use near_primitives::types::BlockReference;
use near_primitives::views::{
    FinalExecutionOutcomeView, FinalExecutionStatus, QueryRequest, TxExecutionStatus,
};
use near_workspaces::{Account, AccountId, types::NearToken};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

const POLL_LIMIT: Duration = Duration::from_secs(60);
const RPC_LIMIT: Duration = Duration::from_secs(10);

fn lock_signer(directory: &Path, sender: &AccountId) -> Result<File> {
    fs::create_dir_all(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join(format!("{sender}.lock")))?;
    file.lock()?;
    Ok(file)
}

pub(super) fn function_call(method: &str, args: Value, deposit: NearToken) -> Result<Action> {
    Ok(Action::FunctionCall(Box::new(FunctionCallAction {
        method_name: method.into(),
        args: serde_json::to_vec(&args)?,
        gas: near_primitives::gas::Gas::from_teragas(300),
        deposit: near_primitives::types::Balance::from_yoctonear(deposit.as_yoctonear()),
    })))
}

pub(super) fn value(outcome: &FinalExecutionOutcomeView) -> Result<Value> {
    match &outcome.status {
        FinalExecutionStatus::SuccessValue(bytes) => {
            if bytes.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(serde_json::from_slice(bytes)?)
            }
        }
        FinalExecutionStatus::Failure(error) => {
            bail!("transaction {} failed: {error:?}", outcome.transaction.hash)
        }
        other => bail!("transaction outcome is not final: {other:?}"),
    }
}

async fn poll(
    client: &JsonRpcClient,
    sender: &AccountId,
    hash: CryptoHash,
    final_receipts: bool,
    limit: Duration,
) -> Result<methods::tx::RpcTransactionResponse> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut last_error = "not yet included".to_owned();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            bail!(
                "transaction {hash} for {sender} has unknown {}: {last_error}; reconcile this hash, do not resend",
                if final_receipts {
                    "finalized outcome"
                } else {
                    "inclusion"
                }
            );
        }
        // NONE returns the currently known status without waiting on yielded MPC
        // receipts. Repeated reads never sign or broadcast another transaction.
        let request = methods::tx::RpcTransactionStatusRequest {
            transaction_info: methods::tx::TransactionInfo::TransactionId {
                tx_hash: hash,
                sender_account_id: sender.clone(),
            },
            wait_until: TxExecutionStatus::None,
        };
        match tokio::time::timeout(remaining.min(RPC_LIMIT), client.call(request)).await {
            Ok(Ok(response)) => {
                let included = response.final_execution_status != TxExecutionStatus::None;
                let finalized = response.final_execution_status == TxExecutionStatus::Final
                    && response.final_execution_outcome.is_some();
                if if final_receipts { finalized } else { included } {
                    return Ok(response);
                }
                last_error = format!("status {:?}", response.final_execution_status);
            }
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => last_error = "RPC status read timed out".into(),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn reconcile_barrier(
    client: &JsonRpcClient,
    directory: &Path,
    sender: &AccountId,
    limit: Duration,
) -> Result<Option<CryptoHash>> {
    let pending = directory.join(format!("{sender}.pending.json"));
    if !pending.exists() {
        return Ok(None);
    }
    let record = read_json(&pending)?;
    let hash = record["tx_hash"]
        .as_str()
        .context("invalid pending transaction")?
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid transaction hash: {error}"))?;
    poll(client, sender, hash, false, limit).await?;
    record_included_nonce(directory, sender, &record)?;
    fs::remove_file(pending)?;
    File::open(directory)?.sync_all()?;
    Ok(Some(hash))
}

fn next_nonce(directory: &Path, sender: &AccountId, observed: u64) -> Result<u64> {
    let path = directory.join(format!("{sender}.nonce.json"));
    let included = if path.exists() {
        read_json(&path)?["nonce"]
            .as_u64()
            .context("invalid included nonce")?
    } else {
        0
    };
    observed
        .max(included)
        .checked_add(1)
        .context("access key nonce overflow")
}

fn record_included_nonce(directory: &Path, sender: &AccountId, record: &Value) -> Result<()> {
    let transaction: SignedTransaction =
        serde_json::from_value(record["signed_transaction"].clone())?;
    let nonce = next_nonce(directory, sender, transaction.transaction.nonce().nonce())? - 1;
    write_json(
        &directory.join(format!("{sender}.nonce.json")),
        &json!({
            "nonce": nonce, "tx_hash": record["tx_hash"],
        }),
    )?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

async fn broadcast_once(
    client: &JsonRpcClient,
    directory: &Path,
    sender: &AccountId,
    transaction: SignedTransaction,
    limit: Duration,
) -> Result<CryptoHash> {
    let pending = directory.join(format!("{sender}.pending.json"));
    let hash = transaction.get_hash();
    let record = json!({"tx_hash":hash,"signer_id":sender,
        "nonce":transaction.transaction.nonce().nonce(), "receiver_id":transaction.transaction.receiver_id(),
        "signed_transaction":transaction});
    // Private signed bytes permit inspection after process death, with no private key.
    write_json(&directory.join(format!("{hash}.json")), &record)?;
    write_json(&pending, &record)?;
    File::open(directory)?.sync_all()?;
    let broadcast = methods::broadcast_tx_async::RpcBroadcastTxAsyncRequest {
        signed_transaction: transaction,
    };
    // A transport error may mean acceptance. Broadcast exactly once, then read only.
    let _response = tokio::time::timeout(RPC_LIMIT, client.call(broadcast)).await;
    poll(client, sender, hash, false, limit).await?;
    // INCLUDED can precede the nonce becoming visible in a subsequent RPC state
    // query. Persist the high-water mark before unlocking for another process.
    record_included_nonce(directory, sender, &record)?;
    fs::remove_file(pending)?;
    File::open(directory)?.sync_all()?;
    Ok(hash)
}

impl Localnet {
    /// The inclusion barrier belongs to the signer, across every adapter process.
    /// A crash after persistence or a lost broadcast response keeps that barrier;
    /// unknown transactions are never replaced by a newly signed nonce.
    pub(super) async fn submit(
        &self,
        actor: &Account,
        target: &AccountId,
        actions: Vec<Action>,
    ) -> Result<CryptoHash> {
        let directory = self.data.join("transactions");
        let sender = actor.id().clone();
        let lock_directory = directory.clone();
        let lock_sender = sender.clone();
        let _lock = tokio::task::spawn_blocking(move || lock_signer(&lock_directory, &lock_sender))
            .await??;
        let client = JsonRpcClient::connect(&self.rpc);
        if let Some(hash) = reconcile_barrier(&client, &directory, &sender, POLL_LIMIT).await? {
            bail!(
                "previous transaction {hash} for {sender} is now included; reconcile its result before submitting another operation"
            );
        }
        let key: near_crypto::SecretKey = actor.secret_key().to_string().parse()?;
        let signer = InMemorySigner::from_secret_key(sender.clone(), key);
        // Transport nonce must use latest state; financial evidence uses FINAL.
        let access = tokio::time::timeout(
            RPC_LIMIT,
            client.call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::Finality(near_primitives::types::Finality::None),
                request: QueryRequest::ViewAccessKey {
                    account_id: sender.clone(),
                    public_key: signer.public_key(),
                },
            }),
        )
        .await
        .context("access-key query timed out before signing")??;
        let access_json = serde_json::to_value(&access)?;
        let observed = access_json["nonce"]
            .as_u64()
            .context("access key nonce missing")?;
        let nonce = next_nonce(&directory, &sender, observed)?;
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: sender.clone(),
            public_key: signer.public_key(),
            nonce,
            receiver_id: target.clone(),
            block_hash: access.block_hash,
            actions,
        })
        .sign(&signer);
        broadcast_once(&client, &directory, &sender, transaction, POLL_LIMIT).await
    }

    pub(super) async fn transact(
        &self,
        actor: &Account,
        target: &AccountId,
        actions: Vec<Action>,
    ) -> Result<FinalExecutionOutcomeView> {
        let hash = self.submit(actor, target, actions).await?;
        let response = poll(
            &JsonRpcClient::connect(&self.rpc),
            actor.id(),
            hash,
            true,
            POLL_LIMIT,
        )
        .await?;
        let outcome = response
            .final_execution_outcome
            .context("finalized transaction outcome absent")?
            .into_outcome();
        // Check execution failure, retaining raw bytes for callers that inspect logs.
        if let FinalExecutionStatus::Failure(error) = &outcome.status {
            bail!("transaction {hash} failed: {error:?}");
        }
        Ok(outcome)
    }

    pub(super) async fn call_outcome(
        &self,
        actor: &Account,
        target: &AccountId,
        method: &str,
        args: Value,
        deposit: NearToken,
    ) -> Result<FinalExecutionOutcomeView> {
        self.transact(actor, target, vec![function_call(method, args, deposit)?])
            .await
            .with_context(|| format!("{target}.{method}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    struct MockRpc {
        url: String,
        requests: Arc<Mutex<Vec<Value>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl MockRpc {
        fn new(included: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let thread = std::thread::spawn(move || {
                while !stopped.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        if line == "\r\n" {
                            break;
                        }
                        if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    let mut bytes = vec![0; length];
                    if reader.read_exact(&mut bytes).is_err() {
                        continue;
                    }
                    let request: Value = serde_json::from_slice(&bytes).unwrap();
                    captured.lock().unwrap().push(request.clone());
                    if request["method"] == "broadcast_tx_async" {
                        // Simulate acceptance followed by a lost HTTP response.
                        continue;
                    }
                    assert_eq!(request["method"], "tx");
                    assert_eq!(request["params"]["wait_until"], "NONE");
                    let body = json!({"jsonrpc":"2.0","id":request["id"],"result":{
                        "final_execution_status":if included {"INCLUDED"} else {"NONE"}
                    }})
                    .to_string();
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                }
            });
            Self {
                url,
                requests,
                stop,
                thread: Some(thread),
            }
        }
    }
    impl Drop for MockRpc {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.take().unwrap().join().unwrap();
        }
    }

    fn fixture(label: &str) -> (PathBuf, AccountId, SignedTransaction) {
        let directory =
            std::env::temp_dir().join(format!("apodect-near-{}-{label}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let sender: AccountId = "agent.test.near".parse().unwrap();
        let signer = InMemorySigner::from_secret_key(
            sender.clone(),
            near_crypto::SecretKey::from_random(near_crypto::KeyType::ED25519),
        );
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: sender.clone(),
            public_key: signer.public_key(),
            nonce: 1,
            receiver_id: "vault.test.near".parse().unwrap(),
            block_hash: CryptoHash::default(),
            actions: vec![
                function_call("fund", json!({"amount":"7"}), NearToken::from_yoctonear(1)).unwrap(),
            ],
        })
        .sign(&signer);
        (directory, sender, transaction)
    }

    #[tokio::test]
    async fn lost_broadcast_response_recovers_same_hash_without_resending() {
        let rpc = MockRpc::new(true);
        let (directory, sender, transaction) = fixture("lost-response");
        let expected = transaction.get_hash();
        let actual = broadcast_once(
            &JsonRpcClient::connect(&rpc.url),
            &directory,
            &sender,
            transaction,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(actual, expected);
        let requests = rpc.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "broadcast_tx_async")
                .count(),
            1
        );
        assert_eq!(
            requests.last().unwrap()["params"]["tx_hash"],
            expected.to_string()
        );
        assert!(!directory.join(format!("{sender}.pending.json")).exists());
        // A new process may still observe the pre-inclusion nonce from RPC.
        assert_eq!(next_nonce(&directory, &sender, 0).unwrap(), 2);
        assert_eq!(next_nonce(&directory, &sender, 20).unwrap(), 21);
        assert_eq!(
            fs::metadata(directory.join(format!("{expected}.json")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn unknown_inclusion_survives_restart_as_read_only_signer_barrier() {
        let rpc = MockRpc::new(false);
        let (directory, sender, transaction) = fixture("unknown");
        let expected = transaction.get_hash();
        let client = JsonRpcClient::connect(&rpc.url);
        let error = broadcast_once(
            &client,
            &directory,
            &sender,
            transaction,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(&expected.to_string()));
        assert!(directory.join(format!("{sender}.pending.json")).exists());
        assert!(
            reconcile_barrier(&client, &directory, &sender, Duration::from_millis(50))
                .await
                .is_err()
        );
        let requests = rpc.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "broadcast_tx_async")
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn independent_handles_serialize_the_same_signer() {
        let (directory, sender, _) = fixture("lock");
        let first = lock_signer(&directory, &sender).unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let second_directory = directory.clone();
        let second_sender = sender.clone();
        let thread = std::thread::spawn(move || {
            let _second = lock_signer(&second_directory, &second_sender).unwrap();
            sent.send(()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn inclusion_alone_does_not_satisfy_finalized_receipt_barrier() {
        let rpc = MockRpc::new(true);
        let (directory, sender, transaction) = fixture("not-final");
        let error = poll(
            &JsonRpcClient::connect(&rpc.url),
            &sender,
            transaction.get_hash(),
            true,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("unknown finalized outcome"));
        assert!(
            rpc.requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request["method"] == "tx")
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
