use anchor_lang::prelude::*;

#[event]
pub struct Transition {
    pub project: Pubkey,
    pub task: Pubkey,
    pub request: Pubkey,
    pub action: Action,
    pub amount: u64,
    pub stop_reason: Option<StopReason>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy)]
pub enum StopReason {
    SupplementRejected,
    AuthorityOutOfBudget,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy)]
pub enum Action {
    Fund,
    WithdrawFree,
    Request,
    Approve,
    Reject,
    Allocate,
    AdvanceRemoved,
    Withdraw,
    Return,
    Release,
    Cancel,
    OutOfBudget,
    Pause,
    Reclaim,
    Restore,
    Continue,
    Finalize,
    Authority,
    Invoice,
    InvoiceResponse,
}

pub fn event(project: Pubkey, task: Pubkey, request: Pubkey, action: Action, amount: u64) {
    let reason = if matches!(action, Action::OutOfBudget) {
        Some(StopReason::AuthorityOutOfBudget)
    } else {
        None
    };
    event_with_stop(project, task, request, action, amount, reason);
}

pub fn event_with_stop(
    project: Pubkey,
    task: Pubkey,
    request: Pubkey,
    action: Action,
    amount: u64,
    stop_reason: Option<StopReason>,
) {
    emit!(Transition {
        project,
        task,
        request,
        action,
        amount,
        stop_reason
    });
}

#[event]
pub struct BridgeTransition {
    pub project: Pubkey,
    pub operation: [u8; 32],
    pub action: u8,
    pub result: [u8; 32],
}

#[event]
pub struct BridgeCollection {
    pub project: Pubkey,
    pub route: [u8; 32],
    pub kind: u8,
    pub amount: u64,
    pub received: u64,
}
