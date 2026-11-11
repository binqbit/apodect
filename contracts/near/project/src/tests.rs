//! Local behavior and fixed serialization/API compatibility checks.

use crate::state::{Invoice, Key, Operation, Request, Task};
use crate::{
    ApplyResult, DepositMessage, OperationState, Project, RequestKind, RequestState, TaskState,
};
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::borsh::{self, BorshSerialize};
use near_sdk::json_types::U128;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{AccountId, Gas, NearToken, PromiseResult, env, testing_env};
use std::collections::BTreeMap;

fn account(name: &str) -> AccountId {
    format!("{name}.test.near").parse().unwrap()
}

fn context(caller: &str) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("p0.workspace.test.near".parse().unwrap())
            .predecessor_account_id(account(caller))
            .signer_account_id(account(caller))
            .attached_deposit(NearToken::from_near(1))
            .account_balance(NearToken::from_near(100))
            .prepaid_gas(Gas::from_tgas(300))
            .build()
    );
}

fn project() -> Project {
    context("workspace");
    Project::new(
        account("workspace"),
        0,
        account("owner"),
        account("authority"),
        account("token"),
    )
}

fn fund(project: &mut Project, amount: u128) {
    context("token");
    let _ = project.ft_on_transfer(account("owner"), U128(amount), r#"{"kind":"fund"}"#.into());
}

fn funded_task(project: &mut Project, amount: u128) -> u64 {
    context("agent");
    let id = project.create_task(U128(amount), [1; 32]);

    context("authority");
    let request = project.get_task(id).unwrap().live_request.unwrap();
    project.approve_request(request);
    assert!(matches!(project.apply(), ApplyResult::Funded { .. }));
    id
}

fn settle(project: &mut Project, operation_id: u64, success: bool) {
    let result = if success {
        PromiseResult::Successful(vec![])
    } else {
        PromiseResult::Failed
    };
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("p0.workspace.test.near".parse().unwrap())
            .predecessor_account_id("p0.workspace.test.near".parse().unwrap())
            .account_balance(NearToken::from_near(100))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![result]
    );
    assert_eq!(project.settle_outflow(operation_id), success);
}

#[test]
fn accounting_example_releases_returns_to_project_without_refilling_allowance() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(40)).detach();
    assert_eq!(p.get_task(task).unwrap().pending_outflow.0, 40);
    assert_eq!(p.get_task(task).unwrap().external_outstanding.0, 0);
    settle(&mut p, 0, true);
    assert!(!p.settle_outflow(0));

    context("token");
    let _ = p.ft_on_transfer(
        account("agent"),
        U128(15),
        format!(r#"{{"kind":"return","task_id":{task}}}"#),
    );
    assert_eq!(p.get_task(task).unwrap().available.0, 20);

    context("agent");
    p.release(task, U128(20));
    assert_eq!(p.finalize(task, Some([2; 32])).0, 25);
    assert_eq!(p.get_project().free.0, 75);
    assert_eq!(p.get_task(task).unwrap().expense.unwrap().0, 25);
}

#[test]
fn fifo_blocks_smaller_head_bypass_and_tombstones_are_constant_work() {
    let mut p = project();
    fund(&mut p, 30);

    context("agent");
    let a = p.create_task(U128(60), [1; 32]);
    let b = p.create_task(U128(10), [1; 32]);

    context("authority");
    p.approve_request(0);
    p.approve_request(1);
    assert_eq!(p.get_project().free.0, 30);
    assert!(matches!(
        p.apply(),
        ApplyResult::Waiting { request_id: 0, .. }
    ));
    p.mark_out_of_budget(a);
    assert!(matches!(p.apply(), ApplyResult::Skipped { request_id: 0 }));
    assert!(matches!(p.apply(), ApplyResult::Funded { task_id, .. } if task_id == b));
    assert_eq!(p.get_task(a).unwrap().state, TaskState::OutOfBudget);
    assert_eq!(p.get_task(b).unwrap().issued.0, 10);
}

#[test]
fn known_failed_outflow_restores_once_and_preserves_pause_and_first_use() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(40)).detach();

    context("owner");
    p.force_cancel(task);
    settle(&mut p, 0, false);
    assert!(!p.settle_outflow(0));
    let view = p.get_task(task).unwrap();
    assert!(view.started && view.paused);
    assert_eq!(view.available.0, 60);
    assert_eq!(view.pending_outflow.0, 0);
    assert_eq!(p.get_operation(0).unwrap().state, OperationState::Failed);
}

