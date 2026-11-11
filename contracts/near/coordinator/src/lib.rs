//! Project-wide authority, FIFO and constrained cross-chain command state machine.
//! Relayers transport immutable records; they cannot select arbitrary signing payloads.

mod evidence;
mod execution;

use apodect_near_protocol::{
    Command, SignPayload, SignRequest, SignatureResponse, Word, ZERO, action, command_state, hash,
    hex, number, request_state, uint, unhex, word,
};
use near_sdk::store::{LookupMap, Vector};
use near_sdk::{
    AccountId, Gas, NearToken, PanicOnDefault, Promise, env, near, require,
    serde_json::{Value, json},
};

#[near(serializers = [borsh])]
#[derive(Clone)]
struct Domain {
    id: Word,
    chain: String,
    vault: String,
    asset: String,
    program: Option<String>,
    owner_recipient: Word,
    owner_identity: Word,
    sequence: u64,
    free: u64,
    held: u64,
    pending_reserves: u64,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct Request {
    domain: Word,
    id: Word,
    task: Word,
    amount: u64,
    kind: u64,
    purpose: Word,
    sequence: u64,
    local_state: u64,
    status: String,
    plan: Option<Word>,
    approval_position: Option<u32>,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct Task {
    sequence: u64,
    state: u64,
    paused: bool,
    started: bool,
    available: u64,
    issued: u64,
    pending: u64,
    nonce: u64,
    live: Option<Word>,
    control: Option<Word>,
}

#[near(serializers = [json, borsh])]
#[derive(Clone)]
pub struct RouteInput {
    pub route: String,
    pub source: String,
    pub input: u64,
    pub min_input: u64,
    pub output: u64,
    pub deposit: String,
    pub refund: String,
    pub recipient: String,
    pub quote_hash: String,
    pub expires: u64,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct Route {
    input: RouteInput,
    delivery: Option<Word>,
    refund: Option<Word>,
    dispatch: Option<Word>,
    received: u64,
    returned: u64,
    dispatched: u64,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct Plan {
    id: Word,
    request: Word,
    destination: Word,
    principal: u64,
    local: u64,
    expires: u64,
    status: String,
    reserves: Vec<(Word, Word)>,
    routes: Vec<Route>,
    allocation: Option<Word>,
    invalidations: Vec<Word>,
    closures: Vec<Word>,
    attempts: u32,
    error: Option<String>,
    queue_released: bool,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct CommandRecord {
    command: Command,
    signature: Option<SignatureResponse>,
    signing: bool,
    signature_error: Option<String>,
    state: u64,
    result: Word,
    sequence: u64,
}

#[near(serializers = [borsh])]
struct Evidence {
    domain: Word,
    payload: String,
    hash: Option<Word>,
    done: bool,
    funding: bool,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct Invoice {
    amount: u64,
    purpose: Word,
    response: Option<u64>,
    response_reference: Option<Word>,
    actual: u64,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Coordinator {
    owner_id: AccountId,
    authority_id: AccountId,
    project_id: Word,
    mpc_id: AccountId,
    sign_key: Vec<u8>,
    read_key: Vec<u8>,
    sign_path: String,
    domains: Vec<Domain>,
    requests: LookupMap<Word, Request>,
    tasks: LookupMap<Word, Task>,
    plans: LookupMap<Word, Plan>,
    commands: LookupMap<Word, CommandRecord>,
    evidence: LookupMap<u64, Evidence>,
    used_routes: LookupMap<Word, Word>,
    queue: Vector<Word>,
    head: u32,
    next_plan: u64,
    next_command: u64,
    next_evidence: u64,
    retry_limit: u32,
    invoices: LookupMap<u64, Invoice>,
    next_invoice: u64,
    funding: LookupMap<Word, bool>,
}

#[near]
impl Coordinator {
    #[init]
    pub fn new(
        owner_id: AccountId,
        authority_id: AccountId,
        project_id: String,
        mpc_id: AccountId,
        sign_key: String,
        read_key: String,
        sign_path: String,
    ) -> Self {
        let sign_key = unhex(&sign_key);
        let read_key = unhex(&read_key);
        require!(
            sign_key.len() == 64 && read_key.len() == 64 && !sign_path.is_empty(),
            "INVALID_PROVIDER_CONFIG"
        );
        Self {
            owner_id,
            authority_id,
            project_id: word(&project_id),
            mpc_id,
            sign_key,
            read_key,
            sign_path,
            domains: vec![],
            requests: LookupMap::new(b"r"),
            tasks: LookupMap::new(b"t"),
            plans: LookupMap::new(b"p"),
            commands: LookupMap::new(b"c"),
            evidence: LookupMap::new(b"e"),
            used_routes: LookupMap::new(b"u"),
            queue: Vector::new(b"q"),
            head: 0,
            next_plan: 1,
            next_command: 1,
            next_evidence: 0,
            retry_limit: 3,
            invoices: LookupMap::new(b"i"),
            next_invoice: 1,
            funding: LookupMap::new(b"f"),
        }
    }

    #[payable]
    pub fn register_domain(
        &mut self,
        domain: String,
        chain: String,
        vault: String,
        asset: String,
        owner_recipient: String,
        owner_identity: String,
        program: Option<String>,
    ) {
        self.owner();
        require!(self.domains.len() < 3, "THREE_DOMAINS_MAXIMUM");
        require!(
            matches!(chain.as_str(), "near" | "ethereum" | "solana"),
            "UNSUPPORTED_CHAIN"
        );
        let id = word(&domain);
        require!(
            id != ZERO && !self.domains.iter().any(|d| d.id == id || d.chain == chain),
            "DOMAIN_EXISTS"
        );
        if chain == "near" {
            vault.parse::<AccountId>().expect("INVALID_NEAR_VAULT");
            asset.parse::<AccountId>().expect("INVALID_NEAR_ASSET");
        }
        if chain == "solana" {
            word(program.as_deref().expect("SOLANA_PROGRAM_REQUIRED"));
        }
        let before = env::storage_usage();
        self.domains.push(Domain {
            id,
            chain,
            vault,
            asset,
            program,
            owner_recipient: word(&owner_recipient),
            owner_identity: word(&owner_identity),
            sequence: 0,
            free: 0,
            held: 0,
            pending_reserves: 0,
        });
        self.charge(before, 0);
    }

    #[payable]
    pub fn set_authority(&mut self, authority_id: AccountId) {
        self.owner();
        let before = env::storage_usage();
        self.authority_id = authority_id;
        self.charge(before, 0);
    }

    #[payable]
    pub fn set_retry_limit(&mut self, attempts: u32) {
        self.owner();
        require!((1..=20).contains(&attempts), "INVALID_RETRY_LIMIT");
        let before = env::storage_usage();
        self.retry_limit = attempts;
        self.charge(before, 0);
    }

    #[payable]
    pub fn approve(&mut self, domain: String, request: String) {
        self.authority();
        let key = identity(word(&domain), word(&request));
        let mut r = self
            .requests
            .get(&key)
            .cloned()
            .expect("REQUEST_NOT_OBSERVED");
        require!(
            r.status == "Pending" && r.local_state == request_state::PENDING,
            "REQUEST_NOT_PENDING"
        );
        require!(self.request_available(&r), "TASK_REQUEST_NOT_AVAILABLE");
        let before = env::storage_usage();
        r.status = "Queued".into();
        r.approval_position = Some(self.queue.len());
        self.requests.insert(key, r);
        self.queue.push(key);
        self.charge(before, 0);
    }

    #[payable]
    pub fn reject(&mut self, domain: String, request: String, expires: u64) -> String {
        self.authority();
        let key = identity(word(&domain), word(&request));
        let mut r = self
            .requests
            .get(&key)
            .cloned()
            .expect("REQUEST_NOT_OBSERVED");
        require!(
            matches!(r.status.as_str(), "Pending" | "Queued")
                && r.local_state == request_state::PENDING,
            "REQUEST_NOT_REJECTABLE"
        );
        let before = env::storage_usage();
        let op = self.command(
            r.domain,
            action::REJECT,
            ZERO,
            r.task,
            r.id,
            0,
            ZERO,
            ZERO,
            0,
            expires,
        );
        r.status = "Rejecting".into();
        self.requests.insert(key, r);
        self.charge(before, 0);
        hex(&op)
    }

    #[payable]
    pub fn apply(
        &mut self,
        domain: String,
        request: String,
        local_amount: u64,
        routes: Vec<RouteInput>,
        expires: u64,
    ) -> String {
        self.authority();
        self.skip_queue();
        let destination = word(&domain);
        let key = identity(destination, word(&request));
        let mut req = self
            .requests
            .get(&key)
            .cloned()
            .expect("REQUEST_NOT_OBSERVED");
        require!(
            req.status == "Queued" && self.queue.get(self.head) == Some(&key),
            "NOT_FIFO_HEAD"
        );
        require!(
            req.local_state == request_state::PENDING && routes.len() <= 2,
            "INVALID_REQUEST_OR_ROUTES"
        );
        require!(self.request_available(&req), "TASK_REQUEST_NOT_AVAILABLE");
        Self::expiry(expires);
        let mut total = local_amount;
        let mut sources = vec![destination];
        let mut route_ids = vec![];
        require!(
            self.available(destination) >= local_amount,
            "INSUFFICIENT_DESTINATION_FUNDS"
        );
        for route in &routes {
            let source = word(&route.source);
            self.domain(source);
            let id = word(&route.route);
            require!(
                id != ZERO && !route_ids.contains(&id) && self.used_routes.get(&id).is_none(),
                "ROUTE_ID_REUSED"
            );
            route_ids.push(id);
            require!(!sources.contains(&source), "DUPLICATE_SOURCE_DOMAIN");
            sources.push(source);
            require!(
                route.input > 0
                    && route.output > 0
                    && route.input >= route.min_input
                    && route.min_input >= route.output,
                "INVALID_ROUTE_AMOUNTS"
            );
            require!(
                self.available(source) >= route.input,
                "INSUFFICIENT_SOURCE_FUNDS_INCLUDING_FEES"
            );
            require!(
                word(&route.deposit) != ZERO
                    && word(&route.quote_hash) != ZERO
                    && !route.refund.is_empty()
                    && !route.recipient.is_empty(),
                "INVALID_QUOTE_BINDING"
            );
            Self::expiry(route.expires);
            require!(route.expires >= expires, "QUOTE_EXPIRES_BEFORE_COMMANDS");
            self.check_near_receiver(source, id, &route.refund);
            self.check_near_receiver(destination, id, &route.recipient);
            total = total.checked_add(route.output).expect("AMOUNT_OVERFLOW");
        }
        require!(total == req.amount, "PRINCIPAL_MUST_BE_FUNDED_IN_FULL");
        let before = env::storage_usage();
        let id = uint(self.next_plan);
        self.next_plan += 1;
        let destination_reserve = self.reserve(destination, id, local_amount, expires);
        let mut reserves = vec![(destination, destination_reserve)];
        let mut stored_routes = vec![];
        for input in routes {
            let source = word(&input.source);
            let op = self.reserve(source, id, input.input, expires);
            reserves.push((source, op));
            self.used_routes.insert(word(&input.route), id);
            stored_routes.push(Route {
                input,
                delivery: None,
                refund: None,
                dispatch: None,
                received: 0,
                returned: 0,
                dispatched: 0,
            });
        }
        self.plans.insert(
            id,
            Plan {
                id,
                request: key,
                destination,
                principal: req.amount,
                local: local_amount,
                expires,
                status: "Reserving".into(),
                reserves,
                routes: stored_routes,
                allocation: None,
                invalidations: vec![],
                closures: vec![],
                attempts: 0,
                error: None,
                queue_released: false,
            },
        );
        req.status = "Reserving".into();
        req.plan = Some(id);
        self.requests.insert(key, req);
        self.charge(before, 0);
        hex(&id)
    }

    #[payable]
    pub fn advance(&mut self, plan: String) -> Value {
        let id = word(&plan);
        let mut p = self.plans.get(&id).cloned().expect("PLAN_NOT_FOUND");
        let before = env::storage_usage();
        self.progress(&mut p);
        self.plans.insert(id, p);
        self.charge(before, 0);
        self.get_plan(plan)
    }

    /// Persist bounded removal of queue tombstones without attempting a financial action.
    pub fn advance_queue(&mut self) -> u32 {
        self.skip_queue();
        self.head
    }

    #[payable]
    pub fn abort(&mut self, plan: String) {
        require!(
            env::predecessor_account_id() == self.owner_id
                || env::predecessor_account_id() == self.authority_id,
            "ONLY_OWNER_OR_AUTHORITY"
        );
        let id = word(&plan);
        let mut p = self.plans.get(&id).cloned().expect("PLAN_NOT_FOUND");
        require!(
            !matches!(p.status.as_str(), "Funded" | "Closed"),
            "PLAN_TERMINAL"
        );
        let before = env::storage_usage();
        self.begin_abort(&mut p);
        self.progress(&mut p);
        self.plans.insert(id, p);
        self.charge(before, 0);
    }

    #[payable]
    pub fn retry(&mut self, plan: String, error: Option<String>, unverified: Option<String>) {
        self.authority();
        let id = word(&plan);
        let mut p = self.plans.get(&id).cloned().expect("PLAN_NOT_FOUND");
        require!(
            !matches!(p.status.as_str(), "Funded" | "Closed"),
            "PLAN_TERMINAL"
        );
        let before = env::storage_usage();
        p.attempts = p.attempts.checked_add(1).expect("RETRY_OVERFLOW");
        let error = error.unwrap_or_else(|| "Observation remains unconfirmed".into());
        let unverified =
            unverified.unwrap_or_else(|| "Immutable command or transfer outcome".into());
        require!(
            error.len() <= 512 && unverified.len() <= 512,
            "RETRY_CONTEXT_TOO_LONG"
        );
        p.error = Some(format!("{error}; unverified: {unverified}"));
        self.plans.insert(id, p);
        self.charge(before, 0);
    }

    /// Reconcile a command that cannot execute; tombstoning never guesses its outcome.
    #[payable]
    pub fn invalidate(&mut self, operation: String, expires: u64) -> String {
        let op = word(&operation);
        let record = self.commands.get(&op).expect("COMMAND_NOT_FOUND");
        let caller = env::predecessor_account_id();
        require!(
            caller == self.owner_id || caller == self.authority_id,
            "ONLY_OWNER_OR_AUTHORITY"
        );
        require!(
            command_state::is_unresolved(record.state),
            "COMMAND_ALREADY_SETTLED"
        );
        let domain = record.command.words[0];
        let before = env::storage_usage();
        let id = self.command(
            domain,
            action::INVALIDATE,
            ZERO,
            ZERO,
            ZERO,
            0,
            ZERO,
            op,
            0,
            expires,
        );
        self.charge(before, 0);
        hex(&id)
    }

    /// Reuse approval after closure and an actual refund; unresolved old cash stays unknown.
    #[payable]
    pub fn requeue(&mut self, plan: String) {
        self.authority();
        let id = word(&plan);
        let p = self.plans.get(&id).cloned().expect("PLAN_NOT_FOUND");
        require!(p.status == "Closed", "PLAN_NOT_CLOSED");
        for route in &p.routes {
            if let Some(op) = route.dispatch {
                let command = self.commands.get(&op).unwrap();
                require!(
                    command_state::is_terminal(command.state),
                    "DISPATCH_STILL_UNKNOWN"
                );
                require!(
                    command.state != command_state::SUCCESS || route.returned > 0,
                    "NO_ACTUAL_SOURCE_REFUND"
                );
            }
        }
        let mut r = self.requests.get(&p.request).cloned().unwrap();
        require!(
            r.local_state == request_state::PENDING,
            "LOCAL_REQUEST_NOT_PENDING"
        );
        require!(r.plan == Some(id), "REQUEST_ALREADY_REPLANNED");
        let before = env::storage_usage();
        r.status = "Queued".into();
        r.plan = None;
        self.requests.insert(p.request, r);
        let position = self
            .requests
            .get(&p.request)
            .unwrap()
            .approval_position
            .expect("APPROVAL_POSITION_MISSING");
        self.head = self.head.min(position);
        self.skip_queue();
        self.charge(before, 0);
    }

    #[payable]
    pub fn resume_plan(&mut self, plan: String) {
        self.authority();
        let before = env::storage_usage();
        let id = word(&plan);
        let mut p = self.plans.get(&id).cloned().expect("PLAN_NOT_FOUND");
        require!(
            !matches!(p.status.as_str(), "Funded" | "Closed"),
            "PLAN_TERMINAL"
        );
        // A terminal failed/invalidation receipt proves this command cannot
        // close the hold later. Expiry alone never establishes nonexecution.
        // Replacement is an explicit authority action, rather than an
        // unbounded automatic financial retry; old commands remain immutable.
        for operation in &mut p.closures {
            let previous = self.commands.get(operation).expect("COMMAND_NOT_FOUND");
            if matches!(
                previous.state,
                command_state::FAILED | command_state::INVALIDATED
            ) {
                *operation = self.command(
                    previous.command.words[0],
                    action::CLOSE_PLAN,
                    id,
                    ZERO,
                    ZERO,
                    0,
                    ZERO,
                    ZERO,
                    0,
                    env::block_timestamp_ms() / 1000 + 3600,
                );
            }
        }
        p.attempts = 0;
        p.error = None;
        self.plans.insert(id, p);
        self.charge(before, 0);
    }

    #[payable]
    pub fn control(
        &mut self,
        domain: String,
        task: String,
        action: u64,
        amount: u64,
        expires: u64,
    ) -> String {
        self.owner();
        require!(
            (action::PAUSE..=action::CANCEL).contains(&action),
            "INVALID_OWNER_ACTION"
        );
        require!(action == action::RECLAIM || amount == 0, "UNUSED_AMOUNT");
        self.task_command(
            word(&domain),
            word(&task),
            action,
            amount,
            ZERO,
            expires,
            true,
        )
    }

    #[payable]
    pub fn restore(
        &mut self,
        domain: String,
        task: String,
        amount: u64,
        purpose: String,
        expires: u64,
    ) -> String {
        self.authority();
        require!(amount > 0 && word(&purpose) != ZERO, "INVALID_RESTORATION");
        self.task_command(
            word(&domain),
            word(&task),
            action::RESTORE_REQUEST,
            amount,
            word(&purpose),
            expires,
            false,
        )
    }

    #[payable]
    pub fn out_of_budget(&mut self, domain: String, task: String, expires: u64) -> String {
        self.authority();
        self.task_command(
            word(&domain),
            word(&task),
            action::OUT_OF_BUDGET,
            0,
            ZERO,
            expires,
            false,
        )
    }

    #[payable]
    pub fn withdraw_free(&mut self, domain: String, amount: u64, expires: u64) -> String {
        self.owner();
        let id = word(&domain);
        let recipient = self.domain(id).owner_recipient;
        require!(
            amount > 0 && self.available(id) >= amount,
            "INSUFFICIENT_FREE_FUNDS"
        );
        let before = env::storage_usage();
        let op = self.command(
            id,
            action::OWNER_WITHDRAW,
            ZERO,
            ZERO,
            ZERO,
            amount,
            recipient,
            ZERO,
            0,
            expires,
        );
        self.domain_mut(id).pending_reserves = self
            .domain(id)
            .pending_reserves
            .checked_add(amount)
            .expect("AMOUNT_OVERFLOW");
        self.charge(before, 0);
        hex(&op)
    }

    #[payable]
    pub fn request_signature(&mut self, operation: String) -> Promise {
        let op = word(&operation);
        let record = self.commands.get(&op).expect("COMMAND_NOT_FOUND");
        require!(
            record.state == command_state::UNOBSERVED
                && !record.signing
                && record.signature.is_none(),
            "COMMAND_NOT_SIGNABLE"
        );
        Self::expiry(number(record.command.words[10]));
        let digest = record.command.digest();
        let before = env::storage_usage();
        self.commands.get_mut(&op).unwrap().signing = true;
        self.commands.get_mut(&op).unwrap().signature_error = None;
        let deposit = NearToken::from_millinear(10);
        self.charge(
            before,
            deposit.as_yoctonear() + NearToken::from_millinear(10).as_yoctonear(),
        );
        Promise::new(self.mpc_id.clone())
            .function_call(
                "sign",
                json!({
                    "request": SignRequest {
                        path: self.sign_path.clone(),
                        domain_id: 0,
                        payload_v2: SignPayload::Ecdsa(hex(&digest)),
                    }
                })
                .to_string()
                .into_bytes(),
                deposit,
                Gas::from_tgas(60),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(35))
                    .on_signature(operation),
            )
    }

    #[private]
    pub fn on_signature(&mut self, operation: String) -> bool {
        let op = word(&operation);
        let r = self.commands.get_mut(&op).expect("COMMAND_NOT_FOUND");
        r.signing = false;
        let Ok(bytes) = env::promise_result_checked(0, 1024) else {
            r.signature_error = Some("SIGNATURE_PROVIDER_FAILED".into());
            return false;
        };
        let Ok(sig) = near_sdk::serde_json::from_slice::<SignatureResponse>(&bytes) else {
            r.signature_error = Some("MALFORMED_SIGNATURE_RESPONSE".into());
            return false;
        };
        if !sig.is_valid(r.command.digest(), &self.sign_key) {
            r.signature_error = Some("INVALID_PROVIDER_SIGNATURE".into());
            return false;
        }
        r.signature_error = None;
        r.signature = Some(sig);
        true
    }

    #[payable]
    pub fn open_invoice(&mut self, amount: u64, purpose: String) -> u64 {
        self.authority();
        require!(amount > 0, "ZERO_AMOUNT");
        let purpose = word(&purpose);
        require!(purpose != ZERO, "ZERO_PURPOSE");
        let before = env::storage_usage();
        let id = self.next_invoice;
        self.next_invoice += 1;
        self.invoices.insert(
            id,
            Invoice {
                amount,
                purpose,
                response: None,
                response_reference: None,
                actual: 0,
            },
        );
        self.charge(before, 0);
        id
    }

    #[payable]
    pub fn respond_invoice(&mut self, invoice: u64, intended: u64, reference: Option<String>) {
        self.owner();
        let before = env::storage_usage();
        let i = self.invoices.get_mut(&invoice).expect("INVOICE_NOT_FOUND");
        require!(i.response.is_none(), "INVOICE_ALREADY_RESPONDED");
        i.response = Some(intended);
        i.response_reference = reference.map(|v| word(&v));
        self.charge(before, 0);
    }

    pub fn get_invoice(&self, invoice: u64) -> Value {
        self.invoices
            .get(&invoice)
            .map(|invoice| {
                json!({
                    "amount": invoice.amount,
                    "purpose": hex(&invoice.purpose),
                    "intended": invoice.response,
                    "closed": invoice.response.is_some(),
                    "response_reference": invoice.response_reference.map(|value| hex(&value)),
                    "actual_funding": invoice.actual,
                })
            })
            .unwrap_or_default()
    }

    pub fn get_funding_seen(&self, domain: String, funding_id: String) -> bool {
        self.funding
            .get(&identity(word(&domain), word(&funding_id)))
            .is_some()
    }

    pub fn get_state(&self) -> Value {
        json!({
            "owner_id": self.owner_id,
            "authority_id": self.authority_id,
            "project_id": hex(&self.project_id),
            "mpc_id": self.mpc_id,
            "sign_key": hex(&self.sign_key),
            "read_key": hex(&self.read_key),
            "sign_path": self.sign_path,
            "queue_head": self.head,
            "queue_length": self.queue.len(),
            "next_plan": self.next_plan,
            "next_command": self.next_command,
            "next_evidence": self.next_evidence,
            "retry_limit": self.retry_limit,
            "next_invoice": self.next_invoice,
        })
    }

    pub fn get_domain(&self, domain: String) -> Value {
        let domain = self.domain(word(&domain));
        json!({
            "domain": hex(&domain.id),
            "chain": domain.chain,
            "vault": domain.vault,
            "asset": domain.asset,
            "program": domain.program,
            "owner_identity": hex(&domain.owner_identity),
            "owner_recipient": hex(&domain.owner_recipient),
            "sequence": domain.sequence,
            "free": domain.free,
            "held": domain.held,
            "pending_reserves": domain.pending_reserves,
            "available": domain.free.saturating_sub(domain.pending_reserves),
        })
    }

    pub fn get_request(&self, domain: String, request: String) -> Value {
        self.requests
            .get(&identity(word(&domain), word(&request)))
            .map(|request| {
                json!({
                    "domain": hex(&request.domain),
                    "request": hex(&request.id),
                    "task": hex(&request.task),
                    "amount": request.amount,
                    "kind": request.kind,
                    "purpose": hex(&request.purpose),
                    "local_state": request.local_state,
                    "status": request.status,
                    "plan": request.plan.map(|plan| hex(&plan)),
                    "approval_position": request.approval_position,
                })
            })
            .unwrap_or_default()
    }

    pub fn get_queue_head(&self) -> Value {
        self.queue
            .get(self.head)
            .and_then(|key| self.requests.get(key))
            .map(|request| self.get_request(hex(&request.domain), hex(&request.id)))
            .unwrap_or_default()
    }

    pub fn get_task(&self, domain: String, task: String) -> Value {
        self.tasks
            .get(&identity(word(&domain), word(&task)))
            .map(|task| {
                json!({
                    "sequence": task.sequence,
                    "state": task.state,
                    "paused": task.paused,
                    "started": task.started,
                    "available": task.available,
                    "issued": task.issued,
                    "pending_outflow": task.pending,
                    "control_nonce": task.nonce,
                    "live_request": task.live.map(|id| hex(&id)),
                    "control": task.control.map(|id| hex(&id)),
                })
            })
            .unwrap_or_default()
    }

    pub fn get_plan(&self, plan: String) -> Value {
        let Some(plan) = self.plans.get(&word(&plan)) else {
            return Value::Null;
        };
        let reserves: Vec<_> = plan
            .reserves
            .iter()
            .map(|(domain, operation)| {
                json!({
                    "domain": hex(domain),
                    "operation": hex(operation)
                })
            })
            .collect();
        let routes: Vec<_> = plan
            .routes
            .iter()
            .map(|route| {
                let buffer = route.input.input - route.input.min_input;
                let settled = route.received == route.input.output
                    && route.dispatched == route.input.input
                    && route.returned >= buffer;
                json!({
                    "quote": route.input,
                    "delivery": route.delivery.map(|value| hex(&value)),
                    "refund": route.refund.map(|value| hex(&value)),
                    "dispatch": route.dispatch.map(|value| hex(&value)),
                    "received": route.received,
                    "returned": route.returned,
                    "dispatched": route.dispatched,
                    "unsettled_input": if settled {
                        0
                    } else {
                        route.dispatched.saturating_sub(route.returned)
                    },
                    "expected_buffer_refund": buffer,
                    "confirmed_transport_expense": settled.then(|| route.input.input
                        .saturating_sub(route.input.output)
                        .saturating_sub(route.returned)),
                })
            })
            .collect();
        json!({
            "plan": hex(&plan.id),
            "destination": hex(&plan.destination),
            "principal": plan.principal,
            "local_amount": plan.local,
            "expires": plan.expires,
            "status": plan.status,
            "reserves": reserves,
            "routes": routes,
            "allocation": plan.allocation.map(|value| hex(&value)),
            "invalidations": plan.invalidations.iter().map(|value| hex(value)).collect::<Vec<_>>(),
            "closures": plan.closures.iter().map(|value| hex(value)).collect::<Vec<_>>(),
            "attempts": plan.attempts,
            "requires_attention": plan.attempts >= self.retry_limit,
            "error": plan.error,
            "queue_released": plan.queue_released,
        })
    }

    pub fn get_command(&self, operation: String) -> Value {
        self.commands
            .get(&word(&operation))
            .map(|record| {
                json!({
                    "operation": hex(&record.command.operation()),
                    "domain": hex(&record.command.words[0]),
                    "command": record.command.hex(),
                    "digest": hex(&record.command.digest()),
                    "action": record.command.action(),
                    "plan": hex(&record.command.plan()),
                    "signature": record.signature,
                    "signing": record.signing,
                    "signature_error": record.signature_error,
                    "state": record.state,
                    "result": hex(&record.result),
                })
            })
            .unwrap_or_default()
    }

    pub fn list_commands(&self, from: Option<u64>, limit: Option<u64>) -> Vec<Value> {
        (from.unwrap_or(1).max(1)..self.next_command)
            .take(limit.unwrap_or(50).min(100) as usize)
            .map(|id| self.get_command(hex(&uint(id))))
            .collect()
    }
}

fn identity(domain: Word, id: Word) -> Word {
    hash(&[domain.as_slice(), id.as_slice()].concat())
}

impl Coordinator {
    fn owner(&self) {
        require!(env::predecessor_account_id() == self.owner_id, "ONLY_OWNER");
    }

    fn authority(&self) {
        require!(
            env::predecessor_account_id() == self.authority_id,
            "ONLY_AUTHORITY"
        );
    }

    fn domain(&self, id: Word) -> &Domain {
        self.domains
            .iter()
            .find(|d| d.id == id)
            .expect("DOMAIN_NOT_REGISTERED")
    }

    fn domain_mut(&mut self, id: Word) -> &mut Domain {
        self.domains
            .iter_mut()
            .find(|d| d.id == id)
            .expect("DOMAIN_NOT_REGISTERED")
    }

    fn available(&self, id: Word) -> u64 {
        let d = self.domain(id);
        require!(d.sequence > 0, "DOMAIN_NOT_OBSERVED");
        d.free.saturating_sub(d.pending_reserves)
    }

    fn expiry(expires: u64) {
        require!(
            expires > env::block_timestamp_ms() / 1000,
            "DEADLINE_EXPIRED"
        );
    }

    fn check_near_receiver(&self, domain: Word, route: Word, receiver: &str) {
        let d = self.domain(domain);
        if d.chain == "near" {
            require!(
                receiver == format!("r{}.{}", &hex(&route)[..24], d.vault),
                "WRONG_NEAR_ROUTE_RECEIVER"
            );
        }
    }

    fn charge(&mut self, before: u64, native: u128) {
        self.requests.flush();
        self.tasks.flush();
        self.plans.flush();
        self.commands.flush();
        self.evidence.flush();
        self.used_routes.flush();
        self.queue.flush();
        self.invoices.flush();
        self.funding.flush();
        env::state_write(self);
        let required = u128::from(env::storage_usage().saturating_sub(before))
            * env::storage_byte_cost().as_yoctonear()
            + native;
        let attached = env::attached_deposit().as_yoctonear();
        require!(attached >= required, "INSUFFICIENT_STORAGE_DEPOSIT");
        if attached > required {
            Promise::new(env::predecessor_account_id())
                .transfer(NearToken::from_yoctonear(attached - required))
                .detach();
        }
    }
}

#[cfg(test)]
mod tests;
