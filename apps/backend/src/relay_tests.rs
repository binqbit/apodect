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

/// A foreign RPC outage must not block confirmed settlement on a healthy chain.
#[actix_web::test]
#[ignore = "requires isolated PostgreSQL via APODECT_TEST_DATABASE_URL"]
async fn unavailable_snapshots_do_not_block_other_chain_observations() -> Result<()> {
    let options: PgConnectOptions = std::env::var("APODECT_TEST_DATABASE_URL")?.parse()?;
    let admin = PgPoolOptions::new().connect_with(options.clone()).await?;
    let schema = format!("relay_outage_{}", Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await?;
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options.options([("search_path", schema.as_str())]))
        .await?;
    let store = Store::new(pool.clone());
    store.migrate().await?;

    let state = Arc::new(Mutex::new(OutageRpc::default()));
    let server_state = state.clone();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let rpc_url = format!("http://{}", listener.local_addr()?);
    let server = HttpServer::new(move || {
        let state = server_state.clone();
        App::new().route(
            "/",
            web::post().to(move |request: web::Json<Value>| {
                let response = state.lock().unwrap().respond(&request);
                async move { HttpResponse::Ok().json(response) }
            }),
        )
    })
    .workers(1)
    .listen(listener)?
    .run();
    let handle = server.handle();
    let running = actix_web::rt::spawn(server);
    let result = exercise_outage_reconciliation(store, &state, &rpc_url).await;
    handle.stop(true).await;
    running.await??;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await?;
    admin.close().await;
    result
}

#[derive(Default)]
struct OutageRpc {
    height: u8,
    observed: bool,
    advances: usize,
    checkpoints: BTreeMap<String, String>,
    transactions: BTreeMap<String, (u64, String)>,
    unavailable_reads: usize,
}

impl OutageRpc {
    fn genesis() -> Value {
        json!({"chain_id":"test-near","genesis_time":"2026-01-01T00:00:00Z","genesis_height":0,"validators":[]})
    }

