use anchor_lang::prelude::Pubkey;
use anchor_spl::token::spl_token;
use apodect_solana::{
    accounts,
    bridge::*,
    instruction,
    state::{RequestState, TaskLifecycle},
};
use k256::ecdsa::SigningKey;
use solana_signer::Signer;

use crate::support::{ix, rejects, send, Fixture, Role};

const DOMAIN: [u8; 32] = [42; 32];

fn signer() -> SigningKey {
    SigningKey::from_slice(&[7; 32]).unwrap()
}
fn key() -> [u8; 64] {
    signer().verifying_key().to_sec1_point(false).as_bytes()[1..]
        .try_into()
        .unwrap()
}
fn id(n: u8) -> [u8; 32] {
    [n; 32]
}

#[derive(Clone)]
struct Cmd([[u8; 32]; 12]);
impl Cmd {
    fn new(action: u64, operation: u8) -> Self {
        let mut c = Self([[0; 32]; 12]);
        c.0[0] = DOMAIN;
        c.0[2] = word(action);
        c.0[3] = id(operation);
        c.0[10] = word(u64::MAX);
        c
    }
    fn set(mut self, i: usize, value: [u8; 32]) -> Self {
        self.0[i] = value;
        self
    }
    fn amount(self, amount: u64) -> Self {
        self.set(7, word(amount))
    }
    fn plan(self, n: u8) -> Self {
        self.set(4, id(n))
    }
    fn task(self, task: Pubkey) -> Self {
        self.set(5, task.to_bytes())
    }
    fn request(self, request: Pubkey) -> Self {
        self.set(6, request.to_bytes())
    }
    fn bytes(&self) -> [u8; 384] {
        self.0.concat().try_into().unwrap()
    }
    fn signed(&self) -> instruction::ExecuteCommand {
        let bytes = self.bytes();
        let (signature, recovery) =
            signer().sign_prehash_recoverable(&solana_sha256_hasher::hash(&bytes).to_bytes());
        instruction::ExecuteCommand {
            bytes,
            signature: signature.to_bytes().into(),
            recovery_id: recovery.to_byte(),
        }
    }
}

fn address(f: &Fixture, seed: &[u8], id: [u8; 32]) -> Pubkey {
    pda(&[seed, f.project.as_ref(), &id])
}
fn bind(f: &mut Fixture) {
    f.fund(1_000).unwrap();
    f.run(
        instruction::BindCoordinator {
            domain: DOMAIN,
            command_key: key(),
        },
        accounts::BindCoordinator {
            workspace: f.workspace,
            project: f.project,
            owner: f.owner.pubkey(),
        },
        Role::Owner,
    )
    .unwrap();
}
fn command_accounts(f: &Fixture, c: &Cmd) -> accounts::ExecuteCommand {
    let action = integer(&c.0[2]).unwrap();
    accounts::ExecuteCommand {
        project: f.project,
        workspace: f.workspace,
        mint: f.mint,
        vault: f.vault,
        receipt: address(f, b"bridge-command", c.0[3]),
        payer: f.owner.pubkey(),
        system_program: solana_system_interface::program::ID,
        token_program: spl_token::ID,
        associated_token_program: ASSOCIATED_TOKEN_PROGRAM,
        task: (c.0[5] != [0; 32]).then(|| Pubkey::new_from_array(c.0[5])),
        request: if matches!(action, 8 | 12 | 13) {
            let task = f.task(Pubkey::new_from_array(c.0[5]));
            if action == 13 {
                Some(pda(&[
                    b"request",
                    &c.0[5],
                    &task.request_count.to_le_bytes(),
                ]))
            } else {
                task.pending
            }
        } else {
            (c.0[6] != [0; 32]).then(|| Pubkey::new_from_array(c.0[6]))
        },
        plan: (1..=6)
            .contains(&action)
            .then(|| address(f, b"bridge-plan", c.0[4])),
        route: matches!(action, 2 | 3 | 4).then(|| address(f, b"bridge-route", c.0[11])),
        escrow: matches!(action, 2 | 3).then(|| route_escrow(f.project, f.mint, c.0[11])),
        recipient: match action {
            4 => Some(associated_account(Pubkey::new_from_array(c.0[9]), f.mint)),
            14 => Some(Pubkey::new_from_array(c.0[9])),
            _ => None,
        },
        target: (action == 15).then(|| address(f, b"bridge-command", c.0[11])),
    }
}
fn execute(f: &mut Fixture, c: &Cmd) -> Result<(), String> {
    let mut instructions = Vec::new();
    if integer(&c.0[2]).unwrap() == 4 {
        instructions.push(create_associated_account(
            f.owner.pubkey(),
            Pubkey::new_from_array(c.0[9]),
            f.mint,
        ));
    }
    instructions.push(ix(c.signed(), command_accounts(f, c)));
    send(&mut f.svm, &f.owner, &instructions, &[])
}
fn collect(f: &mut Fixture, route: u8, plan: u8) -> Result<(), String> {
    let instruction = ix(
        instruction::CollectRoute {},
        accounts::CollectRoute {
            project: f.project,
            mint: f.mint,
            vault: f.vault,
            route: address(f, b"bridge-route", id(route)),
            plan: address(f, b"bridge-plan", id(plan)),
            escrow: route_escrow(f.project, f.mint, id(route)),
            token_program: spl_token::ID,
        },
    );
    send(&mut f.svm, &f.stranger, &[instruction], &[])
}

