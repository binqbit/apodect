//! A relay effect has one persisted, signed transaction. An uncertain RPC result
//! never authorizes preparing another transaction or spending again.

use std::{collections::BTreeMap, path::PathBuf};

use alloy_primitives::{Address, B256, U256};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result, bail, ensure};
use apodect_solana::bridge::action;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, pool::PoolConnection};

use crate::{
    chains::{
        ethereum, near,
        protocol::{Selection, command_bytes, parse_command, word_number},
        solana,
    },
    config::{Chain, ProjectConfig, word},
    indexing::Indexer,
    rpc::Rpc,
    storage::Effect,
};

const STORAGE_DEPOSIT: u128 = 100_000_000_000_000_000_000_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Intent {
    Execute {
        command: String,
        signature: String,
        recovery: u8,
        recipient: Option<String>,
    },
    Checkpoint {
        observation: String,
        selection: Selection,
    },
    Collect {
        route: String,
        refund: bool,
    },
    Coordinator {
        method: String,
        args: Value,
    },
    RegisterTokenAccount {
        operation: String,
        token: String,
        recipient: String,
    },
}

pub struct Relay {
    pub indexer: Indexer,
    ethereum_signer: PrivateKeySigner,
    near_signer: near::RelaySigner,
    solana: BTreeMap<String, solana::Client>,
    // Keep the session lock on a dedicated connection, never return it to the pool.
    lock: PoolConnection<Postgres>,
}

impl Relay {
    pub async fn connect(indexer: Indexer) -> Result<Self> {
        let mut lock = indexer.store.pool.acquire().await?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(626098851920119)")
            .fetch_one(&mut *lock)
            .await?;
        ensure!(acquired, "Another application worker owns the relay keys");
        lock.close_on_drop();

        let ethereum_signer: PrivateKeySigner = std::fs::read_to_string(key_path("ETHEREUM")?)?
            .trim()
            .parse()
            .context("Invalid Ethereum relayer credential")?;
        let near_signer = near::RelaySigner::from_file(
            &key_path("NEAR")?,
            &std::env::var("APODECT_NEAR_RELAYER_ACCOUNT")
                .context("APODECT_NEAR_RELAYER_ACCOUNT is required")?,
        )?;
        let solana_key = key_path("SOLANA")?;
        let mut solana = BTreeMap::new();
        for project in &indexer.config.projects {
            let read_client = indexer.solana(project)?;
            let configured = read_client.binding();
            let binding = solana::Binding {
                genesis_hash: configured.genesis_hash.clone(),
                project: configured.project,
                mint: configured.mint,
                domain: configured.domain,
            };
            let client = solana::Client::new(
                Rpc::new(&indexer.config.solana.rpc_url)?,
                binding,
                Some(&solana_key),
            )?;
            let state = client.read_project().await?.value;
            let payer = client.payer().context("Missing Solana gas payer")?;
            let workspace = client.read_workspace(state.workspace).await?.value;
            ensure!(
                payer != state.authority && payer != workspace.owner,
                "Solana relay key must not hold a Project role"
            );
            solana.insert(indexer.config.project_key(project), client);

            let block = indexer.ethereum.finalized_block().await?;
            let state = indexer
                .ethereum
                .project(project.domains[&Chain::Ethereum].vault.parse()?, &block)
                .await?;
            ensure!(
                ethereum_signer.address() != state.owner
                    && ethereum_signer.address() != state.authority,
                "Ethereum relay key must not hold a Project role"
            );
            let block = indexer.near.finalized_block().await?;
            let state = indexer
                .near
                .coordinator_state(&project.coordinator, &block)
                .await?;
            let domain = indexer
                .near
                .coordinator_domain(
                    &project.coordinator,
                    &project.domains[&Chain::Solana].domain,
                    &block,
                )
                .await?;
            ensure!(
                payer.to_bytes() != word(&domain.owner_identity)?,
                "Solana relay key must not be the Project owner"
            );
            ensure!(
                near_signer.account_id() != state.owner_id
                    && near_signer.account_id() != state.authority_id,
                "NEAR relay key must not hold a coordinator role"
            );
            let local = indexer
                .near
                .local_project(&project.domains[&Chain::Near].vault, &block)
                .await?;
            ensure!(
                near_signer.account_id() != local.owner_id
                    && near_signer.account_id() != local.authority_id,
                "NEAR relay key must not hold a Project role"
            );
        }
        Ok(Self {
            indexer,
            ethereum_signer,
            near_signer,
            solana,
            lock,
        })
    }

