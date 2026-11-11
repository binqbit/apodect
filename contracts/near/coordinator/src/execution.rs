//! Bounded command generation, reservation progression, closure and reconciliation.

use crate::{CommandRecord, Coordinator, Plan, Request, identity};
use apodect_near_protocol::{
    Command, Word, ZERO, action, command_state, hex, number, request_kind, request_state,
    task_state, uint, word,
};
use near_sdk::{env, require};

impl Coordinator {
    pub(crate) fn command(
        &mut self,
        domain: Word,
        action: u64,
        plan: Word,
        task: Word,
        request: Word,
        amount: u64,
        recipient: Word,
        reference: Word,
        nonce: u64,
        expires: u64,
    ) -> Word {
        self.domain(domain);
        Self::expiry(expires);
        let operation = uint(self.next_command);
        self.next_command += 1;
        let command = Command {
            words: [
                domain,
                uint(nonce),
                uint(action),
                operation,
                plan,
                task,
                request,
                uint(amount),
                ZERO,
                recipient,
                uint(expires),
                reference,
            ],
        };
        command.validate(domain);
        self.commands.insert(
            operation,
            CommandRecord {
                command,
                signature: None,
                signing: false,
                signature_error: None,
                state: command_state::UNOBSERVED,
                result: ZERO,
                sequence: 0,
            },
        );
        operation
    }

    pub(crate) fn reserve(&mut self, domain: Word, plan: Word, amount: u64, expires: u64) -> Word {
        let op = self.command(
            domain,
            action::RESERVE,
            plan,
            ZERO,
            ZERO,
            amount,
            ZERO,
            ZERO,
            0,
            expires,
        );
        let d = self.domain_mut(domain);
        d.pending_reserves = d
            .pending_reserves
            .checked_add(amount)
            .expect("AMOUNT_OVERFLOW");
        op
    }

    pub(crate) fn task_command(
        &mut self,
        domain: Word,
        task: Word,
        action: u64,
        amount: u64,
        purpose: Word,
        expires: u64,
        owner_control: bool,
    ) -> String {
        let key = identity(domain, task);
        let t = self.tasks.get(&key).cloned().expect("TASK_NOT_OBSERVED");
        require!(
            t.state < task_state::COMPLETED && t.control.is_none(),
            "TASK_UNAVAILABLE_OR_CONTROL_PENDING"
        );
        match action {
            action::PAUSE => require!(!t.paused, "ALREADY_PAUSED"),
            action::RECLAIM => require!(
                t.paused && amount > 0 && t.available >= amount,
                "INVALID_RECLAIM"
            ),
            action::CONTINUE => require!(
                t.state == task_state::ACTIVE && t.paused && t.available > 0 && t.pending == 0,
                "NOT_CONTINUABLE"
            ),
            action::CANCEL => require!(!t.started && t.pending == 0, "ALREADY_STARTED"),
            action::RESTORE_REQUEST => require!(
                t.state == task_state::ACTIVE && t.paused && t.issued > 0 && t.live.is_none(),
                "NOT_RESTORABLE"
            ),
            action::OUT_OF_BUDGET => require!(
                t.state != task_state::OUT_OF_BUDGET,
                "ALREADY_OUT_OF_BUDGET"
            ),
            _ => unreachable!(),
        }
        let before = env::storage_usage();
        let op = self.command(
            domain,
            action,
            ZERO,
            task,
            ZERO,
            amount,
            ZERO,
            purpose,
            if owner_control { t.nonce } else { 0 },
            expires,
        );
        self.tasks.get_mut(&key).unwrap().control = Some(op);
        self.charge(before, 0);
        hex(&op)
    }

    pub(crate) fn skip_queue(&mut self) {
        for _ in 0..100 {
            let Some(key) = self.queue.get(self.head) else {
                break;
            };
            if self
                .requests
                .get(key)
                .is_some_and(|r| r.status == "Queued" || r.status == "Reserving")
            {
                break;
            }
            self.head += 1;
        }
    }

    pub(crate) fn request_available(&self, request: &Request) -> bool {
        self.tasks
            .get(&identity(request.domain, request.task))
            .is_some_and(|task| {
                task.control.is_none()
                    && task.live == Some(request.id)
                    && (task.state == task_state::ACTIVE
                        || (task.state == task_state::STOPPED
                            && request.kind == request_kind::RESUME))
            })
    }

