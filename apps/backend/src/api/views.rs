//! Read models for people. Financial values remain decimal strings, and each
//! chain retains its observation time rather than implying atomic finality.
use std::collections::BTreeMap;

use alloy_primitives::Address;
use anchor_lang::prelude::Pubkey;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::ApiError;
use crate::{
    accounts::Wallet,
    application::Application,
    chains::near::{
        CoordinatorCommand, CoordinatorDomain, CoordinatorPlan, CoordinatorRequest,
        CoordinatorState,
    },
    config::{Chain, ProjectConfig, word},
    indexing::LocalSnapshot,
};

struct Projection {
    state: CoordinatorState,
    domains: BTreeMap<Chain, CoordinatorDomain>,
    plans: Vec<CoordinatorPlan>,
    commands: Vec<CoordinatorCommand>,
    requests: BTreeMap<Chain, Vec<CoordinatorRequest>>,
    queue_head: Option<CoordinatorRequest>,
    invoices: BTreeMap<u64, Value>,
    observed_at: DateTime<Utc>,
    vaults: BTreeMap<Chain, (LocalSnapshot, DateTime<Utc>)>,
    operations: Vec<Value>,
}

async fn load(app: &Application, key: &str) -> Result<Option<Projection>> {
    let mut tx = app.indexer.store.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query(
        "SELECT state, domains, plans, commands, requests, queue_head, invoices, observed_at \
         FROM project_views WHERE project=$1",
    )
    .bind(key)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let mut vaults = BTreeMap::new();
    for row in
        sqlx::query("SELECT state, observed_at FROM vault_views WHERE project=$1 ORDER BY chain")
            .bind(key)
            .fetch_all(&mut *tx)
            .await?
    {
        let snapshot: LocalSnapshot = serde_json::from_value(row.try_get("state")?)?;
        vaults.insert(snapshot.chain, (snapshot, row.try_get("observed_at")?));
    }
    // Deliberately select only public status, never signed transaction bytes.
    let operations = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'id', id, 'chain', chain, 'state', state,
            'attempt', attempt, 'error', error,
            'transaction_id', COALESCE(
                transaction->>'hash', transaction->>'signature', transaction->>'transaction_hash'
            )
        )
        FROM effects WHERE project=$1 ORDER BY id
        "#,
    )
    .bind(key)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(Projection {
        state: serde_json::from_value(row.try_get("state")?)?,
        domains: serde_json::from_value(row.try_get("domains")?)?,
        plans: serde_json::from_value(row.try_get("plans")?)?,
        commands: serde_json::from_value(row.try_get("commands")?)?,
        requests: serde_json::from_value(row.try_get("requests")?)?,
        queue_head: serde_json::from_value(row.try_get("queue_head")?)?,
        invoices: serde_json::from_value(row.try_get("invoices")?)?,
        observed_at: row.try_get("observed_at")?,
        vaults,
        operations,
    }))
}

pub(super) async fn summary(
    app: &Application,
    project: &ProjectConfig,
    wallets: &[Wallet],
) -> Result<Value> {
    let key = app.indexer.config.project_key(project);
    let projection = load(app, &key).await?;
    summary_value(app, project, wallets, projection.as_ref())
}

pub(super) async fn detail(
    app: &Application,
    project: &ProjectConfig,
    wallets: &[Wallet],
) -> std::result::Result<Value, ApiError> {
    let key = app.indexer.config.project_key(project);
    let projection = load(app, &key)
        .await
        .map_err(ApiError::service)?
        .ok_or_else(|| {
            ApiError::new(
                actix_web::http::StatusCode::CONFLICT,
                "indexing_pending",
                "The first finalized project snapshot is still being indexed.",
            )
        })?;
    detail_value(app, project, wallets, &projection).map_err(ApiError::service)
}

fn summary_value(
    app: &Application,
    project: &ProjectConfig,
    wallets: &[Wallet],
    projection: Option<&Projection>,
) -> Result<Value> {
    let mut roles = json!({"owner":false,"authority":false,"agent":false,"owner_actor":null,"authority_actor":null,"local_owners":{}});
    let mut available = 0_u128;
    let mut held = 0_u128;
    let mut pending_reserves = 0_u128;
    let mut task_count = 0;
    if let Some(projection) = projection {
        roles = roles_value(app, wallets, projection)?;
        for domain in projection.domains.values() {
            available += u128::from(domain.available);
            held += u128::from(domain.held);
            pending_reserves += u128::from(domain.pending_reserves);
        }
        task_count = projection
            .vaults
            .values()
            .map(|(snapshot, _)| snapshot.tasks.len())
            .sum();
    }
    Ok(json!({
        "key": app.indexer.config.project_key(project),
        "coordinator": project.coordinator,
        "project_id": project.project_id,
        "indexed": projection.is_some_and(|p| p.vaults.len()==3),
        "observed_at": projection.map(|p| p.observed_at),
        "roles": roles,
        "available": available.to_string(),
        "held": held.to_string(),
        "pending_reserves": pending_reserves.to_string(),
        "task_count": task_count,
        "attention_count": projection.map_or(0,|p|p.plans.iter().filter(|plan|plan.requires_attention).count()),
    }))
}

