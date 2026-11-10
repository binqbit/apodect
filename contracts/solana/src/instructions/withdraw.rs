use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{positive, sub};
use crate::state::{Project, Task, TaskLifecycle};

#[derive(Accounts)]
pub struct Withdraw<'info> {
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

pub fn handler(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
    positive(amount)?;
    let task = &ctx.accounts.task;
    require!(
        task.lifecycle == TaskLifecycle::Active && !task.paused,
        ContractError::InvalidTaskState
    );
    require!(
        amount <= task.available,
        ContractError::InsufficientAllocation
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
                to: ctx.accounts.agent_token.to_account_info(),
                authority: project.to_account_info(),
            },
        )
        .with_signer(&[seeds]),
        amount,
        ctx.accounts.mint.decimals,
    )?;

    ctx.accounts.task.available = sub(ctx.accounts.task.available, amount)?;
    ctx.accounts.task.started = true;

    event(
        project.key(),
        ctx.accounts.task.key(),
        Pubkey::default(),
        Action::Withdraw,
        amount,
    );
    Ok(())
}