#[test]
fn mixed_out_of_order_callbacks_preserve_task_and_owner_reservations() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(20)).detach();
    p.withdraw(task, U128(15)).detach();
    let pending = p.get_task(task).unwrap();
    assert_eq!(pending.available.0, 25);
    assert_eq!(pending.pending_outflow.0, 35);
    assert_eq!(pending.external_outstanding.0, 0);

    context("owner");
    p.withdraw_free(U128(30)).detach();
    p.force_cancel(task);
    p.reclaim(task, U128(25));
    assert_eq!(p.get_project().free.0, 35);
    assert_eq!(p.get_project().pending_owner_outflow.0, 30);

    // Settle a later task failure first. The unresolved earlier transfer stays occupied.
    settle(&mut p, 1, false);
    assert!(!p.settle_outflow(1));
    let partial = p.get_task(task).unwrap();
    assert!(partial.paused && partial.started);
    assert_eq!(partial.available.0, 15);
    assert_eq!(partial.pending_outflow.0, 20);
    assert_eq!(partial.external_outstanding.0, 0);
    assert_eq!(p.get_operation(0).unwrap().state, OperationState::Pending);

    context("owner");
    p.reclaim(task, U128(15));
    settle(&mut p, 2, false);
    assert!(!p.settle_outflow(2));
    assert_eq!(p.get_project().free.0, 80);
    assert_eq!(p.get_project().pending_owner_outflow.0, 0);

    settle(&mut p, 0, true);
    assert!(!p.settle_outflow(0));
    let settled = p.get_task(task).unwrap();
    assert!(settled.paused && settled.started);
    assert_eq!(settled.available.0, 0);
    assert_eq!(settled.pending_outflow.0, 0);
    assert_eq!(settled.recovered.0, 40);
    assert_eq!(settled.external_outstanding.0, 20);

    context("token");
    let _ = p.ft_on_transfer(
        account("agent"),
        U128(5),
        format!(r#"{{"kind":"return","task_id":{task}}}"#),
    );
    context("agent");
    assert_eq!(p.finalize(task, None).0, 15);
    assert_eq!(p.get_project().free.0, 85);
}

#[test]
#[should_panic(expected = "TASK_FUNDS_NOT_SETTLED")]
fn unsettled_outflow_prevents_finalization_even_without_controlled_reserve() {
    let mut p = project();
    fund(&mut p, 60);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(60)).detach();
    p.finalize(task, None);
}

#[test]
#[should_panic(expected = "CONFIRMED_RESERVE_REQUIRED")]
fn unsettled_outflow_prevents_continue_even_with_retained_reserve() {
    let mut p = project();
    fund(&mut p, 60);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(20)).detach();

    context("owner");
    p.force_cancel(task);
    p.continue_task(task);
}

#[test]
#[should_panic(expected = "EXCESS_TASK_RETURN")]
fn unsettled_outflow_is_not_confirmed_external_principal() {
    let mut p = project();
    fund(&mut p, 60);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(20)).detach();

    context("token");
    let _ = p.ft_on_transfer(
        account("agent"),
        U128(1),
        format!(r#"{{"kind":"return","task_id":{task}}}"#),
    );
}

#[test]
fn retained_continue_and_reclaim_restore_preserve_issuance_accounting() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("owner");
    p.force_cancel(task);
    p.continue_task(task);
    assert_eq!(p.get_task(task).unwrap().issued.0, 60);
    p.force_cancel(task);
    p.reclaim(task, U128(20));

    context("authority");
    p.restore(task, U128(20), [3; 32]);
    p.apply();
    assert!(p.get_task(task).unwrap().paused);

    context("owner");
    p.continue_task(task);
    assert_eq!(p.get_task(task).unwrap().issued.0, 80);
    assert_eq!(p.get_task(task).unwrap().recovered.0, 20);

    context("agent");
    p.release(task, U128(60));
    assert_eq!(p.finalize(task, None).0, 0);
}

