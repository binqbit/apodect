//! A physically isolated, locally funded Project. No cross-chain authority is implied.

use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{AccountId, PanicOnDefault, env, near, require};

mod budget;
mod helpers;
mod state;
mod tasks;
mod treasury;

pub use state::{
    ApplyResult, DepositMessage, InvoiceView, OperationState, OperationView, ProjectView,
    Reference, RequestKind, RequestState, RequestView, TaskState, TaskView,
};
use state::{Invoice, Key, Operation, Request, Task};

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Project {
    owner_id: AccountId,
    authority_id: AccountId,
    token_id: AccountId,

    free: u128,
    pending_owner_outflow: u128,

    next_task: u64,
    next_request: u64,
    next_invoice: u64,
    next_operation: u64,
    queue_head: u64,
    queue_tail: u64,

    tasks: LookupMap<u64, Task>,
    requests: LookupMap<u64, Request>,
    queue: LookupMap<u64, u64>,
    invoices: LookupMap<u64, Invoice>,
    operations: LookupMap<u64, Operation>,
}

#[near]
impl Project {
    #[init]
    pub fn new(
        workspace_id: AccountId,
        project_id: u64,
        owner_id: AccountId,
        authority_id: AccountId,
        token_id: AccountId,
    ) -> Self {
        require!(
            env::predecessor_account_id() == workspace_id,
            "ONLY_WORKSPACE_INITIALIZER"
        );
        require!(
            env::current_account_id().as_str() == format!("p{project_id}.{workspace_id}"),
            "INVALID_PROJECT_ACCOUNT"
        );
        require!(
            owner_id != env::current_account_id() && authority_id != env::current_account_id(),
            "INVALID_ROLE_ACCOUNT"
        );
        Self {
            owner_id,
            authority_id,
            token_id,
            free: 0,
            pending_owner_outflow: 0,
            next_task: 0,
            next_request: 0,
            next_invoice: 0,
            next_operation: 0,
            queue_head: 0,
            queue_tail: 0,
            tasks: LookupMap::new(Key::Tasks),
            requests: LookupMap::new(Key::Requests),
            queue: LookupMap::new(Key::Queue),
            invoices: LookupMap::new(Key::Invoices),
            operations: LookupMap::new(Key::Operations),
        }
    }

    #[payable]
    pub fn set_authority(&mut self, authority_id: AccountId) {
        self.assert_owner();
        require!(
            authority_id != env::current_account_id(),
            "INVALID_ROLE_ACCOUNT"
        );

        let before = env::storage_usage();
        self.authority_id = authority_id;

        self.charge_storage(before, 0);

        Self::emit(
            "authority_changed",
            near_sdk::serde_json::json!({"authority_id": self.authority_id}),
        );
    }

    pub fn get_project(&self) -> ProjectView {
        ProjectView {
            owner_id: self.owner_id.clone(),
            authority_id: self.authority_id.clone(),
            token_id: self.token_id.clone(),
            free: U128(self.free),
            pending_owner_outflow: U128(self.pending_owner_outflow),
            queue_head: self.queue_head,
            queue_tail: self.queue_tail,
            next_task: self.next_task,
            next_request: self.next_request,
            next_invoice: self.next_invoice,
            next_operation: self.next_operation,
        }
    }

    pub fn get_task(&self, task_id: u64) -> Option<TaskView> {
        self.tasks.get(&task_id).map(|task| TaskView {
            agent: task.agent.clone(),
            state: task.state,
            paused: task.paused,
            started: task.started,
            issued: U128(task.issued),
            recovered: U128(task.recovered),
            available: U128(task.available),
            pending_outflow: U128(task.pending_outflow),
            external_outstanding: U128(Self::external(task)),
            expense: (task.state == TaskState::Completed)
                .then(|| U128(Self::sub(task.issued, task.recovered))),
            live_request: task.live_request,
            report: Self::optional_reference(task.report),
        })
    }

    pub fn get_request(&self, request_id: u64) -> Option<RequestView> {
        self.requests.get(&request_id).map(|request| RequestView {
            task_id: request.task_id,
            amount: U128(request.amount),
            purpose: request.purpose,
            state: request.state,
            kind: request.kind,
        })
    }

    pub fn get_invoice(&self, invoice_id: u64) -> Option<InvoiceView> {
        self.invoices.get(&invoice_id).map(|invoice| InvoiceView {
            requested: U128(invoice.requested),
            reference: Self::optional_reference(invoice.reference),
            closed: invoice.closed,
            intended: U128(invoice.intended),
            response_reference: Self::optional_reference(invoice.response_reference),
            actual_funding: U128(invoice.actual_funding),
        })
    }

    pub fn get_operation(&self, operation_id: u64) -> Option<OperationView> {
        self.operations.get(&operation_id).map(|op| OperationView {
            task_id: op.task_id,
            amount: U128(op.amount),
            state: op.state,
        })
    }
}

#[cfg(test)]
mod tests;
