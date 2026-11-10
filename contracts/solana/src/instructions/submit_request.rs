use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::helpers::initialize_request;
use crate::state::{BudgetRequest, Project, RequestKind, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct SubmitRequest<'info> {
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project,
        has_one = agent
    )]
    pub task: Account<'info, Task>,
    #[account(
        init,
        payer = agent,
        space = 8 + BudgetRequest::INIT_SPACE,
        seeds = [b"request", task.key().as_ref(), &task.request_count.to_le_bytes()],
        bump
    )]
    pub request: Account<'info, BudgetRequest>,
    #[account(mut)]
    pub agent: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<SubmitRequest>, amount: u64, reference: [u8; 32]) -> Result<()> {
    let task = &mut ctx.accounts.task;
    task.nonterminal()?;
    require!(
        task.lifecycle != TaskLifecycle::OutOfBudget,
        ContractError::InvalidTaskState
    );
    let kind = if task.lifecycle == TaskLifecycle::Stopped {
        RequestKind::Restart
    } else if task.issued == 0 {
        RequestKind::Initial
    } else {
        require!(task.started, ContractError::InvalidTaskState);
        RequestKind::Supplement
    };
    initialize_request(
        &mut ctx.accounts.request,
        task,
        ctx.accounts.project.key(),
        amount,
        reference,
        kind,
    )
}