    fn respond(&mut self, request: &Value) -> Value {
        let params = &request["params"];
        let block = bs58::encode([self.height; 32]).into_string();
        let result = match request["method"].as_str().unwrap() {
            "eth_getBlockByNumber" | "getAccountInfo" => {
                self.unavailable_reads += 1;
                return json!({"jsonrpc":"2.0","id":request["id"],"error":{
                    "code":-32000,"message":"Native RPC temporarily unavailable"
                }});
            }
            "status" => json!({"chain_id":"test-near"}),
            "genesis_config" => Self::genesis(),
            "block" => json!({"header":{"height":self.height,"hash":block}}),
            "query" if params["request_type"] == "view_access_key" => {
                json!({"nonce":self.transactions.len(),"block_hash":block})
            }
            "query" => {
                let args: Value = serde_json::from_slice(
                    &STANDARD
                        .decode(params["args_base64"].as_str().unwrap())
                        .unwrap(),
                )
                .unwrap();
                let value = match params["method_name"].as_str().unwrap() {
                    "get_state" => json!({
                        "owner_id":"owner.near","authority_id":"manager.near","project_id":"01".repeat(32),
                        "mpc_id":"mpc.near","sign_key":"","read_key":"","sign_path":"project",
                        "queue_head":0,"queue_length":0,"next_plan":2,"next_command":1,"next_invoice":1,"retry_limit":3
                    }),
                    "get_plan" => json!({
                        "plan":crate::config::number_word(1),"destination":"03".repeat(32),
                        "principal":1,"local_amount":0,"expires":100,"status":"Aborting",
                        "reserves":[],"allocation":null,"invalidations":[],"closures":[],
                        "attempts":0,"requires_attention":false,"error":null,"queue_released":false,
                        "routes":[{"quote":{"route":"05".repeat(32),"source":"01".repeat(32),
                            "input":2,"min_input":2,"output":1,"deposit":"06".repeat(32),
                            "refund":"refund","recipient":"destination","quote_hash":"07".repeat(32),"expires":100},
                            "delivery":null,"refund":null,"dispatch":null,"received":0,"returned":0,"dispatched":0,
                            "unsettled_input":0,"confirmed_transport_expense":null}]
                    }),
                    "get_domain" => {
                        let chain = match args["domain"].as_str().unwrap() {
                            domain if domain == "01".repeat(32) => "ethereum",
                            domain if domain == "02".repeat(32) => "solana",
                            _ => "near",
                        };
                        let (vault, asset) = match chain {
                            "ethereum" => (
                                "0x1111111111111111111111111111111111111111",
                                "0x2222222222222222222222222222222222222222",
                            ),
                            "solana" => (
                                "11111111111111111111111111111111",
                                "11111111111111111111111111111111",
                            ),
                            _ => ("vault.near", "usdc.near"),
                        };
                        let free = if chain == "near" && self.observed {
                            10
                        } else {
                            0
                        };
                        json!({"domain":args["domain"],"chain":chain,"vault":vault,"asset":asset,
                            "program":(chain=="solana").then(||hex::encode(apodect_solana::ID.to_bytes())),
                            "owner_identity":"04".repeat(32),"owner_recipient":"04".repeat(32),
                            "sequence":u64::from(free>0),"free":free,"held":0,"pending_reserves":0,"available":free})
                    }
                    "get_queue_head" => Value::Null,
                    "get_project" => {
                        json!({"owner_id":"owner.near","authority_id":"manager.near","token_id":"usdc.near",
                        "free":"10","pending_owner_outflow":"0","next_task":0,"next_request":0,"next_invoice":0,"next_operation":0})
                    }
                    "get_bridge" => {
                        json!({"domain":"03".repeat(32),"key":null,"held":"0","accounted_custody":"10","sequence":1,"next_funding":1})
                    }
                    "get_checkpoint" => {
                        json!(self.checkpoints.get(args["observation"].as_str().unwrap()))
                    }
                    "get_route" => Value::Null,
                    method => panic!("Unexpected outage-test contract view {method}"),
                };
                let hash = params["block_id"].as_str().unwrap();
                let height = bs58::decode(hash).into_vec().unwrap()[0];
                json!({"block_hash":hash,"block_height":height,"result":serde_json::to_vec(&value).unwrap()})
            }
            "send_tx" => {
                let signed: near_primitives::transaction::SignedTransaction =
                    serde_json::from_value(params["signed_tx_base64"].clone()).unwrap();
                let near_primitives::action::Action::FunctionCall(call) =
                    &signed.transaction.actions()[0]
                else {
                    panic!("Expected a function call")
                };
                let args: Value = serde_json::from_slice(&call.args).unwrap();
                match call.method_name.as_str() {
                    "checkpoint" => {
                        let observation = args["observation"].as_str().unwrap();
                        let mut payload = [0; 1248];
                        payload[..32].fill(3);
                        payload[32..64].copy_from_slice(&word(observation).unwrap());
                        payload[95] = 1;
                        payload[191] = 10;
                        payload[287] = 255;
                        payload[703] = 255;
                        self.checkpoints
                            .insert(observation.into(), hex::encode(payload));
                    }
                    "observe_near" => {
                        assert!(
                            self.checkpoints
                                .contains_key(args["observation"].as_str().unwrap())
                        );
                        self.observed = true;
                    }
                    "advance" => self.advances += 1,
                    method => panic!("Unexpected outage-test relay method {method}"),
                }
                self.transactions.insert(
                    signed.get_hash().to_string(),
                    (
                        signed.transaction.nonce().nonce(),
                        signed.transaction.receiver_id().to_string(),
                    ),
                );
                self.height += 1;
                json!({})
            }
            "tx" => {
                let hash = params["tx_hash"].as_str().unwrap();
                let Some((nonce, receiver)) = self.transactions.get(hash) else {
                    return json!({"jsonrpc":"2.0","id":request["id"],"error":{
                        "code":-32000,"message":"Unknown transaction","cause":{"name":"UNKNOWN_TRANSACTION"}
                    }});
                };
                json!({"final_execution_status":"FINAL","status":{"SuccessValue":STANDARD.encode(b"true")},
                    "transaction":{"signer_id":"relay.near","public_key":"ed25519:11111111111111111111111111111111",
                        "nonce":nonce,"receiver_id":receiver,"actions":[],"signature":format!("ed25519:{}","1".repeat(64)),"hash":hash},
                    "transaction_outcome":{"proof":[],"block_hash":block,"id":hash,
                        "outcome":{"logs":[],"receipt_ids":[],"gas_burnt":0,"tokens_burnt":"0",
                            "executor_id":receiver,"status":{"SuccessValue":STANDARD.encode(b"true")}}},"receipts_outcome":[]})
            }
            method => panic!("Unexpected outage-test RPC method {method}"),
        };
        json!({"jsonrpc":"2.0","id":request["id"],"result":result})
    }
}

