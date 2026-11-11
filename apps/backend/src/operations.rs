//! Reconcile facts from the native vaults with the NEAR coordinator. Every step
//! can resume independently; only finalized receipts can change its accounting.

use anyhow::{Context, Result, ensure};
use apodect_solana::bridge::action;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    chains::{
        ethereum,
        near::{CoordinatorCommand, CoordinatorPlan},
        protocol::{
            Checkpoint, Evidence, Selection, command_bytes, identity, mpc_signature, parse_command,
        },
    },
    config::{Chain, ProjectConfig, word},
    indexing::{LocalFunding, LocalSnapshot, LocalTask, ProjectView},
    relay::{Intent, Relay},
};

struct RouteBalance {
    received: u64,
    tokens: u64,
    remaining: u64,
}

impl RouteBalance {
    fn new(received: u64, tokens: u64, limit: u64) -> Result<Self> {
        Ok(Self {
            received,
            tokens,
            remaining: limit
                .checked_sub(received)
                .context("Route receipts exceed confirmed capacity")?,
        })
    }

    fn collectable(&self, refund: bool) -> bool {
        self.remaining > 0 && self.tokens > 0 && (refund || self.tokens >= self.remaining)
    }
}

impl Relay {
    pub async fn reconcile_project(&mut self, project: &ProjectConfig) -> Result<()> {
        self.recover(project).await?;
        let view = self.indexer.index_project(project).await?;
        let snapshots = self.indexer.index_local(project).await?;
        let mut completed = true;

        for snapshot in &snapshots {
            for funding in &snapshot.funding {
                completed &= report(
                    self.observe_funding(project, snapshot.chain, funding).await,
                    "funding",
                    &funding.id,
                );
            }
            completed &= report(
                self.synchronize_local(project, &view, snapshot).await,
                "local state",
                &snapshot.chain.to_string(),
            );
        }
        for command in &view.commands {
            completed &= report(
                self.reconcile_command(project, &view, command).await,
                "command",
                &command.operation,
            );
        }
        for plan in &view.plans {
            completed &= report(
                self.reconcile_plan(project, &view, plan).await,
                "plan",
                &plan.plan,
            );
        }
        completed &= report(
            self.advance_queue(project, &view).await,
            "approval queue",
            &view.state.queue_head.to_string(),
        );
        // Refresh the read model after work. A successful relay submission is not
        // reported as a coordinator state change until this finalized read sees it.
        self.indexer.index_project(project).await?;
        ensure!(completed, "Project reconciliation has unresolved errors");
        Ok(())
    }

    async fn synchronize_local(
        &mut self,
        project: &ProjectConfig,
        view: &ProjectView,
        snapshot: &LocalSnapshot,
    ) -> Result<()> {
        let chain = snapshot.chain;
        let domain = &project.domains[&chain].domain;
        for task in &snapshot.tasks {
            let actual: Value = self
                .indexer
                .near
                .view(
                    &project.coordinator,
                    "get_task",
                    &json!({"domain":domain,"task":task.id}),
                    &view.block,
                )
                .await?;
            let request = task
                .live_request
                .as_ref()
                .and_then(|id| snapshot.requests.iter().find(|r| &r.id == id));
            let mut changed = !task_matches(task, &actual);
            if let Some(request) = request {
                let known: Value = self
                    .indexer
                    .near
                    .view(
                        &project.coordinator,
                        "get_request",
                        &json!({"domain":domain,"request":request.id}),
                        &view.block,
                    )
                    .await?;
                changed |= known.is_null() || known["local_state"] != request.state;
            }
            if changed {
                let fingerprint = serde_json::to_string(&(task, request))?;
                let selection = Selection {
                    task: Some(task.id.clone()),
                    request: request.map(|r| r.id.clone()),
                    ..Default::default()
                };
                self.observe_state(
                    project,
                    chain,
                    &format!("task:{}", task.id),
                    &fingerprint,
                    selection,
                )
                .await?;
            }
        }
        let known = &view.domains[&chain];
        let free = decimal(&snapshot.state["free"])?;
        let held = decimal(&snapshot.state["held"])?;
        if free != known.free || held != known.held || known.sequence == 0 {
            self.observe_state(
                project,
                chain,
                "balance",
                &format!("{free}:{held}"),
                Selection::default(),
            )
            .await?;
        }
        Ok(())
    }

