use super::*;
use crate::chains::near::GenesisIdentity;
use actix_web::{App, HttpResponse, HttpServer, web};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use ed25519_dalek::{Signer, SigningKey};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
};

fn solana_signature(key: &SigningKey, challenge: &Challenge) -> String {
    bs58::encode(key.sign(challenge.message.as_bytes()).to_bytes()).into_string()
}

/// One transactional scenario covers identity creation, replay, concurrent logins,
/// linking conflicts, revocation, expiration and changing on-chain signers.
#[actix_web::test]
#[ignore = "requires an isolated PostgreSQL database via APODECT_TEST_DATABASE_URL"]
async fn wallet_identity_lifecycle() -> Result<()> {
    let url = std::env::var("APODECT_TEST_DATABASE_URL")?;
    let admin = PgPool::connect(&url).await?;
    let schema = format!("accounts_test_{}", Uuid::new_v4().simple());
    // The identifier consists only of a constant prefix and generated UUID hex.
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await?;
    let options: PgConnectOptions = url.parse()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options.options([("search_path", schema.as_str())]))
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0001_initial.sql"))
        .execute(&pool)
        .await?;

    let permitted = Arc::new(AtomicBool::new(true));
    let permission = permitted.clone();
    let genesis = GenesisIdentity {
        chain_id: "test-near".into(),
        genesis_time: "2026-01-01T00:00:00Z".into(),
        genesis_height: 0,
        validators: json!([]),
    };
    let genesis_response = serde_json::to_value(&genesis)?;
    let block_hash = near_primitives::hash::CryptoHash::default().to_string();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let rpc_url = format!("http://{}", listener.local_addr()?);
    let server = HttpServer::new(move || {
        let permission = permission.clone();
        let genesis = genesis_response.clone();
        let hash = block_hash.clone();
        App::new().route("/", web::post().to(move |request: web::Json<Value>| {
            let permitted = permission.load(Ordering::SeqCst);
            let genesis = genesis.clone();
            let hash = hash.clone();
            async move {
                let result = match request["method"].as_str().unwrap() {
                    "status" => json!({"chain_id": "test-near"}),
                    "genesis_config" => genesis,
                    "block" => json!({"header": {"height": 5, "hash": hash}}),
                    "query" => {
                        assert_eq!(request["params"]["block_id"], hash);
                        if request["params"]["request_type"] == "view_access_key" {
                            json!({"block_hash": hash, "block_height": 5, "permission": if permitted {json!("FullAccess")} else {json!({"FunctionCall": {}})}})
                        } else {
                            let value = match request["params"]["method_name"].as_str().unwrap() {
                                "get_state" => json!({"owner_id":"owner.near","authority_id":"authority.near","project_id":"aa".repeat(32),"mpc_id":"mpc.near","sign_key":"","read_key":"","sign_path":"","queue_head":0,"queue_length":0,"next_plan":1,"next_command":1,"next_invoice":1,"retry_limit":3}),
                                "get_project" => json!({"owner_id":"owner.near","authority_id":"authority.near","token_id":"usdc.near","free":"0","pending_owner_outflow":"0","next_task":1,"next_request":1,"next_invoice":1,"next_operation":1}),
                                "get_bridge" => json!({"domain":"01".repeat(32),"key":"","held":"0","accounted_custody":"0","sequence":0,"next_funding":1}),
                                "get_task" => json!({"agent":"another.near","state":"Active","paused":false,"started":true,"issued":"10","recovered":"0","available":"10","pending_outflow":"0","live_request":null,"report":null}),
                                method => panic!("unexpected contract view {method}"),
                            };
                            json!({"block_hash":hash,"block_height":5,"result":serde_json::to_vec(&value).unwrap()})
                        }
                    }
                    "eth_chainId" => json!("0x7a69"),
                    "eth_getBlockByNumber" => json!({"hash": format!("0x{}", "11".repeat(32))}),
                    "eth_getCode" => json!("0x01"),
                    "eth_call" => {
                        assert_eq!(request["params"][1]["requireCanonical"], true);
                        json!(format!("0x{}{}", if permitted {"1626ba7e"} else {"ffffffff"}, "00".repeat(28)))
                    }
                    method => panic!("unexpected test RPC {method}"),
                };
                HttpResponse::Ok().json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
            }
        }))
    }).listen(listener)?.run();
    let handle = server.handle();
    let running = actix_web::rt::spawn(server);
    let rpc = Rpc::new(&rpc_url)?;
    let near = NearClient::connect(rpc.clone(), &genesis).await?;
    let accounts = Accounts::new(
        pool.clone(),
        AuthConfig {
            origin: "https://app.example.com".into(),
            ethereum_chain_id: 31337,
            ethereum_genesis_hash: format!("0x{}", "11".repeat(32)),
            solana_cluster: "localnet".into(),
            solana_genesis_hash: bs58::encode([3; 32]).into_string(),
            near_network: "test-near:genesis".into(),
        },
        rpc,
        near,
    )?;

    let key = SigningKey::from_bytes(&[1; 32]);
    let address = bs58::encode(key.verifying_key().to_bytes()).into_string();
    let challenge = accounts.begin_login(Chain::Solana, &address).await?;
    let signature = solana_signature(&key, &challenge);
    assert!(
        accounts
            .complete_login(
                challenge.id,
                &solana_signature(&SigningKey::from_bytes(&[2; 32]), &challenge),
                None
            )
            .await
            .is_err()
    );
    let first = accounts
        .complete_login(challenge.id, &signature, None)
        .await?;
    assert!(
        accounts
            .complete_login(challenge.id, &signature, None)
            .await
            .is_err()
    );
    let profile = accounts
        .update_profile(&first.session_token, Some("  Alice  "), Some(" Alice "))
        .await?;
    assert_eq!(profile.display_name.as_deref(), Some("Alice"));
    assert_eq!(profile.username.as_deref(), Some("alice"));

    let (a, b) = tokio::try_join!(
        accounts.begin_login(Chain::Solana, &address),
        accounts.begin_login(Chain::Solana, &address)
    )?;
    let (sa, sb) = (solana_signature(&key, &a), solana_signature(&key, &b));
    let (a, b) = tokio::try_join!(
        accounts.complete_login(a.id, &sa, None),
        accounts.complete_login(b.id, &sb, None)
    )?;
    assert_eq!(a.profile.id, first.profile.id);
    assert_eq!(b.profile.id, first.profile.id);

    let other = SigningKey::from_bytes(&[2; 32]);
    let other_address = bs58::encode(other.verifying_key().to_bytes()).into_string();
    let link = accounts
        .begin_link(&first.session_token, Chain::Solana, &other_address)
        .await?;
    let signature = solana_signature(&other, &link);
    assert!(
        accounts
            .complete_login(link.id, &signature, None)
            .await
            .is_err()
    );
    assert!(
        accounts
            .complete_link(&a.session_token, link.id, &signature, None)
            .await
            .is_err()
    );
    let linked = accounts
        .complete_link(&first.session_token, link.id, &signature, None)
        .await?;
    assert_eq!(linked.id, first.profile.id);
    assert_eq!(accounts.wallets(&first.session_token).await?.len(), 2);
    accounts
        .require_wallet(&first.session_token, Chain::Solana, &other_address)
        .await?;
    assert!(
        accounts
            .require_wallet(&first.session_token, Chain::Ethereum, &other_address)
            .await
            .is_err()
    );

    let stranger = SigningKey::from_bytes(&[9; 32]);
    let stranger_address = bs58::encode(stranger.verifying_key().to_bytes()).into_string();
    let challenge = accounts
        .begin_login(Chain::Solana, &stranger_address)
        .await?;
    let stranger_login = accounts
        .complete_login(challenge.id, &solana_signature(&stranger, &challenge), None)
        .await?;
    let conflict = accounts
        .begin_link(&stranger_login.session_token, Chain::Solana, &address)
        .await?;
    assert!(
        accounts
            .complete_link(
                &stranger_login.session_token,
                conflict.id,
                &solana_signature(&key, &conflict),
                None
            )
            .await
            .is_err()
    );

    let expired = accounts.begin_login(Chain::Solana, &address).await?;
    sqlx::query(
        "UPDATE login_challenges SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",
    )
    .bind(expired.id)
    .execute(&pool)
    .await?;
    assert!(
        accounts
            .complete_login(expired.id, &solana_signature(&key, &expired), None)
            .await
            .is_err()
    );
    let pending_link = accounts
        .begin_link(&first.session_token, Chain::Solana, &stranger_address)
        .await?;
    accounts.revoke_session(&first.session_token).await?;
    assert!(accounts.session(&first.session_token).await.is_err());
    assert!(
        accounts
            .complete_link(
                &first.session_token,
                pending_link.id,
                &solana_signature(&stranger, &pending_link),
                None
            )
            .await
            .is_err()
    );

    let ethereum: PrivateKeySigner =
        "0000000000000000000000000000000000000000000000000000000000000001".parse()?;
    let challenge = accounts
        .begin_login(Chain::Ethereum, &ethereum.address().to_string())
        .await?;
    let signature = ethereum
        .sign_message_sync(challenge.message.as_bytes())?
        .to_string();
    let eth_login = accounts
        .complete_login(challenge.id, &signature, None)
        .await?;
    accounts.session(&eth_login.session_token).await?;

    let challenge = accounts
        .begin_login(
            Chain::Ethereum,
            "0x1111111111111111111111111111111111111111",
        )
        .await?;
    let contract = accounts
        .complete_login(challenge.id, "0x1234", None)
        .await?;
    permitted.store(false, Ordering::SeqCst);
    assert!(accounts.session(&contract.session_token).await.is_err());
    permitted.store(true, Ordering::SeqCst);

    let near_key: near_crypto::SecretKey = "ed25519:3D4YudUahN1nawWogh8pAKSj92sUNMdbZGjn7kERKzYoTy8tnFQuwoGUC51DowKqorvkr2pytJSnwuSbsNVfqygr".parse()?;
    let challenge = accounts.begin_login(Chain::Near, "alice.near").await?;
    let digest = nep413_digest(
        &challenge.message,
        challenge.nonce,
        &challenge.recipient,
        None,
    )?;
    let near_crypto::Signature::ED25519(signature) = near_key.sign(&digest) else {
        unreachable!()
    };
    let near_login = accounts
        .complete_login(
            challenge.id,
            &STANDARD.encode(signature.to_bytes()),
            Some(&near_key.public_key().to_string()),
        )
        .await?;
    accounts.session(&near_login.session_token).await?;
    let callback = format!("{}/wallet/callback", accounts.config.origin);
    assert!(
        accounts
            .begin_login_with_callback(
                Chain::Near,
                "alice.near",
                Some("https://other.example/wallet/callback")
            )
            .await
            .is_err()
    );
    let redirect = accounts
        .begin_login_with_callback(Chain::Near, "alice.near", Some(&callback))
        .await?;
    let near_crypto::Signature::ED25519(without_callback) = near_key.sign(&nep413_digest(
        &redirect.message,
        redirect.nonce,
        &redirect.recipient,
        None,
    )?) else {
        unreachable!()
    };
    assert!(
        accounts
            .complete_login(
                redirect.id,
                &STANDARD.encode(without_callback.to_bytes()),
                Some(&near_key.public_key().to_string()),
            )
            .await
            .is_err()
    );
    let near_crypto::Signature::ED25519(signed) = near_key.sign(&nep413_digest(
        &redirect.message,
        redirect.nonce,
        &redirect.recipient,
        redirect.callback_url.as_deref(),
    )?) else {
        unreachable!()
    };
    let redirect_login = accounts
        .complete_login(
            redirect.id,
            &STANDARD.encode(signed.to_bytes()),
            Some(&near_key.public_key().to_string()),
        )
        .await?;
    assert_eq!(redirect_login.profile.id, near_login.profile.id);
    assert_action_authorization(
        &accounts,
        &pool,
        &rpc_url,
        genesis,
        &near_login.session_token,
    )
    .await?;
    permitted.store(false, Ordering::SeqCst);
    assert!(accounts.session(&near_login.session_token).await.is_err());

    Accounts::purge_expired(&accounts.pool).await?;
    handle.stop(true).await;
    running.await??;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await?;
    admin.close().await;
    Ok(())
}

