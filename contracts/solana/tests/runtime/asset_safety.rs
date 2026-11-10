use anchor_lang::prelude::Pubkey;
use anchor_spl::token::spl_token;
use apodect_solana::state::{Invoice, Workspace};
use apodect_solana::{accounts, instruction};
use solana_signer::Signer;

use crate::support::{rejects, send, Fixture, Role};

#[test]
fn frozen_token_cpi_failures_roll_back_counters_and_first_use() {
    let mut f = Fixture::new();
    let (task, _) = f.funded_task(20);
    let freeze = spl_token::instruction::freeze_account(
        &spl_token::ID,
        &f.agent_token,
        &f.mint,
        &f.owner.pubkey(),
        &[],
    )
    .unwrap();

    send(&mut f.svm, &f.owner, &[freeze], &[]).unwrap();

    assert!(f.withdraw(task, 5).is_err());
    assert!(!f.task(task).started);
    assert_eq!(f.task(task).available, 20);
    assert_eq!(f.tokens(f.vault), 20);

    f.cancel(task, None).unwrap();

    assert_eq!(f.project_state().free, 20);

    let invoice = f.invoice(10);

    f.respond(invoice, 10).unwrap();

    let freeze = spl_token::instruction::freeze_account(
        &spl_token::ID,
        &f.owner_token,
        &f.mint,
        &f.owner.pubkey(),
        &[],
    )
    .unwrap();

    send(&mut f.svm, &f.owner, &[freeze], &[]).unwrap();

    assert!(f
        .run(
            instruction::FundProject { amount: 10 },
            f.fund_accounts(Some(invoice)),
            Role::Owner
        )
        .is_err());
    assert_eq!(f.project_state().free, 20);
    assert_eq!(f.read::<Invoice>(invoice).received, 0);
}

#[test]
fn wrong_mint_program_and_unsolicited_token_deposits_create_no_entitlement() {
    let mut f = Fixture::new();
    let (task, request) = f.create_task(10);

    f.approve(task, request).unwrap();

    let donation = spl_token::instruction::transfer_checked(
        &spl_token::ID,
        &f.owner_token,
        &f.mint,
        &f.vault,
        &f.owner.pubkey(),
        &[],
        10,
        6,
    )
    .unwrap();

    send(&mut f.svm, &f.owner, &[donation], &[]).unwrap();

    assert_eq!(f.tokens(f.vault), 10);
    assert_eq!(f.project_state().free, 0);
    assert!(f.apply(request, Some(task)).is_err());
    assert!(f
        .run(
            instruction::WithdrawFree { amount: 1 },
            f.withdraw_free_accounts(),
            Role::Owner
        )
        .is_err());

    let wrong_mint = f.new_mint(6);
    let mut wrong = f.fund_accounts(None);
    wrong.mint = wrong_mint;
    assert!(f
        .run(instruction::FundProject { amount: 1 }, wrong, Role::Owner)
        .is_err());

    let mut wrong_program = f.fund_accounts(None);
    wrong_program.token_program = solana_system_interface::program::ID;
    assert!(f
        .run(
            instruction::FundProject { amount: 1 },
            wrong_program,
            Role::Owner
        )
        .is_err());
    assert_eq!(f.project_state().free, 0);

    f.fund(10).unwrap();
    f.apply(request, Some(task)).unwrap();

    assert_eq!(f.task(task).available, 10);
    assert_eq!(f.tokens(f.vault), 20);
}

#[test]
fn invalid_initial_amount_purpose_and_mint_configuration_roll_back_creation() {
    let mut f = Fixture::new();
    let task = Pubkey::find_program_address(
        &[b"task", f.project.as_ref(), &0u64.to_le_bytes()],
        &apodect_solana::ID,
    )
    .0;
    let request = Pubkey::find_program_address(
        &[b"request", task.as_ref(), &0u64.to_le_bytes()],
        &apodect_solana::ID,
    )
    .0;
    let zero_accounts = accounts::CreateTask {
        project: f.project,
        task,
        request,
        agent: f.agent.pubkey(),
        system_program: solana_system_interface::program::ID,
    };
    let empty_accounts = accounts::CreateTask {
        project: f.project,
        task,
        request,
        agent: f.agent.pubkey(),
        system_program: solana_system_interface::program::ID,
    };
    rejects(
        f.run(
            instruction::CreateTask {
                amount: 0,
                purpose: [1; 32],
            },
            zero_accounts,
            Role::Agent,
        ),
        "ZeroAmount",
    );
    rejects(
        f.run(
            instruction::CreateTask {
                amount: 1,
                purpose: [0; 32],
            },
            empty_accounts,
            Role::Agent,
        ),
        "EmptyPurpose",
    );
    assert_eq!(f.project_state().task_count, 0);
    assert!(f.svm.get_account(&task).is_none());
    assert!(f.svm.get_account(&request).is_none());

    let wrong_mint = f.new_mint(9);
    let project = Pubkey::find_program_address(
        &[b"project", f.workspace.as_ref(), &1u64.to_le_bytes()],
        &apodect_solana::ID,
    )
    .0;
    let vault = Pubkey::find_program_address(&[b"vault", project.as_ref()], &apodect_solana::ID).0;
    rejects(
        f.run(
            instruction::CreateProject {
                authority: f.authority.pubkey(),
            },
            accounts::CreateProject {
                workspace: f.workspace,
                project,
                mint: wrong_mint,
                vault,
                owner: f.owner.pubkey(),
                token_program: spl_token::ID,
                system_program: solana_system_interface::program::ID,
            },
            Role::Owner,
        ),
        "InvalidMint",
    );
    assert_eq!(f.read::<Workspace>(f.workspace).project_count, 1);
    assert!(f.svm.get_account(&project).is_none());
}
