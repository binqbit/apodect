use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::retire_request;
use crate::state::{BudgetRequest, Project, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct MarkOutOfBudget<'info> {
    #[account(has_one = authority)]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project
    )]
    pub task: Account<'info, Task>,
    pub authority: Signer<'info>,
    #[account(mut)]
    pub pending: Option<Account<'info, BudgetRequest>>,
}

pub fn handler(ctx: Context<MarkOutOfBudget>) -> Result<()> {
    ctx.accounts.project.standalone()?;
    ctx.accounts.task.nonterminal()?;
    require!(
        ctx.accounts.task.lifecycle != TaskLifecycle::OutOfBudget,
        ContractError::InvalidTaskState
    );
    retire_request(
        ctx.accounts.project.key(),
        &mut ctx.accounts.task,
        &mut ctx.accounts.pending,
    )?;

    ctx.accounts.task.lifecycle = TaskLifecycle::OutOfBudget;

    event(
        ctx.accounts.project.key(),
        ctx.accounts.task.key(),
        Pubkey::default(),
        Action::OutOfBudget,
        0,
    );
    Ok(())
}
