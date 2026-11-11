//! Coordinated custody: bounded commands, route escrows, asynchronous settlement and immutable evidence.

use crate::state::{RequestKind, RequestState, TaskState};
use crate::{Project, ProjectExt};
use apodect_near_protocol::{
    Checkpoint, Command, Word, ZERO, action, hash, hex, number, uint, unhex, verify_signature, word,
};
use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{AccountId, Gas, NearToken, Promise, PromiseOrValue, env, near, require};

#[cfg(target_arch = "wasm32")]
const ESCROW_WASM: &[u8] = include_bytes!("../../artifacts/apodect_near_route_escrow.wasm");
#[cfg(not(target_arch = "wasm32"))]
const ESCROW_WASM: &[u8] = &[];
const ROUTE_TOKEN_REGISTRATION: NearToken = NearToken::from_millinear(10);

#[near(serializers = [borsh])]
pub(crate) struct Binding {
    domain: Word,
    key: Vec<u8>,
}
#[near(serializers = [borsh])]
#[derive(Clone)]
struct Hold {
    state: u8,
    amount: u64,
}
#[near(serializers = [borsh])]
#[derive(Clone)]
struct Route {
    plan: Word,
    kind: u8,
    expected: u64,
    received: u64,
    dispatched: u64,
    dispatch: Option<Word>,
    ready: bool,
}
#[near(serializers = [borsh])]
#[derive(Clone)]
struct Outcome {
    digest: Word,
    action: u64,
    state: u8,
    result: Word,
    amount: u64,
    plan: Word,
    route: Word,
}

#[near(serializers = [borsh])]
pub(crate) struct Bridge {
    pub(crate) binding: Option<Binding>,
    next_funding: u64,
    funding: LookupMap<Word, String>,
    held: u64,
    sequence: u64,
    holds: LookupMap<Word, Hold>,
    routes: LookupMap<Word, Route>,
    outcomes: LookupMap<Word, Outcome>,
    nonces: LookupMap<u64, u64>,
    checkpoints: LookupMap<Word, String>,
    funding_refs: LookupMap<Word, Word>,
    accounted_custody: u64,
}

impl Bridge {
    pub(crate) fn new() -> Self {
        Self {
            binding: None,
            next_funding: 1,
            funding: LookupMap::new(b"bf".as_slice()),
            held: 0,
            sequence: 0,
            holds: LookupMap::new(b"bh".as_slice()),
            routes: LookupMap::new(b"br".as_slice()),
            outcomes: LookupMap::new(b"bo".as_slice()),
            nonces: LookupMap::new(b"bn".as_slice()),
            checkpoints: LookupMap::new(b"bc".as_slice()),
            funding_refs: LookupMap::new(b"bF".as_slice()),
            accounted_custody: 0,
        }
    }
    pub(crate) fn flush(&mut self) {
        self.funding.flush();
        self.holds.flush();
        self.routes.flush();
        self.outcomes.flush();
        self.nonces.flush();
        self.checkpoints.flush();
        self.funding_refs.flush();
    }
}

#[near]
impl Project {
    #[payable]
    pub fn bind_coordinator(&mut self, domain: String, key: String) {
        self.assert_owner();
        self.assert_standalone();
        require!(
            self.next_task == 0
                && self.next_request == 0
                && self.next_invoice == 0
                && self.next_operation == 0
                && self.pending_owner_outflow == 0,
            "PROJECT_NOT_FRESH"
        );
        require!(
            env::current_account_id().as_str().len() <= 37,
            "PROJECT_ACCOUNT_TOO_LONG_FOR_ROUTES"
        );
        require!(unhex(&key).len() == 64, "INVALID_COMMAND_KEY");
        require!(self.free <= u128::from(u64::MAX), "AMOUNT_EXCEEDS_U64");
        let domain = word(&domain);
        require!(domain != ZERO, "ZERO_DOMAIN");
        let before = env::storage_usage();
        self.bridge.binding = Some(Binding {
            domain,
            key: unhex(&key),
        });
        self.bridge.accounted_custody = self.free.try_into().expect("AMOUNT_EXCEEDS_U64");
        self.charge_storage(before, 0);
    }

    pub fn route_receiver(&self, route: String) -> AccountId {
        let id = word(&route);
        format!("r{}.{}", &hex(&id)[..24], env::current_account_id())
            .parse()
            .expect("ROUTE_ACCOUNT_TOO_LONG")
    }

