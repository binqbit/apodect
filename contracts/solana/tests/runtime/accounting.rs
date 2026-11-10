use apodect_solana::state::{Invoice, RequestState};
use apodect_solana::{accounts, instruction};
use solana_signer::Signer;

use crate::support::{rejects, Fixture, Role};

#[test]
fn accounting_example_uses_real_token_transfers_and_final_expense_25() {
    let mut f = Fixture::new();

    f.fund(100).unwrap();

    let (task, request) = f.create_task(60);

    f.approve(task, request).unwrap();

    assert_eq!(f.project_state().free, 100);

    f.apply(request, Some(task)).unwrap();

    assert_eq!(f.project_state().free, 40);
    assert!(f.finalize(task, None).is_err());

    f.withdraw(task, 40).unwrap();

    assert_eq!(f.tokens(f.agent_token), 40);

    f.return_funds(task, 15).unwrap();

    assert_eq!(f.project_state().free, 55);
    assert_eq!(f.task(task).available, 20);

    f.release(task, 20).unwrap();
    f.finalize(task, None).unwrap();

    let state = f.task(task);
    assert_eq!(state.final_expense().unwrap(), Some(25));
    assert_eq!(state.recovered, 35);
    assert_eq!(f.project_state().free, 75);
    assert_eq!(f.tokens(f.vault), 75);
    assert_eq!(f.tokens(f.agent_token), 25);
    assert_eq!(state.available, 0);
    assert!(f.finalize(task, None).is_err());
    assert!(f.return_funds(task, 1).is_err());
    assert!(f.withdraw(task, 1).is_err());
    assert!(f.submit(task, 1).is_err());
    assert_eq!(f.task(task).final_expense().unwrap(), Some(25));
}

#[test]
fn recycled_principal_follows_fifo_and_regrants_without_refilling_old_rights() {
    let mut f = Fixture::with_supply(20);

    f.fund(20).unwrap();

    let (task, initial) = f.create_task(10);

    f.approve(task, initial).unwrap();
    f.apply(initial, Some(task)).unwrap();
    f.withdraw(task, 10).unwrap();
    f.return_funds(task, 10).unwrap();

    assert_eq!(f.task(task).available, 0);
    assert_eq!(f.project_state().free, 20);
    rejects(f.withdraw(task, 1), "InsufficientAllocation");

    let (earlier, earlier_request) = f.create_task(15);

    f.approve(earlier, earlier_request).unwrap();

    for cycle in 0..3 {
        let supplement = f.submit(task, 10).unwrap();

        f.approve(task, supplement).unwrap();

        if cycle == 0 {
            rejects(f.apply(supplement, Some(task)), "WrongQueueAccount");

            f.apply(earlier_request, Some(earlier)).unwrap();

            rejects(f.apply(supplement, Some(task)), "InsufficientProjectFunds");
            assert_eq!(f.task(task).available, 0);
            assert_eq!(f.task(earlier).available, 15);

            f.release(earlier, 5).unwrap();
        }

        f.apply(supplement, Some(task)).unwrap();

        assert_eq!(f.project_state().free, 0);
        assert_eq!(f.task(task).available, 10);
        assert_eq!(f.task(earlier).available, 10);
        assert_eq!(f.tokens(f.vault), 20);
        rejects(
            f.run(
                instruction::WithdrawFree { amount: 1 },
                f.withdraw_free_accounts(),
                Role::Owner,
            ),
            "InsufficientProjectFunds",
        );

        f.withdraw(task, 10).unwrap();
        f.return_funds(task, 10).unwrap();

        assert_eq!(f.task(task).available, 0);
        assert_eq!(f.task(task).external_outstanding().unwrap(), 0);
        assert_eq!(f.project_state().free, 10);
        assert_eq!(f.tokens(f.vault), 20);
        assert_eq!(f.tokens(f.agent_token), 0);
    }

    f.finalize(task, None).unwrap();

    assert_eq!(f.task(task).issued, 40);
    assert_eq!(f.task(task).recovered, 40);
    assert_eq!(f.task(task).final_expense().unwrap(), Some(0));
    rejects(
        f.run(
            instruction::WithdrawFree { amount: 11 },
            f.withdraw_free_accounts(),
            Role::Owner,
        ),
        "InsufficientProjectFunds",
    );

    f.run(
        instruction::WithdrawFree { amount: 10 },
        f.withdraw_free_accounts(),
        Role::Owner,
    )
    .unwrap();

    assert_eq!(f.tokens(f.vault), 10);
    assert_eq!(f.task(earlier).available, 10);

    f.cancel(earlier, None).unwrap();
    f.run(
        instruction::WithdrawFree { amount: 10 },
        f.withdraw_free_accounts(),
        Role::Owner,
    )
    .unwrap();

    assert_eq!(f.tokens(f.owner_token), 20);
    assert_eq!(f.tokens(f.vault), 0);
    assert_eq!(f.project_state().free, 0);
}

