//! Native wallet call construction. No signer or broadcast capability lives here.
use super::{canonical, nonzero, positive};
use crate::{
    chains::{ethereum, near, protocol::word_number, solana},
    config::{DomainConfig, word},
};
use alloy_primitives::{Address, U256};
use alloy_sol_types::{SolCall, sol};
use anchor_lang::{InstructionData, ToAccountMetas, prelude::Pubkey, system_program};
use anchor_spl::token::spl_token;
use anyhow::{Context, Result, ensure};
use apodect_solana::{accounts, instruction};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

sol! {
    function createTask(uint256 amount, bytes32 purposeRef) external;
    function requestBudget(uint256 taskId, uint256 amount, bytes32 purposeRef) external;
    function withdraw(uint256 taskId, uint256 amount) external;
    function returnFunds(uint256 taskId, uint256 amount) external;
    function release(uint256 taskId, uint256 amount) external;
    function finalize(uint256 taskId, bytes32 reportRef) external;
    function coordinatedFund(uint256 amount, bytes32 invoiceId) external;
    function approve(address spender, uint256 amount) external returns (bool);
}

/// Task and reference IDs are the protocol's canonical 32-byte hexadecimal
/// words. A Solana task word contains its public key; numeric IDs are big-endian.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum LocalAction {
    CreateTask {
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
        purpose: String,
    },
    RequestBudget {
        task: String,
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
        purpose: String,
    },
    Withdraw {
        task: String,
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
    },
    ReturnFunds {
        task: String,
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
    },
    Release {
        task: String,
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
    },
    Finalize {
        task: String,
        report: Option<String>,
    },
    Fund {
        #[serde(with = "crate::decimal_u64")]
        amount: u64,
        invoice: Option<u64>,
        funding_reference: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct EthereumCall {
    pub to: String,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct SolanaInstruction {
    pub program_id: String,
    pub accounts: Vec<SolanaAccount>,
    pub data_base64: String,
}

#[derive(Debug, Serialize)]
pub struct SolanaAccount {
    pub address: String,
    pub signer: bool,
    pub writable: bool,
}

pub(super) async fn ethereum_approval(
    client: &ethereum::Ethereum,
    actor: Address,
    token: Address,
    vault: Address,
    amount: u64,
    block: &ethereum::FinalizedBlock,
) -> Result<Vec<EthereumCall>> {
    let allowance = client.allowance(token, actor, vault, block).await?;
    let amount = U256::from(amount);
    let mut calls = Vec::new();
    if allowance < amount {
        if !allowance.is_zero() {
            calls.push(EthereumCall::new(
                token,
                approveCall {
                    spender: vault,
                    amount: U256::ZERO,
                }
                .abi_encode(),
            ));
        }
        calls.push(EthereumCall::new(
            token,
            approveCall {
                spender: vault,
                amount,
            }
            .abi_encode(),
        ));
    }
    Ok(calls)
}

pub(super) async fn near_token_registration(
    client: &near::NearClient,
    token: &str,
    account: &str,
    block: &near::NearBlock,
) -> Result<Option<near::FunctionCallIntent>> {
    let balance: Option<Value> = client
        .view(
            token,
            "storage_balance_of",
            &json!({"account_id":account}),
            block,
        )
        .await?;
    if balance.is_some() {
        return Ok(None);
    }
    let bounds: Value = client
        .view(token, "storage_balance_bounds", &json!({}), block)
        .await?;
    let deposit: u128 = bounds["min"]
        .as_str()
        .context("Missing token storage minimum")?
        .parse()?;
    Ok(Some(near_call(
        token,
        "storage_deposit",
        json!({"account_id":account,"registration_only":true}),
        deposit,
    )))
}
impl LocalAction {
    pub(super) fn task(&self) -> Option<&str> {
        match self {
            Self::CreateTask { .. } | Self::Fund { .. } => None,
            Self::RequestBudget { task, .. }
            | Self::Withdraw { task, .. }
            | Self::ReturnFunds { task, .. }
            | Self::Release { task, .. }
            | Self::Finalize { task, .. } => Some(task),
        }
    }

    pub(super) fn token_ingress(&self) -> Option<u64> {
        match self {
            Self::Fund { amount, .. } | Self::ReturnFunds { amount, .. } => Some(*amount),
            _ => None,
        }
    }

    pub(super) fn uses_token_account(&self) -> bool {
        self.token_ingress().is_some() || matches!(self, Self::Withdraw { .. })
    }

    pub(super) fn validate(&self) -> Result<()> {
        if let Some(task) = self.task() {
            word(task)?;
        }
        match self {
            Self::CreateTask { amount, purpose }
            | Self::RequestBudget {
                amount, purpose, ..
            } => {
                positive(*amount)?;
                nonzero(purpose)?;
            }
            Self::Fund {
                amount,
                invoice,
                funding_reference,
            } => {
                positive(*amount)?;
                if let Some(invoice) = invoice {
                    positive(*invoice)?;
                }
                if let Some(reference) = funding_reference {
                    nonzero(reference)?;
                }
            }
            Self::Withdraw { amount, .. }
            | Self::ReturnFunds { amount, .. }
            | Self::Release { amount, .. } => positive(*amount)?,
            Self::Finalize { report, .. } => {
                if let Some(report) = report {
                    word(report)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn ethereum_action(action: &LocalAction) -> Result<Vec<u8>> {
    let task = action
        .task()
        .map(word)
        .transpose()?
        .map(U256::from_be_bytes)
        .unwrap_or_default();
    Ok(match action {
        LocalAction::CreateTask { amount, purpose } => createTaskCall {
            amount: U256::from(*amount),
            purposeRef: word(purpose)?.into(),
        }
        .abi_encode(),
        LocalAction::RequestBudget {
            amount, purpose, ..
        } => requestBudgetCall {
            taskId: task,
            amount: U256::from(*amount),
            purposeRef: word(purpose)?.into(),
        }
        .abi_encode(),
        LocalAction::Withdraw { amount, .. } => withdrawCall {
            taskId: task,
            amount: U256::from(*amount),
        }
        .abi_encode(),
        LocalAction::ReturnFunds { amount, .. } => returnFundsCall {
            taskId: task,
            amount: U256::from(*amount),
        }
        .abi_encode(),
        LocalAction::Release { amount, .. } => releaseCall {
            taskId: task,
            amount: U256::from(*amount),
        }
        .abi_encode(),
        LocalAction::Finalize { report, .. } => finalizeCall {
            taskId: task,
            reportRef: report
                .as_deref()
                .map(word)
                .transpose()?
                .unwrap_or_default()
                .into(),
        }
        .abi_encode(),
        LocalAction::Fund {
            amount, invoice, ..
        } => coordinatedFundCall {
            amount: U256::from(*amount),
            invoiceId: U256::from(invoice.unwrap_or(0)).to_be_bytes::<32>().into(),
        }
        .abi_encode(),
    })
}

impl EthereumCall {
    pub(super) fn new(to: Address, data: Vec<u8>) -> Self {
        Self {
            to: to.to_checksum(None),
            data: format!("0x{}", hex::encode(data)),
            value: "0x0".into(),
        }
    }
}

pub(super) fn solana_action(
    client: &solana::Client,
    project_state: &apodect_solana::state::Project,
    actor: Pubkey,
    action: &LocalAction,
    task: Option<&(Pubkey, apodect_solana::state::Task)>,
    token: Option<Pubkey>,
) -> Result<SolanaInstruction> {
    let project = client.binding().project;
    let mint = client.binding().mint;
    let vault = client.vault_address();
    let task_id = || task.map(|(address, _)| *address).context("Missing task");
    Ok(match action {
        LocalAction::CreateTask { amount, purpose } => {
            let task = client.task_address(project_state.task_count);
            let request = request_address(task, 0);
            solana_instruction(
                accounts::CreateTask {
                    project,
                    task,
                    request,
                    agent: actor,
                    system_program: system_program::ID,
                },
                instruction::CreateTask {
                    amount: *amount,
                    purpose: word(purpose)?,
                },
            )
        }
        LocalAction::RequestBudget {
            amount, purpose, ..
        } => {
            let (task, state) = task.context("Missing task")?;
            solana_instruction(
                accounts::SubmitRequest {
                    project,
                    task: *task,
                    request: request_address(*task, state.request_count),
                    agent: actor,
                    system_program: system_program::ID,
                },
                instruction::SubmitRequest {
                    amount: *amount,
                    purpose: word(purpose)?,
                },
            )
        }
        LocalAction::Withdraw { amount, .. } => solana_instruction(
            accounts::Withdraw {
                project,
                task: task_id()?,
                mint,
                vault,
                agent_token: token.context("Missing token account")?,
                agent: actor,
                token_program: spl_token::ID,
            },
            instruction::Withdraw { amount: *amount },
        ),
        LocalAction::ReturnFunds { amount, .. } => solana_instruction(
            accounts::ReturnFunds {
                project,
                task: task_id()?,
                mint,
                vault,
                agent_token: token.context("Missing token account")?,
                agent: actor,
                token_program: spl_token::ID,
            },
            instruction::ReturnFunds { amount: *amount },
        ),
        LocalAction::Release { amount, .. } => solana_instruction(
            accounts::ReleaseFunds {
                project,
                task: task_id()?,
                agent: actor,
            },
            instruction::ReleaseFunds { amount: *amount },
        ),
        LocalAction::Finalize { report, .. } => {
            let (task, state) = task.context("Missing task")?;
            solana_instruction(
                accounts::FinalizeTask {
                    project,
                    workspace: project_state.workspace,
                    task: *task,
                    actor,
                    pending: state.pending,
                },
                instruction::FinalizeTask {
                    report: report.as_deref().map(word).transpose()?.unwrap_or_default(),
                },
            )
        }
        LocalAction::Fund {
            amount,
            invoice,
            funding_reference,
        } => {
            let funding_id = word(
                funding_reference
                    .as_deref()
                    .context("Solana funding requires a unique funding reference")?,
            )?;
            let receipt = Pubkey::find_program_address(
                &[b"funding", project.as_ref(), &funding_id],
                &apodect_solana::ID,
            )
            .0;
            solana_instruction(
                accounts::FundCoordinated {
                    workspace: project_state.workspace,
                    project,
                    mint,
                    vault,
                    owner_token: token.context("Missing token account")?,
                    owner: actor,
                    receipt,
                    token_program: spl_token::ID,
                    system_program: system_program::ID,
                },
                instruction::FundCoordinated {
                    amount: *amount,
                    invoice_id: U256::from(invoice.unwrap_or(0)).to_be_bytes::<32>(),
                    funding_id,
                },
            )
        }
    })
}

fn solana_instruction(
    accounts: impl ToAccountMetas,
    instruction: impl InstructionData,
) -> SolanaInstruction {
    SolanaInstruction {
        program_id: apodect_solana::ID.to_string(),
        accounts: accounts
            .to_account_metas(None)
            .into_iter()
            .map(|meta| SolanaAccount {
                address: meta.pubkey.to_string(),
                signer: meta.is_signer,
                writable: meta.is_writable,
            })
            .collect(),
        data_base64: STANDARD.encode(instruction.data()),
    }
}

fn request_address(task: Pubkey, index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[b"request", task.as_ref(), &index.to_le_bytes()],
        &apodect_solana::ID,
    )
    .0
}

pub(super) fn near_action(
    domain: &DomainConfig,
    action: &LocalAction,
    storage_deposit: u128,
) -> Result<near::FunctionCallIntent> {
    let task = action.task().map(word_number).transpose()?;
    let (receiver, method, args, deposit) = match action {
        LocalAction::CreateTask { amount, purpose } => (
            &domain.vault,
            "create_task",
            json!({"amount":amount.to_string(),"purpose":word(purpose)?}),
            storage_deposit,
        ),
        LocalAction::RequestBudget {
            amount, purpose, ..
        } => (
            &domain.vault,
            "request_budget",
            json!({"task_id":task,"amount":amount.to_string(),"purpose":word(purpose)?}),
            storage_deposit,
        ),
        LocalAction::Withdraw { amount, .. } => (
            &domain.vault,
            "withdraw",
            json!({"task_id":task,"amount":amount.to_string()}),
            storage_deposit,
        ),
        LocalAction::Release { amount, .. } => (
            &domain.vault,
            "release",
            json!({"task_id":task,"amount":amount.to_string()}),
            storage_deposit,
        ),
        LocalAction::Finalize { report, .. } => (
            &domain.vault,
            "finalize",
            json!({"task_id":task,"report":report.as_deref().map(word).transpose()?}),
            storage_deposit,
        ),
        LocalAction::Fund {
            amount,
            invoice,
            funding_reference,
        } => {
            ensure!(
                storage_deposit == 1,
                "Token ingress requires exactly one yoctoNEAR"
            );
            let msg = json!({"kind":"fund","global_invoice":hex::encode(U256::from(invoice.unwrap_or(0)).to_be_bytes::<32>()),"funding_ref":canonical(funding_reference.as_deref().context("NEAR funding requires a unique funding reference")?)?});
            (
                &domain.asset,
                "ft_transfer_call",
                json!({"receiver_id":domain.vault,"amount":amount.to_string(),"msg":msg.to_string()}),
                1,
            )
        }
        LocalAction::ReturnFunds { amount, .. } => {
            ensure!(
                storage_deposit == 1,
                "Token ingress requires exactly one yoctoNEAR"
            );
            let msg = json!({"kind":"return","task_id":task});
            (
                &domain.asset,
                "ft_transfer_call",
                json!({"receiver_id":domain.vault,"amount":amount.to_string(),"msg":msg.to_string()}),
                1,
            )
        }
    };
    Ok(near_call(receiver, method, args, deposit))
}

pub(super) fn near_call(
    receiver: &str,
    method: &str,
    args: Value,
    deposit: u128,
) -> near::FunctionCallIntent {
    near::FunctionCallIntent {
        receiver_id: receiver.into(),
        method_name: method.into(),
        args,
        gas: 300_000_000_000_000,
        deposit,
    }
}
