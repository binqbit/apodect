use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{append_queue, initialize_request};
use crate::state::{BudgetRequest, Project, RequestKind, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct RestoreBudget<'info> {
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
        init,
        payer = authority,
        space = 8 + BudgetRequest::INIT_SPACE,
        seeds = [b"request", task.key().as_ref(), &task.request_count.to_le_bytes()],
        bump
    )]
    pub request: Account<'info, BudgetRequest>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(mut)]
    pub tail: Option<Account<'info, BudgetRequest>>,
}

pub fn handler(ctx: Context<RestoreBudget>, amount: u64, reference: [u8; 32]) -> Result<()> {
    ctx.accounts.project.standalone()?;
    let task = &mut ctx.accounts.task;
    require!(
        task.lifecycle == TaskLifecycle::Active && task.paused && task.issued > 0,
        ContractError::InvalidTaskState
    );
    initialize_request(
        &mut ctx.accounts.request,
        task,
        ctx.accounts.project.key(),
        amount,
        reference,
        RequestKind::Restoration,
    )?;
    append_queue(
        &mut ctx.accounts.project,
        &mut ctx.accounts.request,
        &mut ctx.accounts.tail,
    )?;

    event(
        ctx.accounts.project.key(),
        task.key(),
        ctx.accounts.request.key(),
        Action::Restore,
        amount,
    );
    Ok(())
}
