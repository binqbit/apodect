//! JSON bridge adapter for the shared local coordinator/provider runner.
use std::io;

use anchor_lang::prelude::Pubkey;
use anchor_lang::InstructionData;
use anchor_spl::token::spl_token;
use apodect_solana::{
    accounts,
    bridge::{
        self, action, BridgePlan, BridgeRoute, Checkpoint, Command, CommandReceipt, FundingReceipt,
    },
    instruction,
    state::{BudgetRequest, Project, Task},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use solana_signer::Signer;
use solana_transaction::Transaction;

use super::{
    client::{address, ensure, Localnet, Result},
    ix,
};

pub(super) fn hex(bytes: &[u8]) -> String {
    let mut value = String::from("0x");
    for byte in bytes {
        use std::fmt::Write;
        write!(value, "{byte:02x}").unwrap();
    }
    value
}

fn bytes<const N: usize>(value: &Value) -> Result<[u8; N]> {
    let text = value
        .as_str()
        .ok_or("hex string required")?
        .strip_prefix("0x")
        .ok_or("0x prefix required")?;
    ensure(text.len() == N * 2 && text.is_ascii(), "wrong hex width")?;
    let mut result = [0; N];
    for (i, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)?;
    }
    Ok(result)
}

fn identity(input: &Value, name: &str) -> Result<[u8; 32]> {
    match input.get(name) {
        None | Some(Value::Null) => Ok([0; 32]),
        Some(value) => bytes(value),
    }
}

fn amount(input: &Value) -> Result<u64> {
    Ok(input["amount"]
        .as_str()
        .ok_or("amount must be a decimal string")?
        .parse()?)
}

fn native(input: &Value, name: &str) -> Result<Pubkey> {
    Ok(Pubkey::new_from_array(identity(input, name)?))
}

fn page_integer(input: &Value, name: &str, default: u64) -> Result<u64> {
    match input.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::String(value)) => Ok(value.parse()?),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "unsigned pagination integer required".into()),
    }
}

fn record(project: Pubkey, seed: &[u8], id: &[u8; 32]) -> Pubkey {
    bridge::pda(&[seed, project.as_ref(), id])
}

impl Localnet {
    fn bridge_bootstrap(&self) -> Result<Value> {
        let manifest = self.bootstrap_liquidity_fixture()?;
        let project = address(&manifest["contracts"]["project"])?;
        let state: Project = self
            .read_finalized_optional(project)?
            .ok_or("native Project is not finalized")?;
        ensure(
            state.domain != [0; 32]
                || (state.task_count == 0
                    && state.invoice_count == 0
                    && state.approval_count == 0
                    && state.head.is_none()
                    && state.tail.is_none()
                    && state.held == 0),
            "local Project was already used; refusing to bind existing activity",
        )?;
        let lp_token = address(&manifest["asset"]["liquidity_token"])?;
        let mint = address(&manifest["asset"]["address"])?;
        Ok(json!({
            "chain": "solana",
            "rpc_url": self.url,
            "genesis": self.rpc("getGenesisHash", json!([]))?,
            "project": hex(project.as_ref()),
            "vault_address": project.to_string(),
            "token_vault": manifest["contracts"]["vault"],
            "asset": mint.to_string(),
            "asset_id": hex(mint.as_ref()),
            "decimals": 6,
            "owner": manifest["accounts"]["owner"]["address"],
            "authority": manifest["accounts"]["authority"]["address"],
            "agent": manifest["accounts"]["agent"]["address"],
            "mock_lp": {
                "address": manifest["accounts"]["liquidity"]["address"],
                "token_account": lp_token.to_string(),
                "recipient": hex(lp_token.as_ref()),
                "balance": self.finalized_token(lp_token)?.amount.to_string()
            },
            "manifest": manifest
        }))
    }