    async fn reconcile_command(
        &mut self,
        project: &ProjectConfig,
        view: &ProjectView,
        record: &CoordinatorCommand,
    ) -> Result<()> {
        if matches!(record.state, 1..=3) {
            return Ok(());
        }
        let command = parse_command(&record.command)?;
        ensure!(
            hex::encode(Sha256::digest(command_bytes(&record.command)?)) == record.digest,
            "Coordinator command digest mismatch"
        );
        ensure!(
            command.operation == word(&record.operation)?
                && command.domain == word(&record.domain)?,
            "Coordinator command identity mismatch"
        );
        let chain = binding(project, &record.domain)?;
        let attention = view
            .plans
            .iter()
            .find(|p| word(&p.plan).ok() == Some(command.related))
            .is_some_and(|p| p.requires_attention)
            && command.action != 6;

        let Some(signature) = &record.signature else {
            if !attention && !record.signing {
                self.coordinator(
                    project,
                    &identity(&["signature", &record.operation]),
                    "request_signature",
                    json!({"operation":record.operation}),
                )
                .await?;
            }
            return Ok(());
        };
        let (signature, recovery) = mpc_signature(signature, &record.digest, &view.state.sign_key)?;
        let mut native = self
            .command_state(project, chain, &record.operation)
            .await?;
        if native == 0 && !attention {
            let recipient = self.command_recipient(project, view, record).await?;
            let result = self
                .effect(
                    project,
                    &identity(&["execute", &record.operation]),
                    chain,
                    Intent::Execute {
                        command: record.command.clone(),
                        signature: hex::encode(signature),
                        recovery,
                        recipient,
                    },
                )
                .await;
            // A caught native failure can itself require a checkpoint to release
            // reserved funds. It is not evidence that this command spent money.
            report(result.map(|_| ()), "native execution", &record.operation);
            native = self
                .command_state(project, chain, &record.operation)
                .await?;
        }
        if native == 0 {
            return Ok(());
        }
        let optional = |value: [u8; 32]| (value != [0; 32]).then(|| hex::encode(value));
        let selection = Selection {
            operation: Some(record.operation.clone()),
            plan: optional(command.related),
            // NEAR task and request counters begin at zero. Their presence is
            // determined by the command action, not by a nonzero identity.
            task: (command.action == action::ALLOCATE
                || (action::REJECT..=action::RESTORE_REQUEST).contains(&command.action))
            .then(|| hex::encode(command.task)),
            request: matches!(command.action, action::ALLOCATE | action::REJECT)
                .then(|| hex::encode(command.request)),
            route: matches!(command.action, 2..=4).then(|| hex::encode(command.reference)),
            refund: matches!(command.action, 3 | 4),
        };
        self.observe_state(
            project,
            chain,
            &format!("command:{}", record.operation),
            &native.to_string(),
            selection,
        )
        .await
    }

    async fn command_state(
        &self,
        project: &ProjectConfig,
        chain: Chain,
        operation: &str,
    ) -> Result<u64> {
        let configured = &project.domains[&chain];
        match chain {
            Chain::Ethereum => {
                let block = self.indexer.ethereum.finalized_block().await?;
                Ok(u64::from(
                    self.indexer
                        .ethereum
                        .outcome(configured.vault.parse()?, word(operation)?.into(), &block)
                        .await?
                        .code,
                ))
            }
            Chain::Solana => Ok(self
                .solana(project)
                .read_command(word(operation)?)
                .await?
                .map_or(0, |r| u64::from(r.value.state))),
            Chain::Near => {
                let block = self.indexer.near.finalized_block().await?;
                let state: Value = self
                    .indexer
                    .near
                    .view(
                        &configured.vault,
                        "get_command",
                        &json!({"operation":operation}),
                        &block,
                    )
                    .await?;
                if state.is_null() {
                    Ok(0)
                } else {
                    state["state"]
                        .as_u64()
                        .context("Invalid native command state")
                }
            }
        }
    }

