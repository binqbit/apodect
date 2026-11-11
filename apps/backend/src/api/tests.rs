use super::auth::SESSION_COOKIE;
use super::*;
use crate::{
    accounts::{Accounts, AuthConfig},
    chains::{
        ethereum::Ethereum,
        near::{GenesisIdentity, NearClient},
    },
    config::{
        Config, DomainConfig, EthereumConfig, LiquidityConfig, NearConfig, SolanaConfig,
        number_word,
    },
    indexing::Indexer,
    liquidity::Liquidity,
    storage::Store,
};
use actix_web::cookie::{Cookie, SameSite};
use actix_web::{App, HttpServer, test};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    collections::BTreeMap,
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

/// Exercise the actual HTTP boundary with verified signatures and PostgreSQL:
/// CSRF rejection, cookie isolation, finalized role/read models and logout replay.
#[actix_web::test]
#[ignore = "requires isolated PostgreSQL via APODECT_TEST_DATABASE_URL"]
async fn browser_identity_and_finalized_project_flow() -> Result<()> {
    let url = std::env::var("APODECT_TEST_DATABASE_URL")?;
    let admin = PgPool::connect(&url).await?;
    let schema = format!("api_test_{}", Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await?;
    let options: PgConnectOptions = url.parse()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options.options([("search_path", schema.as_str())]))
        .await?;
    sqlx::raw_sql(include_str!("../../migrations/0001_initial.sql"))
        .execute(&pool)
        .await?;
    let genesis = GenesisIdentity {
        chain_id: "test-near".into(),
        genesis_time: "2026-01-01T00:00:00Z".into(),
        genesis_height: 0,
        validators: json!([]),
    };
    let expected_genesis = genesis.clone();
    let manager_active = Arc::new(AtomicBool::new(true));
    let manager = manager_active.clone();
    let key_active = Arc::new(AtomicBool::new(true));
    let access = key_active.clone();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let rpc_url = format!("http://{}", listener.local_addr()?);
    let server = HttpServer::new(move || {
        let genesis = expected_genesis.clone();
        let manager = manager.clone();
        let access = access.clone();
        App::new().route(
            "/",
            web::post().to(move |input: web::Json<Value>| {
                let genesis = genesis.clone();
                let manager = manager.clone();
                let access = access.clone();
                async move {
                    let value = match input["method"].as_str().unwrap() {
                        "status" => json!({"chain_id":"test-near"}),
                        "genesis_config" => json!(genesis),
                        "block" => json!({"header":{"height":10,"hash":near_primitives::hash::CryptoHash::default().to_string()}}),
                        "query" => {
                            let hash=near_primitives::hash::CryptoHash::default().to_string();
                            if input["params"]["request_type"] == "view_access_key" {
                                json!({"block_hash":hash,"block_height":10,"permission":if access.load(Ordering::SeqCst) { json!("FullAccess") } else { json!({"FunctionCall":{}}) }})
                            } else {
                                let state=json!({
                                    "owner_id": "owner.near",
                                    "authority_id": if manager.load(Ordering::SeqCst){"authority.near"}else{"replacement.near"},
                                    "project_id": number_word(10),
                                    "mpc_id": "mpc.near",
                                    "sign_key": "",
                                    "read_key": "",
                                    "sign_path": "",
                                    "queue_head": 0,
                                    "queue_length": 1,
                                    "next_plan": 1,
                                    "next_command": 1,
                                    "next_invoice": 2,
                                    "retry_limit": 3,
                                });
                                json!({"block_hash":hash,"block_height":10,"result":serde_json::to_vec(&state).unwrap()})
                            }
                        },
                        _ => panic!("Unexpected RPC in persisted-view/auth test"),
                    };
                    HttpResponse::Ok()
                        .json(json!({"jsonrpc":"2.0","id":input["id"],"result":value}))
                }
            }),
        )
    })
    .listen(listener)?
    .run();
    let server_handle = server.handle();
    let running = actix_web::rt::spawn(server);
    let rpc = Rpc::new(&rpc_url)?;
    let near = NearClient::connect(rpc.clone(), &genesis).await?;
    let signing_key = SigningKey::from_bytes(&[81; 32]);
    let address = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
    let project = ProjectConfig {
        coordinator: "coordinator.near".into(),
        project_id: number_word(10),
        ethereum_start_block: 0,
        domains: Chain::ALL
            .into_iter()
            .enumerate()
            .map(|(i, chain)| {
                (
                    chain,
                    DomainConfig {
                        domain: number_word(i as u64 + 1),
                        vault: format!("vault-{chain}"),
                        asset: format!("usdc-{chain}"),
                        asset_id: format!("usdc:{chain}"),
                    },
                )
            })
            .collect(),
    };
    let config = Config {
        ethereum: EthereumConfig {
            rpc_url: rpc_url.clone(),
            chain_id: 31337,
            genesis_hash: format!("0x{}", "11".repeat(32)),
        },
        solana: SolanaConfig {
            rpc_url: rpc_url.clone(),
            genesis_hash: bs58::encode([3; 32]).into_string(),
        },
        near: NearConfig {
            rpc_url: rpc_url.clone(),
            genesis,
        },
        projects: vec![project.clone()],
        liquidity: LiquidityConfig {
            url: rpc_url.clone(),
            local_simulator: true,
            api_key_file: None,
        },
    };
    let accounts = Accounts::new(
        pool.clone(),
        AuthConfig {
            origin: "https://app.example.com".into(),
            ethereum_chain_id: 31337,
            ethereum_genesis_hash: config.ethereum.genesis_hash.clone(),
            solana_cluster: "localnet".into(),
            solana_genesis_hash: config.solana.genesis_hash.clone(),
            near_network: config.network(Chain::Near),
        },
        rpc.clone(),
        near.clone(),
    )?;
    let key = config.project_key(&project);
    let store = Store::new(pool.clone());
    store
        .register_project(&key, &serde_json::to_value(&project)?)
        .await?;
    let ethereum = Ethereum::new(rpc, 31337, config.ethereum.genesis_hash.parse()?);
    let liquidity = Liquidity::new(&config.liquidity)?;
    let application = web::Data::new(Application {
        indexer: Arc::new(Indexer {
            config,
            store,
            near,
            ethereum,
        }),
        accounts,
        liquidity,
    });
    let app = test::init_service(
        App::new()
            .app_data(application.clone())
            .app_data(web::Data::new(BrowserConfig {
                origin: "https://app.example.com".into(),
                secure: true,
            }))
            .configure(configure),
    )
    .await;

    let request = test::TestRequest::post()
        .uri("/api/auth/challenge")
        .insert_header((header::ORIGIN, "https://evil.example.com"))
        .set_json(json!({"chain":"solana","address":address}))
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::FORBIDDEN
    );
    let challenge_count: i64 = sqlx::query_scalar("SELECT count(*) FROM login_challenges")
        .fetch_one(&pool)
        .await?;
    assert_eq!(challenge_count, 0);
    let request = test::TestRequest::get().uri("/api/projects").to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let request = test::TestRequest::post()
        .uri("/api/auth/challenge")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(json!({"chain":"solana","address":address}))
        .to_request();
    let challenge: Value = test::call_and_read_body_json(&app, request).await;
    let signature = bs58::encode(
        signing_key
            .sign(challenge["message"].as_str().unwrap().as_bytes())
            .to_bytes(),
    )
    .into_string();
    let proof = json!({"challenge":challenge["id"],"signature":signature});
    let request = test::TestRequest::post()
        .uri("/api/auth/verify")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(&proof)
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .response()
        .cookies()
        .find(|cookie| cookie.name() == SESSION_COOKIE)
        .unwrap()
        .into_owned();
    assert_eq!(cookie.http_only(), Some(true));
    assert_eq!(cookie.secure(), Some(true));
    assert_eq!(cookie.same_site(), Some(SameSite::Strict));
    assert_eq!(cookie.path(), Some("/api"));
    let body: Value = test::read_body_json(response).await;
    assert!(body.get("session_token").is_none());
    assert!(!body.to_string().contains(cookie.value()));
    assert_eq!(body["wallets"][0]["address"], address);
    let request = test::TestRequest::post()
        .uri("/api/auth/verify")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(proof)
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let ethereum_key: PrivateKeySigner = "11".repeat(32).parse()?;
    let ethereum_address = ethereum_key.address().to_checksum(None);

    const LARGE: u64 = 9_007_199_254_740_993;
    let state = json!({
        "owner_id": "owner.near",
        "authority_id": "authority.near",
        "project_id": project.project_id,
        "mpc_id": "mpc.near",
        "sign_key": "",
        "read_key": "",
        "sign_path": "",
        "queue_head": 0,
        "queue_length": 1,
        "next_plan": 1,
        "next_command": 1,
        "next_invoice": 2,
        "retry_limit": 3,
    });
    let mut domains = BTreeMap::new();
    for chain in Chain::ALL {
        let binding = &project.domains[&chain];
        let identity = match chain {
            Chain::Solana => hex::encode(signing_key.verifying_key().to_bytes()),
            Chain::Ethereum => {
                let mut word = [0_u8; 32];
                word[12..].copy_from_slice(ethereum_key.address().as_slice());
                hex::encode(word)
            }
            Chain::Near => number_word(999),
        };
        domains.insert(
            chain,
            json!({
                "domain": binding.domain,
                "chain": chain,
                "vault": binding.vault,
                "asset": binding.asset,
                "program": null,
                "owner_identity": identity,
                "owner_recipient": identity,
                "sequence": 1,
                "free": LARGE,
                "held": 0,
                "pending_reserves": 0,
                "available": LARGE,
            }),
        );
    }
    let request_id = number_word(1);
    let task_id = number_word(2);
    let coordinator_request = json!({
        "domain": project.domains[&Chain::Solana].domain,
        "request": request_id,
        "task": task_id,
        "amount": LARGE,
        "kind": 0,
        "purpose": number_word(7),
        "local_state": 0,
        "status": "Approved",
        "plan": null,
        "approval_position": 0,
    });
    let requests = json!({"solana":[coordinator_request]});
    let invoices = json!({"1":{"amount":LARGE,"purpose":number_word(7),"intended":LARGE,"closed":true,"response_reference":null,"actual_funding":LARGE}});
    sqlx::query("INSERT INTO project_views(project,height,block_hash,state,domains,plans,commands,invoices,requests,queue_head) VALUES($1,10,'final-block',$2,$3,'[]','[]',$4,$5,$6)").bind(&key).bind(state).bind(json!(domains)).bind(invoices).bind(requests).bind(&coordinator_request).execute(&pool).await?;
    for chain in Chain::ALL {
        let tasks = if chain == Chain::Solana {
            json!([{"id":task_id,"agent":address,"state":0,"paused":false,"started":false,"available":LARGE,"issued":0,"recovered":0,"pending_outflow":0,"control_nonce":0,"live_request":request_id,"report":null}])
        } else if chain == Chain::Ethereum {
            json!([{
                "id": task_id, "agent": ethereum_address.to_lowercase(), "state": 0,
                "paused": false, "started": false, "available": LARGE,
                "issued": 0, "recovered": 0, "pending_outflow": 0,
                "control_nonce": 0, "live_request": null, "report": null,
            }])
        } else {
            json!([])
        };
        let requests = if chain == Chain::Solana {
            json!([{"id":request_id,"task":task_id,"amount":LARGE,"kind":0,"state":0,"purpose":number_word(7)}])
        } else {
            json!([])
        };
        let snapshot = json!({
            "chain": chain,
            "height": 10,
            "block_hash": "final-block",
            "state": {"free":LARGE.to_string()},
            "tasks": tasks,
            "requests": requests,
            "funding": [],
        });
        sqlx::query("INSERT INTO vault_views(project,chain,height,block_hash,state) VALUES($1,$2,10,'final-block',$3)").bind(&key).bind(chain.as_str()).bind(snapshot).execute(&pool).await?;
    }
    sqlx::query("INSERT INTO effects(project,id,chain,network,intent,transaction,state) VALUES($1,'test','solana','network','{}',$2,'submitted')").bind(&key).bind(json!({"signature":"public-transaction","bytes_base64":"SECRET_SIGNED_BYTES"})).execute(&pool).await?;
    let request = test::TestRequest::get()
        .uri(&format!("/api/projects/{key}"))
        .cookie(cookie.clone())
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let detail: Value = test::read_body_json(response).await;
    assert_eq!(detail["available"], (u128::from(LARGE) * 3).to_string());
    assert_eq!(detail["tasks"][0]["available"], LARGE.to_string());
    assert_eq!(detail["invoices"][0]["amount"], LARGE.to_string());
    assert_eq!(detail["roles"]["owner"], false);
    assert_eq!(detail["roles"]["authority"], false);
    assert_eq!(detail["roles"]["agent"], true);
    assert_eq!(detail["roles"]["local_owners"]["solana"], address);
    assert_eq!(detail["requests"][0]["queue_head"], true);
    assert_eq!(detail["requests"][0]["approval_position"], 0);
    assert_eq!(
        detail["operations"][0]["transaction_id"],
        "public-transaction"
    );
    assert!(!detail.to_string().contains("SECRET_SIGNED_BYTES"));
    assert_eq!(detail["tasks"][0]["chain"], "ethereum");
    assert_eq!(detail["tasks"][0]["mine"], false);

    // Ethereum authentication scopes an address by chain ID AND genesis. Its
    // wallet identity must not be compared with the worker's genesis-only key.
    let request = test::TestRequest::post()
        .uri("/api/auth/challenge")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(json!({ "chain": "ethereum", "address": ethereum_address }))
        .to_request();
    let challenge: Value = test::call_and_read_body_json(&app, request).await;
    let signature =
        ethereum_key.sign_message_sync(challenge["message"].as_str().unwrap().as_bytes())?;
    let request = test::TestRequest::post()
        .uri("/api/auth/verify")
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(json!({ "challenge": challenge["id"], "signature": signature.to_string() }))
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let ethereum_cookie = response
        .response()
        .cookies()
        .find(|cookie| cookie.name() == SESSION_COOKIE)
        .unwrap()
        .into_owned();
    let request = test::TestRequest::get()
        .uri(&format!("/api/projects/{key}"))
        .cookie(ethereum_cookie)
        .to_request();
    let ethereum_detail: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(
        ethereum_detail["roles"]["local_owners"]["ethereum"],
        ethereum_address
    );
    assert_eq!(ethereum_detail["roles"]["agent"], true);
    assert_eq!(ethereum_detail["roles"]["owner"], false);
    assert_eq!(ethereum_detail["tasks"][0]["mine"], true);

    let request=test::TestRequest::post().uri(&format!("/api/projects/{key}/actions/coordinator")).cookie(cookie.clone()).insert_header((header::ORIGIN,"https://app.example.com")).set_json(json!({"actor":"owner.near","action":{"action":"set_authority","account":"attacker.near"},"storage_deposit":"0"})).to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let request=test::TestRequest::post().uri("/api/rpc/near").cookie(cookie.clone()).insert_header((header::ORIGIN,"https://app.example.com")).set_json(json!({"jsonrpc":"2.0","id":1,"method":"query","params":{"request_type":"call_function"}})).to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let request = test::TestRequest::patch()
        .uri("/api/account")
        .cookie(cookie.clone())
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(json!({"name":"Agent team","role":"owner"}))
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::BAD_REQUEST
    );
    let request = test::TestRequest::post()
        .uri(&format!("/api/projects/{key}/references"))
        .cookie(cookie.clone())
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(json!({"content":"Private agent delivery report"}))
        .to_request();
    let reference: Value = test::call_and_read_body_json(&app, request).await;
    let digest = reference["digest"].as_str().unwrap();
    assert_eq!(
        digest,
        hex::encode(Sha256::digest(b"Private agent delivery report"))
    );
    let reference_path = format!("/api/projects/{key}/references/{digest}");
    let request = test::TestRequest::get()
        .uri(&reference_path)
        .cookie(cookie.clone())
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::OK
    );

    let stranger = SigningKey::from_bytes(&[82; 32]);
    let stranger_address = bs58::encode(stranger.verifying_key().to_bytes()).into_string();
    let challenge = application
        .accounts
        .begin_login(Chain::Solana, &stranger_address)
        .await?;
    let signature =
        bs58::encode(stranger.sign(challenge.message.as_bytes()).to_bytes()).into_string();
    let login = application
        .accounts
        .complete_login(challenge.id, &signature, None)
        .await?;
    let stranger_cookie = Cookie::new(SESSION_COOKIE, login.session_token.clone());
    // Even an agent reusing the same hash in a task is not the text's author.
    let request = test::TestRequest::get()
        .uri(&reference_path)
        .cookie(stranger_cookie.clone())
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::FORBIDDEN
    );
    let challenge = application
        .accounts
        .begin_link(&login.session_token, Chain::Near, "authority.near")
        .await?;
    let digest = Sha256::digest(borsh::to_vec(&(
        (1_u32 << 31) + 413,
        &challenge.message,
        challenge.nonce,
        &challenge.recipient,
        None::<String>,
    ))?);
    let signature = STANDARD.encode(stranger.sign(&digest).to_bytes());
    let public_key = format!(
        "ed25519:{}",
        bs58::encode(stranger.verifying_key().to_bytes()).into_string()
    );
    application
        .accounts
        .complete_link(
            &login.session_token,
            challenge.id,
            &signature,
            Some(&public_key),
        )
        .await?;
    let request = test::TestRequest::get()
        .uri(&reference_path)
        .cookie(stranger_cookie.clone())
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::OK
    );
    key_active.store(false, Ordering::SeqCst);
    let request = test::TestRequest::get()
        .uri(&reference_path)
        .cookie(stranger_cookie.clone())
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::FORBIDDEN,
        "A historic NEAR link must not grant private reads after its key is revoked"
    );
    key_active.store(true, Ordering::SeqCst);
    manager_active.store(false, Ordering::SeqCst);
    let request = test::TestRequest::get()
        .uri(&reference_path)
        .cookie(stranger_cookie)
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::FORBIDDEN
    );
    // The author's access survives management changes.
    let request = test::TestRequest::get()
        .uri(&reference_path)
        .cookie(cookie.clone())
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::OK
    );

    let request = test::TestRequest::post()
        .uri("/api/auth/logout")
        .cookie(cookie.clone())
        .insert_header((header::ORIGIN, "https://app.example.com"))
        .set_json(json!({}))
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::NO_CONTENT
    );
    let request = test::TestRequest::get()
        .uri("/api/account")
        .cookie(cookie)
        .to_request();
    assert_eq!(
        test::call_service(&app, request).await.status(),
        StatusCode::UNAUTHORIZED
    );

    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await?;
    admin.close().await;
    server_handle.stop(true).await;
    running.await??;
    Ok(())
}
