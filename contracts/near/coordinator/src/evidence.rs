//! Authenticated local reads and official Read MPC evidence consumption.

use crate::{Coordinator, CoordinatorExt, Evidence, Request, Task, identity};
use apodect_near_protocol::foreign::{validate_evidence, validate_funding};
use apodect_near_protocol::{
    Checkpoint, MPC_REQUEST_DEPOSIT_YOCTONEAR, VerificationResponse,
    VerifyForeignTransactionRequest, Word, ZERO, action, command_state, hex, number, plan_state,
    request_state, route_kind, task_state, unhex, word,
};
use near_sdk::{
    AccountId, Gas, NearToken, Promise, env, near, require,
    serde_json::{Value, json},
};

#[near]
impl Coordinator {
    #[payable]
    pub fn observe_near(&mut self, domain: String, observation: String) -> Promise {
        let domain = word(&domain);
        let d = self.domain(domain);
        require!(d.chain == "near", "NEAR_DOMAIN_REQUIRED");
        word(&observation);
        let vault: AccountId = d.vault.parse().unwrap();
        let before = env::storage_usage();
        let id = self.next_evidence;
        self.next_evidence += 1;
        self.evidence.insert(
            id,
            Evidence {
                domain,
                payload: String::new(),
                hash: None,
                done: false,
                funding: false,
            },
        );
        self.charge(before, NearToken::from_millinear(30).as_yoctonear());
        Promise::new(vault)
            .function_call(
                "get_checkpoint",
                json!({"observation": observation}).to_string().into_bytes(),
                NearToken::from_yoctonear(0),
                Gas::from_tgas(15),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(90))
                    .on_near(id),
            )
    }

    #[private]
    pub fn on_near(
        &mut self,
        evidence_id: u64,
        #[callback_result] result: Result<Option<String>, near_sdk::PromiseError>,
    ) -> bool {
        let payload = result
            .expect("NEAR_EVIDENCE_CALL_FAILED")
            .expect("CHECKPOINT_NOT_FOUND");
        let evidence = self.evidence.get(&evidence_id).expect("EVIDENCE_NOT_FOUND");
        require!(!evidence.done, "EVIDENCE_ALREADY_CONSUMED");
        let domain = evidence.domain;
        if evidence.funding {
            self.consume_funding(domain, &payload);
            self.evidence.get_mut(&evidence_id).unwrap().done = true;
            return true;
        }
        let p = Checkpoint::parse(&payload);
        require!(p.words[0] == domain, "EVIDENCE_DOMAIN_MISMATCH");
        self.consume(p);
        self.evidence.get_mut(&evidence_id).unwrap().done = true;
        true
    }

    #[payable]
    pub fn observe_foreign(
        &mut self,
        domain: String,
        payload: String,
        request: Value,
        values: Value,
    ) -> Promise {
        let domain = word(&domain);
        let d = self.domain(domain);
        require!(d.chain != "near", "USE_NEAR_EVIDENCE");
        let p = Checkpoint::parse(&payload);
        require!(p.words[0] == domain, "EVIDENCE_DOMAIN_MISMATCH");
        let expected = validate_evidence(
            &d.chain,
            &d.vault,
            d.program.as_deref(),
            &p,
            &request,
            &values,
        );
        let before = env::storage_usage();
        let id = self.next_evidence;
        self.next_evidence += 1;
        self.evidence.insert(
            id,
            Evidence {
                domain,
                payload,
                hash: Some(expected),
                done: false,
                funding: false,
            },
        );
        let deposit = NearToken::from_yoctonear(MPC_REQUEST_DEPOSIT_YOCTONEAR);
        self.charge(
            before,
            deposit.as_yoctonear() + NearToken::from_millinear(30).as_yoctonear(),
        );
        Promise::new(self.mpc_id.clone())
            .function_call(
                "verify_foreign_transaction",
                json!({
                    "request": VerifyForeignTransactionRequest {
                        request,
                        domain_id: 3,
                        payload_version: 1,
                        expected_payload_hash: hex(&expected),
                    }
                })
                .to_string()
                .into_bytes(),
                deposit,
                Gas::from_tgas(60),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(100))
                    .on_foreign(id),
            )
    }

    #[private]
    pub fn on_foreign(
        &mut self,
        evidence_id: u64,
        #[callback_result] result: Result<VerificationResponse, near_sdk::PromiseError>,
    ) -> bool {
        let result = result.expect("FOREIGN_VERIFICATION_FAILED");
        let evidence = self.evidence.get(&evidence_id).expect("EVIDENCE_NOT_FOUND");
        require!(!evidence.done, "EVIDENCE_ALREADY_CONSUMED");
        let expected = evidence.hash.expect("WRONG_EVIDENCE_KIND");
        require!(
            word(&result.payload_hash) == expected,
            "VERIFICATION_HASH_MISMATCH"
        );
        result.signature.verify(expected, &self.read_key);
        if evidence.funding {
            let domain = evidence.domain;
            let payload = evidence.payload.clone();
            self.consume_funding(domain, &payload);
        } else {
            let p = Checkpoint::parse(&evidence.payload);
            self.consume(p);
        }
        self.evidence.get_mut(&evidence_id).unwrap().done = true;
        true
    }
}

