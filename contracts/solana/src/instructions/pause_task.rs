use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::state::{Project, Task, Workspace};

#[derive(Accounts)]
pub struct PauseTask<'info> {
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

pub fn handler(ctx: Context<PauseTask>) -> Result<()> {
    ctx.accounts.task.nonterminal()?;
    require!(!ctx.accounts.task.paused, ContractError::InvalidTaskState);
    ctx.accounts.task.paused = true;

    event(
        ctx.accounts.project.key(),
        ctx.accounts.task.key(),
        Pubkey::default(),
        Action::Pause,
        0,
    );
    Ok(())
}
