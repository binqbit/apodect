use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{add, positive};
use crate::state::{Project, Task};

#[derive(Accounts)]
pub struct ReturnFunds<'info> {
    #[account(mut)]
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project,
        has_one = agent
    )]
    pub task: Account<'info, Task>,
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
        token::authority = agent
    )]
    pub agent_token: Account<'info, TokenAccount>,
    pub agent: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<ReturnFunds>, amount: u64) -> Result<()> {
    positive(amount)?;
    ctx.accounts.task.nonterminal()?;
    require!(
        amount <= ctx.accounts.task.external_outstanding()?,
        ContractError::ExcessReturn
    );
    let free = add(ctx.accounts.project.free, amount)?;
    let recovered = add(ctx.accounts.task.recovered, amount)?;

    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.agent_token.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.agent.to_account_info(),
            },
        ),
        amount,
        ctx.accounts.mint.decimals,
    )?;

    ctx.accounts.project.free = free;
    ctx.accounts.task.recovered = recovered;

    event(
        ctx.accounts.project.key(),
        ctx.accounts.task.key(),
        Pubkey::default(),
        Action::Return,
        amount,
    );
    Ok(())
}
