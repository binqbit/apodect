use super::*;
use actix_web::{App, HttpResponse, HttpServer, web};
use base64::{Engine, engine::general_purpose::STANDARD};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

use crate::{config::Config, storage::Store};

#[derive(Default)]
struct RpcState {
    paused: AtomicBool,
    finalized: AtomicBool,
    command_action: AtomicU8,
    outcome_reads: AtomicUsize,
    preparations: AtomicUsize,
    ethereum_timestamp: AtomicU64,
    ethereum_outcome_reads: AtomicUsize,
    broadcasts: Mutex<Vec<String>>,
}

fn command(action: u8) -> String {
    let mut bytes = [0; 384];
    bytes[95] = action;
    bytes[96..128].fill(1);
    bytes[128..160].fill(2);
    bytes[351] = 100;
    hex::encode(bytes)
}

fn rpc_result(request: &Value, state: &RpcState, genesis: &Value) -> Value {
    let block = near_primitives::hash::CryptoHash::default().to_string();
    let result = match request["method"].as_str().unwrap() {
        "status" => json!({"chain_id": "test-near"}),
        "genesis_config" => genesis.clone(),
        "block" => json!({"header":{"height":5,"hash":block}}),
        "query" if request["params"]["request_type"] == "view_access_key" => {
            state.preparations.fetch_add(1, Ordering::SeqCst);
            json!({"nonce":0,"block_hash":block})
        }
        "query" => {
            assert_eq!(request["params"]["block_id"], block);
            let value = match request["params"]["method_name"].as_str().unwrap() {
                "get_plan" => json!({
                    "plan":"02".repeat(32),"destination":"00".repeat(32),
                    "principal":1,"local_amount":1,"expires":100,"status":"Reserving",
                    "reserves":[],"routes":[],"allocation":null,"invalidations":[],
                    "closures":[],"attempts":3,"requires_attention":state.paused.load(Ordering::SeqCst),
                    "error":null,"queue_released":false
                }),
                "get_command" => json!({
                    "operation":"01".repeat(32),"domain":"00".repeat(32),
                    "command":command(state.command_action.load(Ordering::SeqCst)),
                    "digest":"00".repeat(32),"action":1,"plan":"02".repeat(32),
                    "signature":null,"signing":false,"signature_error":null,"state":0,"result":"00".repeat(32)
                }),
                method => panic!("unexpected contract view {method}"),
            };
            json!({"block_hash":block,"block_height":5,"result":serde_json::to_vec(&value).unwrap()})
        }
        "tx" => {
            state.outcome_reads.fetch_add(1, Ordering::SeqCst);
            if !state.finalized.load(Ordering::SeqCst) {
                return json!({"jsonrpc":"2.0","id":request["id"],"error":{
                    "code":-32000,"message":"Unknown transaction","cause":{"name":"UNKNOWN_TRANSACTION"}
                }});
            }
            let hash = &request["params"]["tx_hash"];
            json!({
                "final_execution_status":"FINAL","status":{"SuccessValue":STANDARD.encode(b"true")},
                "transaction":{
                    "signer_id":"relay.near","public_key":"ed25519:11111111111111111111111111111111",
                    "nonce":1,"receiver_id":"vault.near","actions":[],
                    "signature":format!("ed25519:{}","1".repeat(64)),"hash":hash
                },
                "transaction_outcome":{
                    "proof":[],"block_hash":block,"id":hash,
                    "outcome":{"logs":[],"receipt_ids":[],"gas_burnt":0,"tokens_burnt":"0",
                        "executor_id":"vault.near","status":{"SuccessValue":STANDARD.encode(b"true")}}
                },
                "receipts_outcome":[]
            })
        }
        "send_tx" => {
            state.broadcasts.lock().unwrap().push(
                request["params"]["signed_tx_base64"]
                    .as_str()
                    .unwrap()
                    .into(),
            );
            json!({})
        }
        "eth_chainId" => json!("0x7a69"),
        "eth_getBlockByNumber" => json!({
            "hash":B256::ZERO,"number":"0x5","baseFeePerGas":"0x1",
            "timestamp":format!("0x{:x}",state.ethereum_timestamp.load(Ordering::SeqCst))
        }),
        "eth_getTransactionCount" => json!("0x7"),
        "eth_estimateGas" => {
            state.preparations.fetch_add(1, Ordering::SeqCst);
            json!("0x186a0")
        }
        "eth_maxPriorityFeePerGas" => json!("0x1"),
        "eth_getTransactionReceipt" => {
            state.ethereum_outcome_reads.fetch_add(1, Ordering::SeqCst);
            Value::Null
        }
        "eth_sendRawTransaction" => {
            let raw = request["params"][0].as_str().unwrap();
            state.broadcasts.lock().unwrap().push(raw.into());
            json!(alloy_primitives::keccak256(
                hex::decode(raw.trim_start_matches("0x")).unwrap()
            ))
        }
        method => panic!("unexpected RPC method {method}"),
    };
    json!({"jsonrpc":"2.0","id":request["id"],"result":result})
}