    fn bridge_execute(&self, input: &Value) -> Result<Value> {
        let manifest = self.manifest()?;
        let project = address(&manifest["contracts"]["project"])?;
        let payer = self.key("owner", false)?;
        let wire = bytes::<384>(&input["command"])?;
        let c = Command::parse(&wire)?;
        let receipt = record(project, b"bridge-command", &c.operation);
        let accounts = accounts::ExecuteCommand {
            project,
            workspace: address(&manifest["contracts"]["workspace"])?,
            mint: address(&manifest["asset"]["address"])?,
            vault: address(&manifest["contracts"]["vault"])?,
            receipt,
            payer: payer.pubkey(),
            system_program: solana_system_interface::program::ID,
            token_program: spl_token::ID,
            task: (c.task != [0; 32]).then(|| Pubkey::new_from_array(c.task)),
            request: if matches!(
                c.action,
                action::OUT_OF_BUDGET | action::CANCEL | action::RESTORE_REQUEST
            ) {
                let task: Task = self.read_confirmed(Pubkey::new_from_array(c.task))?;
                if c.action == action::RESTORE_REQUEST {
                    Some(bridge::pda(&[
                        b"request",
                        &c.task,
                        &task.request_count.to_le_bytes(),
                    ]))
                } else {
                    task.pending
                }
            } else {
                (c.request != [0; 32]).then(|| Pubkey::new_from_array(c.request))
            },
            plan: (action::RESERVE..=action::CLOSE_PLAN)
                .contains(&c.action)
                .then(|| record(project, b"bridge-plan", &c.related)),
            route: matches!(
                c.action,
                action::EXPECT_DELIVERY | action::EXPECT_REFUND | action::DISPATCH
            )
            .then(|| record(project, b"bridge-route", &c.reference)),
            escrow: matches!(c.action, action::EXPECT_DELIVERY | action::EXPECT_REFUND)
                .then(|| record(project, b"bridge-escrow", &c.reference)),
            recipient: matches!(c.action, action::DISPATCH | action::OWNER_WITHDRAW)
                .then(|| Pubkey::new_from_array(c.recipient)),
            target: (c.action == action::INVALIDATE)
                .then(|| record(project, b"bridge-command", &c.reference)),
        };
        let recovery = input["recovery_id"]
            .as_u64()
            .ok_or("recovery_id required")?;
        ensure(recovery <= 1, "recovery_id must be 0 or 1")?;
        let tx = self.send(
            &payer,
            &[ix(
                instruction::ExecuteCommand {
                    bytes: wire,
                    signature: bytes(&input["signature"])?,
                    recovery_id: recovery as u8,
                },
                accounts,
            )],
            &[],
        )?;
        let proof = self.finalized_proof(&tx)?;
        let receipt: CommandReceipt = self
            .read_finalized_optional(receipt)?
            .ok_or("missing finalized command receipt")?;
        Ok(json!({
            "operation": hex(&receipt.operation),
            "digest": hex(&receipt.digest),
            "action": receipt.action,
            "state": receipt.state,
            "result": hex(&receipt.result),
            "proof": proof
        }))
    }

    fn bridge_checkpoint(&self, input: &Value) -> Result<Value> {
        let manifest = self.manifest()?;
        let project = address(&manifest["contracts"]["project"])?;
        let observation = bytes::<32>(&input["observation"])?;
        let task = identity(input, "task")?;
        let request = identity(input, "request")?;
        let plan = identity(input, "plan")?;
        let route = identity(input, "route")?;
        let operation = identity(input, "operation")?;
        let checkpoint = record(project, b"checkpoint", &observation);
        let payer = self.key("authority", false)?;
        let tx = self.send(
            &payer,
            &[ix(
                instruction::RecordCheckpoint {
                    observation,
                    task,
                    request,
                    plan,
                    route,
                    operation,
                },
                accounts::RecordCheckpoint {
                    project,
                    checkpoint,
                    payer: payer.pubkey(),
                    system_program: solana_system_interface::program::ID,
                    task: (task != [0; 32]).then(|| Pubkey::new_from_array(task)),
                    request: (request != [0; 32]).then(|| Pubkey::new_from_array(request)),
                    plan: (plan != [0; 32]).then(|| record(project, b"bridge-plan", &plan)),
                    route: (route != [0; 32]).then(|| record(project, b"bridge-route", &route)),
                    command: (operation != [0; 32])
                        .then(|| record(project, b"bridge-command", &operation)),
                },
            )],
            &[],
        )?;
        let mut proof = self.finalized_proof(&tx)?;
        proof["receipt_account"] = json!(checkpoint.to_string());
        proof["program"] = json!(apodect_solana::ID.to_string());
        let checkpoint: Checkpoint = self
            .read_finalized_optional(checkpoint)?
            .ok_or("missing finalized checkpoint")?;
        Ok(json!({
            "payload": hex(&checkpoint.payload),
            "proof": proof,
            "receipt": decode(&checkpoint.payload)?
        }))
    }

