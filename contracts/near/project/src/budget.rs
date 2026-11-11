//! Budget requests, approval FIFO, full allocation and replenishment invoices.

use crate::state::{Invoice, Request, RequestKind, RequestState, Task, TaskState};
use crate::{ApplyResult, Project, ProjectExt, Reference};
use near_sdk::json_types::U128;
use near_sdk::{env, near, require};

#[near]
impl Project {
    #[payable]
    pub fn request_budget(&mut self, task_id: u64, amount: U128, purpose: Reference) -> u64 {
        Self::assert_positive(amount.0);
        Self::assert_purpose(&purpose);
        let mut task = self.task(task_id);
        Self::assert_agent(&task);
        require!(task.live_request.is_none(), "LIVE_REQUEST_EXISTS");
        let kind = match task.state {
            TaskState::Stopped => RequestKind::Resume,
            TaskState::Active if task.issued == 0 => RequestKind::Initial,
            TaskState::Active if task.started => RequestKind::Supplement,
            _ => env::panic_str("NEW_REQUEST_NOT_ALLOWED"),
        };

        let before = env::storage_usage();
        let request_id =
            self.insert_request(task_id, amount.0, purpose, kind, RequestState::Pending);
        task.live_request = Some(request_id);
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);
        request_id
    }

    #[payable]
    pub fn approve_request(&mut self, request_id: u64) {
        self.assert_authority();
        let mut request = self.request(request_id);
        let task = self.task(request.task_id);
        require!(
            request.state == RequestState::Pending && task.live_request == Some(request_id),
            "REQUEST_NOT_PENDING"
        );
        require!(
            task.state == TaskState::Active || task.state == TaskState::Stopped,
            "TASK_NOT_FUNDABLE"
        );

        let before = env::storage_usage();
        request.state = RequestState::Queued;
        self.requests.insert(request_id, request);
        self.enqueue(request_id);

        self.charge_storage(before, 0);
    }

    #[payable]
    pub fn reject_request(&mut self, request_id: u64) {
        self.assert_authority();
        let mut request = self.request(request_id);
        let mut task = self.task(request.task_id);
        require!(
            request.state == RequestState::Pending && task.live_request == Some(request_id),
            "REQUEST_NOT_PENDING"
        );

        let before = env::storage_usage();
        request.state = RequestState::Rejected;
        task.live_request = None;
        if request.kind == RequestKind::Supplement {
            task.state = TaskState::Stopped;

            Self::emit(
                "task_stopped",
                near_sdk::serde_json::json!({
                    "task_id": request.task_id,
                    "reason": "supplement_rejected"
                }),
            );
        }

        Self::emit(
            "request_rejected",
            near_sdk::serde_json::json!({
                "request_id": request_id,
                "task_id": request.task_id
            }),
        );
        self.tasks.insert(request.task_id, task);
        self.requests.insert(request_id, request);

        self.charge_storage(before, 0);
    }

    /// Constant work: allocate one live head or skip exactly one removed head.
    #[payable]
    pub fn apply(&mut self) -> ApplyResult {
        self.assert_authority();

        let before = env::storage_usage();
        let result = if self.queue_head == self.queue_tail {
            ApplyResult::Empty
        } else {
            let request_id = *self.queue.get(&self.queue_head).expect("QUEUE_CORRUPTED");
            let mut request = self.request(request_id);
            if request.state == RequestState::Removed {
                self.advance_queue();
                ApplyResult::Skipped { request_id }
            } else {
                require!(request.state == RequestState::Queued, "QUEUE_CORRUPTED");
                let mut task = self.task(request.task_id);
                require!(task.live_request == Some(request_id), "QUEUE_CORRUPTED");
                require!(
                    task.state == TaskState::Active || task.state == TaskState::Stopped,
                    "TASK_NOT_FUNDABLE"
                );
                if self.free < request.amount {
                    ApplyResult::Waiting {
                        request_id,
                        required: U128(request.amount),
                        available: U128(self.free),
                    }
                } else {
                    self.free = Self::sub(self.free, request.amount);
                    task.available = Self::add(task.available, request.amount);
                    task.issued = Self::add(task.issued, request.amount);
                    task.live_request = None;
                    if request.kind == RequestKind::Resume {
                        task.state = TaskState::Active;
                    }
                    request.state = RequestState::Funded;
                    self.tasks.insert(request.task_id, task);
                    self.requests.insert(request_id, request.clone());
                    self.advance_queue();
                    ApplyResult::Funded {
                        request_id,
                        task_id: request.task_id,
                        amount: U128(request.amount),
                    }
                }
            }
        };

        self.charge_storage(before, 0);

        Self::emit(
            "apply_result",
            near_sdk::serde_json::to_value(&result).expect("SERIALIZATION_FAILED"),
        );
        result
    }

    #[payable]
    pub fn create_invoice(&mut self, amount: U128, reference: Option<Reference>) -> u64 {
        self.assert_authority();
        Self::assert_positive(amount.0);

        let before = env::storage_usage();
        let invoice_id = self.next_invoice;
        self.next_invoice = invoice_id.checked_add(1).expect("INVOICE_ID_OVERFLOW");
        self.invoices.insert(
            invoice_id,
            Invoice {
                requested: amount.0,
                reference: reference.unwrap_or([0; 32]),
                closed: false,
                intended: 0,
                response_reference: [0; 32],
                actual_funding: 0,
            },
        );

        self.charge_storage(before, 0);

        Self::emit(
            "invoice_created",
            near_sdk::serde_json::json!({
                "invoice_id": invoice_id,
                "amount": amount,
                "reference": reference
            }),
        );
        invoice_id
    }

    #[payable]
    pub fn respond_invoice(
        &mut self,
        invoice_id: u64,
        intended: U128,
        reference: Option<Reference>,
    ) {
        self.assert_owner();
        self.assert_standalone();
        let mut invoice = self
            .invoices
            .get(&invoice_id)
            .cloned()
            .expect("INVOICE_NOT_FOUND");
        require!(!invoice.closed, "INVOICE_ALREADY_CLOSED");

        let before = env::storage_usage();
        invoice.closed = true;
        invoice.intended = intended.0;
        invoice.response_reference = reference.unwrap_or([0; 32]);
        self.invoices.insert(invoice_id, invoice);

        self.charge_storage(before, 0);

        Self::emit(
            "invoice_responded",
            near_sdk::serde_json::json!({
                "invoice_id": invoice_id,
                "intended": intended,
                "reference": reference
            }),
        );
    }
}

