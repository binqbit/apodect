use anchor_lang::prelude::*;
use anchor_lang::Discriminator;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::bridge::{self, FundingReceipt};
use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::helpers::{add, positive};
use crate::state::{Project, Workspace};

#[derive(Accounts)]
pub struct FundCoordinated<'info> {
    #[account(address = project.workspace, has_one = owner)]
    pub workspace: Account<'info, Workspace>,
    #[account(mut, has_one = mint)]
    pub project: Account<'info, Project>,
    pub mint: Account<'info, Mint>,
    #[account(mut, seeds = [b"vault", project.key().as_ref()], bump,
        token::mint = mint, token::authority = project)]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::authority = owner)]
    pub owner_token: Account<'info, TokenAccount>,
    #[account(mut)]
    pub owner: Signer<'info>,
    /// CHECK: Unique funding PDA, initialized once with an exact immutable proof payload.
    #[account(mut)]
    pub receipt: UncheckedAccount<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<FundCoordinated>,
    funding_id: [u8; 32],
    invoice_id: [u8; 32],
    amount: u64,
) -> Result<()> {
    positive(amount)?;
    let a = ctx.accounts;
    require!(
        a.project.domain != [0; 32] && funding_id != [0; 32],
        ContractError::InvalidBinding
    );
    let free = add(a.project.free, amount)?;
    let project = a.project.key();
    bridge::create_pda(
        &a.receipt,
        &a.owner,
        &a.system_program,
        &[b"funding", project.as_ref(), &funding_id],
        8 + 160,
        crate::ID,
    )?;
    token::transfer_checked(
        CpiContext::new(
            a.token_program.key(),
            TransferChecked {
                from: a.owner_token.to_account_info(),
                mint: a.mint.to_account_info(),
                to: a.vault.to_account_info(),
                authority: a.owner.to_account_info(),
            },
        ),
        amount,
        a.mint.decimals,
    )?;
    let words = [
        a.project.domain,
        funding_id,
        invoice_id,
        bridge::word(amount),
        a.owner.key().to_bytes(),
    ];
    let mut data = a.receipt.try_borrow_mut_data()?;
    data[..8].copy_from_slice(FundingReceipt::DISCRIMINATOR);
    for (i, word) in words.iter().enumerate() {
        data[8 + i * 32..8 + (i + 1) * 32].copy_from_slice(word);
    }
    a.project.free = free;
    event(
        project,
        Pubkey::default(),
        Pubkey::default(),
        Action::Fund,
        amount,
    );
    Ok(())
}