#[test]
fn zero_excess_and_integer_boundary_fail_without_financial_mutation() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(10);
    assert!(f.fund(0).is_err());
    assert!(f.withdraw(task, 0).is_err());
    assert!(f.withdraw(task, 11).is_err());
    assert!(f.return_funds(task, 1).is_err());
    assert!(f.release(task, 11).is_err());
    assert_eq!(f.task(task).available, 10);
    assert_eq!(f.task(task).issued, 10);
    assert_eq!(f.project_state().free, 0);
    assert!(!f.task(task).started);

    f.withdraw(task, 5).unwrap();

    assert!(f.return_funds(task, 6).is_err());

    f.return_funds(task, 5).unwrap();

    assert_eq!(f.task(task).available, 5);
    assert_eq!(f.project_state().free, 5);

    let mut boundary = Fixture::with_supply(u64::MAX);
    let (task, _) = boundary.funded_task(u64::MAX);

    boundary.withdraw(task, u64::MAX).unwrap();
    boundary.return_funds(task, u64::MAX).unwrap();

    let extra = boundary.submit(task, 1).unwrap();

    boundary.approve(task, extra).unwrap();

    rejects(boundary.apply(extra, Some(task)), "Arithmetic");
    assert_eq!(boundary.project_state().free, u64::MAX);
    assert_eq!(boundary.task(task).issued, u64::MAX);
    assert_eq!(boundary.task(task).available, 0);
    assert_eq!(boundary.request(extra).state, RequestState::Queued);
}

#[test]
fn authority_rotation_preserves_approved_work_and_invoices() {
    let mut f = Fixture::new();

    f.fund(20).unwrap();

    let (task, request) = f.create_task(20);

    f.approve(task, request).unwrap();

    let invoice = f.invoice(100);

    f.run(
        instruction::SetAuthority {
            authority: f.stranger.pubkey(),
        },
        accounts::SetAuthority {
            workspace: f.workspace,
            project: f.project,
            owner: f.owner.pubkey(),
        },
        Role::Owner,
    )
    .unwrap();

    assert!(f.apply(request, Some(task)).is_err());

    f.run(
        instruction::ApplyNext {},
        accounts::ApplyNext {
            project: f.project,
            head: request,
            authority: f.stranger.pubkey(),
            task: Some(task),
        },
        Role::Stranger,
    )
    .unwrap();
    f.respond(invoice, 60).unwrap();

    let state = f.read::<Invoice>(invoice);
    assert!(state.closed);
    assert_eq!(state.intended_contribution, 60);
    assert_eq!(f.task(task).available, 20);
}

#[test]
fn invoice_closure_never_credits_money_and_linked_funding_is_actual() {
    let mut f = Fixture::new();
    let invoice = f.invoice(100);
    assert!(f
        .run(
            instruction::FundProject { amount: 60 },
            f.fund_accounts(Some(invoice)),
            Role::Owner
        )
        .is_err());

    f.respond(invoice, 60).unwrap();

    assert_eq!(f.project_state().free, 0);
    assert!(f.respond(invoice, 40).is_err());

    f.run(
        instruction::FundProject { amount: 60 },
        f.fund_accounts(Some(invoice)),
        Role::Owner,
    )
    .unwrap();

    assert_eq!(f.project_state().free, 60);
    assert_eq!(f.read::<Invoice>(invoice).received, 60);

    let zero = f.invoice(1);

    f.respond(zero, 0).unwrap();

    assert!(f.read::<Invoice>(zero).closed);
    assert_eq!(f.project_state().free, 60);

    f.run(
        instruction::WithdrawFree { amount: 60 },
        f.withdraw_free_accounts(),
        Role::Owner,
    )
    .unwrap();

    assert_eq!(f.project_state().free, 0);
    assert_eq!(f.tokens(f.vault), 0);
}
