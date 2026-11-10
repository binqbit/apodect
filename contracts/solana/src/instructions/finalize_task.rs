use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::retire_request;
use crate::state::{BudgetRequest, Project, Task, TaskLifecycle, Workspace};

#[derive(Accounts)]
pub struct FinalizeTask<'info> {
    #[account(address = project.workspace)]
    pub workspace: Account<'info, Workspace>,
    #[account(mut)]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project
    )]
    pub task: Account<'info, Task>,
    pub actor: Signer<'info>,
    #[account(mut)]
    pub pending: Option<Account<'info, BudgetRequest>>,
}

pub fn handler(ctx: Context<FinalizeTask>, report: [u8; 32]) -> Result<()> {
    require_keys_eq!(
        ctx.accounts.actor.key(),
        ctx.accounts.task.agent,
        ContractError::Unauthorized
    );
    let task = &mut ctx.accounts.task;
    task.nonterminal()?;
    require!(
        task.issued > 0 && task.available == 0,
        ContractError::CannotFinalize
    );
    retire_request(ctx.accounts.project.key(), task, &mut ctx.accounts.pending)?;
    let expense = task.external_outstanding()?;
    task.report = report;
    task.lifecycle = TaskLifecycle::Completed;

    event(
        ctx.accounts.project.key(),
        task.key(),
        Pubkey::default(),
        Action::Finalize,
        expense,
    );
    Ok(())
}