    /// Native balance consumed by deploying an escrow and registering its token account.
    /// The caller also covers the Project's ordinary storage growth when executing.
    pub fn route_deployment_deposit(&self) -> U128 {
        U128(Self::route_escrow_balance() + ROUTE_TOKEN_REGISTRATION.as_yoctonear())
    }

    pub fn get_funding(&self, funding_id: String) -> Option<String> {
        self.bridge.funding.get(&word(&funding_id)).cloned()
    }

    pub fn get_funding_by_reference(&self, funding_ref: String) -> Option<String> {
        self.bridge
            .funding_refs
            .get(&word(&funding_ref))
            .and_then(|id| self.bridge.funding.get(id))
            .cloned()
    }

    pub fn get_bridge(&self) -> near_sdk::serde_json::Value {
        near_sdk::serde_json::json!({
            "domain": self.bridge.binding.as_ref().map(|b| hex(&b.domain)),
            "key": self.bridge.binding.as_ref().map(|b| hex(&b.key)),
            "held": self.bridge.held.to_string(),
            "sequence": self.bridge.sequence,
            "accounted_custody": self.bridge.accounted_custody.to_string(),
            "next_funding": self.bridge.next_funding
        })
    }

    pub fn get_plan(&self, plan: String) -> near_sdk::serde_json::Value {
        self.bridge
            .holds
            .get(&word(&plan))
            .map(|p| {
                near_sdk::serde_json::json!({
                    "state": p.state,
                    "held": p.amount.to_string()
                })
            })
            .unwrap_or_default()
    }

    pub fn get_route(&self, route: String) -> near_sdk::serde_json::Value {
        self.bridge
            .routes
            .get(&word(&route))
            .map(|r| {
                near_sdk::serde_json::json!({
                    "plan": hex(&r.plan),
                    "kind": r.kind,
                    "expected": r.expected.to_string(),
                    "received": r.received.to_string(),
                    "dispatched": r.dispatched.to_string(),
                    "ready": r.ready,
                    "receiver": self.route_receiver(route.clone())
                })
            })
            .unwrap_or_default()
    }

    pub fn get_command(&self, operation: String) -> near_sdk::serde_json::Value {
        self.bridge
            .outcomes
            .get(&word(&operation))
            .map(|o| {
                near_sdk::serde_json::json!({
                    "digest": hex(&o.digest),
                    "action": o.action,
                    "state": o.state,
                    "result": hex(&o.result)
                })
            })
            .unwrap_or_default()
    }

