use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{add, positive};
use crate::state::{Invoice, Project, Workspace};

#[derive(Accounts)]
pub struct FundProject<'info> {
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
    #[account(mut)]
    pub invoice: Option<Account<'info, Invoice>>,
}

pub fn handler(ctx: Context<FundProject>, amount: u64) -> Result<()> {
    positive(amount)?;
    ctx.accounts.project.standalone()?;
    let accounts = ctx.accounts;
    let free = add(accounts.project.free, amount)?;
    if let Some(invoice) = accounts.invoice.as_mut() {
        require!(
            invoice.project == accounts.project.key() && invoice.closed,
            ContractError::InvalidInvoice
        );
        invoice.received = add(invoice.received, amount)?;
    }

    token::transfer_checked(
        CpiContext::new(
            accounts.token_program.key(),
            TransferChecked {
                from: accounts.owner_token.to_account_info(),
                mint: accounts.mint.to_account_info(),
                to: accounts.vault.to_account_info(),
                authority: accounts.owner.to_account_info(),
            },
        ),
        amount,
        accounts.mint.decimals,
    )?;

    accounts.project.free = free;

    event(
        accounts.project.key(),
        Pubkey::default(),
        accounts
            .invoice
            .as_ref()
            .map(|i| i.key())
            .unwrap_or_default(),
        Action::Fund,
        amount,
    );
    Ok(())
}
