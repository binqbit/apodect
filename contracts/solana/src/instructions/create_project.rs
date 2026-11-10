use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::errors::ContractError;
use crate::helpers::add;
use crate::state::{Project, Workspace};

#[derive(Accounts)]
pub struct CreateProject<'info> {
    #[account(
        mut,
        has_one = owner
    )]
    pub workspace: Account<'info, Workspace>,
    #[account(
        init,
        payer = owner,
        space = 8 + Project::INIT_SPACE,
        seeds = [b"project", workspace.key().as_ref(), &workspace.project_count.to_le_bytes()],
        bump
    )]
    pub project: Account<'info, Project>,
    #[account(constraint = mint.decimals == 6 @ ContractError::InvalidMint)]
    pub mint: Account<'info, Mint>,
    #[account(
        init,
        payer = owner,
        seeds = [b"vault", project.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = project
    )]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<CreateProject>, authority: Pubkey) -> Result<()> {
    require!(authority != Pubkey::default(), ContractError::Unauthorized);
    let workspace = &mut ctx.accounts.workspace;
    ctx.accounts.project.set_inner(Project {
        workspace: workspace.key(),
        index: workspace.project_count,
        bump: ctx.bumps.project,
        authority,
        mint: ctx.accounts.mint.key(),
        free: 0,
        task_count: 0,
        invoice_count: 0,
        approval_count: 0,
        head: None,
        tail: None,
    });
    workspace.project_count = add(workspace.project_count, 1)?;
    Ok(())
}