fn deposit(f: &mut Fixture, route: u8, amount: u64) {
    let token = route_escrow(f.project, f.mint, id(route));
    let ix = spl_token::instruction::transfer_checked(
        &spl_token::ID,
        &f.owner_token,
        &f.mint,
        &token,
        &f.owner.pubkey(),
        &[],
        amount,
        6,
    )
    .unwrap();
    send(&mut f.svm, &f.owner, &[ix], &[]).unwrap();
}
fn grant(f: &mut Fixture, task: Pubkey, request: Pubkey, amount: u64, plan: u8, operation: u8) {
    execute(f, &Cmd::new(1, operation).plan(plan).amount(amount)).unwrap();
    execute(
        f,
        &Cmd::new(5, operation + 1)
            .plan(plan)
            .task(task)
            .request(request)
            .amount(amount),
    )
    .unwrap();
}

#[test]
fn coordinated_binding_gates_local_authority_and_preserves_agent_cash_cycle() {
    let mut f = Fixture::new();
    bind(&mut f);
    rejects(
        f.run(
            instruction::BindCoordinator {
                domain: id(43),
                command_key: key(),
            },
            accounts::BindCoordinator {
                workspace: f.workspace,
                project: f.project,
                owner: f.owner.pubkey(),
            },
            Role::Owner,
        ),
        "CoordinatedProject",
    );
    let (task, request) = f.create_task(100);
    rejects(f.approve(task, request), "CoordinatedProject");
    rejects(f.pause(task), "CoordinatedProject");
    rejects(
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
        ),
        "CoordinatedProject",
    );
    grant(&mut f, task, request, 100, 1, 1);
    assert_eq!(f.project_state().free, 900);
    assert_eq!(f.project_state().held, 0);
    f.withdraw(task, 60).unwrap();
    f.return_funds(task, 20).unwrap();
    f.release(task, 40).unwrap();
    f.finalize(task, None).unwrap();
    assert_eq!(f.task(task).final_expense().unwrap(), Some(40));
    assert_eq!(f.project_state().free, 960);
    assert_eq!(f.tokens(f.vault), 960);
}