    #[payable]
    pub fn execute_command(
        &mut self,
        command: String,
        signature: String,
        recovery_id: u8,
        recipient_id: Option<AccountId>,
    ) -> PromiseOrValue<String> {
        let command = Command::parse(&command);
        let binding = self.bridge.binding.as_ref().expect("NOT_COORDINATED");
        require!(command.words[0] == binding.domain, "WRONG_COMMAND_DOMAIN");
        verify_signature(
            command.digest(),
            &unhex(&signature),
            recovery_id,
            &binding.key,
        );
        let id = command.operation();
        if let Some(old) = self.bridge.outcomes.get(&id) {
            require!(old.digest == command.digest(), "COMMAND_ID_CONFLICT");
            return PromiseOrValue::Value(hex(&old.result));
        }
        // Expiry prevents a new effect, not reconciliation of an immutable
        // already executed command after a delayed response or service restart.
        command.validate(binding.domain);
        let before = env::storage_usage();
        let action = command.action();
        let amount = command.amount();
        let plan = command.plan();
        let route = command.words[11];
        let mut result = uint(amount);
        let mut pending = false;
        let mut promise = None;
        let mut native_used = 0;
        match action {
            action::RESERVE => {
                require!(
                    self.bridge.holds.get(&plan).is_none(),
                    "PLAN_ALREADY_EXISTS"
                );
                require!(self.free >= u128::from(amount), "INSUFFICIENT_FREE_FUNDS");
                self.free -= u128::from(amount);
                self.bridge.held = self.bridge.held.checked_add(amount).expect("HELD_OVERFLOW");
                self.bridge.holds.insert(plan, Hold { state: 1, amount });
            }
            action::EXPECT_DELIVERY | action::EXPECT_REFUND => {
                require!(amount > 0, "ZERO_AMOUNT");
                require!(
                    self.bridge.holds.get(&plan).is_some_and(|p| p.state == 1),
                    "PLAN_NOT_OPEN"
                );
                require!(
                    self.bridge.routes.get(&route).is_none(),
                    "ROUTE_ALREADY_EXISTS"
                );
                let receiver = self.route_receiver(hex(&route));
                let balance = Self::route_escrow_balance();
                native_used = balance + ROUTE_TOKEN_REGISTRATION.as_yoctonear();
                self.bridge.routes.insert(
                    route,
                    Route {
                        plan,
                        kind: if action == action::EXPECT_DELIVERY {
                            1
                        } else {
                            2
                        },
                        expected: amount,
                        received: 0,
                        dispatched: 0,
                        dispatch: None,
                        ready: false,
                    },
                );
                let args = near_sdk::serde_json::json!({
                    "project_id": env::current_account_id(),
                    "token_id": self.token_id,
                    "route": hex(&route),
                    "refund": action == action::EXPECT_REFUND,
                    "expected": amount
                });
                require!(
                    env::prepaid_gas() >= Gas::from_tgas(160),
                    "INSUFFICIENT_ROUTE_GAS"
                );
                promise = Some(
                    Promise::new(receiver)
                        .create_account()
                        .transfer(NearToken::from_yoctonear(balance))
                        .deploy_contract(ESCROW_WASM.to_vec())
                        .function_call(
                            "new",
                            args.to_string().into_bytes(),
                            NearToken::from_yoctonear(0),
                            Gas::from_tgas(20),
                        )
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(Gas::from_tgas(65))
                                .register_bridge_route(hex(&route)),
                        )
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(Gas::from_tgas(25))
                                .settle_bridge(hex(&id)),
                        ),
                );
                pending = true;
            }
            action::DISPATCH | action::OWNER_WITHDRAW => {
                require!(amount > 0, "ZERO_AMOUNT");
                require!(
                    env::prepaid_gas() >= Gas::from_tgas(100),
                    "INSUFFICIENT_OUTFLOW_GAS"
                );
                let receiver = recipient_id.expect("RECIPIENT_REQUIRED");
                require!(
                    hash(receiver.as_bytes()) == command.words[9],
                    "WRONG_RECIPIENT"
                );
                if action == action::DISPATCH {
                    let mut r = self
                        .bridge
                        .routes
                        .get(&route)
                        .cloned()
                        .expect("REFUND_ROUTE_NOT_FOUND");
                    require!(
                        r.kind == 2
                            && r.ready
                            && r.plan == plan
                            && r.expected == amount
                            && r.dispatch.is_none(),
                        "INVALID_DISPATCH_ROUTE"
                    );
                    self.consume_hold(plan, amount);
                    r.dispatch = Some(id);
                    self.bridge.routes.insert(route, r);
                } else {
                    require!(receiver == self.owner_id, "WRONG_OWNER_RECIPIENT");
                    require!(self.free >= u128::from(amount), "INSUFFICIENT_FREE_FUNDS");
                    self.free -= u128::from(amount);
                }
                native_used = 1;
                pending = true;
                promise = Some(
                    Promise::new(self.token_id.clone())
                        .function_call(
                            "ft_transfer",
                            near_sdk::serde_json::json!({
                                "receiver_id": receiver,
                                "amount": amount.to_string(),
                                "memo": format!("apodect:{}", hex(&id))
                            })
                            .to_string()
                            .into_bytes(),
                            NearToken::from_yoctonear(1),
                            Gas::from_tgas(30),
                        )
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(Gas::from_tgas(30))
                                .settle_bridge(hex(&id)),
                        ),
                );
            }
            action::ALLOCATE => {
                let request_id = number(command.words[6]);
                let task_id = number(command.words[5]);
                let mut req = self.request(request_id);
                let mut task = self.task(task_id);
                require!(
                    req.task_id == task_id
                        && req.state == RequestState::Pending
                        && req.amount == u128::from(amount)
                        && task.live_request == Some(request_id),
                    "REQUEST_MISMATCH"
                );
                require!(
                    matches!(task.state, TaskState::Active | TaskState::Stopped),
                    "TASK_NOT_FUNDABLE"
                );
                self.consume_hold(plan, amount);
                require!(
                    self.bridge.holds.get(&plan).unwrap().amount == 0,
                    "EXCESS_PLAN_HOLD"
                );
                self.bridge.holds.get_mut(&plan).unwrap().state = 2;
                task.available = Self::add(task.available, u128::from(amount));
                task.issued = Self::add(task.issued, u128::from(amount));
                require!(task.issued <= u128::from(u64::MAX), "AMOUNT_EXCEEDS_U64");
                task.live_request = None;
                if req.kind == RequestKind::Resume {
                    task.state = TaskState::Active;
                }
                req.state = RequestState::Funded;
                self.tasks.insert(task_id, task);
                self.requests.insert(request_id, req);
            }
            action::CLOSE_PLAN => {
                let mut hold = self
                    .bridge
                    .holds
                    .get(&plan)
                    .cloned()
                    .expect("PLAN_NOT_FOUND");
                result = uint(if hold.state == 1 { hold.amount } else { 0 });
                if hold.state == 1 {
                    self.bridge.held -= hold.amount;
                    self.free = Self::add(self.free, u128::from(hold.amount));
                    hold.amount = 0;
                    hold.state = 3;
                    self.bridge.holds.insert(plan, hold);
                }
            }
            action::REJECT..=action::RESTORE_REQUEST => {
                result = self.control_command(&command);
            }
            action::INVALIDATE => {
                let state = self
                    .bridge
                    .outcomes
                    .get(&route)
                    .map(|o| o.state)
                    .unwrap_or(0);
                if state == 0 {
                    self.bridge.outcomes.insert(
                        route,
                        Outcome {
                            digest: ZERO,
                            action: 0,
                            state: 3,
                            result: ZERO,
                            amount: 0,
                            plan: ZERO,
                            route: ZERO,
                        },
                    );
                }
                result = uint(if state == 0 { 3 } else { u64::from(state) });
            }
            _ => unreachable!(),
        }
        self.bridge.outcomes.insert(
            id,
            Outcome {
                digest: command.digest(),
                action,
                state: if pending { 4 } else { 1 },
                result,
                amount,
                plan,
                route,
            },
        );
        self.charge_storage(before, native_used);
        match promise {
            Some(p) => PromiseOrValue::Promise(p),
            None => PromiseOrValue::Value(hex(&result)),
        }
    }

    #[private]
    pub fn register_bridge_route(&self, route: String) -> Promise {
        require!(
            env::promise_results_count() == 1,
            "EXPECTED_ONE_PROMISE_RESULT"
        );
        require!(
            matches!(
                env::promise_result_checked(0, 0),
                Ok(_) | Err(near_sdk::PromiseError::TooLong(_))
            ),
            "ROUTE_DEPLOYMENT_FAILED"
        );
        Promise::new(self.token_id.clone()).function_call(
            "storage_deposit",
            near_sdk::serde_json::json!({
                "account_id": self.route_receiver(route), "registration_only": true
            })
            .to_string()
            .into_bytes(),
            ROUTE_TOKEN_REGISTRATION,
            Gas::from_tgas(30),
        )
    }

    #[private]
    pub fn settle_bridge(&mut self, operation: String) -> String {
        require!(
            env::promise_results_count() == 1,
            "EXPECTED_ONE_PROMISE_RESULT"
        );
        let id = word(&operation);
        let mut outcome = self
            .bridge
            .outcomes
            .get(&id)
            .cloned()
            .expect("COMMAND_NOT_FOUND");
        if outcome.state != 4 {
            return hex(&outcome.result);
        }
        let success = match env::promise_result_checked(0, 0) {
            Ok(_) | Err(near_sdk::PromiseError::TooLong(_)) => true,
            Err(near_sdk::PromiseError::Failed) => false,
            Err(_) => env::panic_str("UNKNOWN_PROMISE_RESULT"),
        };
        match outcome.action {
            action::EXPECT_DELIVERY | action::EXPECT_REFUND => {
                self.bridge.routes.get_mut(&outcome.route).unwrap().ready = success;
            }
            action::DISPATCH => {
                if success {
                    self.confirm_custody_outflow(u128::from(outcome.amount));
                    self.bridge
                        .routes
                        .get_mut(&outcome.route)
                        .unwrap()
                        .dispatched = outcome.amount;
                } else {
                    self.restore_hold(outcome.plan, outcome.amount);
                }
            }
            action::OWNER_WITHDRAW => {
                if success {
                    self.confirm_custody_outflow(u128::from(outcome.amount));
                } else {
                    self.free = Self::add(self.free, u128::from(outcome.amount));
                }
            }
            _ => env::panic_str("INVALID_PENDING_COMMAND"),
        }
        outcome.state = if success { 1 } else { 2 };
        outcome.result = uint(if success { outcome.amount } else { 0 });
        self.bridge.outcomes.insert(id, outcome.clone());
        hex(&outcome.result)
    }

    #[payable]
    pub fn checkpoint(
        &mut self,
        observation: String,
        task: Option<u64>,
        request: Option<u64>,
        plan: Option<String>,
        route: Option<String>,
        operation: Option<String>,
    ) -> String {
        let observation = word(&observation);
        require!(observation != ZERO, "ZERO_OBSERVATION");
        require!(
            self.bridge.checkpoints.get(&observation).is_none(),
            "OBSERVATION_EXISTS"
        );
        let binding = self.bridge.binding.as_ref().expect("NOT_COORDINATED");
        let mut p = Checkpoint { words: [ZERO; 39] };
        p.words[0] = binding.domain;
        p.words[1] = observation;
        self.bridge.sequence = self
            .bridge
            .sequence
            .checked_add(1)
            .expect("CHECKPOINT_OVERFLOW");
        let before = env::storage_usage();
        p.words[2] = uint(self.bridge.sequence);
        p.words[3] = uint(env::block_height());
        p.words[4] = uint(env::block_timestamp_ms() / 1000);
        p.words[5] = uint(self.free.try_into().expect("AMOUNT_EXCEEDS_U64"));
        p.words[6] = uint(self.bridge.held);
        p.words[8] = uint(255);
        p.words[21] = uint(255);
        if let Some(id) = task {
            p.words[7] = uint(id);
            if let Some(t) = self.tasks.get(&id) {
                p.words[8] = uint(match t.state {
                    TaskState::Active => 0,
                    TaskState::Stopped => 1,
                    TaskState::OutOfBudget => 2,
                    TaskState::Completed => 3,
                    TaskState::Canceled => 4,
                });
                p.words[9] = uint(u64::from(t.paused));
                p.words[10] = uint(u64::from(t.started));
                for (i, a) in [
                    (11, t.available),
                    (12, t.issued),
                    (13, t.recovered),
                    (14, t.pending_outflow),
                ] {
                    p.words[i] = uint(a.try_into().expect("AMOUNT_EXCEEDS_U64"));
                }
                p.words[15] = uint(*self.bridge.nonces.get(&id).unwrap_or(&0));
                p.words[16] = hash(t.agent.as_bytes());
                if let Some(r) = t.live_request {
                    p.words[37] = uint(r);
                    p.words[38] = uint(1);
                }
            }
        }
        if let Some(id) = request {
            p.words[17] = uint(id);
            if let Some(r) = self.requests.get(&id) {
                p.words[18] = uint(r.task_id);
                p.words[19] = uint(r.amount.try_into().expect("AMOUNT_EXCEEDS_U64"));
                p.words[20] = uint(match r.kind {
                    RequestKind::Initial => 0,
                    RequestKind::Supplement => 1,
                    RequestKind::Resume => 2,
                    RequestKind::Restoration => 3,
                });
                p.words[21] = uint(match r.state {
                    RequestState::Pending => 0,
                    RequestState::Queued => 1,
                    RequestState::Funded => 2,
                    RequestState::Rejected => 3,
                    RequestState::Removed => 4,
                });
                p.words[22] = r.purpose;
            }
        }
        if let Some(id) = plan {
            let id = word(&id);
            p.words[23] = id;
            if let Some(h) = self.bridge.holds.get(&id) {
                p.words[24] = uint(u64::from(h.state));
                p.words[25] = uint(h.amount);
            }
        }
        if let Some(id) = route {
            let id = word(&id);
            p.words[26] = id;
            if let Some(r) = self.bridge.routes.get(&id) {
                p.words[27] = r.plan;
                p.words[28] = uint(u64::from(r.kind));
                p.words[29] = uint(r.expected);
                p.words[30] = uint(r.received);
                p.words[31] = uint(r.dispatched);
            }
        }
        if let Some(id) = operation {
            let id = word(&id);
            p.words[32] = id;
            if let Some(o) = self.bridge.outcomes.get(&id) {
                p.words[33] = o.digest;
                p.words[34] = uint(o.action);
                p.words[35] = uint(u64::from(o.state));
                p.words[36] = o.result;
            }
        }
        let payload = p.hex();
        self.bridge.checkpoints.insert(observation, payload.clone());
        self.charge_storage(before, 0);
        payload
    }

    pub fn get_checkpoint(&self, observation: String) -> Option<String> {
        self.bridge.checkpoints.get(&word(&observation)).cloned()
    }
}

