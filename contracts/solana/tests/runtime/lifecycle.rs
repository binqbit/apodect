use anchor_lang::{AnchorDeserialize, Discriminator};
use apodect_solana::events::{Action, StopReason, Transition};
use apodect_solana::instruction;
use apodect_solana::state::{RequestState, TaskLifecycle};
use base64::{engine::general_purpose::STANDARD, Engine};

use crate::support::{ix, rejects, send, send_logs, Fixture, Role};

fn reject_event(logs: &[String]) -> Transition {
    logs.iter()
        .filter_map(|line| line.strip_prefix("Program data: "))
        .filter_map(|text| {
            let data = STANDARD.decode(text).ok()?;
            let mut payload = data.strip_prefix(Transition::DISCRIMINATOR)?;
            Transition::deserialize(&mut payload).ok()
        })
        .find(|event| matches!(event.action, Action::Reject))
        .unwrap_or_else(|| panic!("missing structured rejection event: {logs:?}"))
}

#[test]
fn rejecting_restart_preserves_stopped_without_a_new_supplement_rejection_reason() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(20);

    f.withdraw(task, 1).unwrap();

    let supplement = f.submit(task, 2).unwrap();
    let reject = ix(
        instruction::RejectRequest {},
        f.reject_accounts(task, supplement, Role::Authority),
    );
    let logs = send_logs(&mut f.svm, &f.owner, &[reject], &[&f.authority]).unwrap();
    assert!(matches!(
        reject_event(&logs).stop_reason,
        Some(StopReason::SupplementRejected)
    ));
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Stopped);

    let restart = f.submit(task, 3).unwrap();
    let reject = ix(
        instruction::RejectRequest {},
        f.reject_accounts(task, restart, Role::Authority),
    );
    let logs = send_logs(&mut f.svm, &f.owner, &[reject], &[&f.authority]).unwrap();
    assert!(
        reject_event(&logs).stop_reason.is_none(),
        "rejecting an already-stopped restart is not a new supplement rejection"
    );
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Stopped);
    assert!(f.task(task).pending.is_none());
    assert_eq!(f.task(task).available, 19);
}

#[test]
fn supplement_rejection_stops_spend_and_restart_preserves_owner_pause() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(100);

    f.withdraw(task, 10).unwrap();

    let supplement = f.submit(task, 20).unwrap();
    assert!(f.submit(task, 1).is_err());

    f.reject(task, supplement).unwrap();

    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Stopped);
    assert_eq!(f.task(task).available, 90);
    assert!(f.withdraw(task, 1).is_err());

    f.pause(task).unwrap();

    assert!(f.continue_task(task).is_err());
    assert!(f.restore(task, 1).is_err());

    let restart = f.submit(task, 5).unwrap();

    f.approve(task, restart).unwrap();
    f.fund(5).unwrap();
    f.apply(restart, Some(task)).unwrap();

    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Active);
    assert!(f.task(task).paused);
    assert!(f.withdraw(task, 1).is_err());

    f.continue_task(task).unwrap();
    f.withdraw(task, 1).unwrap();

    assert_eq!(f.task(task).issued, 105);
}

#[test]
fn ordinary_cancel_releases_before_use_and_loses_after_first_withdrawal() {
    let mut f = Fixture::new();
    let (unused, _) = f.funded_task(30);
    assert!(f.submit(unused, 1).is_err());

    f.cancel(unused, None).unwrap();

    assert_eq!(f.task(unused).lifecycle, TaskLifecycle::Canceled);
    assert_eq!(f.project_state().free, 30);
    assert!(f.withdraw(unused, 1).is_err());
    assert!(f.cancel(unused, None).is_err());

    let (used, request) = f.create_task(30);

    f.approve(used, request).unwrap();
    f.apply(request, Some(used)).unwrap();
    f.withdraw(used, 1).unwrap();

    assert!(f.cancel(used, None).is_err());
    assert!(f.task(used).started);
    assert_eq!(f.task(used).available, 29);
}

#[test]
fn cancel_and_first_use_in_one_transaction_cannot_persist_conflicting_outcomes() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(20);
    let cancel = ix(
        instruction::CancelTask {},
        f.cancel_accounts(task, Role::Owner, None),
    );
    let withdraw = ix(
        instruction::Withdraw { amount: 5 },
        f.withdraw_accounts(task),
    );
    let before: Vec<_> = [f.project, task, f.vault, f.agent_token]
        .into_iter()
        .map(|key| {
            let account = f.svm.get_account(&key).unwrap();
            (key, account.data, account.lamports)
        })
        .collect();

    for (instructions, expected) in [
        ([cancel.clone(), withdraw.clone()], "InvalidTaskState"),
        ([withdraw, cancel], "AlreadyStarted"),
    ] {
        rejects(
            send(&mut f.svm, &f.owner, &instructions, &[&f.agent]),
            expected,
        );

        for (key, data, lamports) in &before {
            let account = f.svm.get_account(key).unwrap();
            assert_eq!(&account.data, data);
            assert_eq!(account.lamports, *lamports);
        }
    }

    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Active);
    assert!(!f.task(task).started);
    assert_eq!(f.task(task).available, 20);
    assert_eq!(f.tokens(f.agent_token), 0);
}

