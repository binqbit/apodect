use anchor_lang::prelude::Pubkey;
use apodect_solana::{accounts, instruction};
use solana_signer::Signer;

use crate::support::{ix, rejects, send, Fixture, Role};

#[test]
fn owner_authority_agent_permissions_are_distinct_and_tasks_are_open() {
    let mut f = Fixture::new();
    let (task, request) = f.create_task(10);
    assert!(f
        .run(
            instruction::ApproveRequest {},
            f.approve_accounts(task, request, Role::Owner, None),
            Role::Owner
        )
        .is_err());
    assert!(f
        .run(
            instruction::CancelTask {},
            f.cancel_accounts(task, Role::Authority, Some(request)),
            Role::Authority
        )
        .is_err());

    f.fund(10).unwrap();
    f.approve(task, request).unwrap();
    f.apply(request, Some(task)).unwrap();

    let mut wrong_agent = f.withdraw_accounts(task);
    wrong_agent.agent = f.stranger.pubkey();
    assert!(f
        .run(
            instruction::Withdraw { amount: 1 },
            wrong_agent,
            Role::Stranger
        )
        .is_err());

    let mut wrong_owner = f.withdraw_free_accounts();
    wrong_owner.owner = f.authority.pubkey();
    assert!(f
        .run(
            instruction::WithdrawFree { amount: 1 },
            wrong_owner,
            Role::Authority
        )
        .is_err());

    let next = f.project_state().task_count;
    let other_task = Pubkey::find_program_address(
        &[b"task", f.project.as_ref(), &next.to_le_bytes()],
        &apodect_solana::ID,
    )
    .0;
    let other_request = Pubkey::find_program_address(
        &[b"request", other_task.as_ref(), &0u64.to_le_bytes()],
        &apodect_solana::ID,
    )
    .0;

    f.run(
        instruction::CreateTask {
            amount: 1,
            purpose: [5; 32],
        },
        accounts::CreateTask {
            project: f.project,
            task: other_task,
            request: other_request,
            agent: f.stranger.pubkey(),
            system_program: solana_system_interface::program::ID,
        },
        Role::Stranger,
    )
    .unwrap();

    assert_eq!(f.task(other_task).agent, f.stranger.pubkey());
    assert_eq!(f.task(task).available, 10);
}

#[test]
fn correct_actor_addresses_without_signatures_cannot_change_state_or_money() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(20);
    let (waiting_task, request) = f.create_task(5);
    let attempts = [
        (
            ix(
                instruction::FundProject { amount: 10 },
                f.fund_accounts(None),
            ),
            f.owner.pubkey(),
        ),
        (
            ix(
                instruction::ApproveRequest {},
                f.approve_accounts(waiting_task, request, Role::Authority, None),
            ),
            f.authority.pubkey(),
        ),
        (
            ix(
                instruction::Withdraw { amount: 5 },
                f.withdraw_accounts(task),
            ),
            f.agent.pubkey(),
        ),
    ];
    let protected = [
        f.project,
        task,
        waiting_task,
        request,
        f.vault,
        f.owner_token,
        f.agent_token,
    ];
    let before: Vec<_> = protected
        .iter()
        .map(|key| {
            let account = f.svm.get_account(key).unwrap();
            (*key, account.data, account.lamports)
        })
        .collect();
    for (mut instruction, actor) in attempts {
        assert_ne!(actor, f.stranger.pubkey());
        let mut found_signer = false;
        for meta in &mut instruction.accounts {
            if meta.pubkey == actor {
                found_signer |= meta.is_signer;
                meta.is_signer = false;
            }
        }
        assert!(
            found_signer,
            "valid instruction originally requires actor signature"
        );
        rejects(
            send(&mut f.svm, &f.stranger, &[instruction], &[]),
            "AccountNotSigner",
        );
        for (key, data, lamports) in &before {
            let account = f.svm.get_account(key).unwrap();
            assert_eq!(&account.data, data);
            assert_eq!(account.lamports, *lamports);
        }
    }
}

#[test]
fn project_custody_task_request_and_vault_substitution_are_rejected() {
    let mut f = Fixture::new();
    let (task, request) = f.funded_task(30);
    let original_project = f.project;
    let original_vault = f.vault;
    let (other_project, other_vault) = f.create_project();
    assert_ne!(original_vault, other_vault);

    let mut wrong = f.withdraw_accounts(task);
    wrong.vault = other_vault;
    assert!(f
        .run(instruction::Withdraw { amount: 1 }, wrong, Role::Agent)
        .is_err());

    let mut duplicate = f.withdraw_accounts(task);
    duplicate.agent_token = f.vault;
    assert!(f
        .run(instruction::Withdraw { amount: 1 }, duplicate, Role::Agent)
        .is_err());

    f.project = other_project;
    f.vault = other_vault;
    f.fund(7).unwrap();

    assert!(f.withdraw(task, 1).is_err());
    assert!(f.apply(request, Some(task)).is_err());
    assert_eq!(f.project_state().free, 7);
    assert_eq!(f.tokens(other_vault), 7);

    f.project = original_project;
    f.vault = original_vault;

    assert_eq!(f.task(task).available, 30);
    assert_eq!(f.tokens(original_vault), 30);
}