    fn bridge_view(&self, input: &Value) -> Result<Value> {
        let manifest = self.manifest()?;
        let project = address(&manifest["contracts"]["project"])?;
        let state: Project = self
            .read_finalized_optional(project)?
            .ok_or("missing Project")?;
        let mut output = json!({
            "chain": "solana",
            "project": hex(project.as_ref()),
            "domain": hex(&state.domain),
            "free": state.free.to_string(),
            "held": state.held.to_string(),
            "task_count": state.task_count.to_string(),
            "checkpoint_sequence": state.checkpoint_sequence.to_string(),
            "vault_balance": self.finalized_token(address(&manifest["contracts"]["vault"])?)?.amount.to_string()
        });
        let task = identity(input, "task")?;
        if task != [0; 32] {
            if let Some(state) =
                self.read_finalized_optional::<Task>(Pubkey::new_from_array(task))?
            {
                ensure(state.project == project, "foreign task")?;
                let request = bridge::pda(&[b"request", &task, &state.request_count.to_le_bytes()]);
                output["task"] = json!({
                    "id": hex(&task),
                    "address": Pubkey::new_from_array(task).to_string(),
                    "state": state.lifecycle as u8,
                    "paused": state.paused,
                    "started": state.started,
                    "available": state.available.to_string(),
                    "issued": state.issued.to_string(),
                    "recovered": state.recovered.to_string(),
                    "control_nonce": state.control_nonce.to_string(),
                    "agent": hex(state.agent.as_ref()),
                    "live_request": state.pending.map(|request| hex(request.as_ref())),
                    "next_request": hex(request.as_ref())
                });
            }
        }
        let request = identity(input, "request")?;
        if request != [0; 32] {
            if let Some(state) =
                self.read_finalized_optional::<BudgetRequest>(Pubkey::new_from_array(request))?
            {
                ensure(state.project == project, "foreign request")?;
                output["request"] = json!({
                    "id": hex(&request),
                    "task": hex(state.task.as_ref()),
                    "amount": state.amount.to_string(),
                    "kind": state.kind as u8,
                    "state": state.state as u8,
                    "purpose": hex(&state.purpose)
                });
            }
        }
        let plan = identity(input, "plan")?;
        if plan != [0; 32] {
            if let Some(state) =
                self.read_finalized_optional::<BridgePlan>(record(project, b"bridge-plan", &plan))?
            {
                ensure(state.project == project, "foreign plan")?;
                output["plan"] = json!({
                    "id": hex(&plan),
                    "state": state.state,
                    "held": state.held.to_string()
                });
            }
        }
        let route = identity(input, "route")?;
        if route != [0; 32] {
            let escrow = record(project, b"bridge-escrow", &route);
            output["route"] = json!({
                "id": hex(&route),
                "escrow": escrow.to_string(),
                "recipient": hex(escrow.as_ref()),
                "exists": false,
                "escrow_balance": "0"
            });
            if let Some(state) = self.read_finalized_optional::<BridgeRoute>(record(
                project,
                b"bridge-route",
                &route,
            ))? {
                ensure(state.project == project, "foreign route")?;
                output["route"] = json!({
                    "id": hex(&route),
                    "plan": hex(&state.plan),
                    "kind": state.kind,
                    "expected": state.expected.to_string(),
                    "received": state.received.to_string(),
                    "dispatched": state.dispatched.to_string(),
                    "escrow": escrow.to_string(),
                    "recipient": hex(escrow.as_ref()),
                    "exists": true,
                    "escrow_balance": self.finalized_token(escrow)?.amount.to_string()
                });
            }
        }
        let operation = identity(input, "operation")?;
        if operation != [0; 32] {
            if let Some(state) = self.read_finalized_optional::<CommandReceipt>(record(
                project,
                b"bridge-command",
                &operation,
            ))? {
                ensure(state.project == project, "foreign command")?;
                output["command"] = json!({
                    "operation": hex(&operation),
                    "digest": hex(&state.digest),
                    "action": state.action,
                    "state": state.state,
                    "result": hex(&state.result)
                });
            }
        }
        Ok(output)
    }

