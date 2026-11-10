use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::Action;
use crate::helpers::release;
use crate::state::{Project, Task, Workspace};

#[derive(Accounts)]
pub struct Reclaim<'info> {
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

pub fn handler(ctx: Context<Reclaim>, amount: u64) -> Result<()> {
    require!(ctx.accounts.task.paused, ContractError::InvalidTaskState);
    release(
        &mut ctx.accounts.project,
        &mut ctx.accounts.task,
        amount,
        Action::Reclaim,
    )
}