    pub fn solana(&self, project: &ProjectConfig) -> &solana::Client {
        &self.solana[&self.indexer.config.project_key(project)]
    }

    pub async fn verify_ownership(&mut self) -> Result<()> {
        // A dead lock connection cannot silently be replaced by a pooled connection.
        sqlx::query("SELECT 1")
            .execute(&mut *self.lock)
            .await
            .context("Relay ownership connection lost; worker must stop")?;
        Ok(())
    }

    /// NEP-141 transfers require a registered receiver. Registration is a
    /// separate native-funded effect; dispatch waits for its finalized state.
    pub(crate) async fn ensure_near_token_account(
        &mut self,
        project: &ProjectConfig,
        operation: &str,
        recipient: &str,
    ) -> Result<bool> {
        let token = project.domains[&Chain::Near].asset.clone();
        let block = self.indexer.near.finalized_block().await?;
        if self
            .indexer
            .near
            .token_account_registered(&token, recipient, &block)
            .await?
        {
            return Ok(true);
        }
        let id = crate::chains::protocol::identity(&[
            "near_token_registration",
            operation,
            &token,
            recipient,
        ]);
        let registered = self
            .effect(
                project,
                &id,
                Chain::Near,
                Intent::RegisterTokenAccount {
                    operation: operation.to_owned(),
                    token: token.clone(),
                    recipient: recipient.to_owned(),
                },
            )
            .await?;
        if registered.is_none() {
            return Ok(false);
        }
        let block = self.indexer.near.finalized_block().await?;
        ensure!(
            self.indexer
                .near
                .token_account_registered(&token, recipient, &block)
                .await?,
            "Finalized token registration did not register the dispatch recipient"
        );
        Ok(true)
    }

    pub async fn effect(
        &mut self,
        project: &ProjectConfig,
        id: &str,
        chain: Chain,
        intent: Intent,
    ) -> Result<Option<Value>> {
        self.verify_ownership().await?;
        let key = self.indexer.config.project_key(project);
        let mut effect = self
            .indexer
            .store
            .plan_effect(
                &key,
                id,
                &chain.to_string(),
                &self.indexer.config.network(chain),
                &serde_json::to_value(&intent)?,
            )
            .await?;
        if effect.state == "confirmed" {
            return Ok(effect.result);
        }
        if effect.state == "failed" {
            bail!(
                "Operation {id} failed: {}",
                effect.error.as_deref().unwrap_or("native execution failed")
            );
        }

        if effect.transaction.is_none() {
            if !self.submission_allowed(project, &intent).await? {
                return Ok(None);
            }
            let transaction = self.prepare(project, chain, &intent, id).await?;
            self.verify_ownership().await?;
            self.indexer
                .store
                .prepare_effect(&key, id, &transaction)
                .await?;
            effect.transaction = Some(transaction);
        }

        // Query before any send, including recovery after a crash before recording an acknowledgement.
        let status = self.outcome(project, chain, &effect).await?;
        match status {
            Outcome::Confirmed(result) => {
                self.indexer
                    .store
                    .update_effect(&key, id, "confirmed", Some(&result), None)
                    .await?;
                Ok(Some(result))
            }
            Outcome::Failed(result) => {
                self.indexer
                    .store
                    .update_effect(
                        &key,
                        id,
                        "failed",
                        Some(&result),
                        Some("Finalized native execution failed"),
                    )
                    .await?;
                bail!("Operation {id} has a finalized native failure")
            }
            Outcome::Pending => Ok(None),
            Outcome::Unknown => {
                if effect.state == "attention" {
                    return Ok(None);
                }
                if !self.submission_allowed(project, &intent).await?
                    && !self.expired_ethereum_execution(chain, &intent).await?
                {
                    return Ok(None);
                }
                if chain == Chain::Solana {
                    let transaction = serde_json::from_value(effect.transaction.clone().unwrap())?;
                    if self
                        .solana(project)
                        .transaction_expired(&transaction)
                        .await?
                    {
                        self.indexer.store.update_effect(&key, id, "attention", None,
                            Some("Solana blockhash expired without a finalized outcome; reconcile before authorizing a retry")).await?;
                        return Ok(None);
                    }
                }
                // Rebroadcasting is safe because the signed transaction identity stays unchanged.
                self.verify_ownership().await?;
                let result = self
                    .broadcast(project, chain, effect.transaction.as_ref().unwrap())
                    .await;
                let error = result.err().map(|error| error.to_string());
                self.indexer
                    .store
                    .update_effect(&key, id, "submitted", None, error.as_deref())
                    .await?;
                Ok(None)
            }
        }
    }

