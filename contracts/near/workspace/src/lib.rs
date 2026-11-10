//! A Workspace creates physically separate Project accounts and tracks deployment and refunds.

use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{AccountId, Gas, NearToken, PanicOnDefault, Promise, env, near, require};

#[cfg(target_arch = "wasm32")]
const PROJECT_WASM: &[u8] = include_bytes!("../../artifacts/apodect_near_project.wasm");
#[cfg(not(target_arch = "wasm32"))]
const PROJECT_WASM: &[u8] = &[];
const CREATE_GAS: Gas = Gas::from_tgas(25);
const CALLBACK_GAS: Gas = Gas::from_tgas(20);

#[near(serializers = [json, borsh])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationState {
    Pending,
    Ready,
    Failed,
}

#[near(serializers = [borsh])]
pub(crate) struct Creation {
    pub(crate) account_id: AccountId,
    pub(crate) state: CreationState,
    pub(crate) initial_balance: u128,
    pub(crate) refund_credit: u128,
    pub(crate) refund_pending: bool,
}

#[near(serializers = [json])]
pub struct CreationView {
    pub account_id: AccountId,
    pub state: CreationState,
    pub initial_balance: U128,
    pub refund_credit: U128,
    pub refund_pending: bool,
}

#[near(serializers = [json])]
pub struct WorkspaceView {
    pub owner_id: AccountId,
    pub token_id: AccountId,
    pub next_project: u64,
    pub project_code_hash: Vec<u8>,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Workspace {
    owner_id: AccountId,
    token_id: AccountId,
    next_project: u64,
    projects: LookupMap<u64, Creation>,
}

#[near]
impl Workspace {
    #[init]
    pub fn new(owner_id: AccountId, token_id: AccountId) -> Self {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "ONLY_ACCOUNT_INITIALIZER"
        );
        require!(owner_id != env::current_account_id(), "INVALID_OWNER");
        Self {
            owner_id,
            token_id,
            next_project: 0,
            projects: LookupMap::new(b"p"),
        }
    }

    /// Attached native balance pays factory-record storage; all remaining balance funds the child.
    #[payable]
    pub fn create_project(&mut self, authority_id: AccountId) -> Promise {
        self.assert_owner();
        require!(
            env::prepaid_gas() >= Gas::from_tgas(150),
            "INSUFFICIENT_CREATION_GAS"
        );

        let before = env::storage_usage();
        let project_id = self.next_project;
        let account_id: AccountId = format!("p{project_id}.{}", env::current_account_id())
            .parse()
            .expect("PROJECT_ACCOUNT_TOO_LONG");
        require!(authority_id != account_id, "INVALID_AUTHORITY");
        self.next_project = project_id.checked_add(1).expect("PROJECT_ID_OVERFLOW");
        self.projects.insert(
            project_id,
            Creation {
                account_id: account_id.clone(),
                state: CreationState::Pending,
                initial_balance: 0,
                refund_credit: 0,
                refund_pending: false,
            },
        );
        self.projects.flush();
        env::state_write(self);

        let storage_cost = u128::from(env::storage_usage().saturating_sub(before))
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("STORAGE_OVERFLOW");
        let initial_balance = env::attached_deposit()
            .as_yoctonear()
            .checked_sub(storage_cost)
            .expect("INSUFFICIENT_STORAGE_DEPOSIT");
        let minimum = (PROJECT_WASM.len() as u128 + 4096)
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("STORAGE_OVERFLOW");
        require!(initial_balance >= minimum, "INSUFFICIENT_PROJECT_STORAGE");
        self.projects
            .get_mut(&project_id)
            .expect("PROJECT_NOT_FOUND")
            .initial_balance = initial_balance;
        let args = near_sdk::serde_json::json!({
            "workspace_id": env::current_account_id(),
            "project_id": project_id,
            "owner_id": self.owner_id,
            "authority_id": authority_id,
            "token_id": self.token_id,
        });

        Self::emit(
            "project_creation_requested",
            near_sdk::serde_json::json!({
                "project_id": project_id,
                "account_id": account_id,
                "initial_balance": U128(initial_balance)
            }),
        );

        Promise::new(account_id)
            .create_account()
            .transfer(NearToken::from_yoctonear(initial_balance))
            .deploy_contract(PROJECT_WASM.to_vec())
            .function_call(
                "new",
                near_sdk::serde_json::to_vec(&args).expect("SERIALIZATION_FAILED"),
                NearToken::from_yoctonear(0),
                CREATE_GAS,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .with_unused_gas_weight(0)
                    .settle_creation(project_id),
            )
    }

