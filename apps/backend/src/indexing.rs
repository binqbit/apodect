//! Finalized contract projections. These records describe observed state; all
//! authorization and available-budget decisions remain with the contracts.

use std::collections::BTreeMap;

use alloy_primitives::{Address, B256, U256};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    chains::{
        ethereum::Ethereum,
        near::{
            CoordinatorCommand, CoordinatorDomain, CoordinatorPlan, CoordinatorRequest,
            CoordinatorState, NearBlock, NearClient,
        },
        solana::{self, Client as SolanaClient},
    },
    config::{Chain, Config, ProjectConfig, number_word, word},
    rpc::Rpc,
    storage::{Store, emit},
};

pub struct Indexer {
    pub config: Config,
    pub store: Store,
    pub near: NearClient,
    pub ethereum: Ethereum,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectView {
    pub block: NearBlock,
    pub state: CoordinatorState,
    pub domains: BTreeMap<Chain, CoordinatorDomain>,
    pub plans: Vec<CoordinatorPlan>,
    pub commands: Vec<CoordinatorCommand>,
    pub invoices: BTreeMap<u64, Value>,
    pub requests: BTreeMap<Chain, Vec<CoordinatorRequest>>,
    pub queue_head: Option<CoordinatorRequest>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalTask {
    pub id: String,
    pub agent: String,
    pub state: u64,
    pub paused: bool,
    pub started: bool,
    pub available: u64,
    pub issued: u64,
    pub recovered: u64,
    pub pending_outflow: u64,
    pub control_nonce: Option<u64>,
    pub live_request: Option<String>,
    pub report: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalRequest {
    pub id: String,
    pub task: String,
    pub amount: u64,
    pub kind: u64,
    pub state: u64,
    pub purpose: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalFunding {
    pub id: String,
    pub payload: String,
    pub evidence: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalSnapshot {
    pub chain: Chain,
    pub height: u64,
    pub block_hash: String,
    pub state: Value,
    pub tasks: Vec<LocalTask>,
    pub requests: Vec<LocalRequest>,
    pub funding: Vec<LocalFunding>,
}

impl Indexer {
    pub async fn connect(config: Config, store: Store) -> Result<Self> {
        let near =
            NearClient::connect(Rpc::new(&config.near.rpc_url)?, &config.near.genesis).await?;
        let ethereum = Ethereum::new(
            Rpc::new(&config.ethereum.rpc_url)?,
            config.ethereum.chain_id,
            config
                .ethereum
                .genesis_hash
                .parse()
                .context("Invalid Ethereum genesis hash")?,
        );
        ethereum.verify_network().await?;
        let indexer = Self {
            config,
            store,
            near,
            ethereum,
        };
        for project in &indexer.config.projects {
            indexer.solana(project)?.validate_deployment().await?;
            indexer
                .store
                .register_project(
                    &indexer.config.project_key(project),
                    &serde_json::to_value(project)?,
                )
                .await?;
        }
        Ok(indexer)
    }

    pub fn solana(&self, project: &ProjectConfig) -> Result<SolanaClient> {
        let domain = &project.domains[&Chain::Solana];
        SolanaClient::new(
            Rpc::new(&self.config.solana.rpc_url)?,
            solana::Binding {
                genesis_hash: self.config.solana.genesis_hash.clone(),
                project: domain
                    .vault
                    .parse()
                    .context("Invalid Solana Project address")?,
                mint: domain
                    .asset
                    .parse()
                    .context("Invalid Solana token address")?,
                domain: word(&domain.domain)?,
            },
            None,
        )
    }

    pub async fn index_project(&self, project: &ProjectConfig) -> Result<ProjectView> {
        let block = self.near.finalized_block().await?;
        let state = self
            .near
            .coordinator_state(&project.coordinator, &block)
            .await?;
        ensure!(
            word(&state.project_id)? == word(&project.project_id)?,
            "Coordinator Project identity mismatch"
        );
        let mut domains = BTreeMap::new();
        for (chain, configured) in &project.domains {
            let domain = self
                .near
                .coordinator_domain(&project.coordinator, &configured.domain, &block)
                .await?;
            ensure!(
                word(&domain.domain)? == word(&configured.domain)?
                    && domain.chain == chain.to_string(),
                "Coordinator domain identity mismatch"
            );
            ensure!(
                same_address(*chain, &domain.vault, &configured.vault)?
                    && same_address(*chain, &domain.asset, &configured.asset)?,
                "Coordinator domain custody differs from configuration"
            );
            if *chain == Chain::Solana {
                ensure!(
                    domain.program.as_deref().map(word).transpose()?
                        == Some(apodect_solana::ID.to_bytes()),
                    "Coordinator Solana program mismatch"
                );
            }
            ensure!(
                domain.available == domain.free.saturating_sub(domain.pending_reserves),
                "Coordinator available balance is inconsistent"
            );
            domains.insert(*chain, domain);
        }

        let mut plans = Vec::new();
        for id in 1..state.next_plan {
            let plan = self
                .near
                .coordinator_plan(&project.coordinator, &number_word(id), &block)
                .await?
                .context("Missing finalized coordinator plan")?;
            ensure!(
                word(&plan.plan)? == word(&number_word(id))?,
                "Coordinator plan identity mismatch"
            );
            plans.push(plan);
        }
        let mut commands = Vec::new();
        let mut from = 1;
        while from < state.next_command {
            let page = self
                .near
                .coordinator_commands(&project.coordinator, from, 100, &block)
                .await?;
            ensure!(
                !page.is_empty(),
                "Incomplete finalized coordinator command history"
            );
            for command in page {
                ensure!(
                    word(&command.operation)? == word(&number_word(from))?,
                    "Coordinator command sequence mismatch"
                );
                commands.push(command);
                from += 1;
            }
        }
        ensure!(
            from == state.next_command,
            "Coordinator command history exceeds its counter"
        );
        let mut invoices = BTreeMap::new();
        for id in 1..state.next_invoice {
            let invoice: Value = self
                .near
                .view(
                    &project.coordinator,
                    "get_invoice",
                    &json!({"invoice":id}),
                    &block,
                )
                .await?;
            ensure!(!invoice.is_null(), "Missing finalized coordinator invoice");
            invoices.insert(id, invoice);
        }

        let mut requests = BTreeMap::new();
        for (chain, domain) in &project.domains {
            let mut indexed = Vec::new();
            if let Some(snapshot) = self
                .saved_local(&self.config.project_key(project), *chain)
                .await?
            {
                for request in snapshot.requests {
                    let observed: Option<CoordinatorRequest> = self
                        .near
                        .view(
                            &project.coordinator,
                            "get_request",
                            &json!({"domain":domain.domain,"request":request.id}),
                            &block,
                        )
                        .await?;
                    if let Some(observed) = observed {
                        ensure!(
                            word(&observed.request)? == word(&request.id)?
                                && word(&observed.domain)? == word(&domain.domain)?,
                            "Coordinator request identity mismatch"
                        );
                        indexed.push(observed);
                    }
                }
            }
            requests.insert(*chain, indexed);
        }
        let queue_head = self
            .near
            .view(&project.coordinator, "get_queue_head", &json!({}), &block)
            .await?;

        let view = ProjectView {
            block,
            state,
            domains,
            plans,
            commands,
            invoices,
            requests,
            queue_head,
        };
        Self::save_project(&self.store, &self.config.project_key(project), &view).await?;
        Ok(view)
    }

    pub async fn index_local(&self, project: &ProjectConfig) -> Result<Vec<LocalSnapshot>> {
        let mut snapshots = Vec::new();
        for chain in Chain::ALL {
            snapshots.push(self.index_chain(project, chain).await?);
        }
        Ok(snapshots)
    }

    pub(crate) async fn index_chain(
        &self,
        project: &ProjectConfig,
        chain: Chain,
    ) -> Result<LocalSnapshot> {
        let snapshot = self.local_snapshot(project, chain).await?;
        Self::save_local(&self.store, &self.config.project_key(project), &snapshot).await?;
        Ok(snapshot)
    }

    pub async fn local_snapshot(
        &self,
        project: &ProjectConfig,
        chain: Chain,
    ) -> Result<LocalSnapshot> {
        match chain {
            Chain::Ethereum => self.ethereum_snapshot(project).await,
            Chain::Solana => self.solana_snapshot(project).await,
            Chain::Near => self.near_snapshot(project).await,
        }
    }

    async fn near_snapshot(&self, project: &ProjectConfig) -> Result<LocalSnapshot> {
        let domain = &project.domains[&Chain::Near];
        let block = self.near.finalized_block().await?;
        let state = self.near.local_project(&domain.vault, &block).await?;
        let bridge = self.near.local_bridge(&domain.vault, &block).await?;
        ensure!(
            state.token_id == domain.asset,
            "NEAR Project token mismatch"
        );
        ensure!(
            bridge.domain.as_deref().map(word).transpose()? == Some(word(&domain.domain)?),
            "NEAR Project domain mismatch"
        );
        let mut tasks = Vec::new();
        for id in 0..state.next_task {
            let task = self
                .near
                .local_task(&domain.vault, id, &block)
                .await?
                .context("Missing finalized NEAR task")?;
            tasks.push(LocalTask {
                id: number_word(id),
                agent: task.agent,
                state: state_number(&task.state, false)?,
                paused: task.paused,
                started: task.started,
                available: task.available.try_into()?,
                issued: task.issued.try_into()?,
                recovered: task.recovered.try_into()?,
                pending_outflow: task.pending_outflow.try_into()?,
                control_nonce: None,
                live_request: task.live_request.map(number_word),
                report: task.report.map(hex::encode),
            });
        }
        let mut requests = Vec::new();
        for id in 0..state.next_request {
            let request = self
                .near
                .local_request(&domain.vault, id, &block)
                .await?
                .context("Missing finalized NEAR request")?;
            ensure!(
                request.task_id < state.next_task,
                "NEAR request references an unknown task"
            );
            requests.push(LocalRequest {
                id: number_word(id),
                task: number_word(request.task_id),
                amount: request.amount.try_into()?,
                kind: kind_number(&request.kind)?,
                state: state_number(&request.state, true)?,
                purpose: hex::encode(request.purpose),
            });
        }
        let mut funding = Vec::new();
        let payer: [u8; 32] = Sha256::digest(state.owner_id.as_bytes()).into();
        for id in 1..bridge.next_funding {
            let id = number_word(id);
            let payload: Option<String> = self
                .near
                .view(
                    &domain.vault,
                    "get_funding",
                    &json!({"funding_id":id}),
                    &block,
                )
                .await?;
            let payload = payload.context("Missing finalized NEAR funding receipt")?;
            validate_funding(&payload, &domain.domain, &id, payer)?;
            funding.push(LocalFunding {
                id,
                payload,
                evidence: None,
            });
        }
        Ok(LocalSnapshot {
            chain: Chain::Near,
            height: block.height,
            block_hash: block.hash,
            state: json!({"free":state.free.to_string(),"held":bridge.held.to_string(),
                "pending_owner_outflow":state.pending_owner_outflow.to_string(),"accounted_custody":bridge.accounted_custody.to_string(),
                "task_count":state.next_task,"request_count":state.next_request,"sequence":bridge.sequence,
                "owner":state.owner_id,"authority":state.authority_id}),
            tasks,
            requests,
            funding,
        })
    }

    async fn ethereum_snapshot(&self, project: &ProjectConfig) -> Result<LocalSnapshot> {
        let configured = &project.domains[&Chain::Ethereum];
        let vault: Address = configured.vault.parse()?;
        let domain = B256::from(word(&configured.domain)?);
        let block = self.ethereum.finalized_block().await?;
        ensure!(
            project.ethereum_start_block <= block.height,
            "Ethereum deployment start block is not finalized"
        );
        let state = self.ethereum.project(vault, &block).await?;
        ensure!(
            state.domain == domain && state.token == configured.asset.parse::<Address>()?,
            "Ethereum Project domain or token mismatch"
        );
        let mut tasks = Vec::new();
        for id in 1..=u64::try_from(state.task_count)? {
            let record = self.ethereum.task(vault, U256::from(id), &block).await?;
            let task = record.task;
            tasks.push(LocalTask {
                id: number_word(id),
                agent: task.agent.to_string(),
                state: u64::from(task.status) - 1,
                paused: task.paused,
                started: task.started,
                available: task.available.try_into()?,
                issued: task.issued.try_into()?,
                recovered: task.recovered.try_into()?,
                pending_outflow: 0,
                control_nonce: Some(record.control_nonce),
                report: (task.reportRef != B256::ZERO).then(|| hex::encode(task.reportRef)),
                live_request: if task.liveRequestId == U256::ZERO {
                    None
                } else {
                    Some(number_word(task.liveRequestId.try_into()?))
                },
            });
        }
        let mut requests = Vec::new();
        for id in 1..=u64::try_from(state.request_count)? {
            let request = self.ethereum.request(vault, U256::from(id), &block).await?;
            ensure!(
                request.taskId > U256::ZERO && request.taskId <= state.task_count,
                "Ethereum request references an unknown task"
            );
            requests.push(LocalRequest {
                id: number_word(id),
                task: number_word(request.taskId.try_into()?),
                amount: request.amount.try_into()?,
                state: u64::from(request.status) - 1,
                kind: u64::from(request.kind),
                purpose: hex::encode(request.purposeRef),
            });
        }

        let previous = self
            .saved_local(&self.config.project_key(project), Chain::Ethereum)
            .await?;
        let mut funding = previous
            .as_ref()
            .map(|snapshot| snapshot.funding.clone())
            .unwrap_or_default();
        let from = previous
            .as_ref()
            .and_then(|snapshot| snapshot.state["funding_next_block"].as_u64())
            .unwrap_or(project.ethereum_start_block);
        let mut next_block = from;
        if from <= block.height {
            let through = if block.height - from < 2000 {
                block.clone()
            } else {
                self.ethereum.finalized_block_at(from + 1999).await?
            };
            let receipts = self
                .ethereum
                .list_funding(vault, domain, from, &through)
                .await?;
            let mut payer = [0; 32];
            payer[12..].copy_from_slice(state.owner.as_slice());
            for receipt in receipts {
                let id = hex::encode(&receipt.payload[32..64]);
                let payload = hex::encode(&receipt.payload);
                validate_funding(&payload, &configured.domain, &id, payer)?;
                if let Some(existing) = funding.iter().find(|item| item.id == id) {
                    ensure!(
                        existing.payload == payload,
                        "Immutable Ethereum funding receipt changed"
                    );
                } else {
                    funding.push(LocalFunding {
                        id,
                        payload,
                        evidence: Some(serde_json::to_value(receipt)?),
                    });
                }
            }
            next_block = through
                .height
                .checked_add(1)
                .context("Ethereum funding cursor exhausted")?;
        }
        Ok(LocalSnapshot {
            chain: Chain::Ethereum,
            height: block.height,
            block_hash: block.hash.to_string(),
            state: json!({"free":state.free.to_string(),"held":state.held.to_string(),"allocated":state.allocated.to_string(),
                "token_balance":state.token_balance.to_string(),"task_count":state.task_count.to_string(),
                "request_count":state.request_count.to_string(),"sequence":state.checkpoint_sequence.to_string(),
                "owner":state.owner.to_string(),"authority":state.authority.to_string(),"funding_next_block":next_block}),
            tasks,
            requests,
            funding,
        })
    }

    async fn solana_snapshot(&self, project: &ProjectConfig) -> Result<LocalSnapshot> {
        let client = self.solana(project)?;
        let project_state = client.read_project().await?;
        let start_slot = project_state.slot;
        let mut last_read_slot = start_slot;
        let mut cursor = 0;
        let mut tasks = Vec::new();
        let mut requests = Vec::new();
        while cursor < project_state.value.task_count {
            let page = client
                .scan_tasks(cursor, (project_state.value.task_count - cursor).min(100))
                .await?;
            ensure!(
                page.project.slot >= start_slot,
                "Solana finalized task scan predates the Project snapshot"
            );
            last_read_slot = last_read_slot.max(page.slot);
            for record in page.tasks {
                let id = hex::encode(record.address.as_ref());
                let task = record.task;
                let mut request_cursor = 0;
                while request_cursor < task.request_count {
                    let page = client
                        .scan_requests(
                            record.address,
                            request_cursor,
                            (task.request_count - request_cursor).min(100),
                        )
                        .await?;
                    ensure!(
                        page.slot >= start_slot,
                        "Solana finalized request scan predates the Project snapshot"
                    );
                    last_read_slot = last_read_slot.max(page.slot);
                    for record in page.requests {
                        requests.push(LocalRequest {
                            id: hex::encode(record.address.as_ref()),
                            task: id.clone(),
                            amount: record.request.amount,
                            kind: record.request.kind as u64,
                            state: record.request.state as u64,
                            purpose: hex::encode(record.request.purpose),
                        });
                    }
                    request_cursor = page.next_request;
                }
                tasks.push(LocalTask {
                    id,
                    agent: task.agent.to_string(),
                    state: task.lifecycle as u64,
                    paused: task.paused,
                    started: task.started,
                    available: task.available,
                    issued: task.issued,
                    recovered: task.recovered,
                    pending_outflow: 0,
                    control_nonce: Some(task.control_nonce),
                    live_request: task.pending.map(|key| hex::encode(key.as_ref())),
                    report: (task.report != [0; 32]).then(|| hex::encode(task.report)),
                });
            }
            cursor = page.next_task;
        }
        let block = self.near.finalized_block().await?;
        let domain = self
            .near
            .coordinator_domain(
                &project.coordinator,
                &project.domains[&Chain::Solana].domain,
                &block,
            )
            .await?;
        let payer = anchor_lang::prelude::Pubkey::new_from_array(word(&domain.owner_identity)?);
        let funding = client.funding_receipts(payer).await?;
        ensure!(
            funding.slot >= start_slot,
            "Solana finalized funding scan predates the Project snapshot"
        );
        last_read_slot = last_read_slot.max(funding.slot);
        let mut funding: Vec<_> = funding
            .value
            .into_iter()
            .map(|receipt| LocalFunding {
                id: hex::encode(receipt.id),
                payload: hex::encode(receipt.payload),
                evidence: None,
            })
            .collect();
        funding.sort_by(|left, right| left.id.cmp(&right.id));
        let end = client.finalized_block().await?;
        ensure!(
            end.slot >= last_read_slot,
            "Solana finalized slot regressed during indexing"
        );
        Ok(LocalSnapshot {
            chain: Chain::Solana,
            height: end.slot,
            block_hash: end.hash,
            state: json!({"free":project_state.value.free.to_string(),"held":project_state.value.held.to_string(),
                "task_count":project_state.value.task_count,"sequence":project_state.value.checkpoint_sequence,
                "authority":project_state.value.authority.to_string(),"read_start_slot":start_slot}),
            tasks,
            requests,
            funding,
        })
    }

    async fn saved_local(&self, project: &str, chain: Chain) -> Result<Option<LocalSnapshot>> {
        let row = sqlx::query(
            "SELECT height::text, block_hash, state FROM vault_views WHERE project=$1 AND chain=$2",
        )
        .bind(project)
        .bind(chain.to_string())
        .fetch_optional(&self.store.pool)
        .await?;
        row.map(|row| {
            let mut snapshot: LocalSnapshot = serde_json::from_value(row.try_get("state")?)?;
            snapshot.height = row.try_get::<String, _>("height")?.parse()?;
            snapshot.block_hash = row.try_get("block_hash")?;
            Ok(snapshot)
        })
        .transpose()
    }

    async fn save_project(store: &Store, project: &str, view: &ProjectView) -> Result<()> {
        let mut tx = store.pool.begin().await?;
        sqlx::query("SELECT id FROM projects WHERE id=$1 FOR UPDATE")
            .bind(project)
            .fetch_one(&mut *tx)
            .await?;
        let previous = sqlx::query("SELECT height::text, block_hash, state, domains, plans, commands, invoices, requests, queue_head FROM project_views WHERE project=$1")
            .bind(project).fetch_optional(&mut *tx).await?;
        let values = [
            serde_json::to_value(&view.state)?,
            serde_json::to_value(&view.domains)?,
            serde_json::to_value(&view.plans)?,
            serde_json::to_value(&view.commands)?,
            serde_json::to_value(&view.invoices)?,
            serde_json::to_value(&view.requests)?,
            serde_json::to_value(&view.queue_head)?,
        ];
        let changed = if let Some(previous) = previous {
            check_position(
                previous.try_get::<String, _>("height")?.parse()?,
                &previous.try_get::<String, _>("block_hash")?,
                view.block.height,
                &view.block.hash,
            )?;
            let changed = [
                "state",
                "domains",
                "plans",
                "commands",
                "invoices",
                "requests",
                "queue_head",
            ]
            .into_iter()
            .zip(&values)
            .map(|(name, value)| Ok(previous.try_get::<Value, _>(name)? != *value))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .any(|changed| changed);
            if previous.try_get::<String, _>("height")?.parse::<u64>()? == view.block.height {
                // Newly indexed local requests may reveal coordinator records at
                // the same finalized height. Existing records must stay immutable.
                for (name, value) in [
                    "state",
                    "domains",
                    "plans",
                    "commands",
                    "invoices",
                    "queue_head",
                ]
                .into_iter()
                .zip([
                    &values[0], &values[1], &values[2], &values[3], &values[4], &values[6],
                ]) {
                    ensure!(
                        previous.try_get::<Value, _>(name)? == *value,
                        "Finalized NEAR state changed within one block"
                    );
                }
                let old_requests: BTreeMap<Chain, Vec<CoordinatorRequest>> =
                    serde_json::from_value(previous.try_get("requests")?)?;
                for (chain, requests) in old_requests {
                    for old in requests {
                        let current = view.requests.get(&chain).and_then(|requests| {
                            requests
                                .iter()
                                .find(|request| request.request == old.request)
                        });
                        ensure!(
                            current.map(serde_json::to_value).transpose()?
                                == Some(serde_json::to_value(old)?),
                            "Finalized coordinator request changed within one block"
                        );
                    }
                }
            }
            changed
        } else {
            true
        };
        sqlx::query(
            r#"
            INSERT INTO project_views
                (project, height, block_hash, state, domains, plans, commands, invoices, requests, queue_head)
            VALUES ($1, $2::text::numeric, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (project) DO UPDATE SET
                height = excluded.height,
                block_hash = excluded.block_hash,
                state = excluded.state,
                domains = excluded.domains,
                plans = excluded.plans,
                commands = excluded.commands,
                invoices = excluded.invoices,
                requests = excluded.requests,
                queue_head = excluded.queue_head,
                observed_at = now()
        "#,
        )
        .bind(project)
        .bind(view.block.height.to_string())
        .bind(&view.block.hash)
        .bind(&values[0])
        .bind(&values[1])
        .bind(&values[2])
        .bind(&values[3])
        .bind(&values[4])
        .bind(&values[5])
        .bind(&values[6])
        .execute(&mut *tx)
        .await?;
        if changed {
            emit(
                &mut tx,
                project,
                "project.updated",
                json!({"height":view.block.height,"block_hash":view.block.hash}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn save_local(store: &Store, project: &str, snapshot: &LocalSnapshot) -> Result<()> {
        let mut tx = store.pool.begin().await?;
        sqlx::query("SELECT id FROM projects WHERE id=$1 FOR UPDATE")
            .bind(project)
            .fetch_one(&mut *tx)
            .await?;
        let previous = sqlx::query(
            "SELECT height::text,block_hash,state FROM vault_views WHERE project=$1 AND chain=$2",
        )
        .bind(project)
        .bind(snapshot.chain.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let encoded = serde_json::to_value(snapshot)?;
        let changed = if let Some(previous) = previous {
            let previous_height = previous.try_get::<String, _>("height")?.parse()?;
            let previous_state: Value = previous.try_get("state")?;
            check_position(
                previous_height,
                &previous.try_get::<String, _>("block_hash")?,
                snapshot.height,
                &snapshot.block_hash,
            )?;
            if previous_height == snapshot.height && snapshot.chain != Chain::Solana {
                // Ethereum funding history may still be backfilling. Its pinned
                // contract state, like NEAR's, must remain immutable at this block.
                ensure!(
                    pinned_snapshot(previous_state.clone()) == pinned_snapshot(encoded.clone()),
                    "Finalized native state changed within one block"
                );
            }
            if snapshot.chain == Chain::Ethereum {
                let previous_cursor = previous_state["state"]["funding_next_block"]
                    .as_u64()
                    .context("Missing persisted Ethereum funding cursor")?;
                let next_cursor = snapshot.state["funding_next_block"]
                    .as_u64()
                    .context("Missing Ethereum funding cursor")?;
                ensure!(
                    next_cursor >= previous_cursor,
                    "Ethereum funding scan regressed"
                );
                let previous_funding: Vec<LocalFunding> =
                    serde_json::from_value(previous_state["funding"].clone())?;
                for receipt in previous_funding {
                    ensure!(
                        snapshot
                            .funding
                            .iter()
                            .any(|current| current.id == receipt.id
                                && current.payload == receipt.payload
                                && current.evidence == receipt.evidence),
                        "Previously indexed Ethereum funding provenance changed"
                    );
                }
            }
            comparable_snapshot(previous_state) != comparable_snapshot(encoded.clone())
        } else {
            true
        };
        sqlx::query(
            r#"
            INSERT INTO vault_views (project, chain, height, block_hash, state)
            VALUES ($1, $2, $3::text::numeric, $4, $5)
            ON CONFLICT (project, chain) DO UPDATE SET
                height = excluded.height,
                block_hash = excluded.block_hash,
                state = excluded.state,
                observed_at = now()
        "#,
        )
        .bind(project)
        .bind(snapshot.chain.to_string())
        .bind(snapshot.height.to_string())
        .bind(&snapshot.block_hash)
        .bind(encoded)
        .execute(&mut *tx)
        .await?;
        if changed {
            emit(&mut tx,project,"vault.updated",json!({"chain":snapshot.chain,"height":snapshot.height,"block_hash":snapshot.block_hash})).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

fn pinned_snapshot(mut snapshot: Value) -> Value {
    if let Some(object) = snapshot.as_object_mut() {
        object.remove("funding");
    }
    if let Some(state) = snapshot["state"].as_object_mut() {
        state.remove("funding_next_block");
    }
    comparable_snapshot(snapshot)
}

fn comparable_snapshot(mut snapshot: Value) -> Value {
    if let Some(object) = snapshot.as_object_mut() {
        object.remove("height");
        object.remove("block_hash");
    }
    if let Some(state) = snapshot["state"].as_object_mut() {
        state.remove("read_start_slot");
        state.remove("funding_next_block");
    }
    snapshot
}

fn check_position(previous: u64, previous_hash: &str, next: u64, next_hash: &str) -> Result<()> {
    ensure!(next >= previous, "Finalized chain observation regressed");
    ensure!(
        next != previous || previous_hash == next_hash,
        "Conflicting finalized block hashes"
    );
    Ok(())
}

fn same_address(chain: Chain, left: &str, right: &str) -> Result<bool> {
    Ok(match chain {
        Chain::Ethereum => left.parse::<Address>()? == right.parse::<Address>()?,
        Chain::Solana => {
            left.parse::<anchor_lang::prelude::Pubkey>()?
                == right.parse::<anchor_lang::prelude::Pubkey>()?
        }
        Chain::Near => left == right,
    })
}

fn state_number(value: &str, request: bool) -> Result<u64> {
    let states: &[&str] = if request {
        &["Pending", "Queued", "Funded", "Rejected", "Removed"]
    } else {
        &["Active", "Stopped", "OutOfBudget", "Completed", "Canceled"]
    };
    states
        .iter()
        .position(|state| *state == value)
        .map(|position| position as u64)
        .context("Unknown NEAR lifecycle state")
}

fn kind_number(value: &str) -> Result<u64> {
    match value {
        "Initial" => Ok(0),
        "Supplement" => Ok(1),
        "Resume" => Ok(2),
        "Restoration" => Ok(3),
        _ => bail!("Unknown NEAR request kind"),
    }
}

fn validate_funding(payload: &str, domain: &str, id: &str, payer: [u8; 32]) -> Result<()> {
    let bytes = hex::decode(payload.strip_prefix("0x").unwrap_or(payload))?;
    ensure!(bytes.len() == 160, "Invalid funding receipt length");
    ensure!(
        bytes[..32] == word(domain)? && bytes[32..64] == word(id)? && bytes[128..160] == payer,
        "Funding receipt provenance mismatch"
    );
    ensure!(
        bytes[96..120] == [0; 24],
        "Funding amount exceeds coordinated u64 range"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{
        PgPool,
        postgres::{PgConnectOptions, PgPoolOptions},
    };
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL via APODECT_TEST_DATABASE_URL"]
    async fn projection_updates_are_atomic_monotonic_and_emit_only_changes() -> Result<()> {
        let url = std::env::var("APODECT_TEST_DATABASE_URL")?;
        let admin = PgPool::connect(&url).await?;
        let schema = format!("index_test_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await?;
        let options: PgConnectOptions = url.parse()?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.options([("search_path", schema.as_str())]))
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0001_initial.sql"))
            .execute(&pool)
            .await?;
        let store = Store::new(pool.clone());
        store
            .register_project("project", &json!({"coordinator":"test.near"}))
            .await?;
        let mut view = ProjectView {
            block: NearBlock {
                height: u64::MAX - 2,
                hash: "first".into(),
            },
            state: CoordinatorState {
                owner_id: "owner.near".into(),
                authority_id: "authority.near".into(),
                project_id: number_word(1),
                mpc_id: "mpc.near".into(),
                sign_key: String::new(),
                read_key: String::new(),
                sign_path: "project".into(),
                queue_head: 0,
                queue_length: 0,
                next_plan: 1,
                next_command: 1,
                next_invoice: 1,
                retry_limit: 3,
            },
            domains: BTreeMap::new(),
            plans: vec![],
            commands: vec![],
            invoices: BTreeMap::new(),
            requests: BTreeMap::new(),
            queue_head: None,
        };

        Indexer::save_project(&store, "project", &view).await?;
        Indexer::save_project(&store, "project", &view).await?;
        view.block.height += 1;
        view.block.hash = "second".into();
        Indexer::save_project(&store, "project", &view).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            count, 1,
            "An unchanged finalized refresh must not publish another update"
        );

        view.state.queue_length = 1;
        assert!(
            Indexer::save_project(&store, "project", &view)
                .await
                .is_err(),
            "One finalized block cannot produce different coordinator states"
        );
        view.block.height += 1;
        view.block.hash = "third".into();
        Indexer::save_project(&store, "project", &view).await?;
        let persisted: String = sqlx::query_scalar("SELECT height::text FROM project_views")
            .fetch_one(&pool)
            .await?;
        assert_eq!(persisted, u64::MAX.to_string());
        view.block.hash = "conflicting".into();
        assert!(
            Indexer::save_project(&store, "project", &view)
                .await
                .is_err()
        );
        view.block.height -= 1;
        assert!(
            Indexer::save_project(&store, "project", &view)
                .await
                .is_err()
        );
        let row = sqlx::query("SELECT height::text,block_hash,state FROM project_views")
            .fetch_one(&pool)
            .await?;
        assert_eq!(row.try_get::<String, _>("height")?, u64::MAX.to_string());
        assert_eq!(row.try_get::<String, _>("block_hash")?, "third");
        assert_eq!(row.try_get::<Value, _>("state")?["queue_length"], 1);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            count, 2,
            "Rejected projections must roll back their notifications"
        );

        let mut local = LocalSnapshot {
            chain: Chain::Ethereum,
            height: 100,
            block_hash: "native".into(),
            state: json!({"free":"10", "funding_next_block":50}),
            tasks: vec![],
            requests: vec![],
            funding: vec![],
        };
        Indexer::save_local(&store, "project", &local).await?;
        local.state["funding_next_block"] = json!(100);
        Indexer::save_local(&store, "project", &local).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 3, "A scan cursor alone is not a contract change");
        local.funding.push(LocalFunding {
            id: number_word(1),
            payload: "historical receipt".into(),
            evidence: None,
        });
        Indexer::save_local(&store, "project", &local).await?;
        local.state["funding_next_block"] = json!(99);
        assert!(
            Indexer::save_local(&store, "project", &local)
                .await
                .is_err(),
            "A concurrent stale indexer must not regress funding history"
        );
        local.state["funding_next_block"] = json!(100);
        let receipt = local.funding.pop().unwrap();
        assert!(
            Indexer::save_local(&store, "project", &local)
                .await
                .is_err(),
            "A refresh must preserve immutable funding provenance"
        );
        local.funding.push(receipt);
        local.state["free"] = json!("11");
        assert!(
            Indexer::save_local(&store, "project", &local)
                .await
                .is_err(),
            "Backfilled receipts cannot authorize changing state at a pinned block"
        );
        local.state["free"] = json!("10");
        local.block_hash = "conflicting".into();
        assert!(
            Indexer::save_local(&store, "project", &local)
                .await
                .is_err()
        );
        local.block_hash = "native".into();
        local.height -= 1;
        assert!(
            Indexer::save_local(&store, "project", &local)
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            count, 4,
            "Native backfill emits one real change; rejected reads emit none"
        );

        view.block.height = u64::MAX;
        view.block.hash = "third".into();
        view.requests.insert(
            Chain::Near,
            vec![CoordinatorRequest {
                domain: number_word(1),
                request: number_word(2),
                task: number_word(3),
                amount: 10,
                kind: 0,
                purpose: number_word(4),
                local_state: 0,
                status: "Pending".into(),
                plan: None,
                approval_position: None,
            }],
        );
        Indexer::save_project(&store, "project", &view).await?;
        view.requests.get_mut(&Chain::Near).unwrap()[0].amount = 11;
        assert!(
            Indexer::save_project(&store, "project", &view)
                .await
                .is_err(),
            "Discovering a request at a pinned block must not permit changing existing records"
        );
        view.requests.clear();
        assert!(
            Indexer::save_project(&store, "project", &view)
                .await
                .is_err(),
            "A same-block refresh cannot lose previously discovered requests"
        );

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&admin)
            .await?;
        admin.close().await;
        Ok(())
    }
}
