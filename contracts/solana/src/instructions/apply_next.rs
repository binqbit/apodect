use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{add, sub};
use crate::state::{BudgetRequest, Project, RequestState, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct ApplyNext<'info> {
    #[account(
        mut,
        has_one = authority
    )]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project
    )]
    pub head: Account<'info, BudgetRequest>,
    pub authority: Signer<'info>,
    #[account(mut)]
    pub task: Option<Account<'info, Task>>,
}

pub fn handler(ctx: Context<ApplyNext>) -> Result<()> {
    let project = &mut ctx.accounts.project;
    let head = &mut ctx.accounts.head;
    require!(
        project.head == Some(head.key()),
        ContractError::WrongQueueAccount
    );
    if head.state == RequestState::Removed {
        require!(ctx.accounts.task.is_none(), ContractError::WrongLiveRequest);
        event(
            project.key(),
            head.task,
            head.key(),
            Action::AdvanceRemoved,
            0,
        );
    } else {
        require!(
            head.state == RequestState::Queued,
            ContractError::InvalidRequestState
        );
        let task = ctx
            .accounts
            .task
            .as_mut()
            .ok_or_else(|| error!(ContractError::WrongLiveRequest))?;
        require!(
            task.key() == head.task
                && task.project == project.key()
                && task.pending == Some(head.key()),
            ContractError::WrongLiveRequest
        );
        task.nonterminal()?;
        require!(
            matches!(
                task.lifecycle,
                TaskLifecycle::Active | TaskLifecycle::Stopped
            ),
            ContractError::InvalidTaskState
        );
        require!(
            project.free >= head.amount,
            ContractError::InsufficientProjectFunds
        );
        task.available = add(task.available, head.amount)?;
        task.issued = add(task.issued, head.amount)?;
        task.lifecycle = TaskLifecycle::Active;
        task.pending = None;
        project.free = sub(project.free, head.amount)?;
        head.state = RequestState::Funded;
        event(
            project.key(),
            task.key(),
            head.key(),
            Action::Allocate,
            head.amount,
        );
    }
    project.head = head.next;
    if project.head.is_none() {
        project.tail = None;
    }
    Ok(())
}