    fn read_funding(&self, input: &Value) -> Result<Value> {
        let manifest = self.manifest()?;
        let project = address(&manifest["contracts"]["project"])?;
        let funding_id = bytes::<32>(&input["funding_id"])?;
        let receipt_account = record(project, b"funding", &funding_id);
        let Some(receipt) = self.read_finalized_optional::<FundingReceipt>(receipt_account)? else {
            return Ok(Value::Null);
        };
        let invoice_id = receipt.payload[64..96].try_into().unwrap();
        let amount = bridge::integer(&receipt.payload[96..128])?;
        let expected = instruction::FundCoordinated {
            funding_id,
            invoice_id,
            amount,
        }
        .data();
        let mut before: Option<String> = None;
        loop {
            let mut config = json!({"commitment":"finalized", "limit":1000});
            if let Some(signature) = &before {
                config["before"] = json!(signature);
            }
            let response = self.rpc(
                "getSignaturesForAddress",
                json!([receipt_account.to_string(), config]),
            )?;
            let signatures = response
                .as_array()
                .ok_or("missing funding signature history")?;
            ensure(
                !signatures.is_empty(),
                "finalized funding receipt exists but creation transaction is unavailable",
            )?;
            for entry in signatures {
                if !entry["err"].is_null() || entry["confirmationStatus"] != "finalized" {
                    continue;
                }
                let signature = entry["signature"]
                    .as_str()
                    .ok_or("missing funding signature")?;
                let result = self.rpc("getTransaction", json!([
                    signature, {"commitment":"finalized", "encoding":"base64", "maxSupportedTransactionVersion":0}
                ]))?;
                if result.is_null() || !result["meta"]["err"].is_null() {
                    continue;
                }
                let raw = STANDARD.decode(
                    result["transaction"][0]
                        .as_str()
                        .ok_or("missing encoded funding transaction")?,
                )?;
                let Ok(transaction) = wincode::deserialize::<Transaction>(&raw) else {
                    continue;
                };
                let message = &transaction.message;
                let creates_receipt = message.instructions.iter().any(|instruction| {
                    let key = |index: u8| message.account_keys.get(usize::from(index));
                    key(instruction.program_id_index) == Some(&apodect_solana::ID)
                        && instruction.data == expected
                        && instruction.accounts.get(1).and_then(|index| key(*index))
                            == Some(&project)
                        && instruction.accounts.get(6).and_then(|index| key(*index))
                            == Some(&receipt_account)
                });
                if creates_receipt {
                    let mut proof = self.finalized_proof(signature)?;
                    proof["receipt_account"] = json!(receipt_account.to_string());
                    proof["program"] = json!(apodect_solana::ID.to_string());
                    return Ok(
                        json!({"payload":hex(&receipt.payload), "funding_id":hex(&funding_id), "proof":proof}),
                    );
                }
            }
            before = Some(
                signatures.last().unwrap()["signature"]
                    .as_str()
                    .ok_or("missing history cursor")?
                    .to_owned(),
            );
        }
    }