#[test]
fn supplement_rejection_stops_and_full_new_allocation_resumes_without_unpausing() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(10)).detach();
    settle(&mut p, 0, true);

    context("agent");
    let supplement = p.request_budget(task, U128(5), [1; 32]);

    context("authority");
    p.reject_request(supplement);
    assert_eq!(p.get_task(task).unwrap().state, TaskState::Stopped);

    context("owner");
    p.force_cancel(task);

    context("agent");
    let resume = p.request_budget(task, U128(5), [1; 32]);

    context("authority");
    p.approve_request(resume);
    p.apply();
    assert_eq!(p.get_task(task).unwrap().state, TaskState::Active);
    assert!(p.get_task(task).unwrap().paused);
}

#[test]
fn invoice_response_never_creates_cash_and_rotation_preserves_records() {
    let mut p = project();

    context("authority");
    let invoice = p.create_invoice(U128(100), None);

    context("owner");
    p.set_authority(account("new-authority"));
    p.respond_invoice(invoice, U128(0), Some([4; 32]));
    assert_eq!(p.get_project().free.0, 0);
    assert!(p.get_invoice(invoice).unwrap().closed);

    context("token");
    let _ = p.ft_on_transfer(
        account("owner"),
        U128(25),
        format!(r#"{{"kind":"fund","invoice_id":{invoice}}}"#),
    );
    assert_eq!(p.get_invoice(invoice).unwrap().actual_funding.0, 25);
    assert_eq!(p.get_project().free.0, 25);
}

#[test]
#[should_panic(expected = "ONLY_AUTHORITY")]
fn owner_has_no_implicit_approval_role() {
    let mut p = project();

    context("agent");
    p.create_task(U128(10), [1; 32]);

    context("owner");
    p.approve_request(0);
}

#[test]
#[should_panic(expected = "INVALID_PROJECT_ACCOUNT")]
fn initialization_rejects_nonce_inconsistent_with_account_identity() {
    context("workspace");
    Project::new(
        account("workspace"),
        1,
        account("owner"),
        account("authority"),
        account("token"),
    );
}

#[test]
#[should_panic(expected = "TASK_ALREADY_STARTED")]
fn ordinary_cancel_is_forbidden_after_async_acceptance_even_if_transfer_failed() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(1)).detach();
    settle(&mut p, 0, false);

    context("owner");
    p.cancel(task);
}

#[test]
#[should_panic(expected = "INSUFFICIENT_STORAGE_DEPOSIT")]
fn data_growth_requires_actual_storage_payment() {
    let mut p = project();

    context("agent");
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("p0.workspace.test.near".parse().unwrap())
            .predecessor_account_id(account("agent"))
            .build()
    );
    p.create_task(U128(10), [1; 32]);
}

#[test]
fn completion_removes_own_queued_supplement() {
    let mut p = project();
    fund(&mut p, 100);
    let task = funded_task(&mut p, 60);

    context("agent");
    p.withdraw(task, U128(60)).detach();
    settle(&mut p, 0, true);

    context("agent");
    let supplement = p.request_budget(task, U128(5), [1; 32]);

    context("authority");
    p.approve_request(supplement);

    context("agent");
    p.finalize(task, None);
    assert_eq!(
        p.get_request(supplement).unwrap().state,
        RequestState::Removed
    );

    context("authority");
    assert!(matches!(p.apply(), ApplyResult::Skipped { .. }));
    assert_eq!(p.get_task(task).unwrap().issued.0, 60);
}