#[test]
fn command_authentication_canonical_encoding_replay_and_invalidation_are_atomic() {
    let mut f = Fixture::new();
    bind(&mut f);
    let reserve = Cmd::new(1, 1).plan(1).amount(100);
    let mut signed = reserve.signed();
    signed.signature[0] ^= 1;
    rejects(
        f.run(signed, command_accounts(&f, &reserve), Role::Owner),
        "InvalidSignature",
    );
    let mut high = reserve.signed();
    high.signature[32..].fill(0xff);
    rejects(
        f.run(high, command_accounts(&f, &reserve), Role::Owner),
        "InvalidSignature",
    );
    let reserved = reserve.clone().set(8, word(1));
    rejects(execute(&mut f, &reserved), "InvalidCommand");
    let wrong_domain = reserve.clone().set(0, id(43));
    rejects(execute(&mut f, &wrong_domain), "InvalidBinding");
    execute(&mut f, &reserve).unwrap();
    execute(&mut f, &reserve).unwrap();
    assert_eq!(f.project_state().free, 900);
    rejects(
        execute(&mut f, &reserve.clone().amount(101)),
        "CommandConflict",
    );
    execute(&mut f, &Cmd::new(15, 2).set(11, id(3))).unwrap();
    rejects(
        execute(&mut f, &Cmd::new(1, 3).plan(2).amount(50)),
        "CommandConflict",
    );
    execute(&mut f, &Cmd::new(15, 4).set(11, id(1))).unwrap();
    let recorded: CommandReceipt = f.read(address(&f, b"bridge-command", id(4)));
    assert_eq!(recorded.result, word(SUCCEEDED as u64));
    assert_eq!(f.project_state().free, 900);
}

#[test]
fn delivery_partial_escrow_exact_collection_and_closed_plan_late_credit() {
    let mut f = Fixture::new();
    let owner = f.owner.pubkey();
    bind(&mut f);
    execute(&mut f, &Cmd::new(1, 1).plan(1)).unwrap();
    execute(&mut f, &Cmd::new(2, 2).plan(1).set(11, id(9)).amount(100)).unwrap();
    deposit(&mut f, 9, 40);
    rejects(collect(&mut f, 9, 1), "InvalidRoute");
    assert_eq!(f.project_state().held, 0);
    assert_eq!(f.tokens(route_escrow(f.project, f.mint, id(9))), 40);
    deposit(&mut f, 9, 70);
    collect(&mut f, 9, 1).unwrap();
    assert_eq!(f.project_state().held, 100);
    assert_eq!(f.tokens(route_escrow(f.project, f.mint, id(9))), 10);
    rejects(collect(&mut f, 9, 1), "InvalidRoute");
    execute(&mut f, &Cmd::new(6, 3).plan(1)).unwrap();
    assert_eq!(f.project_state().free, 1100);
    rejects(
        execute(
            &mut f,
            &Cmd::new(4, 4)
                .plan(1)
                .amount(1)
                .set(11, id(9))
                .set(9, owner.to_bytes()),
        ),
        "InvalidPlan",
    );
    execute(&mut f, &Cmd::new(1, 5).plan(2)).unwrap();
    execute(&mut f, &Cmd::new(2, 6).plan(2).set(11, id(10)).amount(50)).unwrap();
    execute(&mut f, &Cmd::new(6, 7).plan(2)).unwrap();
    deposit(&mut f, 10, 50);
    collect(&mut f, 10, 2).unwrap();
    assert_eq!(f.project_state().free, 1150);
    assert_eq!(f.project_state().held, 0);
}

