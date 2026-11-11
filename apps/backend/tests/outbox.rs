use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result, ensure};
use apodect_backend::{
    events::{Events, TOPIC},
    storage::{Store, emit},
};
use rdkafka::{
    ClientConfig, Message, Offset, TopicPartitionList,
    consumer::{Consumer, StreamConsumer},
};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires APODECT_TEST_DATABASE_URL and APODECT_TEST_KAFKA_BROKERS"]
async fn acknowledged_event_survives_database_failure_and_retries_with_same_identity() -> Result<()>
{
    let database = std::env::var("APODECT_TEST_DATABASE_URL")
        .context("APODECT_TEST_DATABASE_URL is required")?;
    let brokers = std::env::var("APODECT_TEST_KAFKA_BROKERS")
        .context("APODECT_TEST_KAFKA_BROKERS is required")?;
    let options = PgConnectOptions::from_str(&database)?;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await?;
    let schema = format!("test_outbox_{}", Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await?;

    let result = run(&options, &schema, &brokers).await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await?;
    admin.close().await;
    result
}

async fn run(options: &PgConnectOptions, schema: &str, brokers: &str) -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone().options([("search_path", schema)]))
        .await?;
    let store = Store::new(pool.clone());
    store.migrate().await?;
    let publisher = Events::connect(brokers).await?;
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .set("group.id", schema)
        .set("enable.auto.commit", "false")
        .create()?;
    let mut assignment = TopicPartitionList::new();
    assignment.add_partition_offset(TOPIC, 0, Offset::Beginning)?;
    consumer.assign(&assignment)?;

    let project = schema;
    store
        .register_project(project, &json!({"test": true}))
        .await?;
    let mut transaction = pool.begin().await?;
    emit(
        &mut transaction,
        project,
        "operation.updated",
        json!({"state": "confirmed"}),
    )
    .await?;
    transaction.commit().await?;
    let event_id: Uuid = sqlx::query_scalar("SELECT id FROM outbox")
        .fetch_one(&pool)
        .await?;
    sqlx::raw_sql(
        "CREATE FUNCTION reject_outbox_delete() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'simulated acknowledgement failure'; END $$;
         CREATE TRIGGER reject_outbox_delete BEFORE DELETE ON outbox
         FOR EACH ROW EXECUTE FUNCTION reject_outbox_delete();",
    )
    .execute(&pool)
    .await?;

    ensure!(
        publisher.publish_pending(&store).await.is_err(),
        "database cleanup must fail"
    );
    let first = receive_event(&consumer, event_id).await?;
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
        .fetch_one(&pool)
        .await?;
    ensure!(
        remaining == 1,
        "acknowledgement failure must retain the event"
    );

    sqlx::query("DROP TRIGGER reject_outbox_delete ON outbox")
        .execute(&pool)
        .await?;
    ensure!(
        publisher.publish_pending(&store).await? == 1,
        "retry must publish the retained event"
    );
    let retry = receive_event(&consumer, event_id).await?;
    ensure!(
        first == retry,
        "retry must preserve the entire event identity and content"
    );
    ensure!(
        publisher.publish_pending(&store).await? == 0,
        "acknowledged outbox must be empty"
    );
    pool.close().await;
    Ok(())
}

async fn receive_event(consumer: &StreamConsumer, identity: Uuid) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let message = consumer.recv().await?;
            let value: Value =
                serde_json::from_slice(message.payload().context("missing event payload")?)?;
            if value["id"] == identity.to_string() {
                ensure!(
                    message.key() == value["project"].as_str().map(str::as_bytes),
                    "project partition key mismatch"
                );
                return Ok(value);
            }
        }
    })
    .await
    .context("Kafka event delivery timed out")?
}
