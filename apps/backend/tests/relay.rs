use std::str::FromStr;

use anyhow::{Context, Result};
use apodect_backend::storage::Store;
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL via APODECT_TEST_DATABASE_URL"]
async fn relay_journal_preserves_identity_and_retries_only_finalized_attempts() -> Result<()> {
    let database = std::env::var("APODECT_TEST_DATABASE_URL")
        .context("APODECT_TEST_DATABASE_URL is required")?;
    let options = PgConnectOptions::from_str(&database)?;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await?;
    let schema = format!("test_relay_{}", Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.options([("search_path", schema.as_str())]))
        .await?;

    let result = exercise_journal(Store::new(pool.clone())).await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await?;
    admin.close().await;
    result
}

async fn exercise_journal(store: Store) -> Result<()> {
    store.migrate().await?;
    let project = "project";
    let operation = "allocate";
    let network = "ethereum:genesis-a";
    let intent = json!({"action":"execute", "command":"approved immutable command"});
    let transaction = json!({"raw":"0x02017e", "hash":"0xabc", "sender":"0xAbC", "nonce":7});
    store
        .register_project(project, &json!({"network":network}))
        .await?;
    let planned = store
        .plan_effect(project, operation, "ethereum", network, &intent)
        .await?;
    assert_eq!(planned.state, "planned");
    assert!(planned.transaction.is_none());
    assert!(
        store
            .plan_effect(
                project,
                operation,
                "ethereum",
                network,
                &json!({"action":"changed"})
            )
            .await
            .is_err()
    );
    assert!(
        store
            .plan_effect(
                project,
                operation,
                "ethereum",
                "ethereum:genesis-b",
                &intent
            )
            .await
            .is_err()
    );

    // Preparing happens before sending; reopening the journal cannot invent new bytes.
    store
        .prepare_effect(project, operation, &transaction)
        .await?;
    let restarted = Store::new(store.pool.clone());
    let recovered = restarted
        .plan_effect(project, operation, "ethereum", network, &intent)
        .await?;
    assert_eq!(recovered.transaction.as_ref(), Some(&transaction));
    assert!(
        restarted
            .prepare_effect(project, operation, &json!({"raw":"replacement"}))
            .await
            .is_err()
    );
    assert_eq!(
        restarted.nonce_floor("ethereum", network, "0xabc").await?,
        8
    );
    assert_eq!(
        restarted
            .nonce_floor("ethereum", "ethereum:genesis-b", "0xabc")
            .await?,
        0
    );
    assert_eq!(
        restarted.nonce_floor("ethereum", network, "0xdef").await?,
        0
    );

    // A lost RPC acknowledgement and an expired, unresolved transaction retain their identity.
    restarted
        .update_effect(
            project,
            operation,
            "submitted",
            None,
            Some("connection lost after send"),
        )
        .await?;
    assert!(restarted.retry_effect(project, operation, 0).await.is_err());
    restarted
        .update_effect(
            project,
            operation,
            "attention",
            None,
            Some("outcome unknown"),
        )
        .await?;
    assert!(restarted.retry_effect(project, operation, 0).await.is_err());
    assert_eq!(
        restarted
            .effect(project, operation)
            .await?
            .unwrap()
            .transaction
            .as_ref(),
        Some(&transaction)
    );

    // No terminal journal change may commit without its corresponding event.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_outbox_insert() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'simulated outbox failure'; END $$;
         CREATE TRIGGER reject_outbox_insert BEFORE INSERT ON outbox
         FOR EACH ROW EXECUTE FUNCTION reject_outbox_insert();",
    )
    .execute(&store.pool)
    .await?;
    let failure = json!({"hash":"0xabc", "height":42});
    assert!(
        restarted
            .update_effect(
                project,
                operation,
                "failed",
                Some(&failure),
                Some("finalized revert")
            )
            .await
            .is_err()
    );
    assert_eq!(
        restarted.effect(project, operation).await?.unwrap().state,
        "attention"
    );
    sqlx::query("DROP TRIGGER reject_outbox_insert ON outbox")
        .execute(&store.pool)
        .await?;
    restarted
        .update_effect(
            project,
            operation,
            "failed",
            Some(&failure),
            Some("finalized revert"),
        )
        .await?;
    // Repeating the same observation does not emit another event.
    restarted
        .update_effect(
            project,
            operation,
            "failed",
            Some(&failure),
            Some("finalized revert"),
        )
        .await?;

    sqlx::raw_sql(
        "CREATE TRIGGER reject_outbox_insert BEFORE INSERT ON outbox
         FOR EACH ROW EXECUTE FUNCTION reject_outbox_insert();",
    )
    .execute(&store.pool)
    .await?;
    assert!(restarted.retry_effect(project, operation, 0).await.is_err());
    assert_eq!(
        restarted.effect(project, operation).await?.unwrap().state,
        "failed"
    );
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM effect_attempts")
        .fetch_one(&store.pool)
        .await?;
    assert_eq!(attempts, 0);
    sqlx::query("DROP TRIGGER reject_outbox_insert ON outbox")
        .execute(&store.pool)
        .await?;

    restarted.retry_effect(project, operation, 0).await?;
    let retry = restarted.effect(project, operation).await?.unwrap();
    assert_eq!(retry.attempt, 1);
    assert_eq!(retry.state, "planned");
    assert_eq!(retry.intent, intent);
    assert!(retry.transaction.is_none());
    assert!(retry.result.is_none());
    assert!(retry.error.is_none());
    assert!(restarted.retry_effect(project, operation, 0).await.is_err());
    assert_eq!(
        restarted.nonce_floor("ethereum", network, "0xabc").await?,
        8
    );
    let archived: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('transaction',transaction,'state',state,'result',result,'error',error)
         FROM effect_attempts WHERE attempt=0",
    ).fetch_one(&store.pool).await?;
    assert_eq!(
        archived,
        json!({"transaction":transaction,"state":"failed","result":failure,"error":"finalized revert"})
    );

    let mut next_transaction = transaction.clone();
    next_transaction["nonce"] = json!(8);
    next_transaction["raw"] = json!("0x02028e");
    next_transaction["hash"] = json!("0xdef");
    restarted
        .prepare_effect(project, operation, &next_transaction)
        .await?;
    restarted
        .update_effect(
            project,
            operation,
            "confirmed",
            Some(&json!({"value":false})),
            None,
        )
        .await?;
    restarted.retry_effect(project, operation, 1).await?;
    assert_eq!(
        restarted.nonce_floor("ethereum", network, "0xABC").await?,
        9
    );
    next_transaction["nonce"] = json!(9);
    next_transaction["raw"] = json!("0x02039e");
    next_transaction["hash"] = json!("0x123");
    restarted
        .prepare_effect(project, operation, &next_transaction)
        .await?;
    restarted
        .update_effect(
            project,
            operation,
            "confirmed",
            Some(&json!({"value":true})),
            None,
        )
        .await?;
    assert!(restarted.retry_effect(project, operation, 2).await.is_err());
    assert_eq!(
        restarted.nonce_floor("ethereum", network, "0xabc").await?,
        10
    );

    let events: Vec<(String, Value)> =
        sqlx::query_as("SELECT kind, payload FROM outbox ORDER BY sequence")
            .fetch_all(&store.pool)
            .await?;
    assert_eq!(
        events
            .iter()
            .map(|(kind, payload)| (
                kind.as_str(),
                payload["state"].as_str(),
                payload["attempt"].as_i64()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("operation.updated", Some("submitted"), None),
            ("operation.updated", Some("attention"), None),
            ("operation.updated", Some("failed"), None),
            ("operation.retry_requested", None, Some(1)),
            ("operation.updated", Some("confirmed"), None),
            ("operation.retry_requested", None, Some(2)),
            ("operation.updated", Some("confirmed"), None),
        ]
    );
    Ok(())
}
