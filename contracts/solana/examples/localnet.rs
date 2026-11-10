use std::fs;
use std::path::PathBuf;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::{instruction::Instruction, program_pack::Pack};
use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token;
use apodect_solana::state::{BudgetRequest, Project, RequestState, Task, TaskLifecycle, Workspace};
use apodect_solana::{accounts, instruction};
use serde_json::{json, Value};
use solana_keypair::Keypair;
use solana_signer::Signer;

#[path = "localnet/client.rs"]
mod client;

use client::{address, ensure, Localnet, Result};

const SUPPLY: u64 = 1_000_000_000_000;
const FUNDING: u64 = 100_000_000_000;

fn ix(data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
    Instruction {
        program_id: apodect_solana::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

impl Localnet {
    fn bootstrap(&self) -> Result<()> {
        if self.dir.join("manifest.json").exists() {
            return self.check();
        }
        ensure(
            !self.dir.join("bootstrap-started.json").exists(),
            "interrupted Solana bootstrap detected; use local-reset before trying again",
        )?;
        self.verify_program()?;
        self.write_json("bootstrap-started.json", &json!({"status":"bootstrapping"}))?;
        let owner = self.key("owner", true)?;
        let authority = self.key("authority", true)?;
        let agent = self.key("agent", true)?;
        for key in [&owner, &authority, &agent] {
            let signature = self.rpc(
                "requestAirdrop",
                json!([key.pubkey().to_string(), 20_000_000_000u64]),
            )?;
            self.confirm(signature.as_str().ok_or("missing airdrop signature")?)?;
        }

        let mint = Keypair::new();
        let rent = self
            .rpc(
                "getMinimumBalanceForRentExemption",
                json!([spl_token::state::Mint::LEN]),
            )?
            .as_u64()
            .ok_or("missing mint rent")?;
        self.send(
            &owner,
            &[
                solana_system_interface::instruction::create_account(
                    &owner.pubkey(),
                    &mint.pubkey(),
                    rent,
                    spl_token::state::Mint::LEN as u64,
                    &spl_token::ID,
                ),
                spl_token::instruction::initialize_mint2(
                    &spl_token::ID,
                    &mint.pubkey(),
                    &owner.pubkey(),
                    Some(&owner.pubkey()),
                    6,
                )?,
            ],
            &[&mint],
        )?;
        let owner_token = self.create_token(&owner, mint.pubkey(), owner.pubkey())?;
        let agent_token = self.create_token(&owner, mint.pubkey(), agent.pubkey())?;
        self.send(
            &owner,
            &[spl_token::instruction::mint_to(
                &spl_token::ID,
                &mint.pubkey(),
                &owner_token,
                &owner.pubkey(),
                &[],
                SUPPLY,
            )?],
            &[],
        )?;

        let workspace = Pubkey::find_program_address(
            &[b"workspace", owner.pubkey().as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        self.send(
            &owner,
            &[ix(
                instruction::InitializeWorkspace { nonce: 0 },
                accounts::InitializeWorkspace {
                    workspace,
                    owner: owner.pubkey(),
                    system_program: solana_system_interface::program::ID,
                },
            )],
            &[],
        )?;
        let project = Pubkey::find_program_address(
            &[b"project", workspace.as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        let vault =
            Pubkey::find_program_address(&[b"vault", project.as_ref()], &apodect_solana::ID).0;
        self.send(
            &owner,
            &[ix(
                instruction::CreateProject {
                    authority: authority.pubkey(),
                },
                accounts::CreateProject {
                    workspace,
                    project,
                    mint: mint.pubkey(),
                    vault,
                    owner: owner.pubkey(),
                    token_program: spl_token::ID,
                    system_program: solana_system_interface::program::ID,
                },
            )],
            &[],
        )?;
        self.send(
            &owner,
            &[ix(
                instruction::FundProject { amount: FUNDING },
                accounts::FundProject {
                    workspace,
                    project,
                    mint: mint.pubkey(),
                    vault,
                    owner_token,
                    owner: owner.pubkey(),
                    token_program: spl_token::ID,
                    invoice: None,
                },
            )],
            &[],
        )?;

        self.write_json("manifest.json", &json!({
            "schema_version":1,"chain":"solana","rpc_url":self.url,
            "contracts":{
                "program":apodect_solana::ID.to_string(),
                "workspace":workspace.to_string(),
                "project":project.to_string(),
                "vault":vault.to_string()
            },
            "accounts":{
                "owner":{"address":owner.pubkey().to_string(),"key_file":self.dir.join("keys/owner.json")},
                "authority":{"address":authority.pubkey().to_string(),"key_file":self.dir.join("keys/authority.json")},
                "agent":{"address":agent.pubkey().to_string(),"key_file":self.dir.join("keys/agent.json")}
            },
            "asset":{
                "address":mint.pubkey().to_string(),
                "decimals":6,
                "owner_token":owner_token.to_string(),
                "agent_token":agent_token.to_string()
            },
            "initial_project_funding":FUNDING,"token_program":spl_token::ID.to_string()
        }))?;
        self.check()?;
        fs::remove_file(self.dir.join("bootstrap-started.json"))?;
        println!("Solana workspace, Project and funded token vault bootstrapped");
        Ok(())
    }

    fn manifest(&self) -> Result<Value> {
        let value: Value = serde_json::from_slice(&fs::read(self.dir.join("manifest.json"))?)?;
        ensure(
            value["schema_version"] == 1
                && value["chain"] == "solana"
                && value["rpc_url"] == self.url,
            "Solana manifest identity mismatch",
        )?;
        Ok(value)
    }

    fn check(&self) -> Result<()> {
        let manifest = self.manifest()?;
        self.verify_program()?;
        ensure(
            address(&manifest["contracts"]["program"])? == apodect_solana::ID,
            "manifest program ID mismatch",
        )?;
        let owner = self.key("owner", false)?;
        let authority = self.key("authority", false)?;
        let agent = self.key("agent", false)?;
        ensure(
            owner.pubkey() != authority.pubkey()
                && owner.pubkey() != agent.pubkey()
                && authority.pubkey() != agent.pubkey(),
            "local roles must be distinct",
        )?;
        for (role, key) in [
            ("owner", &owner),
            ("authority", &authority),
            ("agent", &agent),
        ] {
            ensure(
                address(&manifest["accounts"][role]["address"])? == key.pubkey(),
                "manifest role key mismatch",
            )?;
            ensure(
                manifest["accounts"][role]["key_file"]
                    == self
                        .dir
                        .join(format!("keys/{role}.json"))
                        .to_string_lossy()
                        .as_ref(),
                "manifest key path mismatch",
            )?;
            ensure(
                self.rpc(
                    "getBalance",
                    json!([key.pubkey().to_string(), {"commitment":"confirmed"}]),
                )?["value"]
                    .as_u64()
                    .ok_or("missing role balance")?
                    > 0,
                "local role has no SOL",
            )?;
        }
        let workspace_key = address(&manifest["contracts"]["workspace"])?;
        let workspace: Workspace = self.read(workspace_key)?;
        ensure(
            workspace.owner == owner.pubkey(),
            "Workspace owner mismatch",
        )?;
        let project_key = address(&manifest["contracts"]["project"])?;
        let project: Project = self.read(project_key)?;
        let (expected_project, bump) = Pubkey::find_program_address(
            &[
                b"project",
                workspace_key.as_ref(),
                &project.index.to_le_bytes(),
            ],
            &apodect_solana::ID,
        );
        ensure(
            project_key == expected_project
                && project.bump == bump
                && project.workspace == workspace_key
                && workspace.project_count > project.index,
            "Project identity mismatch",
        )?;
        ensure(
            project.authority == authority.pubkey(),
            "Project authority mismatch",
        )?;
        let mint = address(&manifest["asset"]["address"])?;
        let mint_account = self.account(mint)?;
        ensure(
            address(&mint_account["owner"])? == spl_token::ID,
            "mint token program mismatch",
        )?;
        let mint_state = spl_token::state::Mint::unpack(&self.bytes(&mint_account)?)?;
        ensure(
            project.mint == mint && mint_state.decimals == 6 && manifest["asset"]["decimals"] == 6,
            "Project mint/decimal mismatch",
        )?;
        let vault_key = address(&manifest["contracts"]["vault"])?;
        ensure(
            vault_key
                == Pubkey::find_program_address(
                    &[b"vault", project_key.as_ref()],
                    &apodect_solana::ID,
                )
                .0,
            "vault PDA mismatch",
        )?;
        let vault = self.token(vault_key)?;
        ensure(
            vault.mint == mint && vault.owner == project_key,
            "vault custody mismatch",
        )?;
        for (field, holder) in [
            ("owner_token", owner.pubkey()),
            ("agent_token", agent.pubkey()),
        ] {
            let token = self.token(address(&manifest["asset"][field])?)?;
            ensure(
                token.mint == mint && token.owner == holder,
                "role token account mismatch",
            )?;
        }
        let program_accounts = self.rpc(
            "getProgramAccounts",
            json!([apodect_solana::ID.to_string(), {"encoding":"base64","commitment":"confirmed"}]),
        )?;
        let mut controlled = u128::from(project.free);
        for item in program_accounts
            .as_array()
            .ok_or("missing program accounts")?
        {
            let bytes = self.bytes(&item["account"])?;
            if bytes.starts_with(Task::DISCRIMINATOR) {
                let task = Task::try_deserialize(&mut bytes.as_slice())?;
                if task.project == project_key {
                    task.external_outstanding()?;
                    controlled += u128::from(task.available);
                }
            }
        }
        ensure(
            u128::from(vault.amount) >= controlled,
            "vault cannot cover recorded free and controlled task principal",
        )?;
        println!("Solana RPC program, roles, mint, account identities and custody checked");
        Ok(())
    }

    fn smoke(&self) -> Result<()> {
        self.check()?;
        let manifest = self.manifest()?;
        let authority = self.key("authority", false)?;
        let agent = self.key("agent", false)?;
        let workspace = address(&manifest["contracts"]["workspace"])?;
        let project = address(&manifest["contracts"]["project"])?;
        let vault = address(&manifest["contracts"]["vault"])?;
        let mint = address(&manifest["asset"]["address"])?;
        let agent_token = address(&manifest["asset"]["agent_token"])?;
        let before: Project = self.read(project)?;
        ensure(
            before.head.is_none() && before.tail.is_none(),
            "smoke needs an empty queue; finish existing development requests first",
        )?;
        ensure(
            before.free >= 60_000_000,
            "smoke requires 60 test USDC of free Project funding",
        )?;
        let vault_before = self.token(vault)?.amount;
        let agent_before = self.token(agent_token)?.amount;
        let task = Pubkey::find_program_address(
            &[b"task", project.as_ref(), &before.task_count.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        let request = Pubkey::find_program_address(
            &[b"request", task.as_ref(), &0u64.to_le_bytes()],
            &apodect_solana::ID,
        )
        .0;
        let mut signatures = Vec::new();
        signatures.push(self.send(
            &agent,
            &[ix(
                instruction::CreateTask {
                    amount: 60_000_000,
                    purpose: [1; 32],
                },
                accounts::CreateTask {
                    project,
                    task,
                    request,
                    agent: agent.pubkey(),
                    system_program: solana_system_interface::program::ID,
                },
            )],
            &[],
        )?);
        signatures.push(self.send(
            &authority,
            &[ix(
                instruction::ApproveRequest {},
                accounts::ApproveRequest {
                    project,
                    task,
                    request,
                    authority: authority.pubkey(),
                    tail: None,
                },
            )],
            &[],
        )?);
        ensure(
            self.read::<Project>(project)?.free == before.free,
            "approval reserved cash unexpectedly",
        )?;
        signatures.push(self.send(
            &authority,
            &[ix(
                instruction::ApplyNext {},
                accounts::ApplyNext {
                    project,
                    head: request,
                    authority: authority.pubkey(),
                    task: Some(task),
                },
            )],
            &[],
        )?);
        ensure(
            self.read::<Task>(task)?.available == 60_000_000
                && self.read::<Project>(project)?.free == before.free - 60_000_000,
            "full allocation mismatch",
        )?;
        signatures.push(self.send(
            &agent,
            &[ix(
                instruction::Withdraw { amount: 40_000_000 },
                accounts::Withdraw {
                    project,
                    task,
                    mint,
                    vault,
                    agent_token,
                    agent: agent.pubkey(),
                    token_program: spl_token::ID,
                },
            )],
            &[],
        )?);
        signatures.push(self.send(
            &agent,
            &[ix(
                instruction::ReturnFunds { amount: 15_000_000 },
                accounts::ReturnFunds {
                    project,
                    task,
                    mint,
                    vault,
                    agent_token,
                    agent: agent.pubkey(),
                    token_program: spl_token::ID,
                },
            )],
            &[],
        )?);
        ensure(
            self.read::<Task>(task)?.available == 20_000_000,
            "return refilled task rights unexpectedly",
        )?;
        signatures.push(self.send(
            &agent,
            &[ix(
                instruction::ReleaseFunds { amount: 20_000_000 },
                accounts::ReleaseFunds {
                    project,
                    task,
                    agent: agent.pubkey(),
                },
            )],
            &[],
        )?);
        signatures.push(self.send(
            &agent,
            &[ix(
                instruction::FinalizeTask { report: [9; 32] },
                accounts::FinalizeTask {
                    workspace,
                    project,
                    task,
                    actor: agent.pubkey(),
                    pending: None,
                },
            )],
            &[],
        )?);
        let state: Task = self.read(task)?;
        let final_project: Project = self.read(project)?;
        ensure(
            state.lifecycle == TaskLifecycle::Completed
                && state.started
                && !state.paused
                && state.pending.is_none()
                && state.available == 0
                && state.issued == 60_000_000
                && state.recovered == 35_000_000
                && state.final_expense()? == Some(25_000_000)
                && state.report == [9; 32],
            "completed task accounting mismatch",
        )?;
        ensure(
            self.read::<BudgetRequest>(request)?.state == RequestState::Funded
                && final_project.head.is_none()
                && final_project.tail.is_none()
                && final_project.free == before.free - 25_000_000,
            "completed request/queue/free accounting mismatch",
        )?;
        ensure(
            self.token(vault)?.amount == vault_before - 25_000_000
                && self.token(agent_token)?.amount == agent_before + 25_000_000,
            "real token balance mismatch",
        )?;
        self.write_json(
            "last-smoke.json",
            &json!({
                "chain":"solana",
                "task":task.to_string(),
                "request":request.to_string(),
                "expense":25_000_000,
                "signatures":signatures,
                "smoke":"passed"
            }),
        )?;
        self.check()?;
        println!("Solana RPC smoke passed: created, approved, allocated, withdrew, returned, released, finalized; expense 25 test USDC");
        Ok(())
    }
}

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::var("APODECT_LOCALNET_DIR")?);
    ensure(dir.is_absolute(), "APODECT_LOCALNET_DIR must be absolute")?;
    let localnet = Localnet {
        dir,
        url: std::env::var("APODECT_RPC_URL")?,
    };
    let port = localnet
        .url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|port| port.parse::<u16>().ok());
    ensure(
        port.is_some_and(|port| (1024..=65534).contains(&port)),
        "only a numeric loopback RPC port is allowed",
    )?;
    match std::env::args().nth(1).as_deref() {
        Some("prepare") => localnet.prepare(),
        Some("bootstrap") => localnet.bootstrap(),
        Some("check") => localnet.check(),
        Some("smoke") => localnet.smoke(),
        _ => Err("usage: localnet {prepare|bootstrap|check|smoke}".into()),
    }
}
