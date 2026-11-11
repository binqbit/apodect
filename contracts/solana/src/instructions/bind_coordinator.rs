use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::state::{Project, Workspace};

#[derive(Accounts)]
pub struct BindCoordinator<'info> {
    #[account(address = project.workspace, has_one = owner)]
    pub workspace: Account<'info, Workspace>,
    #[account(mut)]
    pub project: Account<'info, Project>,
    pub owner: Signer<'info>,
}

pub fn handler(
    ctx: Context<BindCoordinator>,
    domain: [u8; 32],
    command_key: [u8; 64],
) -> Result<()> {
    let project = &mut ctx.accounts.project;
    project.standalone()?;
    require!(
        domain != [0; 32]
            && command_key != [0; 64]
            && project.task_count == 0
            && project.invoice_count == 0
            && project.approval_count == 0
            && project.head.is_none()
            && project.tail.is_none()
            && project.held == 0,
        ContractError::InvalidBinding
    );
    project.domain = domain;
    project.command_key = command_key;
    Ok(())
}