#[test]
fn dispatch_and_actual_refunds_are_bounded_and_never_minted_from_observations() {
    let mut f = Fixture::new();
    let owner = f.owner.pubkey();
    let owner_token = associated_account(owner, f.mint);
    bind(&mut f);
    execute(&mut f, &Cmd::new(1, 1).plan(1).amount(200)).unwrap();
    execute(&mut f, &Cmd::new(3, 2).plan(1).set(11, id(9)).amount(200)).unwrap();
    let dispatch = Cmd::new(4, 3)
        .plan(1)
        .set(11, id(9))
        .set(9, owner.to_bytes())
        .amount(150);
    let mut substituted = command_accounts(&f, &dispatch);
    substituted.recipient = Some(f.owner_token);
    rejects(
        f.run(dispatch.signed(), substituted, Role::Owner),
        "WrongBridgeAccount",
    );
    assert_eq!(f.project_state().held, 200);
    assert!(f.svm.get_account(&owner_token).is_none());
    execute(&mut f, &dispatch).unwrap();
    execute(&mut f, &dispatch).unwrap();
    assert_eq!(f.tokens(owner_token), 150);
    assert_eq!(f.project_state().held, 50);
    rejects(
        execute(
            &mut f,
            &Cmd::new(4, 4)
                .plan(1)
                .set(11, id(9))
                .set(9, owner.to_bytes())
                .amount(1),
        ),
        "InvalidRoute",
    );
    rejects(collect(&mut f, 9, 1), "ZeroAmount");
    deposit(&mut f, 9, 20);
    collect(&mut f, 9, 1).unwrap();
    assert_eq!(f.project_state().free, 820);
    deposit(&mut f, 9, 140);
    collect(&mut f, 9, 1).unwrap();
    assert_eq!(f.project_state().free, 950);
    assert_eq!(f.tokens(route_escrow(f.project, f.mint, id(9))), 10);
    rejects(collect(&mut f, 9, 1), "ZeroAmount");
    execute(&mut f, &Cmd::new(6, 5).plan(1)).unwrap();
    assert_eq!(f.project_state().free, 1000);
    assert_eq!(f.project_state().held, 0);
}

#[test]
fn remote_pause_reclaim_restore_continue_and_stale_nonce_keep_full_principal() {
    let mut f = Fixture::new();
    bind(&mut f);
    let (task, request) = f.create_task(100);
    grant(&mut f, task, request, 100, 1, 1);
    execute(&mut f, &Cmd::new(9, 3).task(task)).unwrap();
    execute(
        &mut f,
        &Cmd::new(10, 4).task(task).set(1, word(1)).amount(40),
    )
    .unwrap();
    rejects(
        execute(&mut f, &Cmd::new(11, 5).task(task).set(1, word(1))),
        "WrongControlNonce",
    );
    let next = pda(&[b"request", task.as_ref(), &1u64.to_le_bytes()]);
    execute(
        &mut f,
        &Cmd::new(13, 6).task(task).amount(40).set(11, id(8)),
    )
    .unwrap();
    assert_eq!(f.request(next).state, RequestState::Pending);
    assert!(f.project_state().head.is_none());
    grant(&mut f, task, next, 40, 2, 7);
    assert!(f.task(task).paused);
    assert_eq!(f.task(task).available, 100);
    execute(&mut f, &Cmd::new(11, 9).task(task).set(1, word(2))).unwrap();
    assert!(!f.task(task).paused);
    assert_eq!(f.task(task).issued, 140);
}

#[test]
fn terminal_request_substitution_and_failed_allocation_cannot_release_or_resurrect() {
    let mut f = Fixture::new();
    bind(&mut f);
    let (task, request) = f.create_task(100);
    let (other, wrong) = f.create_task(1);
    execute(&mut f, &Cmd::new(1, 1).plan(1).amount(100)).unwrap();
    rejects(
        execute(
            &mut f,
            &Cmd::new(5, 2).plan(1).task(task).request(wrong).amount(100),
        ),
        "WrongLiveRequest",
    );
    assert_eq!(f.project_state().held, 100);
    assert_eq!(f.task(other).available, 0);
    execute(&mut f, &Cmd::new(12, 3).task(task)).unwrap();
    rejects(
        execute(
            &mut f,
            &Cmd::new(5, 2)
                .plan(1)
                .task(task)
                .request(request)
                .amount(100),
        ),
        "WrongLiveRequest",
    );
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Canceled);
    assert_eq!(f.project_state().held, 100);
    execute(&mut f, &Cmd::new(6, 4).plan(1)).unwrap();
    assert_eq!(f.project_state().free, 1000);
}