    pub(crate) fn succeeded(&self, operation: Word) -> bool {
        self.commands
            .get(&operation)
            .is_some_and(|r| r.state == command_state::SUCCESS)
    }

    pub(crate) fn begin_abort(&mut self, p: &mut Plan) {
        if p.status == "Aborting" {
            return;
        }
        p.status = "Aborting".into();
        let mut ops = p.reserves.iter().map(|(_, op)| *op).collect::<Vec<_>>();
        for route in &p.routes {
            ops.extend(
                [route.delivery, route.refund, route.dispatch]
                    .into_iter()
                    .flatten(),
            );
        }
        ops.extend(p.allocation);
        for op in ops {
            let record = self.commands.get(&op).unwrap();
            if command_state::is_unresolved(record.state) {
                let domain = record.command.words[0];
                let invalidation = self.command(
                    domain,
                    action::INVALIDATE,
                    ZERO,
                    ZERO,
                    ZERO,
                    0,
                    ZERO,
                    op,
                    0,
                    env::block_timestamp_ms() / 1000 + 3600,
                );
                p.invalidations.push(invalidation);
            }
        }
    }

    pub(crate) fn progress(&mut self, p: &mut Plan) {
        if p.status == "Closed" || p.status == "Funded" {
            return;
        }
        if p.status == "Settling"
            || p.allocation
                .is_some_and(|operation| self.succeeded(operation))
        {
            self.settle_allocated(p);
            return;
        }
        if self
            .requests
            .get(&p.request)
            .is_some_and(|request| request.local_state >= request_state::REJECTED)
        {
            self.begin_abort(p);
        }
        if p.status == "Aborting" {
            if !p.invalidations.iter().all(|id| {
                self.commands.get(id).is_some_and(|record| {
                    record.state == command_state::SUCCESS
                        && (command_state::is_terminal(number(record.result))
                            || (number(record.result) == command_state::PENDING
                                && self.commands.get(&record.command.words[11]).is_some_and(
                                    |target| command_state::is_terminal(target.state),
                                )))
                })
            }) {
                return;
            }
            if p.closures.is_empty() {
                if p.reserves.iter().any(|(_, op)| {
                    self.commands
                        .get(op)
                        .is_some_and(|record| command_state::is_unresolved(record.state))
                }) {
                    return;
                }
                for (domain, reserve) in &p.reserves {
                    let r = self.commands.get(reserve).unwrap();
                    if r.state == command_state::SUCCESS {
                        p.closures.push(self.command(
                            *domain,
                            action::CLOSE_PLAN,
                            p.id,
                            ZERO,
                            ZERO,
                            0,
                            ZERO,
                            ZERO,
                            0,
                            env::block_timestamp_ms() / 1000 + 3600,
                        ));
                    } else if r.state == command_state::UNOBSERVED
                        || r.state == command_state::PENDING
                    {
                        return;
                    }
                }
            }
            if p.closures.iter().all(|id| self.succeeded(*id)) {
                p.status = "Closed".into();
                let r = self.requests.get_mut(&p.request).unwrap();
                if r.local_state != request_state::FUNDED {
                    r.status = "Removed".into();
                }
                if !p.queue_released {
                    p.queue_released = true;
                    self.skip_queue();
                }
            }
            return;
        }
        if p.attempts >= self.retry_limit {
            return;
        }
        if p.status == "Reserving" {
            if !p.reserves.iter().all(|(_, id)| self.succeeded(*id)) {
                return;
            }
            p.queue_released = true;
            self.requests.get_mut(&p.request).unwrap().status = "Reserved".into();
            self.skip_queue();
            for route in &mut p.routes {
                let id = word(&route.input.route);
                route.delivery = Some(self.command(
                    p.destination,
                    action::EXPECT_DELIVERY,
                    p.id,
                    ZERO,
                    ZERO,
                    route.input.output,
                    ZERO,
                    id,
                    0,
                    p.expires,
                ));
                route.refund = Some(self.command(
                    word(&route.input.source),
                    action::EXPECT_REFUND,
                    p.id,
                    ZERO,
                    ZERO,
                    route.input.input,
                    ZERO,
                    id,
                    0,
                    p.expires,
                ));
            }
            p.status = "Preparing".into();
        }
        if p.status == "Preparing" {
            if !p
                .routes
                .iter()
                .all(|r| self.succeeded(r.delivery.unwrap()) && self.succeeded(r.refund.unwrap()))
            {
                return;
            }
            for r in &mut p.routes {
                r.dispatch = Some(self.command(
                    word(&r.input.source),
                    action::DISPATCH,
                    p.id,
                    ZERO,
                    ZERO,
                    r.input.input,
                    word(&r.input.deposit),
                    word(&r.input.route),
                    0,
                    p.expires,
                ));
            }
            p.status = "Delivering".into();
        }
        if p.status == "Delivering" {
            if !p.routes.iter().all(|r| {
                r.received == r.input.output
                    && r.dispatched == r.input.input
                    && self.succeeded(r.dispatch.unwrap())
            }) {
                return;
            }
            let r = self.requests.get(&p.request).unwrap();
            p.allocation = Some(self.command(
                p.destination,
                action::ALLOCATE,
                p.id,
                r.task,
                r.id,
                p.principal,
                ZERO,
                ZERO,
                0,
                p.expires,
            ));
            p.status = "Allocating".into();
        }
    }