    async fn command_recipient(
        &self,
        project: &ProjectConfig,
        view: &ProjectView,
        record: &CoordinatorCommand,
    ) -> Result<Option<String>> {
        if binding(project, &record.domain)? != Chain::Near {
            return Ok(None);
        }
        let command = parse_command(&record.command)?;
        let recipient = match command.action {
            14 => {
                let state = self
                    .indexer
                    .near
                    .local_project(&project.domains[&Chain::Near].vault, &view.block)
                    .await?;
                state.owner_id
            }
            4 => {
                let plan = view
                    .plans
                    .iter()
                    .find(|p| word(&p.plan).ok() == Some(command.related))
                    .context("Dispatch plan not indexed")?;
                let route = plan
                    .routes
                    .iter()
                    .find(|r| word(&r.quote.route).ok() == Some(command.reference))
                    .context("Dispatch route not indexed")?;
                let response: Value = sqlx::query_scalar(
                    "SELECT response FROM funding_quotes WHERE project=$1 AND route=$2",
                )
                .bind(self.indexer.config.project_key(project))
                .bind(&route.quote.route)
                .fetch_optional(&self.indexer.store.pool)
                .await?
                .context("Dispatch requires its persisted provider quote")?;
                ensure!(
                    hex::encode(Sha256::digest(crate::chains::protocol::canonical_json(
                        &response
                    )?)) == route.quote.quote_hash,
                    "Provider quote differs from committed route"
                );
                response["quote"]["depositAddress"]
                    .as_str()
                    .context("Missing deposit address")?
                    .to_owned()
            }
            _ => return Ok(None),
        };
        ensure!(
            Sha256::digest(recipient.as_bytes()).as_slice() == command.recipient,
            "Signed NEAR recipient mismatch"
        );
        Ok(Some(recipient))
    }

    async fn reconcile_plan(
        &mut self,
        project: &ProjectConfig,
        view: &ProjectView,
        plan: &CoordinatorPlan,
    ) -> Result<()> {
        let destination = binding(project, &plan.destination)?;
        for route in &plan.routes {
            let source = binding(project, &route.quote.source)?;
            for (chain, refund, expected, known) in [
                (destination, false, route.quote.output, route.received),
                (source, true, route.quote.input, route.returned),
            ] {
                let Some(balance) = self
                    .route_balance(project, chain, &route.quote.route, refund, expected)
                    .await?
                else {
                    continue;
                };
                let received = balance.received;
                if balance.collectable(refund) {
                    // A growing escrow balance must not create another collect
                    // while the transaction for this received amount is unresolved.
                    let id = identity(&[
                        "collect",
                        &chain.to_string(),
                        &route.quote.route,
                        &received.to_string(),
                    ]);
                    self.effect(
                        project,
                        &id,
                        chain,
                        Intent::Collect {
                            route: route.quote.route.clone(),
                            refund,
                        },
                    )
                    .await?;
                }
                if received > known {
                    self.observe_state(
                        project,
                        chain,
                        &format!("route:{}", route.quote.route),
                        &received.to_string(),
                        Selection {
                            plan: Some(plan.plan.clone()),
                            route: Some(route.quote.route.clone()),
                            refund,
                            ..Default::default()
                        },
                    )
                    .await?;
                }
            }
        }
        // The coordinator gates financial progress at its retry limit, while
        // still allowing abort/settlement cleanup to release confirmed holds.
        if !matches!(plan.status.as_str(), "Funded" | "Closed") {
            let relevant: Vec<_> = view
                .commands
                .iter()
                .filter(|c| c.plan == plan.plan || plan.invalidations.contains(&c.operation))
                .map(|c| (&c.operation, c.state))
                .collect();
            let fingerprint = serde_json::to_string(&(plan, relevant))?;
            self.coordinator(
                project,
                &identity(&["advance", &plan.plan, &fingerprint]),
                "advance",
                json!({"plan":plan.plan}),
            )
            .await?;
        }
        Ok(())
    }

    async fn advance_queue(&mut self, project: &ProjectConfig, view: &ProjectView) -> Result<()> {
        if view.state.queue_head == view.state.queue_length {
            return Ok(());
        }
        let head: Value = self
            .indexer
            .near
            .view(
                &project.coordinator,
                "get_queue_head",
                &json!({}),
                &view.block,
            )
            .await?;
        if matches!(head["status"].as_str(), Some("Queued" | "Reserving")) {
            return Ok(());
        }
        // Observation can retire a request without moving the bounded FIFO
        // cursor. Advance only a known tombstone, preserving approval order.
        let id = identity(&[
            "advance_queue",
            &view.state.queue_head.to_string(),
            &serde_json::to_string(&head)?,
        ]);
        self.coordinator(project, &id, "advance_queue", json!({}))
            .await?;
        Ok(())
    }