    async fn submission_allowed(&self, project: &ProjectConfig, intent: &Intent) -> Result<bool> {
        let coordinator_operation = match intent {
            Intent::Coordinator { method, args } if method == "request_signature" => Some(
                args["operation"]
                    .as_str()
                    .context("Signature request has no operation")?,
            ),
            Intent::RegisterTokenAccount { operation, .. } => Some(operation.as_str()),
            _ => None,
        };
        let (command, block) = match (intent, coordinator_operation) {
            (Intent::Execute { command, .. }, _) => (parse_command(command)?, None),
            (_, Some(operation)) => {
                let block = self.indexer.near.finalized_block().await?;
                let record: Option<near::CoordinatorCommand> = self
                    .indexer
                    .near
                    .view(
                        &project.coordinator,
                        "get_command",
                        &json!({"operation": operation}),
                        &block,
                    )
                    .await?;
                let record = record.context("Relay coordinator command no longer exists")?;
                (parse_command(&record.command)?, Some(block))
            }
            _ => return Ok(true),
        };
        if command.action == action::CLOSE_PLAN || command.related == [0; 32] {
            return Ok(true);
        }
        let block = match block {
            Some(block) => block,
            None => self.indexer.near.finalized_block().await?,
        };
        let plan = self
            .indexer
            .near
            .coordinator_plan(&project.coordinator, &hex::encode(command.related), &block)
            .await?
            .context("Command's coordinator plan no longer exists")?;
        Ok(!plan.requires_attention)
    }

    async fn expired_ethereum_execution(&self, chain: Chain, intent: &Intent) -> Result<bool> {
        let Intent::Execute { command, .. } = intent else {
            return Ok(false);
        };
        if chain != Chain::Ethereum {
            return Ok(false);
        }
        let command = parse_command(command)?;
        let block = self.indexer.ethereum.finalized_block().await?;
        // Only an existing signed transaction reaches this check. Once expired,
        // it cannot execute a new token action; consuming its nonce lets later
        // cleanup transactions proceed without replacing either transaction.
        Ok(block.timestamp > command.expires)
    }