async fn assert_action_authorization(
    accounts: &Accounts,
    pool: &PgPool,
    rpc_url: &str,
    genesis: GenesisIdentity,
    session: &str,
) -> Result<()> {
    use crate::{
        actions::{Actions, CoordinatorAction, LocalAction, UnsignedIntent},
        chains::ethereum::Ethereum,
        config::{Chain, Config, number_word},
        indexing::Indexer,
        storage::Store,
    };
    let config: Config = serde_json::from_value(json!({
        "ethereum":{"rpc_url":rpc_url,"chain_id":31337,"genesis_hash":format!("0x{}","11".repeat(32))},
        "solana":{"rpc_url":rpc_url,"genesis_hash":bs58::encode([3;32]).into_string()},
        "near":{"rpc_url":rpc_url,"genesis":genesis},
        "projects":[{"name":"Test project","ethereum_start_block":0,"coordinator":"coordinator.near","project_id":"aa".repeat(32),"domains":{
            "ethereum":{"domain":"02".repeat(32),"vault":"0x1111111111111111111111111111111111111111","asset":"0x2222222222222222222222222222222222222222","asset_id":"eth-usdc"},
            "solana":{"domain":"03".repeat(32),"vault":bs58::encode([4;32]).into_string(),"asset":bs58::encode([5;32]).into_string(),"asset_id":"sol-usdc"},
            "near":{"domain":"01".repeat(32),"vault":"vault.near","asset":"usdc.near","asset_id":"near-usdc"}
        }}],
        "liquidity":{"url":rpc_url,"local_simulator":true,"api_key_file":null}
    }))?;
    let mut indexer = Indexer {
        ethereum: Ethereum::new(
            Rpc::new(rpc_url)?,
            31337,
            config.ethereum.genesis_hash.parse()?,
        ),
        near: accounts.near.clone(),
        store: Store::new(pool.clone()),
        config,
    };
    let key = indexer.config.project_key(&indexer.config.projects[0]);
    let actions = Actions::new(accounts, &indexer);
    let create = LocalAction::CreateTask {
        amount: 10,
        purpose: "11".repeat(32),
    };
    let intent = actions
        .prepare_near(session, &key, "alice.near", create.clone(), 1)
        .await?;
    assert!(
        matches!(intent, UnsignedIntent::Near { transactions, .. } if transactions.len()==1 && transactions[0].method_name=="create_task")
    );

    // Linked wallet ownership is required separately from each contract role.
    assert!(
        actions
            .prepare_near(session, &key, "owner.near", create.clone(), 1)
            .await
            .is_err()
    );
    assert!(
        actions
            .prepare_coordinator(
                session,
                &key,
                "alice.near",
                CoordinatorAction::SetRetryLimit { attempts: 3 },
                1
            )
            .await
            .is_err()
    );
    assert!(
        actions
            .prepare_coordinator(
                session,
                &key,
                "alice.near",
                CoordinatorAction::Approve {
                    chain: Chain::Near,
                    request: number_word(0)
                },
                1
            )
            .await
            .is_err()
    );
    assert!(
        actions
            .prepare_near(
                session,
                &key,
                "alice.near",
                LocalAction::Withdraw {
                    task: number_word(0),
                    amount: 1
                },
                1
            )
            .await
            .is_err()
    );
    assert!(
        actions
            .prepare_near(
                session,
                &key,
                "alice.near",
                LocalAction::Fund {
                    amount: 1,
                    invoice: None,
                    funding_reference: Some("12".repeat(32))
                },
                1
            )
            .await
            .is_err()
    );
    assert!(
        actions
            .prepare_ethereum(session, &key, "alice.near", create.clone())
            .await
            .is_err()
    );

    // A valid account cannot authorize a different configured domain.
    indexer.config.projects[0]
        .domains
        .get_mut(&Chain::Near)
        .unwrap()
        .domain = "ff".repeat(32);
    let actions = Actions::new(accounts, &indexer);
    assert!(
        actions
            .prepare_near(session, &key, "alice.near", create, 1)
            .await
            .is_err()
    );
    Ok(())
}
