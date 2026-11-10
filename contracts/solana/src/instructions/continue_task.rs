use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::state::{Project, Task, TaskLifecycle, Workspace};

#[derive(Accounts)]
pub struct ContinueTask<'info> {
    #[account(
        address = project.workspace,
        has_one = owner
    )]
    pub workspace: Account<'info, Workspace>,
    #[account(mut)]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project
    )]
    pub task: Account<'info, Task>,
    pub owner: Signer<'info>,
}

pub fn handler(ctx: Context<ContinueTask>) -> Result<()> {
    let task = &mut ctx.accounts.task;
    require!(
        task.lifecycle == TaskLifecycle::Active && task.paused && task.available > 0,
        ContractError::InvalidTaskState
    );
    task.paused = false;

    event(
        ctx.accounts.project.key(),
        task.key(),
        Pubkey::default(),
        Action::Continue,
        task.available,
    );
    Ok(())
}