fn encoded(value: &impl BorshSerialize) -> String {
    hex(&borsh::to_vec(value).unwrap())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn state_and_json_compatibility() {
    near_sdk::mock::with_mocked_blockchain(|blockchain| {
        blockchain.take_storage();
    });
    let workspace: AccountId = "workspace.test.near".parse().unwrap();
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("p0.workspace.test.near".parse().unwrap())
            .predecessor_account_id(workspace.clone())
            .account_balance(NearToken::from_near(100))
            .build()
    );
    let mut project = Project::new(
        workspace,
        0,
        "owner.test.near".parse().unwrap(),
        "authority.test.near".parse().unwrap(),
        "token.test.near".parse().unwrap(),
    );
    project.free = 101;
    project.pending_owner_outflow = 102;
    project.next_task = 11;
    project.next_request = 12;
    project.next_invoice = 13;
    project.next_operation = 14;
    project.queue_head = 15;
    project.queue_tail = 16;

    let task = Task {
        agent: "agent.test.near".parse().unwrap(),
        state: TaskState::Stopped,
        paused: true,
        started: true,
        issued: 90,
        recovered: 11,
        available: 22,
        pending_outflow: 3,
        live_request: Some(19),
        report: [31; 32],
    };
    let request = Request {
        task_id: 7,
        amount: 27,
        purpose: [32; 32],
        state: RequestState::Queued,
        kind: RequestKind::Resume,
    };
    let invoice = Invoice {
        requested: 41,
        reference: [33; 32],
        closed: true,
        intended: 23,
        response_reference: [34; 32],
        actual_funding: 17,
    };
    let operation = Operation {
        task_id: Some(7),
        amount: 13,
        state: OperationState::Failed,
    };
    let mut without_request = task.clone();
    without_request.live_request = None;
    let mut owner_operation = operation.clone();
    owner_operation.task_id = None;

    let records = near_sdk::serde_json::json!({"task":encoded(&task),"task_without_request":encoded(&without_request),"request":encoded(&request),"invoice":encoded(&invoice),"operation":encoded(&operation),"owner_operation":encoded(&owner_operation)});
    project.tasks.insert(7, task);
    project.requests.insert(8, request);
    project.invoices.insert(9, invoice);
    project.operations.insert(10, operation);
    project.queue.insert(15, 8);
    project.tasks.flush();
    project.requests.flush();
    project.invoices.flush();
    project.operations.flush();
    project.queue.flush();
    env::state_write(&project);
    let json_api = near_sdk::serde_json::json!({
        "project":project.get_project(),"task":project.get_task(7),"request":project.get_request(8),"invoice":project.get_invoice(9),"operation":project.get_operation(10),
        "messages":[DepositMessage::Fund{invoice_id:None,global_invoice:None,funding_ref:None},DepositMessage::Fund{invoice_id:Some(9),global_invoice:None,funding_ref:None},DepositMessage::Return{task_id:7}],
        "apply":[ApplyResult::Empty,ApplyResult::Skipped{request_id:8},ApplyResult::Waiting{request_id:8,required:U128(27),available:U128(11)},ApplyResult::Funded{request_id:8,task_id:7,amount:U128(27)}]
    });
    let storage: BTreeMap<_, _> =
        near_sdk::mock::with_mocked_blockchain(|blockchain| blockchain.take_storage())
            .into_iter()
            .map(|(key, value)| (hex(&key), hex(&value)))
            .collect();
    let actual = near_sdk::serde_json::json!({
        "root":encoded(&project),"records":records,"storage":storage,"json_api":json_api,
        "keys":[encoded(&Key::Tasks),encoded(&Key::Requests),encoded(&Key::Queue),encoded(&Key::Invoices),encoded(&Key::Operations)],
        "task_states":[encoded(&TaskState::Active),encoded(&TaskState::Stopped),encoded(&TaskState::OutOfBudget),encoded(&TaskState::Completed),encoded(&TaskState::Canceled)],
        "request_states":[encoded(&RequestState::Pending),encoded(&RequestState::Queued),encoded(&RequestState::Funded),encoded(&RequestState::Rejected),encoded(&RequestState::Removed)],
        "request_kinds":[encoded(&RequestKind::Initial),encoded(&RequestKind::Supplement),encoded(&RequestKind::Resume),encoded(&RequestKind::Restoration)],
        "operation_states":[encoded(&OperationState::Pending),encoded(&OperationState::Succeeded),encoded(&OperationState::Failed)]
    });
    let expected: near_sdk::serde_json::Value =
        near_sdk::serde_json::from_str(include_str!("tests/fixtures/state-v3.json")).unwrap();
    assert_eq!(actual, expected);
}

fn coordinated_signature(name: &str) -> near_sdk::serde_json::Value {
    near_sdk::serde_json::from_str::<near_sdk::serde_json::Value>(include_str!(
        "../tests/fixtures/coordinated-signatures.json"
    ))
    .unwrap()[name]
        .clone()
}
fn coordinated_project() -> Project {
    coordinated_project_with_funds(100)
}

