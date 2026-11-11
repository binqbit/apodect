use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::append_queue;
use crate::state::{BudgetRequest, Project, RequestState, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct ApproveRequest<'info> {
    #[account(
        mut,
        has_one = authority
    )]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project
    )]
    pub task: Account<'info, Task>,
    #[account(
        mut,
        has_one = project,
        has_one = task
    )]
    pub request: Account<'info, BudgetRequest>,
    pub authority: Signer<'info>,
    #[account(mut)]
    pub tail: Option<Account<'info, BudgetRequest>>,
}

pub fn handler(ctx: Context<ApproveRequest>) -> Result<()> {
    ctx.accounts.project.standalone()?;
    let task = &ctx.accounts.task;
    task.nonterminal()?;
    require!(
        task.lifecycle != TaskLifecycle::OutOfBudget,
        ContractError::InvalidTaskState
    );
    require!(
        task.pending == Some(ctx.accounts.request.key()),
        ContractError::WrongLiveRequest
    );
    require!(
        ctx.accounts.request.state == RequestState::Pending,
        ContractError::InvalidRequestState
    );
    append_queue(
        &mut ctx.accounts.project,
        &mut ctx.accounts.request,
        &mut ctx.accounts.tail,
    )?;

    event(
        ctx.accounts.project.key(),
        task.key(),
        ctx.accounts.request.key(),
        Action::Approve,
        ctx.accounts.request.amount,
    );
    Ok(())
}
