//! PostgreSQL commits business state and its outbox event together. Kafka delivery
//! is at least once: a crash after the broker acknowledgement can repeat the same event ID.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use rdkafka::{
    ClientConfig,
    admin::{AdminClient, AdminOptions, NewTopic, TopicReplication},
    client::DefaultClientContext,
    error::RDKafkaErrorCode,
    producer::{FutureProducer, FutureRecord},
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::storage::Store;

pub const TOPIC: &str = "apodect.events";

pub struct Events {
    producer: FutureProducer,
}

impl Events {
    pub async fn from_env() -> Result<Self> {
        let brokers = std::env::var("KAFKA_BROKERS").context("KAFKA_BROKERS is required")?;
        Self::connect(&brokers).await
    }

    pub async fn connect(brokers: &str) -> Result<Self> {
        ensure!(!brokers.trim().is_empty(), "Kafka broker list is empty");
        let mut config = ClientConfig::new();
        config
            .set("bootstrap.servers", brokers)
            .set("client.id", "apodect-outbox")
            .set("socket.timeout.ms", "5000");

        let admin: AdminClient<DefaultClientContext> = config.create()?;
        let topic = NewTopic::new(TOPIC, 1, TopicReplication::Fixed(1));
        let options = AdminOptions::new()
            .request_timeout(Some(Duration::from_secs(10)))
            .operation_timeout(Some(Duration::from_secs(5)));
        for result in admin.create_topics(&[topic], &options).await? {
            match result {
                Ok(_) | Err((_, RDKafkaErrorCode::TopicAlreadyExists)) => {}
                Err((name, error)) => anyhow::bail!("Cannot create Kafka topic {name}: {error}"),
            }
        }

        let producer = config
            .set("enable.idempotence", "true")
            .set("acks", "all")
            .set("message.timeout.ms", "15000")
            .set("request.timeout.ms", "5000")
            .create()?;
        Ok(Self { producer })
    }

    /// The worker's PostgreSQL advisory lock owns this publisher as well as chain writes.
    /// No database transaction is held while waiting for Kafka; outbox rows are immutable.
    pub async fn publish_pending(&self, store: &Store) -> Result<usize> {
        let rows = sqlx::query(
            "SELECT id, project, kind, payload FROM outbox ORDER BY sequence LIMIT 100",
        )
        .fetch_all(&store.pool)
        .await?;
        let mut delivered = 0;
        for row in rows {
            let id: Uuid = row.try_get("id")?;
            let project: String = row.try_get("project")?;
            let kind: String = row.try_get("kind")?;
            let payload: Value = row.try_get("payload")?;
            let message = serde_json::to_vec(&json!({
                "id": id, "project": project, "kind": kind, "payload": payload,
            }))?;

            self.producer
                .send(
                    FutureRecord::to(TOPIC).key(&project).payload(&message),
                    Duration::from_secs(1),
                )
                .await
                .map_err(|(error, _)| error)
                .context("Kafka did not confirm outbox delivery; event retained for retry")?;

            let deleted = sqlx::query("DELETE FROM outbox WHERE id = $1")
                .bind(id)
                .execute(&store.pool)
                .await?
                .rows_affected();
            ensure!(
                deleted == 1,
                "Published outbox event was removed by another worker"
            );
            delivered += 1;
        }
        Ok(delivered)
    }
}
