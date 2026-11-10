use anchor_lang::prelude::*;

use crate::helpers::{add, initialize_request};
use crate::state::{BudgetRequest, Project, RequestKind, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct CreateTask<'info> {
    #[account(mut)]
    pub project: Account<'info, Project>,
    #[account(
        init,
        payer = agent,
        space = 8 + Task::INIT_SPACE,
        seeds = [b"task", project.key().as_ref(), &project.task_count.to_le_bytes()],
        bump
    )]
    pub task: Account<'info, Task>,
    #[account(
        init,
        payer = agent,
        space = 8 + BudgetRequest::INIT_SPACE,
        seeds = [b"request", task.key().as_ref(), &0u64.to_le_bytes()],
        bump
    )]
    pub request: Account<'info, BudgetRequest>,
    #[account(mut)]
    pub agent: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<CreateTask>, amount: u64, reference: [u8; 32]) -> Result<()> {
    let project = &mut ctx.accounts.project;
    ctx.accounts.task.set_inner(Task {
        project: project.key(),
        agent: ctx.accounts.agent.key(),
        lifecycle: TaskLifecycle::Active,
        paused: false,
        started: false,
        available: 0,
        issued: 0,
        recovered: 0,
        request_count: 0,
        pending: None,
        report: [0; 32],
    });
    initialize_request(
        &mut ctx.accounts.request,
        &mut ctx.accounts.task,
        project.key(),
        amount,
        reference,
        RequestKind::Initial,
    )?;

    project.task_count = add(project.task_count, 1)?;
    Ok(())
}
