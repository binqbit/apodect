use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::state::{Invoice, Project, Workspace};

#[derive(Accounts)]
pub struct RespondInvoice<'info> {
    #[account(
        address = project.workspace,
        has_one = owner
    )]
    pub workspace: Account<'info, Workspace>,
    pub project: Account<'info, Project>,
    #[account(
        mut,
        has_one = project
    )]
    pub invoice: Account<'info, Invoice>,
    pub owner: Signer<'info>,
}

pub fn handler(ctx: Context<RespondInvoice>, contribution: u64, response: [u8; 32]) -> Result<()> {
    ctx.accounts.project.standalone()?;
    let invoice = &mut ctx.accounts.invoice;
    require!(!invoice.closed, ContractError::InvoiceClosed);
    invoice.closed = true;
    invoice.intended_contribution = contribution;
    invoice.response = response;

    event(
        ctx.accounts.project.key(),
        Pubkey::default(),
        invoice.key(),
        Action::InvoiceResponse,
        contribution,
    );
    Ok(())
}
