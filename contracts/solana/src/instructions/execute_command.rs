use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::bridge::{
    self, action, BridgePlan, BridgeRoute, Command, CommandReceipt, ALLOCATED, CLOSED, DELIVERY,
    INVALIDATED, OPEN, REFUND, SUCCEEDED,
};
use crate::errors::ContractError;
use crate::events::{event, event_with_stop, Action, StopReason};
use crate::helpers::{add, positive, release, sub};
use crate::state::{
    BudgetRequest, Project, RequestKind, RequestState, Task, TaskLifecycle, Workspace,
};

#[derive(Accounts)]
pub struct ExecuteCommand<'info> {
    #[account(mut, has_one = workspace, has_one = mint)]
    pub project: Account<'info, Project>,
    pub workspace: Account<'info, Workspace>,
    pub mint: Account<'info, Mint>,
    #[account(mut, seeds = [b"vault", project.key().as_ref()], bump,
        token::mint = mint, token::authority = project)]
    pub vault: Account<'info, TokenAccount>,
    /// CHECK: Unique operation PDA, owner/discriminator and exact digest checked before replay.
    #[account(mut)]
    pub receipt: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    #[account(mut, has_one = project)]
    pub task: Option<Account<'info, Task>>,
    /// CHECK: Exact command identity and typed record checked in the matching action.
    #[account(mut)]
    pub request: Option<UncheckedAccount<'info>>,
    /// CHECK: Project/plan PDA and typed record checked before use; created only by Reserve.
    #[account(mut)]
    pub plan: Option<UncheckedAccount<'info>>,
    /// CHECK: Project/route PDA and typed record checked before use; created only by Expect.
    #[account(mut)]
    pub route: Option<UncheckedAccount<'info>>,
    /// CHECK: Fixed route token PDA, initialized by Expect with legacy token owner/mint.
    #[account(mut)]
    pub escrow: Option<UncheckedAccount<'info>>,
    #[account(mut, token::mint = mint)]
    pub recipient: Option<Account<'info, TokenAccount>>,
    /// CHECK: Exact command PDA that Invalidate inspects or permanently tombstones.
    #[account(mut)]
    pub target: Option<UncheckedAccount<'info>>,
}

pub fn handler(
    ctx: Context<ExecuteCommand>,
    bytes: [u8; 384],
    signature: [u8; 64],
    recovery_id: u8,
) -> Result<()> {
    let command = Command::parse(&bytes)?;
    let accounts = ctx.accounts;
    require!(
        accounts.project.domain != [0; 32] && command.domain == accounts.project.domain,
        ContractError::InvalidBinding
    );
    let digest = command.authenticate(
        &bytes,
        &signature,
        recovery_id,
        &accounts.project.command_key,
    )?;
    let project = accounts.project.key();
    let receipt_seeds: &[&[u8]] = &[b"bridge-command", project.as_ref(), &command.operation];
    bridge::check_pda(&accounts.receipt, receipt_seeds)?;
    if let Some(previous) = bridge::load::<CommandReceipt>(&accounts.receipt)? {
        require!(
            previous.project == project
                && previous.operation == command.operation
                && previous.state == SUCCEEDED
                && previous.digest == digest,
            ContractError::CommandConflict
        );
        return Ok(());
    }
    let now = Clock::get()?.unix_timestamp;
    require!(
        now >= 0 && now as u64 <= command.expires,
        ContractError::CommandExpired
    );
    require!(
        accounts
            .task
            .as_ref()
            .map(|task| task.key().to_bytes())
            .unwrap_or([0; 32])
            == command.task,
        ContractError::WrongBridgeAccount
    );
    let result = match command.action {
        action::RESERVE..=action::CLOSE_PLAN => execute_custody(accounts, &command)?,
        action::REJECT..=action::RESTORE_REQUEST => execute_task_control(accounts, &command)?,
        action::OWNER_WITHDRAW => {
            let recipient = accounts
                .recipient
                .as_ref()
                .ok_or(ContractError::WrongBridgeAccount)?;
            require!(
                recipient.key().to_bytes() == command.recipient
                    && recipient.owner == accounts.workspace.owner,
                ContractError::Unauthorized
            );
            require!(
                recipient.key() != accounts.vault.key(),
                ContractError::WrongBridgeAccount
            );
            accounts.project.free = sub(accounts.project.free, command.amount)?;
            transfer_from_vault(accounts, recipient.to_account_info(), command.amount)?;
            bridge::word(command.amount)
        }
        action::INVALIDATE => {
            require!(
                command.reference != command.operation,
                ContractError::InvalidCommand
            );
            let target = accounts
                .target
                .as_ref()
                .ok_or(ContractError::WrongBridgeAccount)?;
            let seeds: &[&[u8]] = &[b"bridge-command", project.as_ref(), &command.reference];
            bridge::check_pda(target, seeds)?;
            if let Some(previous) = bridge::load::<CommandReceipt>(target)? {
                require!(
                    previous.project == project && previous.operation == command.reference,
                    ContractError::WrongBridgeAccount
                );
                bridge::word(previous.state as u64)
            } else {
                bridge::create_record(
                    target,
                    &accounts.payer,
                    &accounts.system_program,
                    seeds,
                    8 + CommandReceipt::INIT_SPACE,
                    &CommandReceipt {
                        project,
                        operation: command.reference,
                        digest: [0; 32],
                        action: 0,
                        state: INVALIDATED,
                        result: [0; 32],
                    },
                )?;
                bridge::word(INVALIDATED as u64)
            }
        }
        _ => return err!(ContractError::InvalidCommand),
    };
    bridge::create_record(
        &accounts.receipt,
        &accounts.payer,
        &accounts.system_program,
        receipt_seeds,
        8 + CommandReceipt::INIT_SPACE,
        &CommandReceipt {
            project,
            operation: command.operation,
            digest,
            action: command.action,
            state: SUCCEEDED,
            result,
        },
    )?;
    emit!(crate::events::BridgeTransition {
        project,
        operation: command.operation,
        action: command.action,
        result
    });
    Ok(())
}