    pub async fn recover(&mut self, project: &ProjectConfig) -> Result<()> {
        let key = self.indexer.config.project_key(project);
        let records: Vec<(String, String, Value)> = sqlx::query_as(
            "SELECT id, chain, intent FROM effects WHERE project = $1 AND state IN ('planned','prepared','submitted','attention') ORDER BY (transaction->>'nonce')::numeric NULLS LAST")
            .bind(key).fetch_all(&self.indexer.store.pool).await?;
        for (id, chain, intent) in records {
            let chain: Chain = serde_json::from_value(Value::String(chain))?;
            if let Err(error) = self
                .effect(project, &id, chain, serde_json::from_value(intent)?)
                .await
            {
                tracing::warn!(operation = id, %error, "Effect recovery requires attention");
            }
        }
        Ok(())
    }

    async fn prepare(
        &self,
        project: &ProjectConfig,
        chain: Chain,
        intent: &Intent,
        id: &str,
    ) -> Result<Value> {
        let vault = &project.domains[&chain].vault;
        match chain {
            Chain::Ethereum => {
                let call = ethereum_call(intent)?;
                let floor = self
                    .indexer
                    .store
                    .nonce_floor(
                        "ethereum",
                        &self.indexer.config.network(Chain::Ethereum),
                        &self.ethereum_signer.address().to_string(),
                    )
                    .await?;
                let nonce = self
                    .indexer
                    .ethereum
                    .next_nonce(self.ethereum_signer.address(), floor)
                    .await?;
                let transaction = self
                    .indexer
                    .ethereum
                    .prepare(
                        vault.parse()?,
                        &call,
                        &self.ethereum_signer,
                        nonce,
                        ethereum::GasLimits {
                            gas_limit: 5_000_000,
                            max_fee_per_gas: 100_000_000_000,
                        },
                    )
                    .await?;
                Ok(serde_json::to_value(transaction)?)
            }
            Chain::Solana => {
                let client = self.solana(project);
                let transaction = match intent {
                    Intent::Execute {
                        command,
                        signature,
                        recovery,
                        ..
                    } => {
                        client
                            .prepare_execute(
                                command_bytes(command)?,
                                hex::decode(signature)?
                                    .try_into()
                                    .map_err(|_| anyhow::anyhow!("Invalid signature size"))?,
                                *recovery,
                            )
                            .await?
                    }
                    Intent::Checkpoint {
                        observation,
                        selection,
                    } => {
                        client
                            .prepare_checkpoint(
                                word(observation)?,
                                solana::CheckpointSelection {
                                    task: selection
                                        .task
                                        .as_deref()
                                        .map(word)
                                        .transpose()?
                                        .map(Into::into),
                                    request: selection
                                        .request
                                        .as_deref()
                                        .map(word)
                                        .transpose()?
                                        .map(Into::into),
                                    plan: selection.plan.as_deref().map(word).transpose()?,
                                    route: selection.route.as_deref().map(word).transpose()?,
                                    operation: selection
                                        .operation
                                        .as_deref()
                                        .map(word)
                                        .transpose()?,
                                },
                            )
                            .await?
                    }
                    Intent::Collect { route, .. } => {
                        client.prepare_collect(word(route)?, word(id)?).await?
                    }
                    _ => bail!("Coordinator calls execute only on NEAR"),
                };
                Ok(serde_json::to_value(transaction)?)
            }
            Chain::Near => {
                let call = self.near_call(project, intent).await?;
                let floor = self
                    .indexer
                    .store
                    .nonce_floor(
                        "near",
                        &self.indexer.config.network(Chain::Near),
                        &self.near_signer.account_id(),
                    )
                    .await?;
                Ok(serde_json::to_value(
                    self.indexer
                        .near
                        .prepare_relay_call(&call, &self.near_signer, floor)
                        .await?,
                )?)
            }
        }
    }

