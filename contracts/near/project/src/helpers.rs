//! Shared role checks, checked accounting, storage charging and event envelopes.

use crate::state::Task;
use crate::{Project, Reference};
use near_sdk::{NearToken, Promise, env, require};

impl Project {
    pub(crate) fn assert_owner(&self) {
        require!(env::predecessor_account_id() == self.owner_id, "ONLY_OWNER");
    }

    pub(crate) fn assert_authority(&self) {
        require!(
            env::predecessor_account_id() == self.authority_id,
            "ONLY_AUTHORITY"
        );
    }

    pub(crate) fn assert_agent(task: &Task) {
        require!(
            env::predecessor_account_id() == task.agent,
            "ONLY_TASK_AGENT"
        );
    }

    pub(crate) fn assert_positive(amount: u128) {
        require!(amount > 0, "ZERO_AMOUNT");
    }

    pub(crate) fn add(a: u128, b: u128) -> u128 {
        a.checked_add(b).expect("AMOUNT_OVERFLOW")
    }

    pub(crate) fn sub(a: u128, b: u128) -> u128 {
        a.checked_sub(b).expect("ACCOUNTING_UNDERFLOW")
    }

    pub(crate) fn external(task: &Task) -> u128 {
        Self::sub(
            Self::sub(Self::sub(task.issued, task.recovered), task.available),
            task.pending_outflow,
        )
    }

    pub(crate) fn recover_controlled(&mut self, task: &mut Task, amount: u128) {
        task.available = Self::sub(task.available, amount);
        task.recovered = Self::add(task.recovered, amount);
        self.free = Self::add(self.free, amount);
    }

    pub(crate) fn optional_reference(reference: Reference) -> Option<Reference> {
        (reference != [0; 32]).then_some(reference)
    }

    /// Flush lazy collections and the root before measuring; automatic final serialization is identical.
    pub(crate) fn charge_storage(&mut self, before: u64, native_used: u128) {
        self.tasks.flush();
        self.requests.flush();
        self.queue.flush();
        self.invoices.flush();
        self.operations.flush();
        env::state_write(self);
        let bytes = env::storage_usage().saturating_sub(before);
        let cost = u128::from(bytes)
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("STORAGE_COST_OVERFLOW");
        let required = Self::add(cost, native_used);
        let attached = env::attached_deposit().as_yoctonear();
        require!(attached >= required, "INSUFFICIENT_STORAGE_DEPOSIT");

        let refund = attached - required;
        if refund > 0 {
            Promise::new(env::predecessor_account_id())
                .transfer(NearToken::from_yoctonear(refund))
                .detach();
        }
    }

    pub(crate) fn emit(event: &str, data: near_sdk::serde_json::Value) {
        env::log_str(&format!(
            "EVENT_JSON:{}",
            near_sdk::serde_json::json!({
                "standard": "apodect",
                "version": "1.0.0",
                "event": event,
                "data": [data]
            })
        ));
    }
}