fn roles_value(app: &Application, wallets: &[Wallet], projection: &Projection) -> Result<Value> {
    let mut owner_actor = None;
    let mut authority_actor = None;
    let mut local_owners = BTreeMap::new();
    let mut agent = false;
    for wallet in wallets {
        let chain: Chain = wallet.chain.parse()?;
        if wallet.network != app.accounts.network(chain) {
            continue;
        }
        if chain == Chain::Near {
            if wallet.address == projection.state.owner_id {
                owner_actor = Some(wallet.address.clone());
            }
            if wallet.address == projection.state.authority_id {
                authority_actor = Some(wallet.address.clone());
            }
        }
        let identity = owner_identity(chain, &wallet.address)?;
        if projection
            .domains
            .get(&chain)
            .is_some_and(|domain| word(&domain.owner_identity).ok() == Some(identity))
        {
            local_owners.insert(chain, wallet.address.clone());
        }
        if let Some((snapshot, _)) = projection.vaults.get(&chain) {
            agent |= snapshot
                .tasks
                .iter()
                .any(|task| same_address(chain, &wallet.address, &task.agent));
        }
    }
    Ok(json!({
        "owner": owner_actor.is_some(),
        "authority": authority_actor.is_some(),
        "agent": agent,
        "owner_actor": owner_actor,
        "authority_actor": authority_actor,
        "local_owners": local_owners,
    }))
}

