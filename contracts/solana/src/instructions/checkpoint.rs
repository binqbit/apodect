use anchor_lang::prelude::*;
use anchor_lang::Discriminator;

use crate::bridge::{self, BridgePlan, BridgeRoute, Checkpoint, CommandReceipt};
use crate::errors::ContractError;
use crate::helpers::add;
use crate::state::{BudgetRequest, Project, Task};

#[derive(Accounts)]
pub struct RecordCheckpoint<'info> {
    #[account(mut)]
    pub project: Account<'info, Project>,
    /// CHECK: Unique observation PDA, created exactly once and never modified.
    #[account(mut)]
    pub checkpoint: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: Optional exact selected native identity, typed owner/project checks below.
    pub task: Option<UncheckedAccount<'info>>,
    /// CHECK: Optional exact selected native identity, typed owner/project checks below.
    pub request: Option<UncheckedAccount<'info>>,
    /// CHECK: Optional canonical selected Plan PDA, typed owner/project checks below.
    pub plan: Option<UncheckedAccount<'info>>,
    /// CHECK: Optional canonical selected Route PDA, typed owner/project checks below.
    pub route: Option<UncheckedAccount<'info>>,
    /// CHECK: Optional canonical selected Command PDA, typed owner/project checks below.
    pub command: Option<UncheckedAccount<'info>>,
}

pub fn handler(
    ctx: Context<RecordCheckpoint>,
    observation: [u8; 32],
    task: [u8; 32],
    request: [u8; 32],
    plan: [u8; 32],
    route: [u8; 32],
    operation: [u8; 32],
) -> Result<()> {
    let a = ctx.accounts;
    require!(
        a.project.domain != [0; 32] && observation != [0; 32],
        ContractError::InvalidBinding
    );
    let project = a.project.key();
    let selected_task = selected::<Task>(&a.task, task, Pubkey::new_from_array(task))?;
    let selected_request =
        selected::<BudgetRequest>(&a.request, request, Pubkey::new_from_array(request))?;
    let selected_plan = selected::<BridgePlan>(
        &a.plan,
        plan,
        bridge::pda(&[b"bridge-plan", project.as_ref(), &plan]),
    )?;
    let selected_route = selected::<BridgeRoute>(
        &a.route,
        route,
        bridge::pda(&[b"bridge-route", project.as_ref(), &route]),
    )?;
    let selected_command = selected::<CommandReceipt>(
        &a.command,
        operation,
        bridge::pda(&[b"bridge-command", project.as_ref(), &operation]),
    )?;
    let clock = Clock::get()?;
    require!(clock.unix_timestamp >= 0, ContractError::InvalidCommand);
    a.project.checkpoint_sequence = add(a.project.checkpoint_sequence, 1)?;
    let mut words = vec![[0u8; 32]; 39];
    words[0] = a.project.domain;
    words[1] = observation;
    words[2] = bridge::word(a.project.checkpoint_sequence);
    words[3] = bridge::word(clock.slot);
    words[4] = bridge::word(clock.unix_timestamp as u64);
    words[5] = bridge::word(a.project.free);
    words[6] = bridge::word(a.project.held);
    words[7] = task;
    words[8] = bridge::word(255);
    if let Some(t) = selected_task {
        require_keys_eq!(t.project, project, ContractError::WrongBridgeAccount);
        words[8] = bridge::word(t.lifecycle as u64);
        words[9] = bridge::word(t.paused as u64);
        words[10] = bridge::word(t.started as u64);
        words[11] = bridge::word(t.available);
        words[12] = bridge::word(t.issued);
        words[13] = bridge::word(t.recovered);
        words[15] = bridge::word(t.control_nonce);
        words[16] = t.agent.to_bytes();
        words[37] = t.pending.map(|key| key.to_bytes()).unwrap_or([0; 32]);
        words[38] = bridge::word(t.pending.is_some() as u64);
    }
    words[17] = request;
    words[21] = bridge::word(255);
    if let Some(r) = selected_request {
        require_keys_eq!(r.project, project, ContractError::WrongBridgeAccount);
        words[18] = r.task.to_bytes();
        words[19] = bridge::word(r.amount);
        words[20] = bridge::word(r.kind as u64);
        words[21] = bridge::word(r.state as u64);
        words[22] = r.purpose;
    }
    words[23] = plan;
    if let Some(p) = selected_plan {
        require!(
            p.project == project && p.id == plan,
            ContractError::WrongBridgeAccount
        );
        words[24] = bridge::word(p.state as u64);
        words[25] = bridge::word(p.held);
    }
    words[26] = route;
    if let Some(r) = selected_route {
        require!(
            r.project == project && r.id == route,
            ContractError::WrongBridgeAccount
        );
        words[27] = r.plan;
        words[28] = bridge::word(r.kind as u64);
        words[29] = bridge::word(r.expected);
        words[30] = bridge::word(r.received);
        words[31] = bridge::word(r.dispatched);
    }
    words[32] = operation;
    if let Some(c) = selected_command {
        require!(
            c.project == project && c.operation == operation,
            ContractError::WrongBridgeAccount
        );
        words[33] = c.digest;
        words[34] = bridge::word(c.action as u64);
        words[35] = bridge::word(c.state as u64);
        words[36] = c.result;
    }
    bridge::create_pda(
        &a.checkpoint,
        &a.payer,
        &a.system_program,
        &[b"checkpoint", project.as_ref(), &observation],
        8 + bridge::CHECKPOINT_BYTES,
        crate::ID,
    )?;
    let mut data = a.checkpoint.try_borrow_mut_data()?;
    data[..8].copy_from_slice(Checkpoint::DISCRIMINATOR);
    for (i, word) in words.iter().enumerate() {
        data[8 + i * 32..8 + (i + 1) * 32].copy_from_slice(word);
    }
    Ok(())
}

fn selected<T: AccountDeserialize>(
    account: &Option<UncheckedAccount>,
    id: [u8; 32],
    expected: Pubkey,
) -> Result<Option<T>> {
    if id == [0; 32] {
        require!(account.is_none(), ContractError::WrongBridgeAccount);
        return Ok(None);
    }
    let account = account.as_ref().ok_or(ContractError::WrongBridgeAccount)?;
    require_keys_eq!(account.key(), expected, ContractError::WrongBridgeAccount);
    bridge::load(account)
}
