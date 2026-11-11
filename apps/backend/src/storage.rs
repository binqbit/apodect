use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Effect {
    pub id: String,
    pub chain: String,
    pub intent: Value,
    pub attempt: i32,
    pub transaction: Option<Value>,
    pub state: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn register_project(&self, key: &str, deployment: &Value) -> Result<()> {
        sqlx::query("INSERT INTO projects (id, deployment) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(key)
            .bind(deployment)
            .execute(&self.pool)
            .await?;
        let actual: Value = sqlx::query_scalar("SELECT deployment FROM projects WHERE id = $1")
            .bind(key)
            .fetch_one(&self.pool)
            .await?;
        ensure!(
            actual == *deployment,
            "Configured Project identity changed; register a new deployment"
        );
        Ok(())
    }

    pub async fn effect(&self, project: &str, id: &str) -> Result<Option<Effect>> {
        let record = sqlx::query("SELECT id, chain, intent, attempt, transaction, state, result, error FROM effects WHERE project = $1 AND id = $2")
            .bind(project).bind(id).fetch_optional(&self.pool).await?;
        record
            .map(|row| {
                Ok(Effect {
                    id: row.try_get("id")?,
                    chain: row.try_get("chain")?,
                    intent: row.try_get("intent")?,
                    attempt: row.try_get("attempt")?,
                    transaction: row.try_get("transaction")?,
                    state: row.try_get("state")?,
                    result: row.try_get("result")?,
                    error: row.try_get("error")?,
                })
            })
            .transpose()
    }

    pub async fn plan_effect(
        &self,
        project: &str,
        id: &str,
        chain: &str,
        network: &str,
        intent: &Value,
    ) -> Result<Effect> {
        sqlx::query("INSERT INTO effects (project, id, chain, network, intent) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING")
            .bind(project).bind(id).bind(chain).bind(network).bind(intent).execute(&self.pool).await?;
        let effect = self
            .effect(project, id)
            .await?
            .context("Effect was not persisted")?;
        ensure!(
            effect.chain == chain && effect.intent == *intent,
            "Operation ID reused with different content"
        );
        let saved_network: String =
            sqlx::query_scalar("SELECT network FROM effects WHERE project=$1 AND id=$2")
                .bind(project)
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        ensure!(
            saved_network == network,
            "Operation network identity changed"
        );
        Ok(effect)
    }

    pub async fn prepare_effect(&self, project: &str, id: &str, transaction: &Value) -> Result<()> {
        let count = sqlx::query("UPDATE effects SET transaction = $3, state = 'prepared' WHERE project = $1 AND id = $2 AND state = 'planned' AND transaction IS NULL")
            .bind(project).bind(id).bind(transaction).execute(&self.pool).await?.rows_affected();
        ensure!(count == 1, "Effect already prepared by another worker");
        Ok(())
    }

    pub async fn update_effect(
        &self,
        project: &str,
        id: &str,
        state: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE effects SET state = $3, result = $4, error = $5 WHERE project = $1 AND id = $2 AND state NOT IN ('confirmed', 'failed') AND (state, result, error) IS DISTINCT FROM ($3, $4, $5)")
            .bind(project).bind(id).bind(state).bind(result).bind(error).execute(&mut *tx).await?.rows_affected();
        if changed > 0 {
            emit(
                &mut tx,
                project,
                "operation.updated",
                json!({"operation": id, "state": state, "error": error}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn nonce_floor(&self, chain: &str, network: &str, sender: &str) -> Result<u64> {
        let value: Option<String> = sqlx::query_scalar("SELECT max((transaction->>'nonce')::numeric)::text FROM (SELECT chain, network, transaction FROM effects UNION ALL SELECT e.chain, e.network, a.transaction FROM effect_attempts a JOIN effects e ON e.project=a.project AND e.id=a.effect) history WHERE chain = $1 AND network = $2 AND (CASE WHEN chain = 'ethereum' THEN lower(transaction->>'sender') = lower($3) ELSE transaction->>'sender' = $3 END)")
            .bind(chain).bind(network).bind(sender).fetch_one(&self.pool).await?;
        value
            .map(|value| {
                value
                    .parse::<u64>()
                    .context("Invalid persisted nonce")
                    .and_then(|n| n.checked_add(1).context("Nonce exhausted"))
            })
            .transpose()
            .map(|value| value.unwrap_or(0))
    }

    pub async fn retry_effect(&self, project: &str, id: &str, attempt: i32) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let record = sqlx::query(
            "SELECT state, result, attempt FROM effects WHERE project=$1 AND id=$2 FOR UPDATE",
        )
        .bind(project)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let state: String = record.try_get("state")?;
        let result: Option<Value> = record.try_get("result")?;
        ensure!(
            record.try_get::<i32, _>("attempt")? == attempt,
            "Operation already retried"
        );
        ensure!(
            state == "failed"
                || (state == "confirmed" && result.as_ref().is_some_and(|v| v["value"] == false)),
            "Only a finalized failure or rejected callback can be retried; unresolved transactions retain their original identity"
        );
        sqlx::query("INSERT INTO effect_attempts(project,effect,attempt,transaction,state,result,error) SELECT project,id,attempt,transaction,state,result,error FROM effects WHERE project=$1 AND id=$2")
            .bind(project).bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE effects SET attempt=attempt+1,state='planned',transaction=NULL,result=NULL,error=NULL WHERE project=$1 AND id=$2")
            .bind(project).bind(id).execute(&mut *tx).await?;
        emit(
            &mut tx,
            project,
            "operation.retry_requested",
            json!({"operation":id,"attempt":attempt+1}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn save_quote(&self, project: &str, route: &str, response: &Value) -> Result<()> {
        sqlx::query("INSERT INTO funding_quotes (project, route, response) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(project).bind(route).bind(response).execute(&self.pool).await?;
        let actual: Value = sqlx::query_scalar(
            "SELECT response FROM funding_quotes WHERE project = $1 AND route = $2",
        )
        .bind(project)
        .bind(route)
        .fetch_one(&self.pool)
        .await?;
        ensure!(actual == *response, "Route quote is immutable");
        Ok(())
    }
}

pub async fn emit(
    tx: &mut Transaction<'_, Postgres>,
    project: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO outbox (id, project, kind, payload) VALUES ($1, $2, $3, $4)")
        .bind(Uuid::new_v4())
        .bind(project)
        .bind(kind)
        .bind(payload)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