#[test]
fn retained_continue_and_reclaim_restore_do_not_inflate_expense() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(100);

    f.withdraw(task, 40).unwrap();
    f.pause(task).unwrap();

    assert!(f.withdraw(task, 1).is_err());

    f.return_funds(task, 5).unwrap();
    f.continue_task(task).unwrap();

    assert_eq!(f.task(task).issued, 100);
    assert!(f.task(task).started);

    f.pause(task).unwrap();
    f.reclaim(task, 60).unwrap();

    assert_eq!(f.project_state().free, 65);
    assert!(f.continue_task(task).is_err());

    let restoration = f.restore(task, 20).unwrap();
    assert!(f.continue_task(task).is_err());

    f.apply(restoration, Some(task)).unwrap();

    assert!(f.task(task).paused);
    assert!(f.withdraw(task, 1).is_err());

    f.continue_task(task).unwrap();
    f.release(task, 20).unwrap();
    f.finalize(task, None).unwrap();

    let state = f.task(task);
    assert_eq!(state.issued, 120);
    assert_eq!(state.recovered, 85);
    assert_eq!(state.final_expense().unwrap(), Some(35));
    assert_eq!(f.project_state().free, 65);
    assert_eq!(f.tokens(f.vault), 65);
}

#[test]
fn out_of_budget_preserves_occupied_cash_allows_cleanup_and_disables_reopening() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(50);

    f.withdraw(task, 20).unwrap();

    let request = f.submit(task, 10).unwrap();

    f.approve(task, request).unwrap();

    assert!(f.out_of_budget(task, None).is_err());

    f.out_of_budget(task, Some(request)).unwrap();

    assert_eq!(f.project_state().free, 0);
    assert_eq!(f.task(task).available, 30);
    assert!(f.withdraw(task, 1).is_err());
    assert!(f.submit(task, 1).is_err());

    f.pause(task).unwrap();

    assert!(f.task(task).paused);
    assert!(f.continue_task(task).is_err());
    assert!(f.restore(task, 1).is_err());

    f.return_funds(task, 10).unwrap();
    f.reclaim(task, 10).unwrap();
    f.release(task, 20).unwrap();

    assert_eq!(f.task(task).lifecycle, TaskLifecycle::OutOfBudget);
    assert!(f.task(task).paused);

    f.finalize(task, None).unwrap();

    assert_eq!(f.task(task).final_expense().unwrap(), Some(10));

    f.apply(request, None).unwrap();

    let _ = f.create_task(5);
    assert_eq!(f.project_state().free, 40);
}

#[test]
fn finalize_requires_exact_live_request_and_invalidates_queued_supplement() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(10);

    f.withdraw(task, 10).unwrap();

    let supplement = f.submit(task, 2).unwrap();

    f.approve(task, supplement).unwrap();

    let (_, foreign) = f.create_task(1);
    rejects(f.finalize(task, None), "WrongLiveRequest");
    rejects(f.finalize(task, Some(foreign)), "WrongLiveRequest");
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Active);

    f.finalize(task, Some(supplement)).unwrap();

    assert_eq!(f.request(supplement).state, RequestState::Removed);
    assert_eq!(f.task(task).final_expense().unwrap(), Some(10));
    assert!(f.task(task).pending.is_none());

    f.apply(supplement, None).unwrap();

    let (never, never_request) = f.create_task(1);
    assert!(f.finalize(never, Some(never_request)).is_err());
}

#[test]
fn stopped_and_paused_tasks_can_retire_rights_without_resuming() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(20);

    f.withdraw(task, 5).unwrap();

    let supplement = f.submit(task, 1).unwrap();

    f.reject(task, supplement).unwrap();
    f.pause(task).unwrap();
    f.return_funds(task, 2).unwrap();
    f.release(task, 15).unwrap();

    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Stopped);
    assert!(f.task(task).paused);
    assert!(f.continue_task(task).is_err());

    f.finalize(task, None).unwrap();

    assert_eq!(f.task(task).final_expense().unwrap(), Some(3));
    assert_eq!(f.project_state().free, 17);
}