#[near]
impl Coordinator {
    #[payable]
    pub fn observe_funding_near(&mut self, domain: String, funding_id: String) -> Promise {
        let domain = word(&domain);
        let d = self.domain(domain);
        require!(d.chain == "near", "NEAR_DOMAIN_REQUIRED");
        word(&funding_id);
        let vault: AccountId = d.vault.parse().unwrap();
        let before = env::storage_usage();
        let id = self.next_evidence;
        self.next_evidence += 1;
        self.evidence.insert(
            id,
            Evidence {
                domain,
                payload: String::new(),
                hash: None,
                done: false,
                funding: true,
            },
        );
        self.charge(before, NearToken::from_millinear(10).as_yoctonear());
        Promise::new(vault)
            .function_call(
                "get_funding",
                json!({"funding_id": funding_id}).to_string().into_bytes(),
                NearToken::from_yoctonear(0),
                Gas::from_tgas(15),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(60))
                    .on_near(id),
            )
    }

    #[payable]
    pub fn observe_funding_foreign(
        &mut self,
        domain: String,
        payload: String,
        request: Value,
        values: Value,
    ) -> Promise {
        let domain = word(&domain);
        let d = self.domain(domain);
        require!(d.chain != "near", "USE_NEAR_EVIDENCE");
        let expected = validate_funding(
            &d.chain,
            &d.vault,
            d.program.as_deref(),
            &payload,
            &request,
            &values,
        );
        let raw = unhex(&payload);
        require!(raw[..32] == domain, "FUNDING_DOMAIN_MISMATCH");
        let before = env::storage_usage();
        let id = self.next_evidence;
        self.next_evidence += 1;
        self.evidence.insert(
            id,
            Evidence {
                domain,
                payload,
                hash: Some(expected),
                done: false,
                funding: true,
            },
        );
        let deposit = NearToken::from_yoctonear(MPC_REQUEST_DEPOSIT_YOCTONEAR);
        self.charge(
            before,
            deposit.as_yoctonear() + NearToken::from_millinear(10).as_yoctonear(),
        );
        Promise::new(self.mpc_id.clone())
            .function_call(
                "verify_foreign_transaction",
                json!({
                    "request": VerifyForeignTransactionRequest {
                        request,
                        domain_id: 3,
                        payload_version: 1,
                        expected_payload_hash: hex(&expected),
                    }
                })
                .to_string()
                .into_bytes(),
                deposit,
                Gas::from_tgas(60),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(60))
                    .on_foreign(id),
            )
    }
}