    /// Agent rights are already granted; close every vault hold before declaring the plan settled.
    fn settle_allocated(&mut self, plan: &mut Plan) {
        plan.status = "Settling".into();
        let request = self.requests.get_mut(&plan.request).unwrap();
        request.status = "Funded".into();
        request.local_state = request_state::FUNDED;
        if plan.closures.is_empty() {
            for (domain, _) in &plan.reserves {
                let operation = self.command(
                    *domain,
                    action::CLOSE_PLAN,
                    plan.id,
                    ZERO,
                    ZERO,
                    0,
                    ZERO,
                    ZERO,
                    0,
                    env::block_timestamp_ms() / 1000 + 3600,
                );
                plan.closures.push(operation);
            }
        }
        if plan
            .closures
            .iter()
            .all(|operation| self.succeeded(*operation))
        {
            plan.status = "Funded".into();
        }
    }

    pub(crate) fn accept_command_effects(&mut self, record: &CommandRecord) {
        let c = &record.command;
        let domain = c.words[0];
        match c.action() {
            action::REJECT => {
                if let Some(r) = self.requests.get_mut(&identity(domain, c.words[6])) {
                    r.status = "Rejected".into();
                    r.local_state = request_state::REJECTED;
                }
                self.skip_queue();
            }
            action::PAUSE..=action::CANCEL => {
                if let Some(t) = self.tasks.get_mut(&identity(domain, c.words[5])) {
                    t.nonce = t.nonce.max(number(c.words[1]) + 1);
                }
            }
            action::RESTORE_REQUEST => {
                let id = record.result;
                let key = identity(domain, id);
                if self
                    .requests
                    .get(&key)
                    .is_none_or(|r| r.status == "Pending")
                {
                    self.requests.insert(
                        key,
                        Request {
                            domain,
                            id,
                            task: c.words[5],
                            amount: c.amount(),
                            kind: request_kind::RESTORATION,
                            purpose: c.words[11],
                            sequence: record.sequence,
                            local_state: request_state::PENDING,
                            status: "Queued".into(),
                            plan: None,
                            approval_position: Some(self.queue.len()),
                        },
                    );
                    self.queue.push(key);
                }
            }
            action::INVALIDATE => {
                if matches!(
                    number(record.result),
                    command_state::FAILED | command_state::INVALIDATED
                ) {
                    let target = c.words[11];
                    let mut r = self
                        .commands
                        .get(&target)
                        .cloned()
                        .expect("INVALIDATED_COMMAND_MISSING");
                    if command_state::is_unresolved(r.state) {
                        r.state = number(record.result);
                        r.sequence = record.sequence;
                        if matches!(r.command.action(), action::RESERVE | action::OWNER_WITHDRAW) {
                            let d = self.domain_mut(domain);
                            d.pending_reserves = d
                                .pending_reserves
                                .checked_sub(r.command.amount())
                                .expect("RESERVATION_UNDERFLOW");
                        }
                        if matches!(
                            r.command.action(),
                            action::OUT_OF_BUDGET..=action::RESTORE_REQUEST
                        ) {
                            if let Some(t) =
                                self.tasks.get_mut(&identity(domain, r.command.words[5]))
                            {
                                if t.control == Some(target) {
                                    t.control = None;
                                }
                            }
                        }
                        self.commands.insert(target, r);
                    }
                }
            }
            _ => {}
        }
    }
}