fn coordinated_project_with_funds(amount: u128) -> Project {
    let mut p = project();
    fund(&mut p, amount);
    context("owner");
    let key = coordinated_signature("public_key")
        .as_str()
        .unwrap()
        .to_owned();
    p.bind_coordinator("11".repeat(32), key);
    p
}

#[test]
fn owner_funding_reference_recovers_exact_receipt_and_refunds_duplicates() {
    let mut p = coordinated_project();
    let reference = "ab".repeat(32);
    let message = near_sdk::serde_json::json!({"kind": "fund",
        "global_invoice": "00".repeat(32), "funding_ref": reference})
    .to_string();
    context("token");
    assert!(matches!(
        p.ft_on_transfer(account("owner"), U128(25), message.clone()),
        near_sdk::PromiseOrValue::Value(U128(0))
    ));
    let receipt = p.get_funding_by_reference(reference.clone()).unwrap();
    assert_eq!(p.get_funding(format!("{:064x}", 1)), Some(receipt.clone()));
    assert_eq!(p.free, 125);
    context("token");
    assert!(matches!(
        p.ft_on_transfer(account("owner"), U128(25), message),
        near_sdk::PromiseOrValue::Value(U128(25))
    ));
    assert_eq!(p.free, 125);
    assert_eq!(p.get_funding_by_reference(reference), Some(receipt));
    assert_eq!(p.get_bridge()["next_funding"], 2);
}

#[test]
#[should_panic(expected = "FUNDING_REFERENCE_TERMS_CHANGED")]
fn owner_cannot_reuse_funding_reference_for_different_terms() {
    let mut p = coordinated_project();
    let message = near_sdk::serde_json::json!({"kind": "fund",
        "funding_ref": "ab".repeat(32)})
    .to_string();
    context("token");
    let _ = p.ft_on_transfer(account("owner"), U128(25), message.clone());
    context("token");
    let _ = p.ft_on_transfer(account("owner"), U128(26), message);
}
fn execute(p: &mut Project, name: &str) {
    let value = coordinated_signature(name);
    context("relayer");
    let _ = p.execute_command(
        value["command"].as_str().unwrap().into(),
        value["signature"].as_str().unwrap().into(),
        value["recovery_id"].as_u64().unwrap() as u8,
        match name {
            "dispatch" => Some(account("sink")),
            "owner_withdraw" => Some(account("owner")),
            _ => None,
        },
    );
}

fn settle_bridge(project: &mut Project, operation: u64, success: bool) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("p0.workspace.test.near".parse().unwrap())
            .predecessor_account_id("p0.workspace.test.near".parse().unwrap())
            .account_balance(NearToken::from_near(100))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![if success {
            PromiseResult::Successful(vec![])
        } else {
            PromiseResult::Failed
        }]
    );
    project.settle_bridge(format!("{operation:064x}"));
}

fn refund(project: &mut Project, amount: u128) -> u128 {
    let route = "44".repeat(32);
    let receiver = project.route_receiver(route.clone());
    context("token");
    match project.ft_on_transfer(
        receiver,
        U128(amount),
        near_sdk::serde_json::json!({"kind": "refund", "route": route}).to_string(),
    ) {
        near_sdk::PromiseOrValue::Value(unused) => unused.0,
        _ => panic!("refund must settle synchronously at the Project"),
    }
}

fn accounted_custody(project: &Project) -> u64 {
    project.get_bridge()["accounted_custody"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
#[should_panic(expected = "CUSTODY_CAPACITY_EXCEEDED")]
fn a_full_reservation_does_not_create_capacity_for_more_owner_funding() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    execute(&mut p, "reserve_capacity");
    assert_eq!(p.free, 0);
    assert_eq!(accounted_custody(&p), u64::MAX);
    assert_eq!(p.get_bridge()["held"], u64::MAX.to_string());
    fund(&mut p, 1);
}

#[test]
fn closing_a_full_reservation_only_reclassifies_existing_custody() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    execute(&mut p, "reserve_capacity");
    execute(&mut p, "close_capacity");
    assert_eq!(p.free, u128::from(u64::MAX));
    assert_eq!(accounted_custody(&p), u64::MAX);
    assert_eq!(p.get_bridge()["held"], "0");
}