    async fn near_call(&self, project: &ProjectConfig, intent: &Intent) -> Result<near::RelayCall> {
        let vault = &project.domains[&Chain::Near].vault;
        match intent {
            Intent::Execute {
                command,
                signature,
                recovery,
                recipient,
            } => {
                let action = parse_command(command)?.action;
                let deposit = if matches!(action, action::EXPECT_DELIVERY | action::EXPECT_REFUND) {
                    // The deployed contract knows its embedded escrow WASM size
                    // and network storage cost. Unused storage allowance is refunded.
                    let block = self.indexer.near.finalized_block().await?;
                    let deployment: String = self
                        .indexer
                        .near
                        .view(vault, "route_deployment_deposit", &json!({}), &block)
                        .await?;
                    deployment
                        .parse::<u128>()?
                        .checked_add(STORAGE_DEPOSIT)
                        .context("NEAR route deposit overflow")?
                } else {
                    STORAGE_DEPOSIT
                };
                near::RelayCall::execute(
                    vault,
                    json!({"command":command,"signature":signature,"recovery_id":recovery,"recipient_id":recipient}),
                    deposit,
                )
            }
            Intent::Checkpoint {
                observation,
                selection,
            } => near::RelayCall::checkpoint(
                vault,
                json!({"observation":observation,"task":selection.task.as_deref().map(word_number).transpose()?,
                    "request":selection.request.as_deref().map(word_number).transpose()?,"plan":selection.plan,
                    "route":selection.route,"operation":selection.operation}),
                STORAGE_DEPOSIT,
            ),
            Intent::Collect { route, .. } => {
                let block = self.indexer.near.finalized_block().await?;
                let receiver = self
                    .indexer
                    .near
                    .route_receiver(vault, route, &block)
                    .await?;
                near::RelayCall::collect(&receiver)
            }
            Intent::Coordinator { method, args } => near::RelayCall::coordinator(
                &project.coordinator,
                method,
                args.clone(),
                if method == "advance_queue" {
                    0
                } else {
                    STORAGE_DEPOSIT
                },
            ),
            Intent::RegisterTokenAccount {
                operation,
                token,
                recipient,
            } => {
                ensure!(
                    token == &project.domains[&Chain::Near].asset,
                    "Registration token differs from the Project asset"
                );
                let block = self.indexer.near.finalized_block().await?;
                let record: Option<near::CoordinatorCommand> = self
                    .indexer
                    .near
                    .view(
                        &project.coordinator,
                        "get_command",
                        &json!({"operation": operation}),
                        &block,
                    )
                    .await?;
                let record = record.context("Registration dispatch command is missing")?;
                let command = parse_command(&record.command)?;
                ensure!(
                    record.state == 0
                        && command.action == action::DISPATCH
                        && command.operation == word(operation)?
                        && command.domain == word(&project.domains[&Chain::Near].domain)?
                        && Sha256::digest(recipient.as_bytes()).as_slice() == command.recipient,
                    "Token registration is not bound to the pending NEAR dispatch recipient"
                );
                self.indexer
                    .near
                    .token_registration_call(token, recipient, &block)
                    .await
            }
        }
    }

