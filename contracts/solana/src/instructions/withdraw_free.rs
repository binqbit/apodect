use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{positive, sub};
use crate::state::{Project, Workspace};

#[derive(Accounts)]
pub struct WithdrawFree<'info> {
    #[account(
        address = project.workspace,
        has_one = owner
    )]
    pub workspace: Account<'info, Workspace>,
    #[account(mut)]
    pub project: Account<'info, Project>,
    #[account(address = project.mint)]
    pub mint: Account<'info, Mint>,
    #[account(
        mut,
        seeds = [b"vault", project.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = project
    )]
    pub vault: Account<'info, TokenAccount>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner
    )]
    pub owner_token: Account<'info, TokenAccount>,
    pub owner: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<WithdrawFree>, amount: u64) -> Result<()> {
    ctx.accounts.project.standalone()?;
    positive(amount)?;
    require!(
        amount <= ctx.accounts.project.free,
        ContractError::InsufficientProjectFunds
    );
    let project = &ctx.accounts.project;
    let index = project.index.to_le_bytes();
    let bump = [project.bump];
    let seeds: &[&[u8]] = &[b"project", project.workspace.as_ref(), &index, &bump];

    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.owner_token.to_account_info(),
                authority: project.to_account_info(),
            },
        )
        .with_signer(&[seeds]),
        amount,
        ctx.accounts.mint.decimals,
    )?;

    ctx.accounts.project.free = sub(ctx.accounts.project.free, amount)?;

    event(
        ctx.accounts.project.key(),
        Pubkey::default(),
        Pubkey::default(),
        Action::WithdrawFree,
        amount,
    );
    Ok(())
}
