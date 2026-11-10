//! Task creation, control, controlled recovery and terminal cleanup.

use crate::state::{RequestKind, RequestState, Task, TaskState};
use crate::{Project, ProjectExt, Reference};
use near_sdk::json_types::U128;
use near_sdk::{env, near, require};

#[near]
impl Project {
    #[payable]
    pub fn create_task(&mut self, amount: U128, purpose: Reference) -> u64 {
        Self::assert_positive(amount.0);
        Self::assert_purpose(&purpose);

        let before = env::storage_usage();
        let task_id = self.next_task;
        self.next_task = task_id.checked_add(1).expect("TASK_ID_OVERFLOW");
        let request_id = self.insert_request(
            task_id,
            amount.0,
            purpose,
            RequestKind::Initial,
            RequestState::Pending,
        );
        self.tasks.insert(
            task_id,
            Task {
                agent: env::predecessor_account_id(),
                state: TaskState::Active,
                paused: false,
                started: false,
                issued: 0,
                recovered: 0,
                available: 0,
                pending_outflow: 0,
                live_request: Some(request_id),
                report: [0; 32],
            },
        );

        self.charge_storage(before, 0);

        Self::emit(
            "task_created",
            near_sdk::serde_json::json!({
                "task_id": task_id,
                "request_id": request_id,
                "agent": env::predecessor_account_id()
            }),
        );
        task_id
    }

    #[payable]
    pub fn release(&mut self, task_id: u64, amount: U128) {
        let mut task = self.task(task_id);
        Self::assert_agent(&task);
        Self::assert_nonterminal(&task);
        Self::assert_positive(amount.0);
        require!(task.available >= amount.0, "INSUFFICIENT_ALLOCATION");

        let before = env::storage_usage();
        self.recover_controlled(&mut task, amount.0);
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "controlled_released",
            near_sdk::serde_json::json!({
                "task_id": task_id,
                "amount": amount
            }),
        );
    }

    #[payable]
    pub fn finalize(&mut self, task_id: u64, report: Option<Reference>) -> U128 {
        let mut task = self.task(task_id);
        Self::assert_agent(&task);
        Self::assert_nonterminal(&task);
        require!(task.issued > 0, "TASK_NEVER_FUNDED");
        require!(
            task.available == 0 && task.pending_outflow == 0,
            "TASK_FUNDS_NOT_SETTLED"
        );

        let before = env::storage_usage();
        self.remove_live_request(&mut task);
        task.state = TaskState::Completed;
        task.report = report.unwrap_or([0; 32]);
        let expense = Self::sub(task.issued, task.recovered);
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "task_completed",
            near_sdk::serde_json::json!({
                "task_id": task_id,
                "expense": U128(expense),
                "report": report
            }),
        );
        U128(expense)
    }

    #[payable]
    pub fn cancel(&mut self, task_id: u64) {
        self.assert_owner();
        let mut task = self.task(task_id);
        Self::assert_nonterminal(&task);
        require!(
            !task.started && task.pending_outflow == 0,
            "TASK_ALREADY_STARTED"
        );

        let before = env::storage_usage();
        self.remove_live_request(&mut task);
        let amount = task.available;
        self.recover_controlled(&mut task, amount);
        task.state = TaskState::Canceled;
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "task_canceled",
            near_sdk::serde_json::json!({
                "task_id": task_id,
                "recovered": U128(amount)
            }),
        );
    }

    #[payable]
    pub fn force_cancel(&mut self, task_id: u64) {
        self.assert_owner();
        let mut task = self.task(task_id);
        Self::assert_nonterminal(&task);
        require!(!task.paused, "TASK_ALREADY_PAUSED");

        let before = env::storage_usage();
        task.paused = true;
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "task_paused",
            near_sdk::serde_json::json!({"task_id": task_id}),
        );
    }

    #[payable]
    pub fn reclaim(&mut self, task_id: u64, amount: U128) {
        self.assert_owner();
        let mut task = self.task(task_id);
        Self::assert_nonterminal(&task);
        Self::assert_positive(amount.0);
        require!(task.paused, "TASK_NOT_PAUSED");
        require!(task.available >= amount.0, "INSUFFICIENT_ALLOCATION");

        let before = env::storage_usage();
        self.recover_controlled(&mut task, amount.0);
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "controlled_reclaimed",
            near_sdk::serde_json::json!({
                "task_id": task_id,
                "amount": amount
            }),
        );
    }

    #[payable]
    pub fn restore(&mut self, task_id: u64, amount: U128, purpose: Reference) -> u64 {
        self.assert_authority();
        Self::assert_positive(amount.0);
        Self::assert_purpose(&purpose);
        let mut task = self.task(task_id);
        require!(
            task.state == TaskState::Active && task.paused && task.issued > 0,
            "TASK_NOT_RESTORABLE"
        );
        require!(task.live_request.is_none(), "LIVE_REQUEST_EXISTS");

        let before = env::storage_usage();
        let request_id = self.insert_request(
            task_id,
            amount.0,
            purpose,
            RequestKind::Restoration,
            RequestState::Queued,
        );
        task.live_request = Some(request_id);
        self.tasks.insert(task_id, task);
        self.enqueue(request_id);

        self.charge_storage(before, 0);
        request_id
    }

    #[payable]
    pub fn continue_task(&mut self, task_id: u64) {
        self.assert_owner();
        let mut task = self.task(task_id);
        require!(
            task.state == TaskState::Active && task.paused,
            "TASK_NOT_CONTINUABLE"
        );
        require!(
            task.available > 0 && task.pending_outflow == 0,
            "CONFIRMED_RESERVE_REQUIRED"
        );

        let before = env::storage_usage();
        task.paused = false;
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "task_continued",
            near_sdk::serde_json::json!({"task_id": task_id}),
        );
    }

    #[payable]
    pub fn mark_out_of_budget(&mut self, task_id: u64) {
        self.assert_authority();
        let mut task = self.task(task_id);
        Self::assert_nonterminal(&task);
        require!(
            task.state != TaskState::OutOfBudget,
            "ALREADY_OUT_OF_BUDGET"
        );

        let before = env::storage_usage();
        self.remove_live_request(&mut task);
        task.state = TaskState::OutOfBudget;
        self.tasks.insert(task_id, task);

        self.charge_storage(before, 0);

        Self::emit(
            "task_stopped",
            near_sdk::serde_json::json!({
                "task_id": task_id,
                "reason": "authority_out_of_budget"
            }),
        );
    }
}

impl Project {
    pub(crate) fn task(&self, task_id: u64) -> Task {
        self.tasks.get(&task_id).cloned().expect("TASK_NOT_FOUND")
    }

    pub(crate) fn assert_nonterminal(task: &Task) {
        require!(
            task.state != TaskState::Completed && task.state != TaskState::Canceled,
            "TASK_TERMINAL"
        );
    }
}