    fn bridge_scan(&self, input: &Value) -> Result<Value> {
        let manifest = self.manifest()?;
        let project = address(&manifest["contracts"]["project"])?;
        let state: Project = self
            .read_finalized_optional(project)?
            .ok_or("missing Project")?;
        let start = page_integer(input, "from", 0)?.min(state.task_count);
        let limit = page_integer(input, "limit", 100)?;
        ensure((1..=100).contains(&limit), "scan limit must be 1..100")?;
        let end = start.saturating_add(limit).min(state.task_count);
        let task_keys: Vec<_> = (start..end)
            .map(|index| bridge::pda(&[b"task", project.as_ref(), &index.to_le_bytes()]))
            .collect();
        let tasks = self.read_finalized_batch::<Task>(&task_keys)?;
        let request_keys: Vec<_> = tasks.iter().filter_map(|task| task.pending).collect();
        let requests = self.read_finalized_batch::<BudgetRequest>(&request_keys)?;
        let mut records = vec![];
        for (task_key, task) in task_keys.iter().zip(tasks) {
            ensure(task.project == project, "scan foreign task")?;
            let request = if let Some(key) = task.pending {
                let index = request_keys
                    .iter()
                    .position(|id| *id == key)
                    .ok_or("missing live request")?;
                let request = &requests[index];
                ensure(
                    request.project == project && request.task == *task_key,
                    "scan foreign live request",
                )?;
                json!({
                    "id":hex(key.as_ref()), "task":hex(request.task.as_ref()),
                    "amount":request.amount.to_string(), "kind":request.kind as u8,
                    "state":request.state as u8, "purpose":hex(&request.purpose)
                })
            } else {
                Value::Null
            };
            let record = json!({
                "state":task.lifecycle as u8, "paused":task.paused, "started":task.started,
                "agent":hex(task.agent.as_ref()), "available":task.available.to_string(),
                "issued":task.issued.to_string(), "recovered":task.recovered.to_string(),
                "request_count":task.request_count.to_string(), "control_nonce":task.control_nonce.to_string(),
                "report":hex(&task.report), "request":request
            });
            let fingerprint = solana_sha256_hasher::hash(&serde_json::to_vec(&record)?).to_bytes();
            records.push(json!({
                "task":hex(task_key.as_ref()), "request":task.pending.map(|key|hex(key.as_ref())),
                "fingerprint":hex(&fingerprint), "record":record
            }));
        }
        Ok(json!({
            "next_task":end.to_string(), "total":state.task_count.to_string(),
            "free":state.free.to_string(), "held":state.held.to_string(), "tasks":records
        }))
    }

    fn bridge_agent(&self, input: &Value) -> Result<Value> {
        let m = self.manifest()?;
        let project = address(&m["contracts"]["project"])?;
        let agent = self.key("agent", false)?;
        let task = native(input, "task")?;
        let method = input["method"].as_str().ok_or("agent method required")?;
        let mut output = json!({});
        let instruction = match method {
            "create" => {
                let state: Project = self.read_confirmed(project)?;
                let task =
                    bridge::pda(&[b"task", project.as_ref(), &state.task_count.to_le_bytes()]);
                let request = bridge::pda(&[b"request", task.as_ref(), &0u64.to_le_bytes()]);
                output = json!({
                    "task":hex(task.as_ref()), "task_address":task.to_string(),
                    "request":hex(request.as_ref()), "request_address":request.to_string()
                });
                ix(
                    instruction::CreateTask {
                        amount: amount(input)?,
                        purpose: bytes(&input["purpose"])?,
                    },
                    accounts::CreateTask {
                        project,
                        task,
                        request,
                        agent: agent.pubkey(),
                        system_program: solana_system_interface::program::ID,
                    },
                )
            }
            "request" => {
                let state: Task = self.read_confirmed(task)?;
                let request = bridge::pda(&[
                    b"request",
                    task.as_ref(),
                    &state.request_count.to_le_bytes(),
                ]);
                output = json!({"task":hex(task.as_ref()),"request":hex(request.as_ref())});
                ix(
                    instruction::SubmitRequest {
                        amount: amount(input)?,
                        purpose: bytes(&input["purpose"])?,
                    },
                    accounts::SubmitRequest {
                        project,
                        task,
                        request,
                        agent: agent.pubkey(),
                        system_program: solana_system_interface::program::ID,
                    },
                )
            }
            "withdraw" => ix(
                instruction::Withdraw {
                    amount: amount(input)?,
                },
                accounts::Withdraw {
                    project,
                    task,
                    mint: address(&m["asset"]["address"])?,
                    vault: address(&m["contracts"]["vault"])?,
                    agent_token: address(&m["asset"]["agent_token"])?,
                    agent: agent.pubkey(),
                    token_program: spl_token::ID,
                },
            ),
            "return" => ix(
                instruction::ReturnFunds {
                    amount: amount(input)?,
                },
                accounts::ReturnFunds {
                    project,
                    task,
                    mint: address(&m["asset"]["address"])?,
                    vault: address(&m["contracts"]["vault"])?,
                    agent_token: address(&m["asset"]["agent_token"])?,
                    agent: agent.pubkey(),
                    token_program: spl_token::ID,
                },
            ),
            "release" => ix(
                instruction::ReleaseFunds {
                    amount: amount(input)?,
                },
                accounts::ReleaseFunds {
                    project,
                    task,
                    agent: agent.pubkey(),
                },
            ),
            "finalize" => {
                let state: Task = self.read_confirmed(task)?;
                ix(
                    instruction::FinalizeTask {
                        report: bytes(&input["report"])?,
                    },
                    accounts::FinalizeTask {
                        project,
                        task,
                        workspace: address(&m["contracts"]["workspace"])?,
                        actor: agent.pubkey(),
                        pending: state.pending,
                    },
                )
            }
            _ => return Err("unknown agent method".into()),
        };
        let tx = self.send(&agent, &[instruction], &[])?;
        output["proof"] = self.finalized_proof(&tx)?;
        Ok(output)
    }
}

