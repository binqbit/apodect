use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{release, retire_request};
use crate::state::{BudgetRequest, Project, Task, TaskLifecycle, Workspace};

#[derive(Accounts)]
pub struct CancelTask<'info> {
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

pub fn handler(ctx: Context<CancelTask>) -> Result<()> {
    require_keys_eq!(
        ctx.accounts.actor.key(),
        ctx.accounts.workspace.owner,
        ContractError::Unauthorized
    );
    let task = &mut ctx.accounts.task;
    task.nonterminal()?;
    require!(!task.started, ContractError::AlreadyStarted);
    retire_request(ctx.accounts.project.key(), task, &mut ctx.accounts.pending)?;
    let amount = task.available;
    if amount > 0 {
        release(&mut ctx.accounts.project, task, amount, Action::Release)?;
    }
    task.lifecycle = TaskLifecycle::Canceled;

    event(
        ctx.accounts.project.key(),
        task.key(),
        Pubkey::default(),
        Action::Cancel,
        amount,
    );
    Ok(())
}
