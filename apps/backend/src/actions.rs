//! User actions end in unsigned wallet intents. Authentication selects an actor;
//! finalized contract state establishes its role, and the contract rechecks all
//! authorization and accounting when the user's transaction executes.

mod coordinator;
mod local;

pub use coordinator::{CoordinatorAction, TaskControl};
pub use local::{EthereumCall, LocalAction, SolanaAccount, SolanaInstruction};

use crate::{
    accounts::Accounts,
    chains::{near, protocol::word_number},
    config::{Chain, ProjectConfig, word},
    indexing::Indexer,
    liquidity::{FundingCosts, FundingProposal, Liquidity},
};
use alloy_primitives::{Address, B256, U256};
use anchor_lang::prelude::Pubkey;
use anyhow::{Context, Result, ensure};
use coordinator::{Role, validate_domain};
use local::{
    ethereum_action, ethereum_approval, near_action, near_call, near_token_registration,
    solana_action,
};
use serde::{Deserialize, Serialize};

pub struct Actions<'a> {
    accounts: &'a Accounts,
    indexer: &'a Indexer,
}

/// Execute transactions in order, waiting for each to succeed. Wallets choose
/// current gas fees, nonces and recent block hashes immediately before signing.
#[derive(Debug, Serialize)]
#[serde(tag = "chain", rename_all = "lowercase")]
pub enum UnsignedIntent {
    Ethereum {
        chain_id: u64,
        actor: String,
        transactions: Vec<EthereumCall>,
    },
    Solana {
        genesis_hash: String,
        actor: String,
        instructions: Vec<SolanaInstruction>,
    },
    Near {
        network: near::GenesisIdentity,
        actor: String,
        transactions: Vec<near::FunctionCallIntent>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplyRequest {
    pub destination: Chain,
    pub request: String,
    pub slippage_bps: u16,
    pub expires: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApplyPreparation {
    Waiting {
        #[serde(with = "crate::decimal_u64")]
        shortfall: u64,
    },
    Ready {
        intent: UnsignedIntent,
        costs: FundingCosts,
    },
}

impl<'a> Actions<'a> {
    pub fn new(accounts: &'a Accounts, indexer: &'a Indexer) -> Self {
        Self { accounts, indexer }
    }

    pub async fn prepare_apply(
        &self,
        session: &str,
        project_key: &str,
        actor: &str,
        request: ApplyRequest,
        liquidity: &Liquidity,
    ) -> Result<ApplyPreparation> {
        let actor = self
            .accounts
            .require_wallet(session, Chain::Near, actor)
            .await?
            .address;
        let project = self.project(project_key)?;
        let block = self.indexer.near.finalized_block().await?;
        let state = self
            .indexer
            .near
            .coordinator_state(&project.coordinator, &block)
            .await?;
        ensure!(
            word(&state.project_id)? == word(&project.project_id)?,
            "Coordinator Project identity mismatch"
        );
        Role::Authority.require(&actor, &state)?;

        match liquidity
            .propose_apply(
                self.indexer,
                project,
                request.destination,
                &request.request,
                request.slippage_bps,
                request.expires,
            )
            .await?
        {
            FundingProposal::Waiting { shortfall } => Ok(ApplyPreparation::Waiting { shortfall }),
            FundingProposal::Ready {
                authority,
                call,
                costs,
            } => {
                ensure!(
                    authority == actor,
                    "Coordinator authority changed while preparing the plan"
                );
                Ok(ApplyPreparation::Ready {
                    intent: self.near_intent(actor, vec![call]),
                    costs,
                })
            }
        }
    }

    pub async fn prepare_coordinator(
        &self,
        session: &str,
        project_key: &str,
        actor: &str,
        action: CoordinatorAction,
        storage_deposit: u128,
    ) -> Result<UnsignedIntent> {
        let actor = self
            .accounts
            .require_wallet(session, Chain::Near, actor)
            .await?
            .address;
        let project = self.project(project_key)?;
        let block = self.indexer.near.finalized_block().await?;
        let state = self
            .indexer
            .near
            .coordinator_state(&project.coordinator, &block)
            .await?;
        ensure!(
            word(&state.project_id)? == word(&project.project_id)?,
            "Coordinator Project identity mismatch"
        );
        action.role().require(&actor, &state)?;

        if let Some(chain) = action.chain() {
            let domain = &project.domains[&chain];
            let registered = self
                .indexer
                .near
                .coordinator_domain(&project.coordinator, &domain.domain, &block)
                .await?;
            validate_domain(chain, domain, &registered)?;
        }
        let (method, args) = action.call(project)?;
        Ok(self.near_intent(
            actor,
            vec![near_call(
                &project.coordinator,
                method,
                args,
                storage_deposit,
            )],
        ))
    }

    pub async fn prepare_ethereum(
        &self,
        session: &str,
        project_key: &str,
        actor: &str,
        action: LocalAction,
    ) -> Result<UnsignedIntent> {
        let actor: Address = self
            .accounts
            .require_wallet(session, Chain::Ethereum, actor)
            .await?
            .address
            .parse()?;
        action.validate()?;
        if let LocalAction::Fund {
            funding_reference, ..
        } = &action
        {
            ensure!(
                funding_reference.is_none(),
                "Ethereum funding assigns its receipt ID on-chain"
            );
        }
        let domain = &self.project(project_key)?.domains[&Chain::Ethereum];
        let vault: Address = domain.vault.parse()?;
        self.indexer.ethereum.verify_network().await?;
        let block = self.indexer.ethereum.finalized_block().await?;
        let state = self.indexer.ethereum.project(vault, &block).await?;
        ensure!(
            state.domain == B256::from(word(&domain.domain)?)
                && state.token == domain.asset.parse::<Address>()?,
            "Ethereum Project binding mismatch"
        );
        if let Some(task) = action.task() {
            let task = self
                .indexer
                .ethereum
                .task(vault, U256::from_be_bytes(word(task)?), &block)
                .await?;
            ensure!(task.task.agent == actor, "Actor is not this task's agent");
        }
        if matches!(action, LocalAction::Fund { .. }) {
            ensure!(state.owner == actor, "Actor is not the Project owner");
        }

        let mut transactions = Vec::new();
        if let Some(amount) = action.token_ingress() {
            transactions.extend(
                ethereum_approval(
                    &self.indexer.ethereum,
                    actor,
                    state.token,
                    vault,
                    amount,
                    &block,
                )
                .await?,
            );
        }
        transactions.push(EthereumCall::new(vault, ethereum_action(&action)?));
        Ok(UnsignedIntent::Ethereum {
            chain_id: self.indexer.config.ethereum.chain_id,
            actor: actor.to_checksum(None),
            transactions,
        })
    }

    pub async fn prepare_solana(
        &self,
        session: &str,
        project_key: &str,
        actor: &str,
        action: LocalAction,
        token_account: Option<&str>,
    ) -> Result<UnsignedIntent> {
        let actor: Pubkey = self
            .accounts
            .require_wallet(session, Chain::Solana, actor)
            .await?
            .address
            .parse()?;
        action.validate()?;
        let client = self.indexer.solana(self.project(project_key)?)?;
        client.validate_deployment().await?;
        let project = client.read_project().await?.value;
        let task = match action.task() {
            Some(id) => {
                let address = Pubkey::new_from_array(word(id)?);
                let task = client
                    .read_task(address)
                    .await?
                    .context("Task does not exist")?
                    .value;
                ensure!(task.agent == actor, "Actor is not this task's agent");
                Some((address, task))
            }
            None => None,
        };
        if matches!(action, LocalAction::Fund { .. }) {
            let workspace = client.read_workspace(project.workspace).await?.value;
            ensure!(workspace.owner == actor, "Actor is not the Project owner");
        }
        let token = if action.uses_token_account() {
            let address: Pubkey = token_account
                .context("This action requires the actor's token account")?
                .parse()?;
            let balance = client.token_balance(address, actor).await?.value;
            if let Some(amount) = action.token_ingress() {
                ensure!(balance >= amount, "Insufficient actor token balance");
            }
            Some(address)
        } else {
            ensure!(
                token_account.is_none(),
                "This action does not use a token account"
            );
            None
        };

        let instruction = solana_action(&client, &project, actor, &action, task.as_ref(), token)?;
        Ok(UnsignedIntent::Solana {
            genesis_hash: self.indexer.config.solana.genesis_hash.clone(),
            actor: actor.to_string(),
            instructions: vec![instruction],
        })
    }

    pub async fn prepare_near(
        &self,
        session: &str,
        project_key: &str,
        actor: &str,
        action: LocalAction,
        storage_deposit: u128,
    ) -> Result<UnsignedIntent> {
        let actor = self
            .accounts
            .require_wallet(session, Chain::Near, actor)
            .await?
            .address;
        action.validate()?;
        let domain = &self.project(project_key)?.domains[&Chain::Near];
        let block = self.indexer.near.finalized_block().await?;
        let project = self
            .indexer
            .near
            .local_project(&domain.vault, &block)
            .await?;
        let bridge = self
            .indexer
            .near
            .local_bridge(&domain.vault, &block)
            .await?;
        ensure!(
            project.token_id == domain.asset
                && bridge.domain.as_deref().map(word).transpose()? == Some(word(&domain.domain)?),
            "NEAR Project binding mismatch"
        );
        if let Some(task) = action.task() {
            let task = self
                .indexer
                .near
                .local_task(&domain.vault, word_number(task)?, &block)
                .await?
                .context("Task does not exist")?;
            ensure!(task.agent == actor, "Actor is not this task's agent");
        }
        if matches!(action, LocalAction::Fund { .. }) {
            ensure!(project.owner_id == actor, "Actor is not the Project owner");
        }

        let mut transactions = Vec::new();
        let registration = match &action {
            LocalAction::Fund { .. } => Some(domain.vault.as_str()),
            LocalAction::Withdraw { .. } => Some(actor.as_str()),
            _ => None,
        };
        if let Some(account) = registration
            && let Some(call) =
                near_token_registration(&self.indexer.near, &domain.asset, account, &block).await?
        {
            transactions.push(call);
        }
        transactions.push(near_action(domain, &action, storage_deposit)?);
        Ok(self.near_intent(actor, transactions))
    }

    fn project(&self, key: &str) -> Result<&ProjectConfig> {
        self.indexer
            .config
            .projects
            .iter()
            .find(|project| self.indexer.config.project_key(project) == key)
            .context("Project is not registered in this deployment")
    }

    fn near_intent(
        &self,
        actor: String,
        transactions: Vec<near::FunctionCallIntent>,
    ) -> UnsignedIntent {
        UnsignedIntent::Near {
            network: self.indexer.config.near.genesis.clone(),
            actor,
            transactions,
        }
    }
}

fn canonical(value: &str) -> Result<String> {
    Ok(hex::encode(word(value)?))
}

fn nonzero(value: &str) -> Result<String> {
    let bytes = word(value)?;
    ensure!(bytes != [0; 32], "Reference must be nonzero");
    Ok(hex::encode(bytes))
}

fn positive(amount: u64) -> Result<()> {
    ensure!(amount > 0, "Amount must be positive");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};

    #[test]
    fn wallet_amounts_and_near_arguments_preserve_integer_precision() {
        const AMOUNT: u64 = 9_007_199_254_740_993;
        let input = json!({
            "action": "create_task", "amount": AMOUNT.to_string(), "purpose": "01".repeat(32),
        });
        let action: LocalAction = serde_json::from_value(input.clone()).unwrap();
        assert!(matches!(
            action,
            LocalAction::CreateTask { amount: AMOUNT, .. }
        ));
        assert_eq!(serde_json::to_value(action).unwrap(), input);
        for invalid in [json!(AMOUNT), json!("-1"), json!("18446744073709551616")] {
            let mut input = input.clone();
            input["amount"] = invalid;
            assert!(serde_json::from_value::<LocalAction>(input).is_err());
        }

        let decision: CoordinatorAction = serde_json::from_value(json!({
            "action": "open_invoice", "amount": AMOUNT.to_string(), "purpose": "01".repeat(32),
        }))
        .unwrap();
        let project = ProjectConfig {
            coordinator: "coordinator.near".into(),
            project_id: "01".repeat(32),
            ethereum_start_block: 0,
            domains: Default::default(),
        };
        let (method, args) = decision.call(&project).unwrap();
        assert_eq!(args["amount"].as_u64(), Some(AMOUNT));
        let call = near_call(&project.coordinator, method, args, 1);
        let wire = serde_json::to_value(&call).unwrap();
        assert!(wire.get("args").is_none());
        let bytes = STANDARD
            .decode(wire["args_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, serde_json::to_vec(&call.args).unwrap());
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("9007199254740993")
        );
        let restored: near::FunctionCallIntent = serde_json::from_value(wire).unwrap();
        assert_eq!(restored.args, call.args);

        let costs = FundingCosts {
            principal: AMOUNT,
            local_amount: AMOUNT,
            expected_transport_cost: 0,
            maximum_transport_cost: 0,
            warnings: Vec::new(),
        };
        let wire = serde_json::to_value(costs).unwrap();
        assert_eq!(wire["principal"], Value::String(AMOUNT.to_string()));
        assert_eq!(wire["local_amount"], Value::String(AMOUNT.to_string()));
        assert_eq!(wire["expected_transport_cost"], "0");
        assert_eq!(wire["maximum_transport_cost"], "0");
    }
}