fn decode(payload: &[u8; 1248]) -> Result<Value> {
    const NAMES: [&str; 39] = [
        "domain",
        "observation",
        "sequence",
        "height",
        "timestamp",
        "free",
        "held",
        "task",
        "task_state",
        "paused",
        "started",
        "available",
        "issued",
        "recovered",
        "pending_outflow",
        "control_nonce",
        "agent",
        "request",
        "request_task",
        "request_amount",
        "request_kind",
        "request_state",
        "purpose",
        "plan",
        "plan_state",
        "plan_held",
        "route",
        "route_plan",
        "route_kind",
        "route_expected",
        "route_received",
        "route_dispatched",
        "operation",
        "command_digest",
        "command_action",
        "command_state",
        "command_result",
        "live_request",
        "live_request_exists",
    ];
    const IDENTITIES: [usize; 14] = [0, 1, 7, 16, 17, 18, 22, 23, 26, 27, 32, 33, 36, 37];
    let mut result = json!({});
    for (i, name) in NAMES.iter().enumerate() {
        let word = &payload[i * 32..(i + 1) * 32];
        result[*name] = if IDENTITIES.contains(&i) {
            json!(hex(word))
        } else {
            json!(bridge::integer(word)?.to_string())
        };
    }
    Ok(result)
}