impl Project {
    pub(crate) fn assert_purpose(purpose: &Reference) {
        require!(*purpose != [0; 32], "EMPTY_PURPOSE");
    }

    pub(crate) fn request(&self, request_id: u64) -> Request {
        self.requests
            .get(&request_id)
            .cloned()
            .expect("REQUEST_NOT_FOUND")
    }

    pub(crate) fn insert_request(
        &mut self,
        task_id: u64,
        amount: u128,
        purpose: Reference,
        kind: RequestKind,
        state: RequestState,
    ) -> u64 {
        require!(
            self.bridge.binding.is_none() || amount <= u128::from(u64::MAX),
            "AMOUNT_EXCEEDS_U64"
        );
        let id = self.next_request;
        self.next_request = id.checked_add(1).expect("REQUEST_ID_OVERFLOW");
        self.requests.insert(
            id,
            Request {
                task_id,
                amount,
                purpose,
                kind,
                state,
            },
        );

        Self::emit(
            "request_created",
            near_sdk::serde_json::json!({
                "request_id": id,
                "task_id": task_id,
                "amount": U128(amount),
                "purpose": purpose,
                "kind": kind
            }),
        );
        id
    }

    pub(crate) fn enqueue(&mut self, request_id: u64) {
        let tail = self.queue_tail;
        self.queue_tail = tail.checked_add(1).expect("QUEUE_ID_OVERFLOW");
        self.queue.insert(tail, request_id);

        Self::emit(
            "request_approved",
            near_sdk::serde_json::json!({
                "request_id": request_id,
                "approval_sequence": tail
            }),
        );
    }

    pub(crate) fn advance_queue(&mut self) {
        self.queue.remove(&self.queue_head);
        self.queue_head = self.queue_head.checked_add(1).expect("QUEUE_ID_OVERFLOW");
    }

    pub(crate) fn remove_live_request(&mut self, task: &mut Task) {
        if let Some(id) = task.live_request.take() {
            let mut request = self.request(id);
            require!(
                request.state == RequestState::Pending || request.state == RequestState::Queued,
                "REQUEST_STATE_CORRUPTED"
            );
            request.state = RequestState::Removed;
            self.requests.insert(id, request);

            Self::emit(
                "request_removed",
                near_sdk::serde_json::json!({"request_id": id}),
            );
        }
    }
}