#[test]
fn immutable_checkpoint_encodes_canonical_snapshot_and_rejects_reuse() {
    let mut f = Fixture::new();
    bind(&mut f);
    let (task, request) = f.create_task(100);
    grant(&mut f, task, request, 100, 1, 1);
    let checkpoint = address(&f, b"checkpoint", id(50));
    let args = instruction::RecordCheckpoint {
        observation: id(50),
        task: task.to_bytes(),
        request: request.to_bytes(),
        plan: id(1),
        route: [0; 32],
        operation: id(2),
    };
    let accounts = accounts::RecordCheckpoint {
        project: f.project,
        checkpoint,
        payer: f.stranger.pubkey(),
        system_program: solana_system_interface::program::ID,
        task: Some(task),
        request: Some(request),
        plan: Some(address(&f, b"bridge-plan", id(1))),
        route: None,
        command: Some(address(&f, b"bridge-command", id(2))),
    };
    f.run(args, accounts, Role::Stranger).unwrap();
    let receipt: Checkpoint = f.read(checkpoint);
    assert_eq!(f.svm.get_account(&checkpoint).unwrap().data.len(), 1256);
    let w = |i: usize| receipt.payload[i * 32..(i + 1) * 32].to_vec();
    assert_eq!(w(0), DOMAIN);
    assert_eq!(w(2), word(1));
    assert_eq!(w(5), word(900));
    assert_eq!(w(8), word(0));
    assert_eq!(w(11), word(100));
    assert_eq!(w(21), word(2));
    assert_eq!(w(24), word(2));
    assert_eq!(w(35), word(1));
    assert_eq!(w(38), word(0));
    let before = f.svm.get_account(&checkpoint).unwrap().data;
    rejects(
        f.run(
            instruction::RecordCheckpoint {
                observation: id(50),
                task: [0; 32],
                request: [0; 32],
                plan: [0; 32],
                route: [0; 32],
                operation: [0; 32],
            },
            accounts::RecordCheckpoint {
                project: f.project,
                checkpoint,
                payer: f.stranger.pubkey(),
                system_program: solana_system_interface::program::ID,
                task: None,
                request: None,
                plan: None,
                route: None,
                command: None,
            },
            Role::Stranger,
        ),
        "WrongBridgeAccount",
    );
    assert_eq!(f.svm.get_account(&checkpoint).unwrap().data, before);
    assert_eq!(f.project_state().checkpoint_sequence, 1);
}

#[test]
fn frozen_dispatch_failure_rolls_back_hold_route_and_command_receipt() {
    let mut f = Fixture::new();
    bind(&mut f);
    execute(&mut f, &Cmd::new(1, 1).plan(1).amount(100)).unwrap();
    execute(&mut f, &Cmd::new(3, 2).plan(1).set(11, id(9)).amount(100)).unwrap();
    let owner = f.owner.pubkey();
    let owner_token = associated_account(owner, f.mint);
    let create = create_associated_account(owner, owner, f.mint);
    send(&mut f.svm, &f.owner, &[create], &[]).unwrap();
    let dispatch = Cmd::new(4, 3)
        .plan(1)
        .set(11, id(9))
        .set(9, owner.to_bytes())
        .amount(100);
    let freeze = spl_token::instruction::freeze_account(
        &spl_token::ID,
        &owner_token,
        &f.mint,
        &f.owner.pubkey(),
        &[],
    )
    .unwrap();
    send(&mut f.svm, &f.owner, &[freeze], &[]).unwrap();
    assert!(execute(&mut f, &dispatch).is_err());
    assert_eq!(f.project_state().held, 100);
    let plan: BridgePlan = f.read(address(&f, b"bridge-plan", id(1)));
    let route: BridgeRoute = f.read(address(&f, b"bridge-route", id(9)));
    assert_eq!(plan.held, 100);
    assert_eq!(route.dispatched, 0);
    assert!(f
        .svm
        .get_account(&address(&f, b"bridge-command", id(3)))
        .is_none());
    let thaw = spl_token::instruction::thaw_account(
        &spl_token::ID,
        &owner_token,
        &f.mint,
        &f.owner.pubkey(),
        &[],
    )
    .unwrap();
    send(&mut f.svm, &f.owner, &[thaw], &[]).unwrap();
    execute(&mut f, &dispatch).unwrap();
    assert_eq!(f.project_state().held, 0);
}

