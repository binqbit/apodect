//! Contract records and JSON shapes. Serialization attributes distinguish persisted state from DTOs.

use near_sdk::json_types::U128;
use near_sdk::{AccountId, BorshStorageKey, near};

pub type Reference = [u8; 32];

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
pub(super) enum Key {
    Tasks,
    Requests,
    Queue,
    Invoices,
    Operations,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Active,
    Stopped,
    OutOfBudget,
    Completed,
    Canceled,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    Pending,
    Queued,
    Funded,
    Rejected,
    Removed,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Initial,
    Supplement,
    Resume,
    Restoration,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationState {
    Pending,
    Succeeded,
    Failed,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub(super) struct Task {
    pub(super) agent: AccountId,
    pub(super) state: TaskState,
    pub(super) paused: bool,
    pub(super) started: bool,
    pub(super) issued: u128,
    pub(super) recovered: u128,
    pub(super) available: u128,
    pub(super) pending_outflow: u128,
    pub(super) live_request: Option<u64>,
    pub(super) report: Reference,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub(super) struct Request {
    pub(super) task_id: u64,
    pub(super) amount: u128,
    pub(super) purpose: Reference,
    pub(super) state: RequestState,
    pub(super) kind: RequestKind,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub(super) struct Invoice {
    pub(super) requested: u128,
    pub(super) reference: Reference,
    pub(super) closed: bool,
    pub(super) intended: u128,
    pub(super) response_reference: Reference,
    pub(super) actual_funding: u128,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub(super) struct Operation {
    pub(super) task_id: Option<u64>,
    pub(super) amount: u128,
    pub(super) state: OperationState,
}

#[near(serializers = [json])]
pub struct ProjectView {
    pub owner_id: AccountId,
    pub authority_id: AccountId,
    pub token_id: AccountId,
    pub free: U128,
    pub pending_owner_outflow: U128,
    pub queue_head: u64,
    pub queue_tail: u64,
    pub next_task: u64,
    pub next_request: u64,
    pub next_invoice: u64,
    pub next_operation: u64,
}

#[near(serializers = [json])]
pub struct TaskView {
    pub agent: AccountId,
    pub state: TaskState,
    pub paused: bool,
    pub started: bool,
    pub issued: U128,
    pub recovered: U128,
    pub available: U128,
    pub pending_outflow: U128,
    pub external_outstanding: U128,
    pub expense: Option<U128>,
    pub live_request: Option<u64>,
    pub report: Option<Reference>,
}

#[near(serializers = [json])]
pub struct RequestView {
    pub task_id: u64,
    pub amount: U128,
    pub purpose: Reference,
    pub state: RequestState,
    pub kind: RequestKind,
}

#[near(serializers = [json])]
pub struct InvoiceView {
    pub requested: U128,
    pub reference: Option<Reference>,
    pub closed: bool,
    pub intended: U128,
    pub response_reference: Option<Reference>,
    pub actual_funding: U128,
}

#[near(serializers = [json])]
pub struct OperationView {
    pub task_id: Option<u64>,
    pub amount: U128,
    pub state: OperationState,
}

#[near(serializers = [json])]
pub enum ApplyResult {
    Empty,
    Skipped {
        request_id: u64,
    },
    Waiting {
        request_id: u64,
        required: U128,
        available: U128,
    },
    Funded {
        request_id: u64,
        task_id: u64,
        amount: U128,
    },
}

#[near(serializers = [json])]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DepositMessage {
    Fund { invoice_id: Option<u64> },
    Return { task_id: u64 },
}