fn execute_custody(accounts: &mut ExecuteCommand, command: &Command) -> Result<[u8; 32]> {
    let project = accounts.project.key();
    let plan_account = accounts
        .plan
        .as_ref()
        .ok_or(ContractError::WrongBridgeAccount)?;
    let plan_seeds: &[&[u8]] = &[b"bridge-plan", project.as_ref(), &command.related];
    bridge::check_pda(plan_account, plan_seeds)?;
    if command.action == action::RESERVE {
        require!(
            bridge::load::<BridgePlan>(plan_account)?.is_none(),
            ContractError::InvalidPlan
        );
        accounts.project.free = sub(accounts.project.free, command.amount)?;
        accounts.project.held = add(accounts.project.held, command.amount)?;
        bridge::create_record(
            plan_account,
            &accounts.payer,
            &accounts.system_program,
            plan_seeds,
            8 + BridgePlan::INIT_SPACE,
            &BridgePlan {
                project,
                id: command.related,
                state: OPEN,
                held: command.amount,
            },
        )?;
        return Ok(bridge::word(command.amount));
    }
    let mut plan = bridge::load::<BridgePlan>(plan_account)?.ok_or(ContractError::InvalidPlan)?;
    require!(
        plan.project == project && plan.id == command.related,
        ContractError::WrongBridgeAccount
    );
    if command.action == action::CLOSE_PLAN {
        if plan.state == ALLOCATED {
            return Ok(bridge::word(0));
        }
        let amount = plan.held;
        accounts.project.free = add(accounts.project.free, amount)?;
        accounts.project.held = sub(accounts.project.held, amount)?;
        plan.held = 0;
        plan.state = CLOSED;
        bridge::save(plan_account, &plan)?;
        return Ok(bridge::word(amount));
    }
    require!(plan.state == OPEN, ContractError::InvalidPlan);
    if matches!(
        command.action,
        action::EXPECT_DELIVERY | action::EXPECT_REFUND
    ) {
        let route_account = accounts
            .route
            .as_ref()
            .ok_or(ContractError::WrongBridgeAccount)?;
        let escrow = accounts
            .escrow
            .as_ref()
            .ok_or(ContractError::WrongBridgeAccount)?;
        let route_seeds: &[&[u8]] = &[b"bridge-route", project.as_ref(), &command.reference];
        let escrow_seeds: &[&[u8]] = &[b"bridge-escrow", project.as_ref(), &command.reference];
        bridge::check_pda(route_account, route_seeds)?;
        require!(
            bridge::load::<BridgeRoute>(route_account)?.is_none(),
            ContractError::InvalidRoute
        );
        bridge::create_pda(
            escrow,
            &accounts.payer,
            &accounts.system_program,
            escrow_seeds,
            anchor_spl::token::spl_token::state::Account::LEN,
            accounts.token_program.key(),
        )?;
        token::initialize_account3(CpiContext::new(
            accounts.token_program.key(),
            token::InitializeAccount3 {
                account: escrow.to_account_info(),
                mint: accounts.mint.to_account_info(),
                authority: accounts.project.to_account_info(),
            },
        ))?;
        bridge::create_record(
            route_account,
            &accounts.payer,
            &accounts.system_program,
            route_seeds,
            8 + BridgeRoute::INIT_SPACE,
            &BridgeRoute {
                project,
                id: command.reference,
                plan: command.related,
                kind: if command.action == action::EXPECT_DELIVERY {
                    DELIVERY
                } else {
                    REFUND
                },
                expected: command.amount,
                received: 0,
                dispatched: 0,
            },
        )?;
    } else if command.action == action::DISPATCH {
        let route_account = accounts
            .route
            .as_ref()
            .ok_or(ContractError::WrongBridgeAccount)?;
        bridge::check_pda(
            route_account,
            &[b"bridge-route", project.as_ref(), &command.reference],
        )?;
        let mut route =
            bridge::load::<BridgeRoute>(route_account)?.ok_or(ContractError::InvalidRoute)?;
        require!(
            route.project == project
                && route.id == command.reference
                && route.plan == command.related
                && route.kind == REFUND
                && route.dispatched == 0
                && command.amount <= route.expected,
            ContractError::InvalidRoute
        );
        let recipient = accounts
            .recipient
            .as_ref()
            .ok_or(ContractError::WrongBridgeAccount)?;
        require!(
            recipient.key().to_bytes() == command.recipient
                && recipient.key() != accounts.vault.key(),
            ContractError::WrongBridgeAccount
        );
        plan.held = sub(plan.held, command.amount)?;
        accounts.project.held = sub(accounts.project.held, command.amount)?;
        transfer_from_vault(accounts, recipient.to_account_info(), command.amount)?;
        route.dispatched = command.amount;
        bridge::save(route_account, &route)?;
    } else if command.action == action::ALLOCATE {
        let task = accounts
            .task
            .as_mut()
            .ok_or(ContractError::WrongBridgeAccount)?;
        let request_account = accounts
            .request
            .as_ref()
            .ok_or(ContractError::WrongLiveRequest)?;
        require!(
            request_account.key().to_bytes() == command.request
                && task.pending == Some(request_account.key()),
            ContractError::WrongLiveRequest
        );
        let mut request = bridge::load::<BudgetRequest>(request_account)?
            .ok_or(ContractError::WrongLiveRequest)?;
        require!(
            request.project == project
                && request.task == task.key()
                && request.amount == command.amount
                && request.state == RequestState::Pending,
            ContractError::InvalidRequestState
        );
        task.nonterminal()?;
        require!(
            task.lifecycle == TaskLifecycle::Active
                || (task.lifecycle == TaskLifecycle::Stopped
                    && request.kind == RequestKind::Restart),
            ContractError::InvalidTaskState
        );
        require!(plan.held == command.amount, ContractError::InvalidPlan);
        plan.held = sub(plan.held, command.amount)?;
        accounts.project.held = sub(accounts.project.held, command.amount)?;
        task.available = add(task.available, command.amount)?;
        task.issued = add(task.issued, command.amount)?;
        task.lifecycle = TaskLifecycle::Active;
        task.pending = None;
        request.state = RequestState::Funded;
        plan.state = ALLOCATED;
        bridge::save(request_account, &request)?;
        event(
            project,
            task.key(),
            request_account.key(),
            Action::Allocate,
            command.amount,
        );
    }
    bridge::save(plan_account, &plan)?;
    Ok(bridge::word(command.amount))
}

