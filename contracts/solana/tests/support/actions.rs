use anchor_lang::prelude::Pubkey;
use anchor_spl::token::spl_token;
use apodect_solana::{accounts, instruction};
use solana_signer::Signer;

use super::{Fixture, Role};

impl Fixture {
    pub fn create_task(&mut self, amount: u64) -> (Pubkey, Pubkey) {
        let index = self.project_state().task_count;
        let task = Pubkey::find_program_address(
            &[b"task", self.project.as_ref(), &index.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        let request = Pubkey::find_program_address(
            &[b"request", task.as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        self.run(
            instruction::CreateTask {
                amount,
                purpose: [1; 32],
            },
            accounts::CreateTask {
                project: self.project,
                task,
                request,
                agent: self.agent.pubkey(),
                system_program: solana_system_interface::program::ID,
            },
            Role::Agent,
        )
        .unwrap();
        (task, request)
    }

    pub fn next_request(&self, task: Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[
                b"request",
                task.as_ref(),
                &self.task(task).request_count.to_le_bytes(),
            ],
            &apodect_solana::ID,
        )
        .0
    }

    pub fn submit(&mut self, task: Pubkey, amount: u64) -> Result<Pubkey, String> {
        let request = self.next_request(task);
        self.run(
            instruction::SubmitRequest {
                amount,
                purpose: [2; 32],
            },
            accounts::SubmitRequest {
                project: self.project,
                task,
                request,
                agent: self.agent.pubkey(),
                system_program: solana_system_interface::program::ID,
            },
            Role::Agent,
        )?;
        Ok(request)
    }

    pub fn approve_accounts(
        &self,
        task: Pubkey,
        request: Pubkey,
        role: Role,
        tail: Option<Pubkey>,
    ) -> accounts::ApproveRequest {
        accounts::ApproveRequest {
            project: self.project,
            task: task,
            request: request,
            authority: self.key(role),
            tail: tail,
        }
    }

    pub fn reject_accounts(
        &self,
        task: Pubkey,
        request: Pubkey,
        role: Role,
    ) -> accounts::RejectRequest {
        accounts::RejectRequest {
            project: self.project,
            task: task,
            request: request,
            authority: self.key(role),
            tail: None,
        }
    }

    pub fn approve(&mut self, task: Pubkey, request: Pubkey) -> Result<(), String> {
        self.run(
            instruction::ApproveRequest {},
            self.approve_accounts(task, request, Role::Authority, self.project_state().tail),
            Role::Authority,
        )
    }

    pub fn reject(&mut self, task: Pubkey, request: Pubkey) -> Result<(), String> {
        self.run(
            instruction::RejectRequest {},
            self.reject_accounts(task, request, Role::Authority),
            Role::Authority,
        )
    }

    pub fn apply(&mut self, request: Pubkey, task: Option<Pubkey>) -> Result<(), String> {
        self.run(
            instruction::ApplyNext {},
            accounts::ApplyNext {
                project: self.project,
                head: request,
                authority: self.authority.pubkey(),
                task,
            },
            Role::Authority,
        )
    }

    pub fn restore(&mut self, task: Pubkey, amount: u64) -> Result<Pubkey, String> {
        let request = self.next_request(task);
        self.run(
            instruction::RestoreBudget {
                amount,
                purpose: [3; 32],
            },
            accounts::RestoreBudget {
                project: self.project,
                task,
                request,
                authority: self.authority.pubkey(),
                system_program: solana_system_interface::program::ID,
                tail: self.project_state().tail,
            },
            Role::Authority,
        )?;
        Ok(request)
    }

    pub fn out_of_budget(&mut self, task: Pubkey, pending: Option<Pubkey>) -> Result<(), String> {
        self.run(
            instruction::MarkOutOfBudget {},
            accounts::MarkOutOfBudget {
                project: self.project,
                task,
                authority: self.authority.pubkey(),
                pending,
            },
            Role::Authority,
        )
    }

    pub fn withdraw_accounts(&self, task: Pubkey) -> accounts::Withdraw {
        accounts::Withdraw {
            project: self.project,
            task: task,
            mint: self.mint,
            vault: self.vault,
            agent_token: self.agent_token,
            agent: self.agent.pubkey(),
            token_program: spl_token::ID,
        }
    }

    pub fn cancel_accounts(
        &self,
        task: Pubkey,
        role: Role,
        pending: Option<Pubkey>,
    ) -> accounts::CancelTask {
        accounts::CancelTask {
            workspace: self.workspace,
            project: self.project,
            task: task,
            actor: self.key(role),
            pending: pending,
        }
    }

    pub fn withdraw(&mut self, task: Pubkey, amount: u64) -> Result<(), String> {
        self.run(
            instruction::Withdraw { amount },
            self.withdraw_accounts(task),
            Role::Agent,
        )
    }

    pub fn return_funds(&mut self, task: Pubkey, amount: u64) -> Result<(), String> {
        self.run(
            instruction::ReturnFunds { amount },
            accounts::ReturnFunds {
                project: self.project,
                task: task,
                mint: self.mint,
                vault: self.vault,
                agent_token: self.agent_token,
                agent: self.agent.pubkey(),
                token_program: spl_token::ID,
            },
            Role::Agent,
        )
    }

    pub fn release(&mut self, task: Pubkey, amount: u64) -> Result<(), String> {
        self.run(
            instruction::ReleaseFunds { amount },
            accounts::ReleaseFunds {
                project: self.project,
                task,
                agent: self.agent.pubkey(),
            },
            Role::Agent,
        )
    }

    pub fn pause(&mut self, task: Pubkey) -> Result<(), String> {
        self.run(
            instruction::PauseTask {},
            accounts::PauseTask {
                workspace: self.workspace,
                project: self.project,
                task: task,
                owner: self.owner.pubkey(),
            },
            Role::Owner,
        )
    }

    pub fn continue_task(&mut self, task: Pubkey) -> Result<(), String> {
        self.run(
            instruction::ContinueTask {},
            accounts::ContinueTask {
                workspace: self.workspace,
                project: self.project,
                task: task,
                owner: self.owner.pubkey(),
            },
            Role::Owner,
        )
    }

    pub fn reclaim(&mut self, task: Pubkey, amount: u64) -> Result<(), String> {
        self.run(
            instruction::Reclaim { amount },
            accounts::Reclaim {
                workspace: self.workspace,
                project: self.project,
                task: task,
                owner: self.owner.pubkey(),
            },
            Role::Owner,
        )
    }

    pub fn finalize(&mut self, task: Pubkey, pending: Option<Pubkey>) -> Result<(), String> {
        self.run(
            instruction::FinalizeTask { report: [9; 32] },
            accounts::FinalizeTask {
                workspace: self.workspace,
                project: self.project,
                task: task,
                actor: self.agent.pubkey(),
                pending: pending,
            },
            Role::Agent,
        )
    }

    pub fn cancel(&mut self, task: Pubkey, pending: Option<Pubkey>) -> Result<(), String> {
        self.run(
            instruction::CancelTask {},
            self.cancel_accounts(task, Role::Owner, pending),
            Role::Owner,
        )
    }

    pub fn funded_task(&mut self, amount: u64) -> (Pubkey, Pubkey) {
        self.fund(amount).unwrap();
        let (task, request) = self.create_task(amount);
        self.approve(task, request).unwrap();
        self.apply(request, Some(task)).unwrap();
        (task, request)
    }

    pub fn fund_accounts(&self, invoice: Option<Pubkey>) -> accounts::FundProject {
        accounts::FundProject {
            workspace: self.workspace,
            project: self.project,
            mint: self.mint,
            vault: self.vault,
            owner_token: self.owner_token,
            owner: self.owner.pubkey(),
            token_program: spl_token::ID,
            invoice: invoice,
        }
    }

    pub fn withdraw_free_accounts(&self) -> accounts::WithdrawFree {
        accounts::WithdrawFree {
            workspace: self.workspace,
            project: self.project,
            mint: self.mint,
            vault: self.vault,
            owner_token: self.owner_token,
            owner: self.owner.pubkey(),
            token_program: spl_token::ID,
        }
    }

    pub fn fund(&mut self, amount: u64) -> Result<(), String> {
        self.run(
            instruction::FundProject { amount },
            self.fund_accounts(None),
            Role::Owner,
        )
    }

    pub fn invoice(&mut self, amount: u64) -> Pubkey {
        let invoice = Pubkey::find_program_address(
            &[
                b"invoice",
                self.project.as_ref(),
                &self.project_state().invoice_count.to_le_bytes(),
            ],
            &apodect_solana::ID,
        )
        .0;
        self.run(
            instruction::CreateInvoice {
                amount,
                purpose: [0; 32],
            },
            accounts::CreateInvoice {
                project: self.project,
                invoice,
                authority: self.authority.pubkey(),
                system_program: solana_system_interface::program::ID,
            },
            Role::Authority,
        )
        .unwrap();
        invoice
    }

    pub fn respond(&mut self, invoice: Pubkey, contribution: u64) -> Result<(), String> {
        self.run(
            instruction::RespondInvoice {
                contribution,
                response: [4; 32],
            },
            accounts::RespondInvoice {
                workspace: self.workspace,
                project: self.project,
                invoice,
                owner: self.owner.pubkey(),
            },
            Role::Owner,
        )
    }
}
