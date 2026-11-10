//! Actual FT ingress and cohesive asynchronous outflow admission, dispatch and settlement.

use crate::state::{Operation, OperationState, TaskState};
use crate::{DepositMessage, Project, ProjectExt};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::json_types::U128;
use near_sdk::{
    AccountId, Gas, NearToken, Promise, PromiseOrValue, env, ext_contract, near, require,
};

const FT_GAS: Gas = Gas::from_tgas(30);
const CALLBACK_GAS: Gas = Gas::from_tgas(25);
const MIN_OUTFLOW_GAS: Gas = Gas::from_tgas(90);

#[allow(dead_code)]
#[ext_contract(ft)]
trait FungibleToken {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);

    fn storage_deposit(&mut self, account_id: Option<AccountId>, registration_only: Option<bool>);
}

#[near]
impl Project {
    /// Owner pays the exact token's native storage registration deposit separately.
    #[payable]
    pub fn register_token(&mut self) -> Promise {
        self.assert_owner();
        require!(
            env::attached_deposit().as_yoctonear() > 0,
            "REGISTRATION_DEPOSIT_REQUIRED"
        );
        ft::ext(self.token_id.clone())
            .with_attached_deposit(env::attached_deposit())
            .with_static_gas(FT_GAS)
            .with_unused_gas_weight(0)
            .storage_deposit(Some(env::current_account_id()), Some(true))
    }

    #[payable]
    pub fn withdraw(&mut self, task_id: u64, amount: U128) -> Promise {
        Self::assert_positive(amount.0);
        let mut task = self.task(task_id);
        Self::assert_agent(&task);
        require!(
            task.state == TaskState::Active && !task.paused,
            "TASK_NOT_SPENDABLE"
        );
        require!(task.available >= amount.0, "INSUFFICIENT_ALLOCATION");
        require!(
            env::prepaid_gas() >= MIN_OUTFLOW_GAS,
            "INSUFFICIENT_OUTFLOW_GAS"
        );

        let before = env::storage_usage();
        task.available = Self::sub(task.available, amount.0);
        task.pending_outflow = Self::add(task.pending_outflow, amount.0);
        task.started = true;
        let receiver_id = task.agent.clone();
        self.tasks.insert(task_id, task);
        let operation_id = self.insert_operation(Some(task_id), amount.0);

        self.charge_storage(before, 1);
        self.send_tokens(operation_id, receiver_id, amount.0)
    }

    #[payable]
    pub fn withdraw_free(&mut self, amount: U128) -> Promise {
        self.assert_owner();
        Self::assert_positive(amount.0);
        require!(self.free >= amount.0, "INSUFFICIENT_FREE_FUNDS");
        require!(
            env::prepaid_gas() >= MIN_OUTFLOW_GAS,
            "INSUFFICIENT_OUTFLOW_GAS"
        );

        let before = env::storage_usage();
        self.free = Self::sub(self.free, amount.0);
        self.pending_owner_outflow = Self::add(self.pending_owner_outflow, amount.0);
        let operation_id = self.insert_operation(None, amount.0);

        self.charge_storage(before, 1);
        self.send_tokens(operation_id, self.owner_id.clone(), amount.0)
    }