#[test]
fn pending_task_failure_retains_capacity_and_restores_after_intervening_funding() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX - 10));
    context("agent");
    p.create_task(U128(60), [1; 32]);
    execute(&mut p, "reserve");
    execute(&mut p, "allocate");
    context("agent");
    p.withdraw(0, U128(20)).detach();
    assert_eq!(accounted_custody(&p), u64::MAX - 10);
    fund(&mut p, 10);
    assert_eq!(accounted_custody(&p), u64::MAX);
    settle(&mut p, 0, false);
    assert_eq!(p.get_task(0).unwrap().available.0, 60);
    assert_eq!(p.get_task(0).unwrap().pending_outflow.0, 0);
    assert_eq!(accounted_custody(&p), u64::MAX);
    assert_eq!(
        p.free + p.get_task(0).unwrap().available.0,
        u128::from(u64::MAX)
    );
}

#[test]
#[should_panic(expected = "CUSTODY_CAPACITY_EXCEEDED")]
fn unresolved_task_withdrawal_never_frees_ingress_capacity() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    context("agent");
    p.create_task(U128(60), [1; 32]);
    execute(&mut p, "reserve");
    execute(&mut p, "allocate");
    context("agent");
    p.withdraw(0, U128(20)).detach();
    fund(&mut p, 1);
}

#[test]
fn pending_bridge_failures_restore_once_after_intervening_funding_and_closure() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX - 20));
    execute(&mut p, "reserve");
    execute(&mut p, "refund");
    settle_bridge(&mut p, 11, true);
    execute(&mut p, "dispatch");
    assert_eq!(accounted_custody(&p), u64::MAX - 20);
    fund(&mut p, 10);
    execute(&mut p, "close_transport");
    settle_bridge(&mut p, 12, false);
    settle_bridge(&mut p, 12, false);
    assert_eq!(p.free, u128::from(u64::MAX - 10));
    assert_eq!(accounted_custody(&p), u64::MAX - 10);

    execute(&mut p, "owner_withdraw");
    assert_eq!(accounted_custody(&p), u64::MAX - 10);
    fund(&mut p, 10);
    settle_bridge(&mut p, 22, false);
    settle_bridge(&mut p, 22, false);
    assert_eq!(p.free, u128::from(u64::MAX));
    assert_eq!(accounted_custody(&p), u64::MAX);
}

#[test]
#[should_panic(expected = "CUSTODY_CAPACITY_EXCEEDED")]
fn unresolved_dispatch_never_frees_ingress_capacity() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    execute(&mut p, "reserve");
    execute(&mut p, "refund");
    settle_bridge(&mut p, 11, true);
    execute(&mut p, "dispatch");
    fund(&mut p, 1);
}

#[test]
#[should_panic(expected = "CUSTODY_CAPACITY_EXCEEDED")]
fn unresolved_owner_withdrawal_never_frees_ingress_capacity() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    execute(&mut p, "owner_withdraw");
    fund(&mut p, 1);
}

#[test]
fn reclassification_regrant_and_actual_return_preserve_aggregate_custody() {
    let mut p = coordinated_project();
    context("agent");
    p.create_task(U128(60), [1; 32]);
    execute(&mut p, "reserve");
    execute(&mut p, "allocate");
    execute(&mut p, "pause");
    execute(&mut p, "reclaim");
    execute(&mut p, "restore");
    execute(&mut p, "reserve_restoration");
    execute(&mut p, "allocate_restoration");
    execute(&mut p, "continue");
    assert_eq!(accounted_custody(&p), 100);
    assert_eq!(p.get_task(0).unwrap().available.0, 70);
    context("agent");
    p.withdraw(0, U128(20)).detach();
    assert_eq!(accounted_custody(&p), 100);
    settle(&mut p, 0, true);
    assert_eq!(accounted_custody(&p), 80);
    assert!(!p.settle_outflow(0));
    assert_eq!(accounted_custody(&p), 80);
    context("token");
    let _ = p.ft_on_transfer(
        account("agent"),
        U128(7),
        r#"{"kind":"return","task_id":0}"#.into(),
    );
    assert_eq!(accounted_custody(&p), 87);
    context("agent");
    p.release(0, U128(50));
    assert_eq!(accounted_custody(&p), 87);
    assert_eq!(p.request_budget(0, U128(30), [1; 32]), 2);
    execute(&mut p, "reserve_regrant");
    execute(&mut p, "allocate_regrant");
    assert_eq!(p.get_task(0).unwrap().available.0, 30);
    assert_eq!(p.get_task(0).unwrap().issued.0, 120);
    assert_eq!(accounted_custody(&p), 87);
    context("agent");
    p.release(0, U128(30));
    assert_eq!(p.finalize(0, None).0, 13);
    assert_eq!(p.free, 87);
    assert_eq!(accounted_custody(&p), 87);
}

