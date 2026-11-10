use anchor_lang::prelude::*;

use crate::state::Workspace;

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct InitializeWorkspace<'info> {
    #[account(
        init,
        payer = owner,
        space = 8 + Workspace::INIT_SPACE,
        seeds = [b"workspace", owner.key().as_ref(), &nonce.to_le_bytes()],
        bump
    )]
    pub workspace: Account<'info, Workspace>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeWorkspace>, _nonce: u64) -> Result<()> {
    ctx.accounts.workspace.set_inner(Workspace {
        owner: ctx.accounts.owner.key(),
        project_count: 0,
    });
    Ok(())
}
