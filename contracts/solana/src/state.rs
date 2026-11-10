use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::helpers::sub;

#[account]
#[derive(InitSpace)]
pub struct Workspace {
    pub owner: Pubkey,
    pub project_count: u64,
}

#[account]
#[derive(InitSpace)]
pub struct Project {
    pub workspace: Pubkey,
    pub index: u64,
    pub bump: u8,
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub free: u64,
    pub task_count: u64,
    pub invoice_count: u64,
    pub approval_count: u64,
    pub head: Option<Pubkey>,
    pub tail: Option<Pubkey>,
}

#[account]
#[derive(InitSpace)]
pub struct Task {
    pub project: Pubkey,
    pub agent: Pubkey,
    pub lifecycle: TaskLifecycle,
    pub paused: bool,
    pub started: bool,
    pub available: u64,
    pub issued: u64,
    pub recovered: u64,
    pub request_count: u64,
    pub pending: Option<Pubkey>,
    pub report: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum TaskLifecycle {
    Active,
    Stopped,
    OutOfBudget,
    Completed,
    Canceled,
}

impl Task {
    pub fn final_expense(&self) -> Result<Option<u64>> {
        if self.lifecycle == TaskLifecycle::Completed {
            Ok(Some(sub(self.issued, self.recovered)?))
        } else {
            Ok(None)
        }
    }

    pub fn nonterminal(&self) -> Result<()> {
        require!(
            !matches!(
                self.lifecycle,
                TaskLifecycle::Completed | TaskLifecycle::Canceled
            ),
            ContractError::TerminalTask
        );
        Ok(())
    }

    pub fn external_outstanding(&self) -> Result<u64> {
        sub(sub(self.issued, self.recovered)?, self.available)
    }
}

#[account]
#[derive(InitSpace)]
pub struct BudgetRequest {
    pub project: Pubkey,
    pub task: Pubkey,
    pub amount: u64,
    pub purpose: [u8; 32],
    pub kind: RequestKind,
    pub state: RequestState,
    pub approval_sequence: u64,
    pub next: Option<Pubkey>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum RequestKind {
    Initial,
    Supplement,
    Restart,
    Restoration,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum RequestState {
    Pending,
    Queued,
    Funded,
    Rejected,
    Removed,
}

#[account]
#[derive(InitSpace)]
pub struct Invoice {
    pub project: Pubkey,
    pub amount: u64,
    pub purpose: [u8; 32],
    pub closed: bool,
    pub intended_contribution: u64,
    pub response: [u8; 32],
    pub received: u64,
}
