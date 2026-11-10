use anchor_lang::prelude::*;

use crate::events::{event, Action};
use crate::helpers::{add, positive};
use crate::state::{Invoice, Project};

#[derive(Accounts)]
pub struct CreateInvoice<'info> {
    #[account(
        mut,
        has_one = authority
    )]
    pub project: Account<'info, Project>,
    #[account(
        init,
        payer = authority,
        space = 8 + Invoice::INIT_SPACE,
        seeds = [b"invoice", project.key().as_ref(), &project.invoice_count.to_le_bytes()],
        bump
    )]
    pub invoice: Account<'info, Invoice>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<CreateInvoice>, amount: u64, reference: [u8; 32]) -> Result<()> {
    positive(amount)?;
    ctx.accounts.invoice.set_inner(Invoice {
        project: ctx.accounts.project.key(),
        amount,
        purpose: reference,
        closed: false,
        intended_contribution: 0,
        response: [0; 32],
        received: 0,
    });
    ctx.accounts.project.invoice_count = add(ctx.accounts.project.invoice_count, 1)?;

    event(
        ctx.accounts.project.key(),
        Pubkey::default(),
        ctx.accounts.invoice.key(),
        Action::Invoice,
        amount,
    );
    Ok(())
}