    /// The complete create/transfer/deploy/init batch is one atomic action receipt.
    #[private]
    pub fn settle_creation(&mut self, project_id: u64) -> bool {
        require!(
            env::promise_results_count() == 1,
            "EXPECTED_ONE_PROMISE_RESULT"
        );
        let Some(creation) = self.projects.get_mut(&project_id) else {
            return false;
        };
        if creation.state != CreationState::Pending {
            return false;
        }

        let success = match env::promise_result_checked(0, 0) {
            Ok(_) | Err(near_sdk::PromiseError::TooLong(_)) => true,
            Err(near_sdk::PromiseError::Failed) => false,
            Err(_) => env::panic_str("UNKNOWN_PROMISE_RESULT"),
        };
        creation.state = if success {
            CreationState::Ready
        } else {
            CreationState::Failed
        };
        if !success {
            creation.refund_credit = creation.initial_balance;
        }

        Self::emit(
            "project_creation_settled",
            near_sdk::serde_json::json!({
                "project_id": project_id,
                "success": success
            }),
        );
        success
    }

    /// Only a known failed atomic creation has refundable principal; success never does.
    pub fn claim_failed_creation(&mut self, project_id: u64) -> Promise {
        self.assert_owner();
        let creation = self
            .projects
            .get_mut(&project_id)
            .expect("PROJECT_NOT_FOUND");
        require!(
            creation.state == CreationState::Failed
                && creation.refund_credit > 0
                && !creation.refund_pending,
            "NO_CREATION_REFUND"
        );
        let amount = creation.refund_credit;
        creation.refund_pending = true;

        Self::emit(
            "creation_refund_requested",
            near_sdk::serde_json::json!({
                "project_id": project_id,
                "amount": U128(amount)
            }),
        );

        Promise::new(self.owner_id.clone())
            .transfer(NearToken::from_yoctonear(amount))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .with_unused_gas_weight(0)
                    .settle_creation_refund(project_id),
            )
    }

    #[private]
    pub fn settle_creation_refund(&mut self, project_id: u64) -> bool {
        require!(
            env::promise_results_count() == 1,
            "EXPECTED_ONE_PROMISE_RESULT"
        );
        let Some(creation) = self.projects.get_mut(&project_id) else {
            return false;
        };
        if !creation.refund_pending {
            return false;
        }

        let success = match env::promise_result_checked(0, 0) {
            Ok(_) | Err(near_sdk::PromiseError::TooLong(_)) => true,
            Err(near_sdk::PromiseError::Failed) => false,
            Err(_) => env::panic_str("UNKNOWN_PROMISE_RESULT"),
        };
        creation.refund_pending = false;
        if success {
            creation.refund_credit = 0;
        }

        Self::emit(
            "creation_refund_settled",
            near_sdk::serde_json::json!({
                "project_id": project_id,
                "success": success
            }),
        );
        success
    }

    pub fn get_workspace(&self) -> WorkspaceView {
        WorkspaceView {
            owner_id: self.owner_id.clone(),
            token_id: self.token_id.clone(),
            next_project: self.next_project,
            project_code_hash: env::sha256(PROJECT_WASM),
        }
    }

    pub fn get_project(&self, project_id: u64) -> Option<CreationView> {
        self.projects.get(&project_id).map(|c| CreationView {
            account_id: c.account_id.clone(),
            state: c.state,
            initial_balance: U128(c.initial_balance),
            refund_credit: U128(c.refund_credit),
            refund_pending: c.refund_pending,
        })
    }
}

impl Workspace {
    fn emit(event: &str, data: near_sdk::serde_json::Value) {
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

    fn assert_owner(&self) {
        require!(env::predecessor_account_id() == self.owner_id, "ONLY_OWNER");
    }
}

#[cfg(test)]
mod tests;