#[test]
fn coordinated_stop_resume_out_of_budget_and_control_cleanup_preserve_pause() {
    let mut f = Fixture::new();
    bind(&mut f);
    let (task, request) = f.create_task(100);
    grant(&mut f, task, request, 100, 1, 1);
    f.withdraw(task, 10).unwrap();
    let supplement = f.submit(task, 20).unwrap();
    execute(&mut f, &Cmd::new(7, 3).task(task).request(supplement)).unwrap();
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Stopped);
    execute(&mut f, &Cmd::new(9, 4).task(task)).unwrap();
    let resume = f.submit(task, 20).unwrap();
    grant(&mut f, task, resume, 20, 2, 5);
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Active);
    assert!(f.task(task).paused);
    let pending = f.submit(task, 30).unwrap();
    let mut omitted = command_accounts(&f, &Cmd::new(8, 7).task(task));
    omitted.request = None;
    rejects(
        f.run(Cmd::new(8, 7).task(task).signed(), omitted, Role::Owner),
        "WrongLiveRequest",
    );
    execute(&mut f, &Cmd::new(8, 7).task(task)).unwrap();
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::OutOfBudget);
    assert_eq!(f.request(pending).state, RequestState::Removed);
    rejects(
        execute(&mut f, &Cmd::new(11, 8).task(task).set(1, word(1))),
        "InvalidTaskState",
    );
    execute(
        &mut f,
        &Cmd::new(10, 9).task(task).set(1, word(1)).amount(110),
    )
    .unwrap();
    f.return_funds(task, 5).unwrap();
    f.finalize(task, None).unwrap();
    assert_eq!(f.task(task).final_expense().unwrap(), Some(5));
}

#[test]
fn coordinated_owner_funding_receipt_is_actual_immutable_and_domain_bound() {
    let mut f = Fixture::new();
    bind(&mut f);
    rejects(f.fund(20), "CoordinatedProject");
    let receipt = address(&f, b"funding", id(6));
    let accounts = accounts::FundCoordinated {
        workspace: f.workspace,
        project: f.project,
        mint: f.mint,
        vault: f.vault,
        owner_token: f.owner_token,
        owner: f.owner.pubkey(),
        receipt,
        token_program: spl_token::ID,
        system_program: solana_system_interface::program::ID,
    };
    f.run(
        instruction::FundCoordinated {
            funding_id: id(6),
            invoice_id: id(9),
            amount: 20,
        },
        accounts,
        Role::Owner,
    )
    .unwrap();
    let record: FundingReceipt = f.read(receipt);
    assert_eq!(f.svm.get_account(&receipt).unwrap().data.len(), 168);
    assert_eq!(record.payload[..32], DOMAIN);
    assert_eq!(record.payload[32..64], id(6));
    assert_eq!(record.payload[64..96], id(9));
    assert_eq!(record.payload[96..128], word(20));
    assert_eq!(record.payload[128..160], f.owner.pubkey().to_bytes());
    assert_eq!(f.project_state().free, 1020);
    assert_eq!(f.tokens(f.vault), 1020);
    rejects(
        f.run(
            instruction::FundCoordinated {
                funding_id: id(6),
                invoice_id: id(9),
                amount: 20,
            },
            accounts::FundCoordinated {
                workspace: f.workspace,
                project: f.project,
                mint: f.mint,
                vault: f.vault,
                owner_token: f.owner_token,
                owner: f.owner.pubkey(),
                receipt,
                token_program: spl_token::ID,
                system_program: solana_system_interface::program::ID,
            },
            Role::Owner,
        ),
        "WrongBridgeAccount",
    );
    assert_eq!(f.project_state().free, 1020);
}

