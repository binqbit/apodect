use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::state::{Project, Workspace};

#[derive(Accounts)]
pub struct SetAuthority<'info> {
    #[account(
        address = project.workspace,
        has_one = owner
    )]
    pub workspace: Account<'info, Workspace>,
    #[account(mut)]
    pub project: Account<'info, Project>,
    pub owner: Signer<'info>,
}

pub fn handler(ctx: Context<SetAuthority>, authority: Pubkey) -> Result<()> {
    require!(authority != Pubkey::default(), ContractError::Unauthorized);
    ctx.accounts.project.authority = authority;

    event(
        ctx.accounts.project.key(),
        Pubkey::default(),
        Pubkey::default(),
        Action::Authority,
        0,
    );
    Ok(())
}