    /// Authentication comes from the runtime promise result, never caller-supplied success.
    #[private]
    pub fn settle_outflow(&mut self, operation_id: u64) -> bool {
        require!(
            env::promise_results_count() == 1,
            "EXPECTED_ONE_PROMISE_RESULT"
        );
        let Some(mut operation) = self.operations.get(&operation_id).cloned() else {
            return false;
        };
        if operation.state != OperationState::Pending {
            return false;
        }

        let success = match env::promise_result_checked(0, 0) {
            Ok(_) | Err(near_sdk::PromiseError::TooLong(_)) => true,
            Err(near_sdk::PromiseError::Failed) => false,
            Err(_) => env::panic_str("UNKNOWN_PROMISE_RESULT"),
        };
        if let Some(task_id) = operation.task_id {
            let mut task = self.task(task_id);
            task.pending_outflow = Self::sub(task.pending_outflow, operation.amount);
            if !success {
                task.available = Self::add(task.available, operation.amount);
            }
            self.tasks.insert(task_id, task);
        } else {
            self.pending_owner_outflow = Self::sub(self.pending_owner_outflow, operation.amount);
            if !success {
                self.free = Self::add(self.free, operation.amount);
            }
        }
        operation.state = if success {
            OperationState::Succeeded
        } else {
            OperationState::Failed
        };

        Self::emit(
            "outflow_settled",
            near_sdk::serde_json::json!({
                "operation_id": operation_id,
                "task_id": operation.task_id,
                "amount": U128(operation.amount),
                "success": success
            }),
        );
        self.operations.insert(operation_id, operation);
        success
    }
}

#[near]
impl FungibleTokenReceiver for Project {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        require!(
            env::predecessor_account_id() == self.token_id,
            "WRONG_TOKEN"
        );
        Self::assert_positive(amount.0);
        require!(msg.len() <= 160, "DEPOSIT_MESSAGE_TOO_LONG");
        let message: DepositMessage =
            near_sdk::serde_json::from_str(&msg).expect("INVALID_DEPOSIT_MESSAGE");
        match message {
            DepositMessage::Fund { invoice_id } => {
                require!(sender_id == self.owner_id, "ONLY_OWNER_FUNDING");
                if let Some(invoice_id) = invoice_id {
                    let mut invoice = self
                        .invoices
                        .get(&invoice_id)
                        .cloned()
                        .expect("INVOICE_NOT_FOUND");
                    require!(invoice.closed, "INVOICE_NOT_RESPONDED");
                    invoice.actual_funding = Self::add(invoice.actual_funding, amount.0);
                    self.invoices.insert(invoice_id, invoice);
                }
                self.free = Self::add(self.free, amount.0);

                Self::emit(
                    "project_funded",
                    near_sdk::serde_json::json!({
                        "amount": amount,
                        "invoice_id": invoice_id
                    }),
                );
            }
            DepositMessage::Return { task_id } => {
                let mut task = self.task(task_id);
                require!(sender_id == task.agent, "ONLY_TASK_AGENT");
                Self::assert_nonterminal(&task);
                require!(amount.0 <= Self::external(&task), "EXCESS_TASK_RETURN");
                task.recovered = Self::add(task.recovered, amount.0);
                self.free = Self::add(self.free, amount.0);
                self.tasks.insert(task_id, task);

                Self::emit(
                    "external_returned",
                    near_sdk::serde_json::json!({
                        "task_id": task_id,
                        "amount": amount
                    }),
                );
            }
        }
        PromiseOrValue::Value(U128(0))
    }
}

impl Project {
    fn insert_operation(&mut self, task_id: Option<u64>, amount: u128) -> u64 {
        let operation_id = self.next_operation;
        self.next_operation = operation_id.checked_add(1).expect("OPERATION_ID_OVERFLOW");
        self.operations.insert(
            operation_id,
            Operation {
                task_id,
                amount,
                state: OperationState::Pending,
            },
        );

        Self::emit(
            "outflow_accepted",
            near_sdk::serde_json::json!({
                "operation_id": operation_id,
                "task_id": task_id,
                "amount": U128(amount)
            }),
        );
        operation_id
    }

    fn send_tokens(&self, operation_id: u64, receiver_id: AccountId, amount: u128) -> Promise {
        ft::ext(self.token_id.clone())
            .with_attached_deposit(NearToken::from_yoctonear(1))
            .with_static_gas(FT_GAS)
            .with_unused_gas_weight(0)
            .ft_transfer(
                receiver_id,
                U128(amount),
                Some(format!("apodect:{operation_id}")),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .with_unused_gas_weight(0)
                    .settle_outflow(operation_id),
            )
    }
}