#[test]
fn binding_rejects_an_existing_task_and_preserves_standalone_controls() {
    let mut f = Fixture::new();
    let (task, request) = f.create_task(10);
    rejects(
        f.run(
            instruction::BindCoordinator {
                domain: DOMAIN,
                command_key: key(),
            },
            accounts::BindCoordinator {
                workspace: f.workspace,
                project: f.project,
                owner: f.owner.pubkey(),
            },
            Role::Owner,
        ),
        "InvalidBinding",
    );
    assert_eq!(f.project_state().domain, [0; 32]);
    f.approve(task, request).unwrap();
    f.pause(task).unwrap();
}

#[test]
fn command_expiry_extreme_amount_and_owner_recipient_fail_without_financial_mutation() {
    use anchor_lang::prelude::Clock;
    let mut f = Fixture::new();
    bind(&mut f);
    let mut clock = f.svm.get_sysvar::<Clock>();
    clock.unix_timestamp = 100;
    f.svm.set_sysvar(&clock);
    rejects(
        execute(&mut f, &Cmd::new(1, 1).plan(1).amount(1).set(10, word(99))),
        "CommandExpired",
    );
    assert_eq!(f.project_state().free, 1000);
    rejects(
        execute(&mut f, &Cmd::new(1, 2).plan(2).amount(u64::MAX)),
        "Arithmetic",
    );
    let owner = f.owner_token;
    let agent = f.agent_token;
    rejects(
        execute(&mut f, &Cmd::new(14, 3).amount(1).set(9, agent.to_bytes())),
        "Unauthorized",
    );
    execute(
        &mut f,
        &Cmd::new(14, 4)
            .amount(10)
            .set(9, owner.to_bytes())
            .set(10, word(100)),
    )
    .unwrap();
    assert_eq!(f.project_state().free, 990);
    assert_eq!(f.tokens(f.vault), 990);
    execute(&mut f, &Cmd::new(1, 5).plan(3).amount(990)).unwrap();
    rejects(
        execute(&mut f, &Cmd::new(14, 6).amount(1).set(9, owner.to_bytes())),
        "Arithmetic",
    );
    assert_eq!(f.project_state().held, 990);
    execute(&mut f, &Cmd::new(6, 7).plan(3)).unwrap();
    assert_eq!(f.project_state().free, 990);
}

#[test]
fn prefunded_bridge_pdas_initialize_without_reinitializing_existing_records() {
    let mut f = Fixture::new();
    bind(&mut f);
    let plan = address(&f, b"bridge-plan", id(1));
    let receipt = address(&f, b"bridge-command", id(1));
    let route = address(&f, b"bridge-route", id(9));
    let escrow = route_escrow(f.project, f.mint, id(9));
    let transfers: Vec<_> = [plan, receipt, route, escrow]
        .into_iter()
        .map(|target| {
            solana_system_interface::instruction::transfer(&f.stranger.pubkey(), &target, 1_000_000)
        })
        .collect();
    send(&mut f.svm, &f.stranger, &transfers, &[]).unwrap();

    execute(&mut f, &Cmd::new(1, 1).plan(1)).unwrap();
    execute(&mut f, &Cmd::new(2, 2).plan(1).set(11, id(9)).amount(20)).unwrap();
    deposit(&mut f, 9, 20);
    collect(&mut f, 9, 1).unwrap();
    assert_eq!(f.read::<BridgePlan>(plan).held, 20);
    assert_eq!(f.read::<BridgeRoute>(route).received, 20);

    rejects(
        execute(&mut f, &Cmd::new(1, 3).plan(1).amount(20)),
        "InvalidPlan",
    );
    rejects(
        execute(&mut f, &Cmd::new(2, 4).plan(1).set(11, id(9)).amount(30)),
        "InvalidRoute",
    );
    assert_eq!(f.project_state().free, 1000);
    assert_eq!(f.project_state().held, 20);
    assert_eq!(f.tokens(f.vault), 1020);
}