    async fn outcome(
        &self,
        project: &ProjectConfig,
        chain: Chain,
        effect: &Effect,
    ) -> Result<Outcome> {
        let transaction = effect
            .transaction
            .clone()
            .context("Missing signed transaction")?;
        match chain {
            Chain::Ethereum => {
                let transaction: ethereum::PreparedTransaction =
                    serde_json::from_value(transaction)?;
                match self.indexer.ethereum.receipt(transaction.hash).await? {
                    None => Ok(Outcome::Unknown),
                    Some(receipt) => {
                        ensure!(
                            receipt.sender == transaction.sender
                                && receipt.destination
                                    == Some(project.domains[&chain].vault.parse::<Address>()?),
                            "Ethereum receipt binding mismatch"
                        );
                        let value = json!({"hash":receipt.hash,"height":receipt.block.height,"block_hash":receipt.block.hash});
                        Ok(if receipt.succeeded {
                            Outcome::Confirmed(value)
                        } else {
                            Outcome::Failed(value)
                        })
                    }
                }
            }
            Chain::Solana => {
                let transaction: solana::PreparedTransaction = serde_json::from_value(transaction)?;
                Ok(
                    match self
                        .solana(project)
                        .transaction_outcome(&transaction.signature)
                        .await?
                    {
                        solana::TransactionOutcome::Unknown => Outcome::Unknown,
                        solana::TransactionOutcome::Pending => Outcome::Pending,
                        solana::TransactionOutcome::Finalized { slot } => Outcome::Confirmed(
                            json!({"signature":transaction.signature,"slot":slot}),
                        ),
                        solana::TransactionOutcome::Failed { slot, error } => {
                            Outcome::Failed(json!({"slot":slot,"error":error}))
                        }
                    },
                )
            }
            Chain::Near => {
                let transaction: near::PreparedTransaction = serde_json::from_value(transaction)?;
                Ok(
                    match self
                        .indexer
                        .near
                        .transaction_outcome(&transaction.hash, &transaction.sender)
                        .await?
                    {
                        near::TransactionOutcome::Unknown => Outcome::Unknown,
                        near::TransactionOutcome::Pending => Outcome::Pending,
                        near::TransactionOutcome::Final(execution) => match execution.json_result()
                        {
                            Ok(value) => {
                                Outcome::Confirmed(json!({"hash":transaction.hash,"value":value}))
                            }
                            Err(_) => Outcome::Failed(
                                json!({"hash":transaction.hash,"failures":execution.failures,"status":execution.outcome.status}),
                            ),
                        },
                    },
                )
            }
        }
    }

    async fn broadcast(
        &self,
        project: &ProjectConfig,
        chain: Chain,
        transaction: &Value,
    ) -> Result<()> {
        match chain {
            Chain::Ethereum => {
                self.indexer
                    .ethereum
                    .broadcast(&serde_json::from_value(transaction.clone())?)
                    .await
            }
            Chain::Solana => {
                self.solana(project)
                    .broadcast(&serde_json::from_value(transaction.clone())?)
                    .await
            }
            Chain::Near => {
                self.indexer
                    .near
                    .broadcast(&serde_json::from_value(transaction.clone())?)
                    .await
            }
        }
    }
}

enum Outcome {
    Unknown,
    Pending,
    Confirmed(Value),
    Failed(Value),
}

fn key_path(chain: &str) -> Result<PathBuf> {
    Ok(std::env::var(format!("APODECT_{chain}_RELAYER_KEY_FILE"))
        .with_context(|| format!("APODECT_{chain}_RELAYER_KEY_FILE is required"))?
        .into())
}

fn ethereum_call(intent: &Intent) -> Result<ethereum::RelayCall> {
    let identity = |value: &Option<String>| -> Result<B256> {
        Ok(value
            .as_deref()
            .map(word)
            .transpose()?
            .unwrap_or([0; 32])
            .into())
    };
    let number = |value: &Option<String>| -> Result<U256> {
        Ok(U256::from(
            value.as_deref().map(word_number).transpose()?.unwrap_or(0),
        ))
    };
    Ok(match intent {
        Intent::Execute {
            command,
            signature,
            recovery,
            ..
        } => ethereum::RelayCall::Execute {
            command: command_bytes(command)?.to_vec().into(),
            signature: hex::decode(signature)?.into(),
            recovery_id: *recovery,
        },
        Intent::Checkpoint {
            observation,
            selection,
        } => ethereum::RelayCall::Checkpoint(ethereum::CheckpointRequest {
            observation: word(observation)?.into(),
            task: number(&selection.task)?,
            request: number(&selection.request)?,
            plan: identity(&selection.plan)?,
            route: identity(&selection.route)?,
            refund: selection.refund,
            operation: identity(&selection.operation)?,
        }),
        Intent::Collect { route, refund } => ethereum::RelayCall::Collect {
            route: word(route)?.into(),
            refund: *refund,
        },
        _ => bail!("Coordinator calls execute only on NEAR"),
    })
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
