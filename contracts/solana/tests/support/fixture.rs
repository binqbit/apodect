use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::{prelude::Pubkey, AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token;
use apodect_solana::{
    accounts, instruction,
    state::{BudgetRequest, Project, Task, Workspace},
};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_signer::Signer;

use super::transport::{ix, send, Role};

pub struct Fixture {
    pub svm: LiteSVM,
    pub owner: Keypair,
    pub authority: Keypair,
    pub agent: Keypair,
    pub stranger: Keypair,
    pub mint: Pubkey,
    pub workspace: Pubkey,
    pub project: Pubkey,
    pub vault: Pubkey,
    pub owner_token: Pubkey,
    pub agent_token: Pubkey,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_supply(1_000_000_000)
    }

    pub fn with_supply(supply: u64) -> Self {
        let mut svm = LiteSVM::new();
        let program = std::env::var("APODECT_SOLANA_PROGRAM")
            .unwrap_or_else(|_| "target/deploy/apodect_solana.so".into());
        svm.add_program_from_file(apodect_solana::ID, program)
            .unwrap();
        let owner = Keypair::new();
        let authority = Keypair::new();
        let agent = Keypair::new();
        let stranger = Keypair::new();
        for key in [&owner, &authority, &agent, &stranger] {
            svm.airdrop(&key.pubkey(), 10_000_000_000).unwrap();
        }
        let mut fixture = Self {
            svm,
            owner,
            authority,
            agent,
            stranger,
            mint: Pubkey::default(),
            workspace: Pubkey::default(),
            project: Pubkey::default(),
            vault: Pubkey::default(),
            owner_token: Pubkey::default(),
            agent_token: Pubkey::default(),
        };
        fixture.mint = fixture.new_mint(6);
        fixture.owner_token = fixture.new_token(fixture.owner.pubkey(), fixture.mint);
        fixture.agent_token = fixture.new_token(fixture.agent.pubkey(), fixture.mint);
        let mint_to = spl_token::instruction::mint_to(
            &spl_token::ID,
            &fixture.mint,
            &fixture.owner_token,
            &fixture.owner.pubkey(),
            &[],
            supply,
        )
        .unwrap();
        send(&mut fixture.svm, &fixture.owner, &[mint_to], &[]).unwrap();
        fixture.workspace = Pubkey::find_program_address(
            &[
                b"workspace",
                fixture.owner.pubkey().as_ref(),
                &0u64.to_le_bytes(),
            ],
            &apodect_solana::ID,
        )
        .0;
        fixture
            .run(
                instruction::InitializeWorkspace { nonce: 0 },
                accounts::InitializeWorkspace {
                    workspace: fixture.workspace,
                    owner: fixture.owner.pubkey(),
                    system_program: solana_system_interface::program::ID,
                },
                Role::Owner,
            )
            .unwrap();
        let (project, vault) = fixture.create_project();
        fixture.project = project;
        fixture.vault = vault;
        fixture
    }

    pub fn key(&self, role: Role) -> Pubkey {
        match role {
            Role::Owner => self.owner.pubkey(),
            Role::Authority => self.authority.pubkey(),
            Role::Agent => self.agent.pubkey(),
            Role::Stranger => self.stranger.pubkey(),
        }
    }

    pub fn run(
        &mut self,
        data: impl InstructionData,
        accounts: impl ToAccountMetas,
        role: Role,
    ) -> Result<(), String> {
        let instruction = ix(data, accounts);
        let signer = match role {
            Role::Owner => &self.owner,
            Role::Authority => &self.authority,
            Role::Agent => &self.agent,
            Role::Stranger => &self.stranger,
        };
        send(&mut self.svm, &self.owner, &[instruction], &[signer])
    }

    pub fn read<T: AccountDeserialize>(&self, key: Pubkey) -> T {
        let account = self.svm.get_account(&key).expect("created account");
        T::try_deserialize(&mut account.data.as_slice()).unwrap()
    }

    pub fn project_state(&self) -> Project {
        self.read(self.project)
    }

    pub fn task(&self, task: Pubkey) -> Task {
        self.read(task)
    }

    pub fn request(&self, request: Pubkey) -> BudgetRequest {
        self.read(request)
    }

    pub fn tokens(&self, token: Pubkey) -> u64 {
        spl_token::state::Account::unpack(&self.svm.get_account(&token).unwrap().data)
            .unwrap()
            .amount
    }

    pub fn new_mint(&mut self, decimals: u8) -> Pubkey {
        let mint = Keypair::new();
        let create = solana_system_interface::instruction::create_account(
            &self.owner.pubkey(),
            &mint.pubkey(),
            self.svm
                .minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN),
            spl_token::state::Mint::LEN as u64,
            &spl_token::ID,
        );
        let initialize = spl_token::instruction::initialize_mint2(
            &spl_token::ID,
            &mint.pubkey(),
            &self.owner.pubkey(),
            Some(&self.owner.pubkey()),
            decimals,
        )
        .unwrap();
        send(&mut self.svm, &self.owner, &[create, initialize], &[&mint]).unwrap();
        mint.pubkey()
    }

    pub fn new_token(&mut self, owner: Pubkey, mint: Pubkey) -> Pubkey {
        let account = Keypair::new();
        let create = solana_system_interface::instruction::create_account(
            &self.owner.pubkey(),
            &account.pubkey(),
            self.svm
                .minimum_balance_for_rent_exemption(spl_token::state::Account::LEN),
            spl_token::state::Account::LEN as u64,
            &spl_token::ID,
        );
        let initialize = spl_token::instruction::initialize_account3(
            &spl_token::ID,
            &account.pubkey(),
            &mint,
            &owner,
        )
        .unwrap();
        send(
            &mut self.svm,
            &self.owner,
            &[create, initialize],
            &[&account],
        )
        .unwrap();
        account.pubkey()
    }

    pub fn create_project(&mut self) -> (Pubkey, Pubkey) {
        let index = self.read::<Workspace>(self.workspace).project_count;
        let project = Pubkey::find_program_address(
            &[b"project", self.workspace.as_ref(), &index.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        let vault =
            Pubkey::find_program_address(&[b"vault", project.as_ref()], &apodect_solana::ID).0;
        self.run(
            instruction::CreateProject {
                authority: self.authority.pubkey(),
            },
            accounts::CreateProject {
                workspace: self.workspace,
                project,
                mint: self.mint,
                vault,
                owner: self.owner.pubkey(),
                token_program: spl_token::ID,
                system_program: solana_system_interface::program::ID,
            },
            Role::Owner,
        )
        .unwrap();
        (project, vault)
    }
}