impl Project {
    fn route_escrow_balance() -> u128 {
        (ESCROW_WASM.len() as u128 + 4096) * env::storage_byte_cost().as_yoctonear()
            + NearToken::from_millinear(100).as_yoctonear()
    }

    /// Pending outflows retain capacity until the token receipt confirms success.
    pub(crate) fn account_custody_ingress(&mut self, amount: u128) {
        if self.bridge.binding.is_some() {
            let amount = u64::try_from(amount).expect("AMOUNT_EXCEEDS_U64");
            self.bridge.accounted_custody = self
                .bridge
                .accounted_custody
                .checked_add(amount)
                .expect("CUSTODY_CAPACITY_EXCEEDED");
        }
    }

    pub(crate) fn confirm_custody_outflow(&mut self, amount: u128) {
        if self.bridge.binding.is_some() {
            let amount = u64::try_from(amount).expect("AMOUNT_EXCEEDS_U64");
            self.bridge.accounted_custody = self
                .bridge
                .accounted_custody
                .checked_sub(amount)
                .expect("CUSTODY_ACCOUNTING_UNDERFLOW");
        }
    }

    pub(crate) fn record_funding(
        &mut self,
        invoice: Option<String>,
        amount: u128,
        reference: Option<String>,
    ) -> bool {
        if let Some(binding) = &self.bridge.binding {
            let id = uint(self.bridge.next_funding);
            let amount = uint(amount.try_into().expect("AMOUNT_EXCEEDS_U64"));
            let invoice = invoice.map(|v| word(&v)).unwrap_or(ZERO);
            let reference = reference.map(|value| word(&value));
            if let Some(reference) = reference {
                require!(reference != ZERO, "ZERO_FUNDING_REFERENCE");
                if let Some(existing) = self.bridge.funding_refs.get(&reference) {
                    let prior = unhex(self.bridge.funding.get(existing).unwrap());
                    require!(
                        prior[64..96] == invoice && prior[96..128] == amount,
                        "FUNDING_REFERENCE_TERMS_CHANGED"
                    );
                    return false;
                }
            }
            let payload = hex(&[
                binding.domain,
                id,
                invoice,
                amount,
                hash(self.owner_id.as_bytes()),
            ]
            .concat());
            self.bridge.next_funding = self
                .bridge
                .next_funding
                .checked_add(1)
                .expect("FUNDING_ID_OVERFLOW");
            self.bridge.funding.insert(id, payload.clone());
            if let Some(reference) = reference {
                self.bridge.funding_refs.insert(reference, id);
            }
            Self::emit(
                "funding_recorded",
                near_sdk::serde_json::json!({
                    "funding_id": hex(&id),
                    "payload": payload,
                    "funding_ref": reference.map(|value| hex(&value))
                }),
            );
        } else {
            require!(invoice.is_none(), "GLOBAL_INVOICE_REQUIRES_COORDINATOR");
            require!(
                reference.is_none(),
                "FUNDING_REFERENCE_REQUIRES_COORDINATOR"
            );
        }
        true
    }