async fn exercise_outage_reconciliation(
    store: Store,
    state: &Mutex<OutageRpc>,
    rpc_url: &str,
) -> Result<()> {
    let config: Config = serde_json::from_value(json!({
        "ethereum":{"rpc_url":rpc_url,"chain_id":31337,"genesis_hash":"00".repeat(32)},
        "solana":{"rpc_url":rpc_url,"genesis_hash":"local"},
        "near":{"rpc_url":rpc_url,"genesis":OutageRpc::genesis()},
        "liquidity":{"url":rpc_url,"local_simulator":true,"api_key_file":null},
        "projects":[{"coordinator":"coordinator.near","project_id":"01".repeat(32),"ethereum_start_block":0,
            "domains":{
                "ethereum":{"domain":"01".repeat(32),"vault":"0x1111111111111111111111111111111111111111","asset":"0x2222222222222222222222222222222222222222","asset_id":"usdc"},
                "solana":{"domain":"02".repeat(32),"vault":"11111111111111111111111111111111","asset":"11111111111111111111111111111111","asset_id":"usdc"},
                "near":{"domain":"03".repeat(32),"vault":"vault.near","asset":"usdc.near","asset_id":"usdc"}
            }}]
    }))?;
    let project = config.projects[0].clone();
    let key = config.project_key(&project);
    store
        .register_project(&key, &serde_json::to_value(&project)?)
        .await?;
    let rpc = Rpc::new(rpc_url)?;
    let near = near::NearClient::connect(rpc.clone(), &config.near.genesis).await?;
    let secret =
        near_crypto::SecretKey::from_seed(near_crypto::KeyType::ED25519, "relay-outage-test");
    let key_file = std::env::temp_dir().join(format!("relay-outage-test-{}.json", Uuid::new_v4()));
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
    for _ in 0..3 {
        assert!(
            relay.reconcile_project(&project).await.is_err(),
            "The outage must remain visible"
        );
    }
    ensure!(
        state.lock().unwrap().observed,
        "A foreign snapshot outage blocked a confirmed NEAR observation"
    );
    ensure!(
        state.lock().unwrap().advances == 1,
        "A source route outage blocked coordinator settlement cleanup"
    );
    let domains: Value = sqlx::query_scalar("SELECT domains FROM project_views WHERE project=$1")
        .bind(&key)
        .fetch_one(&relay.indexer.store.pool)
        .await?;
    assert_eq!(domains["near"]["free"], 10);
    assert_eq!(domains["ethereum"]["free"], 0);
    assert_eq!(domains["solana"]["free"], 0);
    let snapshots: Vec<String> =
        sqlx::query_scalar("SELECT chain FROM vault_views WHERE project=$1")
            .bind(&key)
            .fetch_all(&relay.indexer.store.pool)
            .await?;
    assert_eq!(snapshots, ["near"]);
    let effects: Vec<String> = sqlx::query_scalar("SELECT state FROM effects WHERE project=$1")
        .bind(&key)
        .fetch_all(&relay.indexer.store.pool)
        .await?;
    assert_eq!(
        effects.len(),
        3,
        "The checkpoint, observation and cleanup retain their original transactions"
    );
    assert!(effects.iter().all(|state| state == "confirmed"));
    assert_eq!(state.lock().unwrap().unavailable_reads, 9);
    Ok(())
}
