use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
mod helpers;
pub mod instructions;
pub mod state;

pub use instructions::*;

declare_id!("9HLdesoinPvKWr56HRfpbGL9M8yKWzzBFNAr4rbZLkQP");

#[program]
pub mod apodect_solana {
    use super::*;

    pub fn initialize_workspace(ctx: Context<InitializeWorkspace>, nonce: u64) -> Result<()> {
        instructions::initialize_workspace::handler(ctx, nonce)
    }

    pub fn create_project(ctx: Context<CreateProject>, authority: Pubkey) -> Result<()> {
        instructions::create_project::handler(ctx, authority)
    }

    pub fn set_authority(ctx: Context<SetAuthority>, authority: Pubkey) -> Result<()> {
        instructions::set_authority::handler(ctx, authority)
    }

    pub fn fund_project(ctx: Context<FundProject>, amount: u64) -> Result<()> {
        instructions::fund_project::handler(ctx, amount)
    }

    pub fn withdraw_free(ctx: Context<WithdrawFree>, amount: u64) -> Result<()> {
        instructions::withdraw_free::handler(ctx, amount)
    }

    pub fn create_task(ctx: Context<CreateTask>, amount: u64, purpose: [u8; 32]) -> Result<()> {
        instructions::create_task::handler(ctx, amount, purpose)
    }

    pub fn submit_request(
        ctx: Context<SubmitRequest>,
        amount: u64,
        purpose: [u8; 32],
    ) -> Result<()> {
        instructions::submit_request::handler(ctx, amount, purpose)
    }

    pub fn approve_request(ctx: Context<ApproveRequest>) -> Result<()> {
        instructions::approve_request::handler(ctx)
    }

    pub fn reject_request(ctx: Context<RejectRequest>) -> Result<()> {
        instructions::reject_request::handler(ctx)
    }

    pub fn apply_next(ctx: Context<ApplyNext>) -> Result<()> {
        instructions::apply_next::handler(ctx)
    }

    pub fn withdraw(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
        instructions::withdraw::handler(ctx, amount)
    }

    pub fn return_funds(ctx: Context<ReturnFunds>, amount: u64) -> Result<()> {
        instructions::return_funds::handler(ctx, amount)
    }

    pub fn release_funds(ctx: Context<ReleaseFunds>, amount: u64) -> Result<()> {
        instructions::release_funds::handler(ctx, amount)
    }

    pub fn finalize_task(ctx: Context<FinalizeTask>, report: [u8; 32]) -> Result<()> {
        instructions::finalize_task::handler(ctx, report)
    }

    pub fn cancel_task(ctx: Context<CancelTask>) -> Result<()> {
        instructions::cancel_task::handler(ctx)
    }

    pub fn mark_out_of_budget(ctx: Context<MarkOutOfBudget>) -> Result<()> {
        instructions::mark_out_of_budget::handler(ctx)
    }

    pub fn pause_task(ctx: Context<PauseTask>) -> Result<()> {
        instructions::pause_task::handler(ctx)
    }

    pub fn reclaim(ctx: Context<Reclaim>, amount: u64) -> Result<()> {
        instructions::reclaim::handler(ctx, amount)
    }

    pub fn restore_budget(
        ctx: Context<RestoreBudget>,
        amount: u64,
        purpose: [u8; 32],
    ) -> Result<()> {
        instructions::restore_budget::handler(ctx, amount, purpose)
    }

    pub fn continue_task(ctx: Context<ContinueTask>) -> Result<()> {
        instructions::continue_task::handler(ctx)
    }

    pub fn create_invoice(
        ctx: Context<CreateInvoice>,
        amount: u64,
        purpose: [u8; 32],
    ) -> Result<()> {
        instructions::create_invoice::handler(ctx, amount, purpose)
    }

    pub fn respond_invoice(
        ctx: Context<RespondInvoice>,
        contribution: u64,
        response: [u8; 32],
    ) -> Result<()> {
        instructions::respond_invoice::handler(ctx, contribution, response)
    }
}
