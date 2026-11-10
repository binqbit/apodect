use apodect_solana::instruction;
use apodect_solana::state::{RequestState, TaskLifecycle};

use crate::support::{rejects, Fixture, Role};

#[test]
fn strict_fifo_full_allocations_and_repeated_apply() {
    let mut f = Fixture::new();

    f.fund(50).unwrap();

    let (first, a) = f.create_task(60);
    let (second, b) = f.create_task(10);

    f.approve(first, a).unwrap();
    f.approve(second, b).unwrap();

    assert_eq!(f.project_state().free, 50);
    assert_eq!(f.request(a).approval_sequence, 1);
    assert_eq!(f.request(b).approval_sequence, 2);
    rejects(f.apply(b, Some(second)), "WrongQueueAccount");
    rejects(f.apply(a, Some(first)), "InsufficientProjectFunds");
    assert_eq!(f.task(first).issued, 0);

    f.fund(10).unwrap();
    f.apply(a, Some(first)).unwrap();

    assert_eq!(f.task(first).available, 60);
    assert_eq!(f.project_state().free, 0);
    assert!(f.apply(a, Some(first)).is_err());
    assert!(f.apply(b, Some(second)).is_err());

    f.fund(10).unwrap();
    f.apply(b, Some(second)).unwrap();

    assert_eq!(f.task(second).issued, 10);
    assert!(f.project_state().head.is_none());
}

#[test]
fn removed_queue_heads_advance_one_at_a_time_and_live_heads_cannot_be_skipped() {
    let mut f = Fixture::new();

    f.fund(20).unwrap();

    let (a_task, a) = f.create_task(90);
    let (b_task, b) = f.create_task(80);
    let (c_task, c) = f.create_task(20);

    f.approve(a_task, a).unwrap();
    f.approve(b_task, b).unwrap();
    f.approve(c_task, c).unwrap();

    assert!(f.cancel(a_task, None).is_err());

    f.cancel(a_task, Some(a)).unwrap();
    f.out_of_budget(b_task, Some(b)).unwrap();

    assert_eq!(f.project_state().head, Some(a));
    assert!(f.apply(c, Some(c_task)).is_err());
    assert!(f.apply(a, Some(a_task)).is_err());

    f.apply(a, None).unwrap();

    assert_eq!(f.project_state().head, Some(b));
    assert!(f.apply(c, Some(c_task)).is_err());

    f.apply(b, None).unwrap();

    assert_eq!(f.project_state().head, Some(c));

    f.apply(c, Some(c_task)).unwrap();

    assert_eq!(f.task(c_task).available, 20);
}

#[test]
fn append_after_removed_tail_and_rejected_initial_replacement() {
    let mut f = Fixture::new();
    let (a_task, a) = f.create_task(10);

    f.reject(a_task, a).unwrap();

    assert_eq!(f.task(a_task).lifecycle, TaskLifecycle::Active);

    let replacement = f.submit(a_task, 11).unwrap();

    f.approve(a_task, replacement).unwrap();
    f.cancel(a_task, Some(replacement)).unwrap();

    let (b_task, b) = f.create_task(12);

    f.approve(b_task, b).unwrap();

    assert_eq!(f.request(replacement).next, Some(b));

    f.apply(replacement, None).unwrap();
    f.fund(12).unwrap();
    f.apply(b, Some(b_task)).unwrap();

    assert_eq!(f.request(a).state, RequestState::Rejected);
    assert_eq!(f.task(b_task).issued, 12);
}

#[test]
fn restoration_joins_fifo_and_never_funded_task_cannot_restore() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(20);

    f.pause(task).unwrap();
    f.reclaim(task, 20).unwrap();

    let (earlier, request) = f.create_task(30);

    f.approve(earlier, request).unwrap();

    let restoration = f.restore(task, 10).unwrap();
    assert_eq!(f.request(request).next, Some(restoration));
    assert!(f.apply(restoration, Some(task)).is_err());
    assert!(f.apply(request, Some(earlier)).is_err());

    f.fund(20).unwrap();
    f.apply(request, Some(earlier)).unwrap();
    f.apply(restoration, Some(task)).unwrap();

    let (never, _) = f.create_task(1);

    f.pause(never).unwrap();

    assert!(f.restore(never, 1).is_err());
}

#[test]
fn head_tail_and_live_request_omission_or_substitution_cannot_change_the_queue() {
    let mut f = Fixture::new();
    let (a_task, a) = f.create_task(1);
    let (b_task, b) = f.create_task(2);

    f.approve(a_task, a).unwrap();

    assert!(f
        .run(
            instruction::ApproveRequest {},
            f.approve_accounts(b_task, b, Role::Authority, None),
            Role::Authority
        )
        .is_err());
    assert!(f
        .run(
            instruction::ApproveRequest {},
            f.approve_accounts(b_task, b, Role::Authority, Some(b)),
            Role::Authority
        )
        .is_err());

    f.approve(b_task, b).unwrap();

    assert!(f.apply(a, None).is_err());
    assert!(f.apply(a, Some(b_task)).is_err());
    assert!(f.cancel(a_task, Some(b)).is_err());
    assert_eq!(f.task(a_task).pending, Some(a));
    assert_eq!(f.project_state().head, Some(a));

    f.cancel(a_task, Some(a)).unwrap();
    f.apply(a, None).unwrap();

    assert!(f.cancel(a_task, Some(a)).is_err());
    assert_eq!(f.project_state().head, Some(b));
}