    fn consume_hold(&mut self, plan: Word, amount: u64) {
        let hold = self.bridge.holds.get_mut(&plan).expect("PLAN_NOT_FOUND");
        require!(hold.state == 1, "PLAN_NOT_OPEN");
        hold.amount = hold
            .amount
            .checked_sub(amount)
            .expect("INSUFFICIENT_PLAN_HOLD");
        self.bridge.held -= amount;
    }

    fn restore_hold(&mut self, plan: Word, amount: u64) {
        let hold = self.bridge.holds.get_mut(&plan).expect("PLAN_NOT_FOUND");
        if hold.state == 1 {
            hold.amount = hold.amount.checked_add(amount).expect("HOLD_OVERFLOW");
            self.bridge.held = self.bridge.held.checked_add(amount).expect("HELD_OVERFLOW");
        } else {
            self.free = Self::add(self.free, u128::from(amount));
        }
    }
    pub(crate) fn receive_route(&mut self, route: &str, amount: U128, refund: bool) -> U128 {
        require!(self.bridge.binding.is_some(), "NOT_COORDINATED");
        // ft_on_transfer already authenticated the exact token contract.
        let id = word(route);
        let mut r = self
            .bridge
            .routes
            .get(&id)
            .cloned()
            .expect("ROUTE_NOT_FOUND");
        require!(
            r.ready && r.kind == if refund { 2 } else { 1 },
            "WRONG_ROUTE_KIND"
        );
        // Registration authorizes a maximum refund; only a confirmed outgoing
        // transfer establishes input that can actually have returned. Pending
        // and failed dispatches leave this limit at zero.
        let limit = if refund { r.dispatched } else { r.expected };
        let remaining = limit
            .checked_sub(r.received)
            .expect("ROUTE_ACCOUNTING_UNDERFLOW");
        if !refund {
            require!(amount.0 >= u128::from(remaining), "INCOMPLETE_DELIVERY");
        }
        let accept = amount.0.min(u128::from(remaining));
        let accept = u64::try_from(accept).unwrap();
        self.account_custody_ingress(u128::from(accept));
        if refund {
            r.received += accept;
            self.free = Self::add(self.free, u128::from(accept));
        } else if accept > 0 {
            r.received += accept;
            self.restore_hold(r.plan, accept);
        }
        self.bridge.routes.insert(id, r);
        U128(amount.0 - u128::from(accept))
    }