#[test]
fn checkpoint_rejects_foreign_or_substituted_accounts_before_recording_evidence() {
    let mut f = Fixture::new();
    bind(&mut f);
    let (task, request) = f.create_task(10);
    let original_project = f.project;
    let original_vault = f.vault;
    let (other_project, other_vault) = f.create_project();
    f.project = other_project;
    f.vault = other_vault;
    let (foreign_task, _) = f.create_task(10);
    f.project = original_project;
    f.vault = original_vault;

    for (selected_task, supplied_task, selected_request, supplied_request, error) in [
        (task, foreign_task, request, request, "WrongBridgeAccount"),
        (
            foreign_task,
            foreign_task,
            request,
            request,
            "WrongBridgeAccount",
        ),
        (task, task, request, task, "WrongBridgeAccount"),
        (task, task, task, task, "AccountDiscriminatorMismatch"),
    ] {
        let checkpoint = address(&f, b"checkpoint", id(50));
        rejects(
            f.run(
                instruction::RecordCheckpoint {
                    observation: id(50),
                    task: selected_task.to_bytes(),
                    request: selected_request.to_bytes(),
                    plan: [0; 32],
                    route: [0; 32],
                    operation: [0; 32],
                },
                accounts::RecordCheckpoint {
                    project: f.project,
                    checkpoint,
                    payer: f.stranger.pubkey(),
                    system_program: solana_system_interface::program::ID,
                    task: Some(supplied_task),
                    request: Some(supplied_request),
                    plan: None,
                    route: None,
                    command: None,
                },
                Role::Stranger,
            ),
            error,
        );
        assert!(f.svm.get_account(&checkpoint).is_none());
        assert_eq!(f.project_state().checkpoint_sequence, 0);
    }
}

#[test]
fn closing_allocated_plan_and_finalizing_before_allocate_cannot_reclaim_or_resurrect() {
    let mut f = Fixture::new();
    bind(&mut f);
    let (task, request) = f.create_task(30);
    grant(&mut f, task, request, 30, 1, 1);
    execute(&mut f, &Cmd::new(6, 3).plan(1)).unwrap();
    assert_eq!(f.task(task).available, 30);
    assert_eq!(f.project_state().free, 970);
    let closed: CommandReceipt = f.read(address(&f, b"bridge-command", id(3)));
    assert_eq!(closed.result, word(0));
    assert_eq!(
        f.read::<BridgePlan>(address(&f, b"bridge-plan", id(1)))
            .state,
        ALLOCATED
    );

    f.withdraw(task, 30).unwrap();
    let supplement = f.submit(task, 5).unwrap();
    execute(&mut f, &Cmd::new(1, 4).plan(2).amount(5)).unwrap();
    f.finalize(task, Some(supplement)).unwrap();
    rejects(
        execute(
            &mut f,
            &Cmd::new(5, 5)
                .plan(2)
                .task(task)
                .request(supplement)
                .amount(5),
        ),
        "WrongLiveRequest",
    );
    assert_eq!(f.task(task).lifecycle, TaskLifecycle::Completed);
    assert_eq!(f.request(supplement).state, RequestState::Removed);
    assert_eq!(f.project_state().held, 5);
    assert!(f
        .svm
        .get_account(&address(&f, b"bridge-command", id(5)))
        .is_none());
    execute(&mut f, &Cmd::new(6, 6).plan(2)).unwrap();
    assert_eq!(f.project_state().free, 970);
    assert_eq!(f.project_state().held, 0);
    assert_eq!(f.tokens(f.vault), 970);
}
