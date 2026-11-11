//! Typed owner and authority decisions for the coordinator.
use super::{canonical, nonzero, positive};
use crate::{
    chains::near,
    config::{Chain, DomainConfig, ProjectConfig, word},
};
use alloy_primitives::Address;
use anchor_lang::prelude::Pubkey;
use anyhow::{Result, ensure};
use apodect_solana::bridge::action;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CoordinatorAction {
    SetAuthority {
        account: String,
    },
    SetRetryLimit {
        attempts: u32,
    },
    Approve {
        chain: Chain,
        request: String,
    },
    Reject {
        chain: Chain,
        request: String,
        expires: u64,
    },
    Control {
        chain: Chain,
        task: String,
        control: TaskControl,
        expires: u64,
    },
    Restore {
        chain: Chain,
        task: String,
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
        purpose: String,
        expires: u64,
    },
    OutOfBudget {
        chain: Chain,
        task: String,
        expires: u64,
    },
    WithdrawFree {
        chain: Chain,
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
        expires: u64,
    },
    Abort {
        plan: String,
    },
    Retry {
        plan: String,
        error: Option<String>,
        unverified: Option<String>,
    },
    Invalidate {
        operation: String,
        expires: u64,
    },
    Requeue {
        plan: String,
    },
    ResumePlan {
        plan: String,
    },
    OpenInvoice {
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
        purpose: String,
    },
    RespondInvoice {
        #[serde(with = "crate::decimal_u64")]
        invoice: u64,
        #[serde(with = "crate::decimal_u64")]
        intended: u64,
        reference: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TaskControl {
    Pause,
    Reclaim {
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
    },
    Continue,
    Cancel,
}

#[derive(Clone, Copy)]
pub(super) enum Role {
    Owner,
    Authority,
    OwnerOrAuthority,
}

impl Role {
    pub(super) fn require(self, actor: &str, state: &near::CoordinatorState) -> Result<()> {
        let permitted = match self {
            Self::Owner => actor == state.owner_id,
            Self::Authority => actor == state.authority_id,
            Self::OwnerOrAuthority => actor == state.owner_id || actor == state.authority_id,
        };
        ensure!(
            permitted,
            "Actor does not hold the required coordinator role"
        );
        Ok(())
    }
}

impl CoordinatorAction {
    pub(super) fn role(&self) -> Role {
        match self {
            Self::SetAuthority { .. }
            | Self::SetRetryLimit { .. }
            | Self::Control { .. }
            | Self::WithdrawFree { .. }
            | Self::RespondInvoice { .. } => Role::Owner,
            Self::Abort { .. } | Self::Invalidate { .. } => Role::OwnerOrAuthority,
            Self::Approve { .. }
            | Self::Reject { .. }
            | Self::Restore { .. }
            | Self::OutOfBudget { .. }
            | Self::Retry { .. }
            | Self::Requeue { .. }
            | Self::ResumePlan { .. }
            | Self::OpenInvoice { .. } => Role::Authority,
        }
    }

    pub(super) fn chain(&self) -> Option<Chain> {
        match self {
            Self::Approve { chain, .. }
            | Self::Reject { chain, .. }
            | Self::Control { chain, .. }
            | Self::Restore { chain, .. }
            | Self::OutOfBudget { chain, .. }
            | Self::WithdrawFree { chain, .. } => Some(*chain),
            _ => None,
        }
    }

    pub(super) fn call(self, project: &ProjectConfig) -> Result<(&'static str, Value)> {
        let domain = self
            .chain()
            .map(|chain| canonical(&project.domains[&chain].domain))
            .transpose()?;
        Ok(match self {
            Self::SetAuthority { account } => {
                let account: near_primitives::types::AccountId = account.parse()?;
                ("set_authority", json!({"authority_id":account}))
            }
            Self::SetRetryLimit { attempts } => {
                ensure!(
                    (1..=20).contains(&attempts),
                    "Retry limit must be between 1 and 20"
                );
                ("set_retry_limit", json!({"attempts":attempts}))
            }
            Self::Approve { request, .. } => (
                "approve",
                json!({"domain":domain,"request":canonical(&request)?}),
            ),
            Self::Reject {
                request, expires, ..
            } => (
                "reject",
                json!({"domain":domain,"request":canonical(&request)?,"expires":expires}),
            ),
            Self::Control {
                task,
                control,
                expires,
                ..
            } => {
                let (action, amount) = match control {
                    TaskControl::Pause => (action::PAUSE, 0),
                    TaskControl::Reclaim { amount } => {
                        positive(amount)?;
                        (action::RECLAIM, amount)
                    }
                    TaskControl::Continue => (action::CONTINUE, 0),
                    TaskControl::Cancel => (action::CANCEL, 0),
                };
                (
                    "control",
                    json!({"domain":domain,"task":canonical(&task)?,"action":action,"amount":amount,"expires":expires}),
                )
            }
            Self::Restore {
                task,
                amount,
                purpose,
                expires,
                ..
            } => {
                positive(amount)?;
                (
                    "restore",
                    json!({"domain":domain,"task":canonical(&task)?,"amount":amount,"purpose":nonzero(&purpose)?,"expires":expires}),
                )
            }
            Self::OutOfBudget { task, expires, .. } => (
                "out_of_budget",
                json!({"domain":domain,"task":canonical(&task)?,"expires":expires}),
            ),
            Self::WithdrawFree {
                amount, expires, ..
            } => {
                positive(amount)?;
                (
                    "withdraw_free",
                    json!({"domain":domain,"amount":amount,"expires":expires}),
                )
            }
            Self::Abort { plan } => ("abort", json!({"plan":nonzero(&plan)?})),
            Self::Retry {
                plan,
                error,
                unverified,
            } => {
                ensure!(
                    error.as_ref().is_none_or(|s| s.len() <= 512)
                        && unverified.as_ref().is_none_or(|s| s.len() <= 512),
                    "Retry explanation exceeds 512 bytes"
                );
                (
                    "retry",
                    json!({"plan":nonzero(&plan)?,"error":error,"unverified":unverified}),
                )
            }
            Self::Invalidate { operation, expires } => (
                "invalidate",
                json!({"operation":nonzero(&operation)?,"expires":expires}),
            ),
            Self::Requeue { plan } => ("requeue", json!({"plan":nonzero(&plan)?})),
            Self::ResumePlan { plan } => ("resume_plan", json!({"plan":nonzero(&plan)?})),
            Self::OpenInvoice { amount, purpose } => {
                positive(amount)?;
                (
                    "open_invoice",
                    json!({"amount":amount,"purpose":nonzero(&purpose)?}),
                )
            }
            Self::RespondInvoice {
                invoice,
                intended,
                reference,
            } => {
                positive(invoice)?;
                (
                    "respond_invoice",
                    json!({"invoice":invoice,"intended":intended,"reference":reference.as_deref().map(canonical).transpose()?}),
                )
            }
        })
    }
}

pub(super) fn validate_domain(
    chain: Chain,
    expected: &DomainConfig,
    actual: &near::CoordinatorDomain,
) -> Result<()> {
    ensure!(
        actual.chain == chain.to_string() && word(&actual.domain)? == word(&expected.domain)?,
        "Coordinator domain identity mismatch"
    );
    let bindings_match = match chain {
        Chain::Ethereum => {
            actual.vault.parse::<Address>()? == expected.vault.parse::<Address>()?
                && actual.asset.parse::<Address>()? == expected.asset.parse::<Address>()?
        }
        Chain::Solana => {
            actual.program.as_deref().map(word).transpose()? == Some(apodect_solana::ID.to_bytes())
                && actual.vault.parse::<Pubkey>()? == expected.vault.parse::<Pubkey>()?
                && actual.asset.parse::<Pubkey>()? == expected.asset.parse::<Pubkey>()?
        }
        Chain::Near => actual.vault == expected.vault && actual.asset == expected.asset,
    };
    ensure!(
        bindings_match,
        "Coordinator vault or token binding mismatch"
    );
    Ok(())
}