    fn control_command(&mut self, c: &Command) -> Word {
        let id = number(c.words[5]);
        let mut task = self.task(id);
        Self::assert_nonterminal(&task);
        let action = c.action();
        let amount = u128::from(c.amount());
        let mut result = uint(c.amount());
        if matches!(action, action::PAUSE..=action::CANCEL) {
            let n = *self.bridge.nonces.get(&id).unwrap_or(&0);
            require!(number(c.words[1]) == n, "CONTROL_NONCE_MISMATCH");
            self.bridge
                .nonces
                .insert(id, n.checked_add(1).expect("NONCE_OVERFLOW"));
        }
        match action {
            action::REJECT => {
                let rid = number(c.words[6]);
                let mut r = self.request(rid);
                require!(
                    r.task_id == id
                        && r.state == RequestState::Pending
                        && task.live_request == Some(rid),
                    "REQUEST_MISMATCH"
                );
                r.state = RequestState::Rejected;
                task.live_request = None;
                if r.kind == RequestKind::Supplement {
                    task.state = TaskState::Stopped;
                }
                self.requests.insert(rid, r);
            }
            action::OUT_OF_BUDGET => {
                require!(
                    task.state != TaskState::OutOfBudget,
                    "ALREADY_OUT_OF_BUDGET"
                );
                self.remove_live_request(&mut task);
                task.state = TaskState::OutOfBudget;
            }
            action::PAUSE => {
                require!(!task.paused, "TASK_ALREADY_PAUSED");
                task.paused = true;
            }
            action::RECLAIM => {
                require!(
                    task.paused && amount > 0 && task.available >= amount,
                    "INVALID_RECLAIM"
                );
                self.recover_controlled(&mut task, amount);
            }
            action::CONTINUE => {
                require!(
                    task.state == TaskState::Active
                        && task.paused
                        && task.available > 0
                        && task.pending_outflow == 0,
                    "TASK_NOT_CONTINUABLE"
                );
                task.paused = false;
            }
            action::CANCEL => {
                require!(
                    !task.started && task.pending_outflow == 0,
                    "TASK_ALREADY_STARTED"
                );
                self.remove_live_request(&mut task);
                let a = task.available;
                self.recover_controlled(&mut task, a);
                task.state = TaskState::Canceled;
                result = uint(a.try_into().expect("AMOUNT_EXCEEDS_U64"));
            }
            action::RESTORE_REQUEST => {
                require!(
                    task.state == TaskState::Active
                        && task.paused
                        && task.issued > 0
                        && task.live_request.is_none()
                        && amount > 0,
                    "TASK_NOT_RESTORABLE"
                );
                let rid = self.insert_request(
                    id,
                    amount,
                    c.words[11],
                    RequestKind::Restoration,
                    RequestState::Pending,
                );
                task.live_request = Some(rid);
                result = uint(rid);
            }
            _ => unreachable!(),
        }
        self.tasks.insert(id, task);
        result
    }
}
