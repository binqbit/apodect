//! Keyless receiver for ordinary NEP-141 payments, swept into an attributed Project route.

use near_sdk::json_types::U128;
use near_sdk::{
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue, env, near, require,
};

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct RouteEscrow {
    project_id: AccountId,
    token_id: AccountId,
    route: String,
    refund: bool,
    pending: bool,
    expected: u64,
    collected: u64,
}

#[near]
impl RouteEscrow {
    #[init]
    pub fn new(
        project_id: AccountId,
        token_id: AccountId,
        route: String,
        refund: bool,
        expected: u64,
    ) -> Self {
        require!(env::predecessor_account_id() == project_id, "ONLY_PROJECT");
        require!(route.len() == 64, "INVALID_ROUTE");
        Self {
            project_id,
            token_id,
            route,
            refund,
            pending: false,
            expected,
            collected: 0,
        }
    }

    /// Concurrent collectors cannot initiate two transfers against the same balance.
    #[payable]
    pub fn collect(&mut self) -> Promise {
        require!(!self.pending, "COLLECTION_PENDING");
        require!(
            env::attached_deposit().as_yoctonear() >= 1,
            "ONE_YOCTO_REQUIRED"
        );
        require!(
            env::prepaid_gas() >= Gas::from_tgas(180),
            "INSUFFICIENT_COLLECTION_GAS"
        );
        self.pending = true;
        Promise::new(self.token_id.clone())
            .function_call(
                "ft_balance_of",
                near_sdk::serde_json::to_vec(&near_sdk::serde_json::json!({
                    "account_id": env::current_account_id()
                }))
                .unwrap(),
                NearToken::from_yoctonear(0),
                Gas::from_tgas(10),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(140))
                    .on_balance(),
            )
    }

    #[private]
    pub fn on_balance(
        &mut self,
        #[callback_result] result: Result<U128, near_sdk::PromiseError>,
    ) -> PromiseOrValue<U128> {
        require!(self.pending, "NO_PENDING_COLLECTION");
        let Ok(mut amount) = result else {
            self.pending = false;
            return PromiseOrValue::Value(U128(0));
        };
        let remaining = self.expected - self.collected;
        if amount.0 == 0 || remaining == 0 || (!self.refund && amount.0 < u128::from(remaining)) {
            self.pending = false;
            return PromiseOrValue::Value(U128(0));
        }
        amount.0 = amount.0.min(u128::from(remaining));
        let msg = near_sdk::serde_json::json!({
            "kind": if self.refund { "refund" } else { "delivery" },
            "route": self.route
        })
        .to_string();
        PromiseOrValue::Promise(
            Promise::new(self.token_id.clone())
                .function_call(
                    "ft_transfer_call",
                    near_sdk::serde_json::to_vec(&near_sdk::serde_json::json!({
                        "receiver_id": self.project_id,
                        "amount": amount,
                        "memo": null,
                        "msg": msg
                    }))
                    .unwrap(),
                    NearToken::from_yoctonear(1),
                    Gas::from_tgas(100),
                )
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(Gas::from_tgas(15))
                        .on_sweep(),
                ),
        )
    }

    #[private]
    pub fn on_sweep(
        &mut self,
        #[callback_result] result: Result<U128, near_sdk::PromiseError>,
    ) -> U128 {
        require!(self.pending, "NO_PENDING_COLLECTION");
        self.pending = false;
        let used = result.unwrap_or(U128(0));
        self.collected = self
            .collected
            .checked_add(used.0.try_into().expect("COLLECTION_OVERFLOW"))
            .expect("COLLECTION_OVERFLOW");
        require!(self.collected <= self.expected, "EXCESS_COLLECTION");
        used
    }

    pub fn get_route(&self) -> near_sdk::serde_json::Value {
        near_sdk::serde_json::json!({
            "project_id": self.project_id,
            "token_id": self.token_id,
            "route": self.route,
            "refund": self.refund,
            "pending": self.pending,
            "expected": self.expected,
            "collected": self.collected
        })
    }
}