    async fn route_balance(
        &self,
        project: &ProjectConfig,
        chain: Chain,
        route: &str,
        refund: bool,
        expected: u64,
    ) -> Result<Option<RouteBalance>> {
        let domain = &project.domains[&chain];
        match chain {
            Chain::Ethereum => {
                let block = self.indexer.ethereum.finalized_block().await?;
                let value = self
                    .indexer
                    .ethereum
                    .route(domain.vault.parse()?, word(route)?.into(), refund, &block)
                    .await?;
                if value.route.escrow.is_zero() {
                    Ok(None)
                } else {
                    ensure!(
                        value.route.expected == expected,
                        "Ethereum route amount mismatch"
                    );
                    Ok(Some(RouteBalance::new(
                        value.route.received,
                        value.token_balance.try_into()?,
                        if refund {
                            value.route.dispatched
                        } else {
                            expected
                        },
                    )?))
                }
            }
            Chain::Solana => {
                let Some(route) = self.solana(project).route_state(word(route)?).await? else {
                    return Ok(None);
                };
                ensure!(
                    route.record.kind == if refund { 2 } else { 1 }
                        && route.record.expected == expected,
                    "Solana route kind or amount mismatch"
                );
                Ok(Some(RouteBalance::new(
                    route.record.received,
                    route.escrow_balance,
                    if refund {
                        route.record.dispatched
                    } else {
                        expected
                    },
                )?))
            }
            Chain::Near => {
                let block = self.indexer.near.finalized_block().await?;
                let state: Value = self
                    .indexer
                    .near
                    .view(&domain.vault, "get_route", &json!({"route":route}), &block)
                    .await?;
                if state.is_null() || state["ready"] != true {
                    return Ok(None);
                }
                ensure!(
                    state["kind"] == if refund { 2 } else { 1 }
                        && decimal(&state["expected"])? == expected,
                    "NEAR route kind or amount mismatch"
                );
                let balance: String = self
                    .indexer
                    .near
                    .view(
                        &domain.asset,
                        "ft_balance_of",
                        &json!({"account_id":state["receiver"]}),
                        &block,
                    )
                    .await?;
                Ok(Some(RouteBalance::new(
                    decimal(&state["received"])?,
                    balance.parse()?,
                    if refund {
                        decimal(&state["dispatched"])?
                    } else {
                        expected
                    },
                )?))
            }
        }
    }

    async fn observe_state(
        &mut self,
        project: &ProjectConfig,
        chain: Chain,
        subject: &str,
        fingerprint: &str,
        selection: Selection,
    ) -> Result<()> {
        let key = self.indexer.config.project_key(project);
        let new_id = identity(&[&key, &Uuid::new_v4().to_string()]);
        let observation: String = sqlx::query_scalar(
            "INSERT INTO observations(project,chain,subject,fingerprint,observation) VALUES($1,$2,$3,$4,$5) ON CONFLICT(project,chain,subject) DO UPDATE SET observation=CASE WHEN observations.fingerprint=EXCLUDED.fingerprint THEN observations.observation ELSE EXCLUDED.observation END, fingerprint=EXCLUDED.fingerprint RETURNING observation")
            .bind(&key).bind(chain.to_string()).bind(subject).bind(fingerprint).bind(new_id).fetch_one(&self.indexer.store.pool).await?;
        let Some(evidence) = self
            .checkpoint(project, chain, &observation, selection)
            .await?
        else {
            return Ok(());
        };
        let checkpoint = Checkpoint::parse(&evidence.payload)?;
        ensure!(
            word(&checkpoint.domain)? == word(&project.domains[&chain].domain)?
                && checkpoint.observation == observation,
            "Native checkpoint identity mismatch"
        );
        self.observe(project, chain, &observation, evidence, false)
            .await
    }