/// A restart must respect the current pause without losing receipts for earlier submissions.
#[actix_web::test]
#[ignore = "requires isolated PostgreSQL via APODECT_TEST_DATABASE_URL"]
async fn paused_recovery_preserves_signed_bytes_and_still_observes_receipts() -> Result<()> {
    let url = std::env::var("APODECT_TEST_DATABASE_URL")?;
    let options: PgConnectOptions = url.parse()?;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await?;
    let schema = format!("relay_pause_{}", Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await?;
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options.options([("search_path", schema.as_str())]))
        .await?;
    let store = Store::new(pool.clone());
    store.migrate().await?;

    let state = Arc::new(RpcState::default());
    state.paused.store(true, Ordering::SeqCst);
    state
        .command_action
        .store(action::RESERVE, Ordering::SeqCst);
    let genesis = json!({"chain_id":"test-near","genesis_time":"2026-01-01T00:00:00Z","genesis_height":0,"validators":[]});
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let rpc_url = format!("http://{}", listener.local_addr()?);
    let server_state = state.clone();
    let server_genesis = genesis.clone();
    let server = HttpServer::new(move || {
        let state = server_state.clone();
        let genesis = server_genesis.clone();
        App::new().route(
            "/",
            web::post().to(move |request: web::Json<Value>| {
                let response = rpc_result(&request, &state, &genesis);
                async move { HttpResponse::Ok().json(response) }
            }),
        )
    })
    .workers(1)
    .listen(listener)?
    .run();
    let handle = server.handle();
    let running = actix_web::rt::spawn(server);

    let result = exercise_paused_recovery(store, &state, &rpc_url, genesis).await;

    handle.stop(true).await;
    running.await??;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await?;
    admin.close().await;
    result
}