#[test]
fn confirmed_owner_withdrawal_releases_capacity_exactly_once() {
    let mut p = coordinated_project();
    execute(&mut p, "owner_withdraw");
    assert_eq!(accounted_custody(&p), 100);
    settle_bridge(&mut p, 22, true);
    settle_bridge(&mut p, 22, true);
    assert_eq!(p.free, 80);
    assert_eq!(accounted_custody(&p), 80);
    fund(&mut p, 20);
    assert_eq!(p.free, 100);
    assert_eq!(accounted_custody(&p), 100);
}

#[test]
#[should_panic(expected = "CUSTODY_CAPACITY_EXCEEDED")]
fn an_actual_agent_return_cannot_exceed_aggregate_capacity() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    context("agent");
    p.create_task(U128(60), [1; 32]);
    execute(&mut p, "reserve");
    execute(&mut p, "allocate");
    context("agent");
    p.withdraw(0, U128(20)).detach();
    settle(&mut p, 0, true);
    fund(&mut p, 20);
    context("token");
    let _ = p.ft_on_transfer(
        account("agent"),
        U128(1),
        r#"{"kind":"return","task_id":0}"#.into(),
    );
}

#[test]
#[should_panic(expected = "CUSTODY_CAPACITY_EXCEEDED")]
fn actual_route_delivery_cannot_exceed_aggregate_capacity() {
    let mut p = coordinated_project_with_funds(u128::from(u64::MAX));
    execute(&mut p, "reserve");
    execute(&mut p, "delivery");
    settle_bridge(&mut p, 10, true);
    let route = "22".repeat(32);
    let sender = p.route_receiver(route.clone());
    context("token");
    let _ = p.ft_on_transfer(
        sender,
        U128(40),
        near_sdk::serde_json::json!({"kind":"delivery", "route":route}).to_string(),
    );
}

#[test]
fn refunds_require_confirmed_dispatch_and_are_cumulatively_bounded() {
    let mut p = coordinated_project();
    execute(&mut p, "reserve");
    execute(&mut p, "refund");
    settle_bridge(&mut p, 11, true);
    assert_eq!(refund(&mut p, 7), 7);
    assert_eq!(p.free, 40);

    execute(&mut p, "dispatch");
    assert_eq!(p.get_command(format!("{:064x}", 12))["state"], 4);
    assert_eq!(refund(&mut p, 7), 7);
    assert_eq!(p.free, 40);
    assert_eq!(p.get_route("44".repeat(32))["dispatched"], "0");

    settle_bridge(&mut p, 12, true);
    assert_eq!(refund(&mut p, 7), 0);
    assert_eq!(refund(&mut p, 20), 7);
    assert_eq!(refund(&mut p, 1), 1);
    assert_eq!(p.free, 60);
    let route = p.get_route("44".repeat(32));
    assert_eq!(route["received"], "20");
    assert_eq!(route["dispatched"], "20");
    // A replayed callback cannot reverse success or restore dispatched funds.
    settle_bridge(&mut p, 12, false);
    assert_eq!(p.get_command(format!("{:064x}", 12))["state"], 1);
    assert_eq!(p.get_plan(format!("{:064x}", 1))["held"], "40");
    assert_eq!(p.free, 60);
}