fn owner_identity(chain: Chain, address: &str) -> Result<[u8; 32]> {
    Ok(match chain {
        Chain::Ethereum => {
            let address: Address = address.parse()?;
            let mut bytes = [0; 32];
            bytes[12..].copy_from_slice(address.as_slice());
            bytes
        }
        Chain::Solana => address.parse::<Pubkey>()?.to_bytes(),
        Chain::Near => Sha256::digest(address.as_bytes()).into(),
    })
}
fn same_address(chain: Chain, left: &str, right: &str) -> bool {
    if chain == Chain::Ethereum {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn detail_value(
    app: &Application,
    project: &ProjectConfig,
    wallets: &[Wallet],
    projection: &Projection,
) -> Result<Value> {
    let mut result = summary_value(app, project, wallets, Some(projection))?;
    let mut chains = Vec::new();
    let mut tasks = Vec::new();
    let mut requests = Vec::new();
    for chain in Chain::ALL {
        let domain = projection
            .domains
            .get(&chain)
            .context("Missing coordinator chain projection")?;
        let snapshot = projection.vaults.get(&chain);
        let local_free = snapshot
            .map(|(snapshot, _)| {
                snapshot.state["free"]
                    .as_str()
                    .map(str::to_owned)
                    .context("Missing local free balance")
            })
            .transpose()?;
        chains.push(json!({
            "chain": chain,
            "vault": domain.vault,
            "asset": domain.asset,
            "available": domain.available.to_string(),
            "free": domain.free.to_string(),
            "held": domain.held.to_string(),
            "pending_reserves": domain.pending_reserves.to_string(),
            "local_free": local_free,
            "observed_at": snapshot.map(|(_, at)| at),
            "height": snapshot.map(|(snapshot, _)| snapshot.height.to_string()),
            "block_hash": snapshot.map(|(snapshot, _)| &snapshot.block_hash),
        }));
        let Some((snapshot, _)) = snapshot else {
            continue;
        };
        for task in &snapshot.tasks {
            let mine = wallets.iter().any(|wallet| {
                wallet.chain == chain.as_str()
                    && wallet.network == app.accounts.network(chain)
                    && same_address(chain, &wallet.address, &task.agent)
            });
            tasks.push(json!({
                "id": task.id,
                "chain": chain,
                "agent": task.agent,
                "mine": mine,
                "state": state_name(task.state, false)?,
                "paused": task.paused,
                "started": task.started,
                "available": task.available.to_string(),
                "issued": task.issued.to_string(),
                "recovered": task.recovered.to_string(),
                "pending_outflow": task.pending_outflow.to_string(),
                "live_request": task.live_request,
                "report": task.report,
            }));
        }
        for request in &snapshot.requests {
            let coordinator = projection
                .requests
                .get(&chain)
                .and_then(|requests| requests.iter().find(|item| item.request == request.id));
            let head = projection
                .queue_head
                .as_ref()
                .is_some_and(|head| head.domain == domain.domain && head.request == request.id);
            requests.push(json!({
                "id": request.id,
                "chain": chain,
                "task": request.task,
                "amount": request.amount.to_string(),
                "purpose": request.purpose,
                "kind": kind_name(request.kind)?,
                "local_state": state_name(request.state, true)?,
                "status": coordinator.map_or_else(|| "syncing".to_owned(), |request| request.status.to_lowercase()),
                "approval_position": coordinator.and_then(|request| request.approval_position),
                "plan": coordinator.and_then(|request| request.plan.as_ref()),
                "queue_head": head,
            }));
        }
    }
    let mut plans = Vec::new();
    for plan in &projection.plans {
        let routes = plan.routes.iter().map(|route| Ok(json!({
            "id": route.quote.route,
            "source": domain_chain(projection, &route.quote.source)?,
            "input": route.quote.input.to_string(),
            "min_input": route.quote.min_input.to_string(),
            "output": route.quote.output.to_string(),
            "received": route.received.to_string(),
            "returned": route.returned.to_string(),
            "dispatched": route.dispatched.to_string(),
            "unsettled_input": route.unsettled_input.to_string(),
            "confirmed_transport_expense": route.confirmed_transport_expense.map(|amount| amount.to_string()),
            "expires": route.quote.expires.to_string(),
        }))).collect::<Result<Vec<_>>>()?;
        plans.push(json!({
            "id": plan.plan,
            "destination": domain_chain(projection, &plan.destination)?,
            "principal": plan.principal.to_string(),
            "local_amount": plan.local_amount.to_string(),
            "expires": plan.expires.to_string(),
            "status": plan.status.to_lowercase(),
            "attempts": plan.attempts,
            "requires_attention": plan.requires_attention,
            "error": plan.error,
            "queue_released": plan.queue_released,
            "routes": routes,
        }));
    }
    let commands = projection
        .commands
        .iter()
        .map(|command| {
            let status = [
                "unobserved",
                "succeeded",
                "failed",
                "invalidated",
                "pending",
            ]
            .get(usize::try_from(command.state)?)
            .context("Unknown command status")?;
            let action = [
                "unknown",
                "reserve",
                "expect_delivery",
                "expect_refund",
                "dispatch",
                "allocate",
                "close_plan",
                "reject",
                "out_of_budget",
                "pause",
                "reclaim",
                "continue",
                "cancel",
                "restore_request",
                "owner_withdraw",
                "invalidate",
            ]
            .get(usize::try_from(command.action)?)
            .context("Unknown command action")?;
            Ok(json!({
                "id": command.operation,
                "chain": domain_chain(projection, &command.domain)?,
                "status": status,
                "action": action,
                "plan": command.plan,
                "signature_error": command.signature_error,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let invoices = projection.invoices.iter().map(|(id,invoice)| {
        Ok(json!({
            "id": id.to_string(),
            "amount": decimal_field(invoice, "amount")?,
            "purpose": invoice["purpose"],
            "intended": if invoice["intended"].is_null() { None } else { Some(decimal_field(invoice, "intended")?) },
            "closed": invoice["closed"],
            "response_reference": invoice["response_reference"],
            "actual_funding": decimal_field(invoice, "actual_funding")?,
        }))
    }).collect::<Result<Vec<_>>>()?;
    let object = result
        .as_object_mut()
        .context("Project summary must be an object")?;
    object.extend(serde_json::Map::from_iter([
        ("owner".into(), json!(projection.state.owner_id)),
        ("authority".into(), json!(projection.state.authority_id)),
        ("retry_limit".into(), json!(projection.state.retry_limit)),
        ("chains".into(), json!(chains)),
        ("tasks".into(), json!(tasks)),
        ("requests".into(), json!(requests)),
        ("plans".into(), json!(plans)),
        ("invoices".into(), json!(invoices)),
        ("operations".into(), json!(projection.operations)),
        ("commands".into(), json!(commands)),
    ]));
    Ok(result)
}
fn state_name(state: u64, request: bool) -> Result<&'static str> {
    let states: &[&str] = if request {
        &["pending", "queued", "funded", "rejected", "removed"]
    } else {
        &[
            "active",
            "stopped",
            "out_of_budget",
            "completed",
            "canceled",
        ]
    };
    states
        .get(usize::try_from(state)?)
        .copied()
        .context("Unknown indexed lifecycle state")
}
fn kind_name(kind: u64) -> Result<&'static str> {
    ["initial", "supplement", "resume", "restoration"]
        .get(usize::try_from(kind)?)
        .copied()
        .context("Unknown indexed request kind")
}
fn domain_chain(projection: &Projection, domain: &str) -> Result<Chain> {
    projection
        .domains
        .iter()
        .find(|(_, value)| value.domain == domain)
        .map(|(chain, _)| *chain)
        .context("Unknown plan chain domain")
}
fn decimal_field(value: &Value, name: &str) -> Result<String> {
    let value = value.get(name).context("Missing invoice amount")?;
    if let Some(value) = value.as_u64() {
        return Ok(value.to_string());
    }
    let value = value.as_str().context("Invalid invoice amount")?;
    ensure!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "Invalid invoice decimal amount"
    );
    Ok(value.to_owned())
}
