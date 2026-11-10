use anchor_lang::prelude::*;

use crate::events::Action;
use crate::helpers::release;
use crate::state::{Project, Task};

#[derive(Accounts)]
pub struct ReleaseFunds<'info> {
    #[account(mut)]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project,
        has_one = agent
    )]
    pub task: Account<'info, Task>,
    pub agent: Signer<'info>,
}

pub fn handler(ctx: Context<ReleaseFunds>, amount: u64) -> Result<()> {
    release(
        &mut ctx.accounts.project,
        &mut ctx.accounts.task,
        amount,
        Action::Release,
    )
}