fn execute_task_control(accounts: &mut ExecuteCommand, command: &Command) -> Result<[u8; 32]> {
    let project = accounts.project.key();
    let task = accounts
        .task
        .as_mut()
        .ok_or(ContractError::WrongBridgeAccount)?;
    task.nonterminal()?;
    if (action::PAUSE..=action::CANCEL).contains(&command.action) {
        require!(
            task.control_nonce == command.nonce,
            ContractError::WrongControlNonce
        );
        task.control_nonce = add(task.control_nonce, 1)?;
    }
    if matches!(
        command.action,
        action::REJECT | action::OUT_OF_BUDGET | action::CANCEL
    ) {
        require!(
            (command.action != action::REJECT
                || task.pending.map(|id| id.to_bytes()).unwrap_or([0; 32]) == command.request)
                && accounts.request.as_ref().map(|r| r.key()) == task.pending,
            ContractError::WrongLiveRequest
        );
        if let Some(account) = &accounts.request {
            let mut request =
                bridge::load::<BudgetRequest>(account)?.ok_or(ContractError::WrongLiveRequest)?;
            require!(
                request.project == project
                    && request.task == task.key()
                    && request.state == RequestState::Pending,
                ContractError::InvalidRequestState
            );
            if command.action == action::REJECT {
                request.state = RequestState::Rejected;
                let stop = if request.kind == RequestKind::Supplement {
                    task.lifecycle = TaskLifecycle::Stopped;
                    Some(StopReason::SupplementRejected)
                } else {
                    None
                };
                event_with_stop(
                    project,
                    task.key(),
                    account.key(),
                    Action::Reject,
                    request.amount,
                    stop,
                );
            } else {
                request.state = RequestState::Removed;
            }
            bridge::save(account, &request)?;
        }
        task.pending = None;
    }
    match command.action {
        action::REJECT => {}
        action::OUT_OF_BUDGET => {
            require!(
                task.lifecycle != TaskLifecycle::OutOfBudget,
                ContractError::InvalidTaskState
            );
            task.lifecycle = TaskLifecycle::OutOfBudget;
            event(
                project,
                task.key(),
                Pubkey::default(),
                Action::OutOfBudget,
                0,
            );
        }
        action::PAUSE => {
            require!(!task.paused, ContractError::InvalidTaskState);
            task.paused = true;
            event(project, task.key(), Pubkey::default(), Action::Pause, 0);
        }
        action::RECLAIM => {
            require!(task.paused, ContractError::InvalidTaskState);
            release(&mut accounts.project, task, command.amount, Action::Reclaim)?;
        }
        action::CONTINUE => {
            require!(
                task.lifecycle == TaskLifecycle::Active && task.paused && task.available > 0,
                ContractError::InvalidTaskState
            );
            task.paused = false;
            event(project, task.key(), Pubkey::default(), Action::Continue, 0);
        }
        action::CANCEL => {
            require!(!task.started, ContractError::AlreadyStarted);
            let amount = task.available;
            if amount > 0 {
                release(&mut accounts.project, task, amount, Action::Release)?;
            }
            task.lifecycle = TaskLifecycle::Canceled;
            event(
                project,
                task.key(),
                Pubkey::default(),
                Action::Cancel,
                amount,
            );
        }
        action::RESTORE_REQUEST => {
            require!(
                task.lifecycle == TaskLifecycle::Active && task.paused && task.issued > 0,
                ContractError::InvalidTaskState
            );
            require!(task.pending.is_none(), ContractError::LiveRequest);
            positive(command.amount)?;
            let account = accounts
                .request
                .as_ref()
                .ok_or(ContractError::WrongLiveRequest)?;
            let task_key = task.key();
            let count = task.request_count.to_le_bytes();
            bridge::create_record(
                account,
                &accounts.payer,
                &accounts.system_program,
                &[b"request", task_key.as_ref(), &count],
                8 + BudgetRequest::INIT_SPACE,
                &BudgetRequest {
                    project,
                    task: task_key,
                    amount: command.amount,
                    purpose: command.reference,
                    kind: RequestKind::Restoration,
                    state: RequestState::Pending,
                    approval_sequence: 0,
                    next: None,
                },
            )?;
            task.pending = Some(account.key());
            task.request_count = add(task.request_count, 1)?;
            event(
                project,
                task.key(),
                account.key(),
                Action::Restore,
                command.amount,
            );
            return Ok(account.key().to_bytes());
        }
        _ => return err!(ContractError::InvalidCommand),
    }
    Ok(bridge::word(command.amount))
}

fn transfer_from_vault<'info>(
    accounts: &ExecuteCommand<'info>,
    recipient: AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    let index = accounts.project.index.to_le_bytes();
    let bump = [accounts.project.bump];
    let seeds: &[&[u8]] = &[
        b"project",
        accounts.project.workspace.as_ref(),
        &index,
        &bump,
    ];
    token::transfer_checked(
        CpiContext::new_with_signer(
            accounts.token_program.key(),
            TransferChecked {
                from: accounts.vault.to_account_info(),
                mint: accounts.mint.to_account_info(),
                to: recipient,
                authority: accounts.project.to_account_info(),
            },
            &[seeds],
        ),
        amount,
        accounts.mint.decimals,
    )
}