async fn exercise_paused_recovery(
    store: Store,
    state: &RpcState,
    rpc_url: &str,
    genesis: Value,
) -> Result<()> {
    let config: Config = serde_json::from_value(json!({
        "ethereum":{"rpc_url":rpc_url,"chain_id":31337,"genesis_hash":"00".repeat(32)},
        "solana":{"rpc_url":rpc_url,"genesis_hash":"local"},
        "near":{"rpc_url":rpc_url,"genesis":genesis},
        "liquidity":{"url":rpc_url,"local_simulator":true,"api_key_file":null},
        "projects":[{"coordinator":"coordinator.near","project_id":"01".repeat(32),"ethereum_start_block":0,
            "domains":{
                "near":{"domain":"00".repeat(32),"vault":"vault.near","asset":"usdc.near","asset_id":"usdc"},
                "ethereum":{"domain":"00".repeat(32),"vault":"0x1111111111111111111111111111111111111111","asset":"0x2222222222222222222222222222222222222222","asset_id":"usdc"}
            }}]
    }))?;
    let project = config.projects[0].clone();
    let key = config.project_key(&project);
    store
        .register_project(&key, &serde_json::to_value(&project)?)
        .await?;
    let rpc = Rpc::new(rpc_url)?;
    let near = near::NearClient::connect(rpc.clone(), &config.near.genesis).await?;
    let secret = near_crypto::SecretKey::from_seed(near_crypto::KeyType::ED25519, "relay-test");
    let key_file = std::env::temp_dir().join(format!("relay-test-{}.json", Uuid::new_v4()));
    std::fs::write(
        &key_file,
        serde_json::to_vec(
            &json!({"account_id":"relay.near","public_key":secret.public_key(),"private_key":secret}),
        )?,
    )?;
    let signer = near::RelaySigner::from_file(&key_file, "relay.near");
    std::fs::remove_file(key_file)?;
    let lock = store.pool.acquire().await?;
    let mut relay = Relay {
        indexer: Indexer {
            config,
            store,
            near,
            ethereum: ethereum::Ethereum::new(rpc, 31337, B256::ZERO),
        },
        ethereum_signer: PrivateKeySigner::random(),
        near_signer: signer?,
        solana: BTreeMap::new(),
        lock,
    };
    let intent = Intent::Execute {
        command: command(action::RESERVE),
        signature: "00".repeat(64),
        recovery: 0,
        recipient: None,
    };

    // A planned effect is recovered without preparing bytes while the plan is paused.
    relay
        .indexer
        .store
        .plan_effect(
            &key,
            "planned",
            "near",
            &relay.indexer.config.network(Chain::Near),
            &serde_json::to_value(&intent)?,
        )
        .await?;
    relay.recover(&project).await?;
    assert_eq!(state.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(state.outcome_reads.load(Ordering::SeqCst), 0);
    assert!(
        relay
            .indexer
            .store
            .effect(&key, "planned")
            .await?
            .unwrap()
            .transaction
            .is_none()
    );

    // Simulate bytes persisted before the process stopped and the plan reached attention.
    let transaction = relay
        .prepare(&project, Chain::Near, &intent, "planned")
        .await?;
    relay
        .indexer
        .store
        .prepare_effect(&key, "planned", &transaction)
        .await?;
    relay.recover(&project).await?;
    assert_eq!(state.outcome_reads.load(Ordering::SeqCst), 1);
    assert!(state.broadcasts.lock().unwrap().is_empty());
    assert_eq!(
        relay
            .indexer
            .store
            .effect(&key, "planned")
            .await?
            .unwrap()
            .transaction,
        Some(transaction.clone())
    );

    let signature = Intent::Coordinator {
        method: "request_signature".into(),
        args: json!({"operation":"01".repeat(32)}),
    };
    assert!(!relay.submission_allowed(&project, &signature).await?);
    state
        .command_action
        .store(action::CLOSE_PLAN, Ordering::SeqCst);
    assert!(relay.submission_allowed(&project, &signature).await?);
    assert!(
        relay
            .submission_allowed(
                &project,
                &Intent::Execute {
                    command: command(action::CLOSE_PLAN),
                    signature: String::new(),
                    recovery: 0,
                    recipient: None
                }
            )
            .await?
    );
    assert!(
        relay
            .submission_allowed(
                &project,
                &Intent::Coordinator {
                    method: "advance".into(),
                    args: json!({})
                }
            )
            .await?
    );

    // Resumption sends precisely the saved transaction, without reserving another nonce.
    state.paused.store(false, Ordering::SeqCst);
    relay.recover(&project).await?;
    assert_eq!(state.preparations.load(Ordering::SeqCst), 1);
    assert_eq!(
        *state.broadcasts.lock().unwrap(),
        vec![transaction["signed_tx_base64"].as_str().unwrap()]
    );

    // A receipt arriving after another pause still updates the durable outcome.
    state.paused.store(true, Ordering::SeqCst);
    state.finalized.store(true, Ordering::SeqCst);
    relay.recover(&project).await?;
    let effect = relay.indexer.store.effect(&key, "planned").await?.unwrap();
    assert_eq!(effect.state, "confirmed");
    assert_eq!(effect.result.unwrap()["value"], true);
    assert_eq!(state.outcome_reads.load(Ordering::SeqCst), 3);
    assert_eq!(state.broadcasts.lock().unwrap().len(), 1);

    // A persisted Ethereum nonce may block cleanup. Its exact transaction may
    // resume after the finalized chain clock proves the command cannot execute.
    relay
        .indexer
        .store
        .plan_effect(
            &key,
            "expired-ethereum",
            "ethereum",
            &relay.indexer.config.network(Chain::Ethereum),
            &serde_json::to_value(&intent)?,
        )
        .await?;
    let ethereum_transaction = relay
        .prepare(&project, Chain::Ethereum, &intent, "expired-ethereum")
        .await?;
    relay
        .indexer
        .store
        .prepare_effect(&key, "expired-ethereum", &ethereum_transaction)
        .await?;
    for timestamp in [99, 100] {
        state.ethereum_timestamp.store(timestamp, Ordering::SeqCst);
        relay.recover(&project).await?;
        assert_eq!(state.broadcasts.lock().unwrap().len(), 1);
    }
    state.ethereum_timestamp.store(101, Ordering::SeqCst);
    relay.recover(&project).await?;
    assert_eq!(state.ethereum_outcome_reads.load(Ordering::SeqCst), 3);
    assert_eq!(state.broadcasts.lock().unwrap().len(), 2);
    assert_eq!(
        state.broadcasts.lock().unwrap()[1],
        ethereum_transaction["raw"].as_str().unwrap()
    );
    assert_eq!(state.preparations.load(Ordering::SeqCst), 2);
    assert_eq!(
        relay
            .indexer
            .store
            .effect(&key, "expired-ethereum")
            .await?
            .unwrap()
            .transaction,
        Some(ethereum_transaction)
    );

    // Expiry never permits preparing a new transaction for a paused plan.
    relay
        .effect(&project, "new-paused-ethereum", Chain::Ethereum, intent)
        .await?;
    assert!(
        relay
            .indexer
            .store
            .effect(&key, "new-paused-ethereum")
            .await?
            .unwrap()
            .transaction
            .is_none()
    );
    assert_eq!(state.preparations.load(Ordering::SeqCst), 2);
    Ok::<_, anyhow::Error>(())
}
