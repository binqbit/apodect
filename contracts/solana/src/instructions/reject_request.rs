use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event_with_stop, Action, StopReason};
use crate::state::{BudgetRequest, Project, RequestKind, RequestState, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct RejectRequest<'info> {
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

pub fn handler(ctx: Context<RejectRequest>) -> Result<()> {
    ctx.accounts.project.standalone()?;
    require!(
        ctx.accounts.tail.is_none(),
        ContractError::WrongQueueAccount
    );
    let task = &mut ctx.accounts.task;
    task.nonterminal()?;
    require!(
        task.pending == Some(ctx.accounts.request.key()),
        ContractError::WrongLiveRequest
    );
    let request = &mut ctx.accounts.request;
    require!(
        request.state == RequestState::Pending,
        ContractError::InvalidRequestState
    );
    request.state = RequestState::Rejected;
    task.pending = None;
    let stopped = request.kind == RequestKind::Supplement;
    if stopped {
        task.lifecycle = TaskLifecycle::Stopped;
    }

    event_with_stop(
        ctx.accounts.project.key(),
        task.key(),
        request.key(),
        Action::Reject,
        request.amount,
        stopped.then_some(StopReason::SupplementRejected),
    );
    Ok(())
}