pub(super) fn run(localnet: &Localnet) -> Result<()> {
    let input: Value = serde_json::from_reader(io::stdin().lock())?;
    let result = match input["action"].as_str().ok_or("action required")? {
        "bootstrap" => localnet.bridge_bootstrap()?,
        "relay_account" => {
            let relay = localnet.key("relayer", true)?;
            let balance = localnet.rpc(
                "getBalance",
                json!([
                    relay.pubkey().to_string(), {"commitment":"finalized"}
                ]),
            )?["value"]
                .as_u64()
                .ok_or("missing relay gas balance")?;
            if balance < 10_000_000_000 {
                let signature = localnet.rpc(
                    "requestAirdrop",
                    json!([relay.pubkey().to_string(), 20_000_000_000u64 - balance]),
                )?;
                localnet.finalized_proof(signature.as_str().ok_or("missing airdrop signature")?)?;
            }
            json!({"address":relay.pubkey().to_string(), "key_file":localnet.dir.join("keys/relayer.json")})
        }
        "bind" => {
            let m = localnet.manifest()?;
            let owner = localnet.key("owner", false)?;
            let tx = localnet.send(
                &owner,
                &[ix(
                    instruction::BindCoordinator {
                        domain: bytes(&input["domain"])?,
                        command_key: bytes(&input["public_key"])?,
                    },
                    accounts::BindCoordinator {
                        workspace: address(&m["contracts"]["workspace"])?,
                        project: address(&m["contracts"]["project"])?,
                        owner: owner.pubkey(),
                    },
                )],
                &[],
            )?;
            json!({"proof":localnet.finalized_proof(&tx)?})
        }
        "execute" => localnet.bridge_execute(&input)?,
        "checkpoint" => localnet.bridge_checkpoint(&input)?,
        "view" => localnet.bridge_view(&input)?,
        "scan" => localnet.bridge_scan(&input)?,
        "read_funding" => localnet.read_funding(&input)?,
        "agent" => localnet.bridge_agent(&input)?,
        "route_address" => {
            let m = localnet.manifest()?;
            let project = address(&m["contracts"]["project"])?;
            let route = bytes::<32>(&input["route"])?;
            let escrow = record(project, b"bridge-escrow", &route);
            json!({"route":hex(&route),"address":escrow.to_string(),"recipient":hex(escrow.as_ref())})
        }
        "route_deposit" => {
            let m = localnet.manifest()?;
            let project = address(&m["contracts"]["project"])?;
            let route = bytes::<32>(&input["route"])?;
            let r: BridgeRoute =
                localnet.read_confirmed(record(project, b"bridge-route", &route))?;
            let kind = match input["kind"].as_str() {
                Some("delivery") => 1,
                Some("refund") => 2,
                _ => return Err("route kind required".into()),
            };
            ensure(
                r.project == project && r.kind == kind,
                "route kind/project mismatch",
            )?;
            let lp = localnet.key("liquidity", false)?;
            let escrow = record(project, b"bridge-escrow", &route);
            let tx = localnet.send(
                &lp,
                &[spl_token::instruction::transfer_checked(
                    &spl_token::ID,
                    &address(&m["asset"]["liquidity_token"])?,
                    &address(&m["asset"]["address"])?,
                    &escrow,
                    &lp.pubkey(),
                    &[],
                    amount(&input)?,
                    6,
                )?],
                &[],
            )?;
            json!({
                "proof":localnet.finalized_proof(&tx)?, "escrow":escrow.to_string(),
                "escrow_balance":localnet.finalized_token(escrow)?.amount.to_string()
            })
        }
        "collect" => {
            let m = localnet.manifest()?;
            let project = address(&m["contracts"]["project"])?;
            let route = bytes::<32>(&input["route"])?;
            let r: BridgeRoute =
                localnet.read_confirmed(record(project, b"bridge-route", &route))?;
            let payer = localnet.key("authority", false)?;
            let tx = localnet.send(
                &payer,
                &[ix(
                    instruction::CollectRoute {},
                    accounts::CollectRoute {
                        project,
                        mint: address(&m["asset"]["address"])?,
                        vault: address(&m["contracts"]["vault"])?,
                        route: record(project, b"bridge-route", &route),
                        plan: record(project, b"bridge-plan", &r.plan),
                        escrow: record(project, b"bridge-escrow", &route),
                        token_program: spl_token::ID,
                    },
                )],
                &[],
            )?;
            json!({"proof":localnet.finalized_proof(&tx)?,"view":localnet.bridge_view(&input)?})
        }
        "fund" => {
            let funding_id = bytes::<32>(
                input
                    .get("funding_id")
                    .ok_or("funding_id is required; persist it before sending funds")?,
            )?;
            let m = localnet.manifest()?;
            let owner = localnet.key("owner", false)?;
            let project = address(&m["contracts"]["project"])?;
            let invoice_id = identity(&input, "invoice_id")?;
            let receipt_account = record(project, b"funding", &funding_id);
            let tx = localnet.send(
                &owner,
                &[ix(
                    instruction::FundCoordinated {
                        funding_id,
                        invoice_id,
                        amount: amount(&input)?,
                    },
                    accounts::FundCoordinated {
                        workspace: address(&m["contracts"]["workspace"])?,
                        project,
                        mint: address(&m["asset"]["address"])?,
                        vault: address(&m["contracts"]["vault"])?,
                        owner_token: address(&m["asset"]["owner_token"])?,
                        owner: owner.pubkey(),
                        receipt: receipt_account,
                        token_program: spl_token::ID,
                        system_program: solana_system_interface::program::ID,
                    },
                )],
                &[],
            )?;
            let mut proof = localnet.finalized_proof(&tx)?;
            proof["receipt_account"] = json!(receipt_account.to_string());
            proof["program"] = json!(apodect_solana::ID.to_string());
            let receipt: FundingReceipt = localnet
                .read_finalized_optional(receipt_account)?
                .ok_or("missing finalized funding receipt")?;
            json!({"payload": hex(&receipt.payload), "funding_id": hex(&funding_id), "proof": proof})
        }
        _ => return Err("unknown bridge action".into()),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
