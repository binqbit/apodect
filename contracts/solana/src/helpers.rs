use anchor_lang::prelude::*;

use crate::errors::ContractError;
use crate::events::{event, Action};
use crate::state::{BudgetRequest, Project, RequestKind, RequestState, Task};

pub(crate) fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| error!(ContractError::Arithmetic))
}

pub(crate) fn sub(a: u64, b: u64) -> Result<u64> {
    a.checked_sub(b)
        .ok_or_else(|| error!(ContractError::Arithmetic))
}

pub(crate) fn positive(amount: u64) -> Result<()> {
    require!(amount > 0, ContractError::ZeroAmount);
    Ok(())
}

pub(crate) fn initialize_request(
    request: &mut Account<BudgetRequest>,
    task: &mut Account<Task>,
    project: Pubkey,
    amount: u64,
    reference: [u8; 32],
    kind: RequestKind,
) -> Result<()> {
    positive(amount)?;
    require!(reference != [0; 32], ContractError::EmptyPurpose);
    require!(task.pending.is_none(), ContractError::LiveRequest);
    request.set_inner(BudgetRequest {
        project,
        task: task.key(),
        amount,
        purpose: reference,
        kind,
        state: RequestState::Pending,
        approval_sequence: 0,
        next: None,
    });
    task.pending = Some(request.key());
    task.request_count = add(task.request_count, 1)?;

    event(project, task.key(), request.key(), Action::Request, amount);
    Ok(())
}

pub(crate) fn retire_request(
    project: Pubkey,
    task: &mut Account<Task>,
    pending: &mut Option<Account<BudgetRequest>>,
) -> Result<()> {
    require!(
        pending.as_ref().map(|r| r.key()) == task.pending,
        ContractError::WrongLiveRequest
    );
    if let Some(request) = pending.as_mut() {
        require!(
            request.project == project && request.task == task.key(),
            ContractError::WrongLiveRequest
        );
        require!(
            matches!(request.state, RequestState::Pending | RequestState::Queued),
            ContractError::InvalidRequestState
        );
        request.state = RequestState::Removed;
    }
    task.pending = None;
    Ok(())
}

pub(crate) fn append_queue(
    project: &mut Account<Project>,
    request: &mut Account<BudgetRequest>,
    tail: &mut Option<Account<BudgetRequest>>,
) -> Result<()> {
    require!(
        tail.as_ref().map(|t| t.key()) == project.tail,
        ContractError::WrongQueueAccount
    );
    if let Some(previous) = tail.as_mut() {
        require!(
            previous.project == project.key()
                && previous.next.is_none()
                && matches!(previous.state, RequestState::Queued | RequestState::Removed),
            ContractError::WrongQueueAccount
        );
        require_keys_neq!(
            previous.key(),
            request.key(),
            ContractError::WrongQueueAccount
        );
        previous.next = Some(request.key());
    } else {
        require!(project.head.is_none(), ContractError::WrongQueueAccount);
        project.head = Some(request.key());
    }
    project.approval_count = add(project.approval_count, 1)?;
    project.tail = Some(request.key());
    request.approval_sequence = project.approval_count;
    request.state = RequestState::Queued;
    Ok(())
}

pub(crate) fn release(
    project: &mut Account<Project>,
    task: &mut Account<Task>,
    amount: u64,
    action: Action,
) -> Result<()> {
    positive(amount)?;
    task.nonterminal()?;
    require!(
        amount <= task.available,
        ContractError::InsufficientAllocation
    );
    task.available = sub(task.available, amount)?;
    task.recovered = add(task.recovered, amount)?;
    project.free = add(project.free, amount)?;

    event(project.key(), task.key(), Pubkey::default(), action, amount);
    Ok(())
}
