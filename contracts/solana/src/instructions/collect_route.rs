use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::bridge::{BridgePlan, BridgeRoute, DELIVERY, OPEN, REFUND};
use crate::errors::ContractError;
use crate::helpers::{add, positive, sub};
use crate::state::Project;

#[derive(Accounts)]
pub struct CollectRoute<'info> {
    #[account(mut, has_one = mint)]
    pub project: Account<'info, Project>,
    pub mint: Account<'info, Mint>,
    #[account(mut, seeds = [b"vault", project.key().as_ref()], bump,
        token::mint = mint, token::authority = project)]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut, has_one = project,
        seeds = [b"bridge-route", project.key().as_ref(), &route.id], bump)]
    pub route: Account<'info, BridgeRoute>,
    #[account(mut, has_one = project, constraint = plan.id == route.plan @ ContractError::InvalidRoute,
        seeds = [b"bridge-plan", project.key().as_ref(), &route.plan], bump)]
    pub plan: Account<'info, BridgePlan>,
    #[account(mut, address = crate::bridge::associated_account(route.key(), mint.key()),
        token::mint = mint, token::authority = route)]
    pub escrow: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<CollectRoute>) -> Result<()> {
    let a = ctx.accounts;
    require!(a.project.domain != [0; 32], ContractError::InvalidBinding);
    let amount = match a.route.kind {
        DELIVERY => {
            require!(
                a.route.received == 0 && a.escrow.amount >= a.route.expected,
                ContractError::InvalidRoute
            );
            a.route.expected
        }
        REFUND => {
            require!(a.route.dispatched > 0, ContractError::InvalidRoute);
            a.escrow
                .amount
                .min(sub(a.route.dispatched, a.route.received)?)
        }
        _ => return err!(ContractError::InvalidRoute),
    };
    positive(amount)?;
    let project = a.project.key();
    let bump = [ctx.bumps.route];
    let seeds: &[&[u8]] = &[b"bridge-route", project.as_ref(), &a.route.id, &bump];
    token::transfer_checked(
        CpiContext::new_with_signer(
            a.token_program.key(),
            TransferChecked {
                from: a.escrow.to_account_info(),
                mint: a.mint.to_account_info(),
                to: a.vault.to_account_info(),
                authority: a.route.to_account_info(),
            },
            &[seeds],
        ),
        amount,
        a.mint.decimals,
    )?;
    if a.route.kind == DELIVERY && a.plan.state == OPEN {
        a.plan.held = add(a.plan.held, amount)?;
        a.project.held = add(a.project.held, amount)?;
    } else {
        a.project.free = add(a.project.free, amount)?;
    }
    a.route.received = add(a.route.received, amount)?;
    emit!(crate::events::BridgeCollection {
        project: a.project.key(),
        route: a.route.id,
        kind: a.route.kind,
        amount,
        received: a.route.received
    });
    Ok(())
}