impl Coordinator {
    pub(crate) fn consume(&mut self, p: Checkpoint) {
        let domain = p.words[0];
        let sequence = p.value(2);
        require!(sequence > 0 && p.words[1] != ZERO, "INVALID_CHECKPOINT_ID");
        for i in [8, 9, 10, 20, 21, 24, 28, 35, 38] {
            p.value(i);
        }
        for i in [9, 10, 38] {
            require!(p.value(i) <= 1, "NONCANONICAL_BOOLEAN");
        }
        let d = self.domain_mut(domain);
        if sequence > d.sequence {
            d.sequence = sequence;
            d.free = p.value(5);
            d.held = p.value(6);
        }
        let taskkey = identity(domain, p.words[7]);
        if p.value(8) != task_state::MISSING
            && self
                .tasks
                .get(&taskkey)
                .is_none_or(|t| sequence > t.sequence)
        {
            let old_task = self.tasks.get(&taskkey).cloned();
            let control = old_task.as_ref().and_then(|t| t.control);
            let live = (p.value(38) == 1).then_some(p.words[37]);
            if let Some(previous) = old_task.and_then(|task| task.live) {
                if Some(previous) != live || p.value(8) >= task_state::OUT_OF_BUDGET {
                    if let Some(request) = self.requests.get_mut(&identity(domain, previous)) {
                        if request.local_state <= request_state::QUEUED
                            && request.sequence <= sequence
                        {
                            request.local_state = request_state::REMOVED;
                            request.status = "Removed".into();
                            request.sequence = sequence;
                        }
                    }
                }
            }
            self.tasks.insert(
                taskkey,
                Task {
                    sequence,
                    state: p.value(8),
                    paused: p.value(9) == 1,
                    started: p.value(10) == 1,
                    available: p.value(11),
                    issued: p.value(12),
                    pending: p.value(14),
                    nonce: p.value(15),
                    live,
                    control,
                },
            );
        }
        if p.value(21) != request_state::MISSING {
            let key = identity(domain, p.words[17]);
            let old = self.requests.get(&key).cloned();
            if old.as_ref().is_none_or(|r| sequence >= r.sequence) {
                if let Some(ref r) = old {
                    require!(
                        r.task == p.words[18]
                            && r.amount == p.value(19)
                            && r.purpose == p.words[22]
                            && r.kind == p.value(20),
                        "REQUEST_IDENTITY_CHANGED"
                    );
                }
                let mut r = old.unwrap_or(Request {
                    domain,
                    id: p.words[17],
                    task: p.words[18],
                    amount: p.value(19),
                    kind: p.value(20),
                    purpose: p.words[22],
                    sequence,
                    local_state: p.value(21),
                    status: "Pending".into(),
                    plan: None,
                    approval_position: None,
                });
                r.sequence = sequence;
                r.local_state = p.value(21);
                if r.local_state == request_state::FUNDED {
                    r.status = "Funded".into();
                } else if r.local_state >= request_state::REJECTED {
                    r.status = if r.local_state == request_state::REJECTED {
                        "Rejected"
                    } else {
                        "Removed"
                    }
                    .into();
                }
                if r.local_state <= request_state::QUEUED
                    && self
                        .tasks
                        .get(&identity(domain, r.task))
                        .is_some_and(|task| {
                            task.sequence >= sequence
                                && (task.live != Some(r.id)
                                    || task.state >= task_state::OUT_OF_BUDGET)
                        })
                {
                    r.local_state = request_state::REMOVED;
                    r.status = "Removed".into();
                }
                self.requests.insert(key, r);
            }
        }
        if p.words[23] != ZERO && p.value(24) == plan_state::ALLOCATED {
            if let Some(plan) = self.plans.get_mut(&p.words[23]) {
                if plan.destination == domain {
                    if plan.status != "Funded" {
                        plan.status = "Settling".into();
                    }
                    let request = self.requests.get_mut(&plan.request).unwrap();
                    request.status = "Funded".into();
                    request.local_state = request_state::FUNDED;
                }
            }
        }
        if p.words[26] != ZERO && p.value(28) != route_kind::MISSING {
            if let Some(planid) = self.used_routes.get(&p.words[26]).copied() {
                let plan = self.plans.get_mut(&planid).unwrap();
                require!(p.words[27] == planid, "ROUTE_PLAN_MISMATCH");
                let route = plan
                    .routes
                    .iter_mut()
                    .find(|r| word(&r.input.route) == p.words[26])
                    .unwrap();
                if domain == plan.destination {
                    require!(
                        p.value(28) == route_kind::DELIVERY
                            && p.value(29) == route.input.output
                            && p.value(30) <= route.input.output,
                        "DELIVERY_MISMATCH"
                    );
                    route.received = route.received.max(p.value(30));
                } else {
                    require!(
                        domain == word(&route.input.source)
                            && p.value(28) == route_kind::REFUND
                            && p.value(29) == route.input.input
                            && p.value(30) <= route.input.input
                            && p.value(31) <= route.input.input,
                        "REFUND_MISMATCH"
                    );
                    route.returned = route.returned.max(p.value(30));
                    route.dispatched = route.dispatched.max(p.value(31));
                }
            }
        }
        if p.words[32] != ZERO && p.value(35) > 0 {
            let op = p.words[32];
            let mut record = self
                .commands
                .get(&op)
                .cloned()
                .expect("UNRECOGNIZED_COMMAND");
            require!(record.command.words[0] == domain, "COMMAND_DOMAIN_MISMATCH");
            if record.command.action() == action::CLOSE_PLAN
                && p.value(35) == command_state::SUCCESS
            {
                require!(
                    p.words[23] == record.command.plan()
                        && matches!(p.value(24), plan_state::ALLOCATED | plan_state::CLOSED),
                    "CLOSE_REQUIRES_PLAN_STATE"
                );
            }
            let invalidated = p.value(35) == command_state::INVALIDATED
                && p.words[33] == ZERO
                && p.value(34) == 0;
            require!(
                invalidated
                    || (p.words[33] == record.command.digest()
                        && p.value(34) == record.command.action()),
                "COMMAND_EVIDENCE_MISMATCH"
            );
            if sequence > record.sequence
                && !(command_state::is_terminal(record.state)
                    && p.value(35) == command_state::PENDING)
            {
                let old = record.state;
                record.sequence = sequence;
                record.state = p.value(35);
                record.result = p.words[36];
                if matches!(
                    record.command.action(),
                    action::RESERVE | action::OWNER_WITHDRAW
                ) && command_state::is_unresolved(old)
                    && command_state::is_terminal(record.state)
                {
                    let d = self.domain_mut(domain);
                    d.pending_reserves = d
                        .pending_reserves
                        .checked_sub(record.command.amount())
                        .expect("RESERVATION_UNDERFLOW");
                }
                if matches!(
                    record.command.action(),
                    action::OUT_OF_BUDGET..=action::RESTORE_REQUEST
                ) && command_state::is_terminal(record.state)
                {
                    let key = identity(domain, record.command.words[5]);
                    if let Some(t) = self.tasks.get_mut(&key) {
                        if t.control == Some(op) {
                            t.control = None;
                        }
                    }
                }
                if record.state == command_state::SUCCESS && old != command_state::SUCCESS {
                    self.accept_command_effects(&record);
                } else if command_state::is_unresolved(old)
                    && matches!(
                        record.state,
                        command_state::FAILED | command_state::INVALIDATED
                    )
                {
                    self.accept_failed_command(&record);
                }
                self.commands.insert(op, record);
            }
        }
    }

    pub(crate) fn consume_funding(&mut self, domain: Word, payload: &str) {
        let raw = unhex(payload);
        require!(
            raw.len() == 160 && raw[..32] == domain,
            "FUNDING_DOMAIN_MISMATCH"
        );
        let funding_id: Word = raw[32..64].try_into().unwrap();
        require!(funding_id != ZERO, "ZERO_FUNDING_ID");
        let key = identity(domain, funding_id);
        if self.funding.get(&key).is_some() {
            return;
        }
        require!(
            raw[128..] == self.domain(domain).owner_identity,
            "WRONG_FUNDING_OWNER"
        );
        let invoice: Word = raw[64..96].try_into().unwrap();
        let amount = number(raw[96..128].try_into().unwrap());
        require!(amount > 0, "ZERO_FUNDING");
        if invoice != ZERO {
            let invoice = number(invoice);
            let i = self.invoices.get_mut(&invoice).expect("INVOICE_NOT_FOUND");
            require!(i.response.is_some(), "INVOICE_NOT_RESPONDED");
            i.actual = i.actual.checked_add(amount).expect("AMOUNT_OVERFLOW");
        }
        self.funding.insert(key, true);
    }
}