    async fn checkpoint(
        &mut self,
        project: &ProjectConfig,
        chain: Chain,
        observation: &str,
        selection: Selection,
    ) -> Result<Option<Evidence>> {
        let key = self.indexer.config.project_key(project);
        sqlx::query("INSERT INTO checkpoints(project,observation,chain,selectors) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(&key).bind(observation).bind(chain.to_string()).bind(serde_json::to_value(&selection)?).execute(&self.indexer.store.pool).await?;
        let saved: Option<Value> = sqlx::query_scalar(
            "SELECT evidence FROM checkpoints WHERE project=$1 AND observation=$2",
        )
        .bind(&key)
        .bind(observation)
        .fetch_one(&self.indexer.store.pool)
        .await?;
        if let Some(saved) = saved {
            return Ok(Some(serde_json::from_value(saved)?));
        }
        let result = self
            .effect(
                project,
                &identity(&["checkpoint", observation]),
                chain,
                Intent::Checkpoint {
                    observation: observation.to_owned(),
                    selection,
                },
            )
            .await?;
        let Some(result) = result else {
            return Ok(None);
        };
        let domain = &project.domains[&chain];
        let evidence = match chain {
            Chain::Ethereum => {
                let receipt = self
                    .indexer
                    .ethereum
                    .receipt(
                        result["hash"]
                            .as_str()
                            .context("Missing transaction hash")?
                            .parse()?,
                    )
                    .await?
                    .context("Finalized checkpoint receipt disappeared")?;
                ethereum_evidence(receipt.checkpoint_evidence(
                    domain.vault.parse()?,
                    word(&domain.domain)?.into(),
                    word(observation)?.into(),
                )?)
            }
            Chain::Solana => {
                let receipt = self
                    .solana(project)
                    .checkpoint_evidence(
                        result["signature"].as_str().context("Missing signature")?,
                        word(observation)?,
                    )
                    .await?;
                Evidence {
                    payload: hex::encode(receipt.payload),
                    request: Some(receipt.request),
                    values: Some(receipt.values),
                }
            }
            Chain::Near => {
                let block = self.indexer.near.finalized_block().await?;
                let payload: Option<String> = self
                    .indexer
                    .near
                    .view(
                        &domain.vault,
                        "get_checkpoint",
                        &json!({"observation":observation}),
                        &block,
                    )
                    .await?;
                Evidence {
                    payload: payload.context("Finalized checkpoint not found")?,
                    request: None,
                    values: None,
                }
            }
        };
        sqlx::query("UPDATE checkpoints SET evidence=$3 WHERE project=$1 AND observation=$2 AND evidence IS NULL")
            .bind(&key).bind(observation).bind(serde_json::to_value(&evidence)?).execute(&self.indexer.store.pool).await?;
        Ok(Some(evidence))
    }

    async fn observe_funding(
        &mut self,
        project: &ProjectConfig,
        chain: Chain,
        funding: &LocalFunding,
    ) -> Result<()> {
        let domain = &project.domains[&chain].domain;
        let block = self.indexer.near.finalized_block().await?;
        let seen: bool = self
            .indexer
            .near
            .view(
                &project.coordinator,
                "get_funding_seen",
                &json!({"domain":domain,"funding_id":funding.id}),
                &block,
            )
            .await?;
        if seen {
            return Ok(());
        }
        let method = if chain == Chain::Near {
            "observe_funding_near"
        } else {
            "observe_funding_foreign"
        };
        let id = identity(&[method, &chain.to_string(), &funding.id]);
        let key = self.indexer.config.project_key(project);
        if let Some(effect) = self.indexer.store.effect(&key, &id).await? {
            let Intent::Coordinator {
                method: saved_method,
                args,
            } = serde_json::from_value(effect.intent)?
            else {
                anyhow::bail!("Funding observation has an incompatible persisted intent");
            };
            ensure!(
                effect.chain == "near" && saved_method == method && args["domain"] == *domain,
                "Persisted funding observation binding mismatch"
            );
            if chain == Chain::Near {
                ensure!(
                    args["funding_id"] == funding.id,
                    "Persisted NEAR funding identity mismatch"
                );
            } else {
                ensure!(
                    args["payload"] == funding.payload,
                    "Immutable funding payload changed after observation preparation"
                );
            }
            // Keep the first finalized proof and its exact signed intent. Later
            // transactions may reference the same immutable Solana receipt.
            self.coordinator(project, &id, method, args).await?;
            return Ok(());
        }
        let evidence = match chain {
            Chain::Ethereum => ethereum_evidence(serde_json::from_value(
                funding
                    .evidence
                    .clone()
                    .context("Missing Ethereum funding provenance")?,
            )?),
            Chain::Solana => {
                let domain_state = self
                    .indexer
                    .near
                    .coordinator_domain(&project.coordinator, domain, &block)
                    .await?;
                let client = self.solana(project);
                let signature = client
                    .receipt_signature(word(&funding.id)?, true)
                    .await?
                    .context("Funding transaction is not finalized")?;
                let receipt = client
                    .funding_evidence(
                        &signature,
                        word(&funding.id)?,
                        word(&domain_state.owner_identity)?.into(),
                    )
                    .await?;
                Evidence {
                    payload: hex::encode(receipt.payload),
                    request: Some(receipt.request),
                    values: Some(receipt.values),
                }
            }
            Chain::Near => Evidence {
                payload: funding.payload.clone(),
                request: None,
                values: None,
            },
        };
        ensure!(
            evidence.payload == funding.payload,
            "Immutable funding payload changed"
        );
        self.observe(project, chain, &funding.id, evidence, true)
            .await
    }

    async fn observe(
        &mut self,
        project: &ProjectConfig,
        chain: Chain,
        id: &str,
        evidence: Evidence,
        funding: bool,
    ) -> Result<()> {
        let domain = &project.domains[&chain].domain;
        let (method, args) = match (chain, funding) {
            (Chain::Near, false) => ("observe_near", json!({"domain":domain,"observation":id})),
            (Chain::Near, true) => (
                "observe_funding_near",
                json!({"domain":domain,"funding_id":id}),
            ),
            (_, funding) => (
                if funding {
                    "observe_funding_foreign"
                } else {
                    "observe_foreign"
                },
                json!({"domain":domain,"payload":evidence.payload,"request":evidence.request.context("Missing MPC request")?,"values":evidence.values.context("Missing MPC values")?}),
            ),
        };
        self.coordinator(
            project,
            &identity(&[method, &chain.to_string(), id]),
            method,
            args,
        )
        .await?;
        Ok(())
    }

    async fn coordinator(
        &mut self,
        project: &ProjectConfig,
        id: &str,
        method: &str,
        args: Value,
    ) -> Result<Option<Value>> {
        let key = self.indexer.config.project_key(project);
        if let Some(effect) = self.indexer.store.effect(&key, id).await? {
            let rejected = effect.state == "failed"
                || (effect.state == "confirmed"
                    && effect
                        .result
                        .as_ref()
                        .is_some_and(|result| result["value"] == false));
            if rejected {
                let block = self.indexer.near.finalized_block().await?;
                let state = self
                    .indexer
                    .near
                    .coordinator_state(&project.coordinator, &block)
                    .await?;
                ensure!(
                    effect.attempt < i32::try_from(state.retry_limit)?,
                    "Coordinator callback retry limit reached; inspect operation {id}"
                );
                // This path only repeats a completed permissionless coordinator
                // call. Native token movements and unknown transactions never use it.
                self.indexer
                    .store
                    .retry_effect(&key, id, effect.attempt)
                    .await?;
            }
        }
        self.effect(
            project,
            id,
            Chain::Near,
            Intent::Coordinator {
                method: method.to_owned(),
                args,
            },
        )
        .await
    }
}

fn ethereum_evidence(receipt: ethereum::EventEvidence) -> Evidence {
    let (request, values) = receipt.mpc_evidence();
    Evidence {
        payload: hex::encode(receipt.payload),
        request: Some(request),
        values: Some(json!(values)),
    }
}

fn binding(project: &ProjectConfig, domain: &str) -> Result<Chain> {
    let domain = word(domain)?;
    project
        .domains
        .iter()
        .find(|(_, value)| word(&value.domain).ok() == Some(domain))
        .map(|(chain, _)| *chain)
        .context("Command references an unregistered domain")
}

fn task_matches(task: &LocalTask, known: &Value) -> bool {
    !known.is_null()
        && known["state"] == task.state
        && known["paused"] == task.paused
        && known["started"] == task.started
        && known["available"] == task.available
        && known["issued"] == task.issued
        && known["pending_outflow"] == task.pending_outflow
        && known["live_request"] == json!(task.live_request)
        && task
            .control_nonce
            .is_none_or(|nonce| known["control_nonce"] == nonce)
}

fn decimal(value: &Value) -> Result<u64> {
    value
        .as_str()
        .context("Expected a decimal token amount")?
        .parse()
        .context("Token amount exceeds u64")
}

fn report(result: Result<()>, stage: &str, operation: &str) -> bool {
    if let Err(error) = result {
        tracing::warn!(stage, operation, %error, "Reconciliation requires attention");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escrow_tokens_do_not_authorize_refunds_before_confirmed_dispatch() {
        assert!(!RouteBalance::new(0, 100, 0).unwrap().collectable(true));
        assert!(RouteBalance::new(20, 30, 100).unwrap().collectable(true));
        assert!(!RouteBalance::new(20, 79, 100).unwrap().collectable(false));
        assert!(RouteBalance::new(20, 80, 100).unwrap().collectable(false));
        assert!(!RouteBalance::new(100, 20, 100).unwrap().collectable(true));
        assert!(RouteBalance::new(101, 20, 100).is_err());
    }
}