#[test]
fn failed_dispatch_after_plan_closure_restores_cash_once_without_refund_credit() {
    let mut p = coordinated_project();
    execute(&mut p, "reserve");
    execute(&mut p, "refund");
    settle_bridge(&mut p, 11, true);
    execute(&mut p, "dispatch");
    execute(&mut p, "close_transport");
    assert_eq!(p.free, 80);
    assert_eq!(refund(&mut p, 20), 20);

    settle_bridge(&mut p, 12, false);
    settle_bridge(&mut p, 12, false);
    assert_eq!(p.free, 100);
    assert_eq!(refund(&mut p, 20), 20);
    let route = p.get_route("44".repeat(32));
    assert_eq!(route["received"], "0");
    assert_eq!(route["dispatched"], "0");
    assert_eq!(p.get_plan(format!("{:064x}", 1))["held"], "0");
    assert_eq!(p.get_bridge()["held"], "0");
}

fn after_command_deadline() {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id("p0.workspace.test.near".parse().unwrap())
            .predecessor_account_id(account("relayer"))
            .attached_deposit(NearToken::from_near(1))
            .account_balance(NearToken::from_near(100))
            .prepaid_gas(Gas::from_tgas(300))
            .block_timestamp(2_000_000_001_000_000_000)
            .build()
    );
}

#[test]
fn expired_exact_command_replay_keeps_immutable_success_without_another_reservation() {
    let mut p = coordinated_project();
    execute(&mut p, "reserve");
    let before = p.get_command(format!("{:064x}", 1));
    let signed = coordinated_signature("reserve");
    after_command_deadline();
    assert!(matches!(
        p.execute_command(
            signed["command"].as_str().unwrap().into(),
            signed["signature"].as_str().unwrap().into(),
            signed["recovery_id"].as_u64().unwrap() as u8,
            None,
        ),
        near_sdk::PromiseOrValue::Value(_)
    ));
    assert_eq!(p.get_command(format!("{:064x}", 1)), before);
    assert_eq!(p.free, 40);
    assert_eq!(p.get_plan(format!("{:064x}", 1))["held"], "60");
}

#[test]
#[should_panic(expected = "COMMAND_EXPIRED")]
fn expired_unexecuted_command_cannot_create_a_reservation() {
    let mut p = coordinated_project();
    let signed = coordinated_signature("reserve");
    after_command_deadline();
    let _ = p.execute_command(
        signed["command"].as_str().unwrap().into(),
        signed["signature"].as_str().unwrap().into(),
        signed["recovery_id"].as_u64().unwrap() as u8,
        None,
    );
}
#[test]
fn signed_coordinated_plan_preserves_agent_and_owner_local_rules() {
    let mut p = coordinated_project();
    context("agent");
    p.create_task(U128(60), [1; 32]);
    execute(&mut p, "reserve");
    assert_eq!(p.free, 40);
    execute(&mut p, "reserve");
    assert_eq!(p.free, 40);
    execute(&mut p, "allocate");
    assert_eq!(p.get_task(0).unwrap().available.0, 60);
    execute(&mut p, "allocate");
    assert_eq!(p.get_task(0).unwrap().issued.0, 60);
    execute(&mut p, "pause");
    execute(&mut p, "reclaim");
    assert_eq!(p.free, 60);
    assert_eq!(p.get_task(0).unwrap().available.0, 40);
    execute(&mut p, "restore");
    assert_eq!(p.get_task(0).unwrap().live_request, Some(1));
    assert_eq!(p.get_request(1).unwrap().state, RequestState::Pending);
    execute(&mut p, "continue");
    assert!(!p.get_task(0).unwrap().paused);
    execute(&mut p, "close");
    assert_eq!(p.free, 60);
    assert_eq!(p.get_task(0).unwrap().available.0, 40);
}
#[test]
#[should_panic(expected = "COORDINATOR_REQUIRED")]
fn coordinated_binding_disables_direct_local_authority() {
    let mut p = coordinated_project();
    context("agent");
    p.create_task(U128(60), [1; 32]);
    context("authority");
    p.approve_request(0);
}
#[test]
#[should_panic(expected = "UNAUTHORIZED_COMMAND_SIGNATURE")]
fn modifying_signed_amount_cannot_reserve_funds() {
    let mut p = coordinated_project();
    let v = coordinated_signature("reserve");
    let mut c = apodect_near_protocol::Command::parse(v["command"].as_str().unwrap());
    c.words[7] = apodect_near_protocol::uint(61);
    context("relayer");
    let _ = p.execute_command(
        c.hex(),
        v["signature"].as_str().unwrap().into(),
        v["recovery_id"].as_u64().unwrap() as u8,
        None,
    );
}
