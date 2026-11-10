//! Persistent local sandbox deployment and RPC lifecycle tooling. No public network support.

use anyhow::{Context, Result, bail, ensure};
use near_workspaces::network::{Sandbox, ValidatorKey};
use near_workspaces::types::{KeyType, NearToken, SecretKey};
use near_workspaces::{Account, AccountId, Worker};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT_ACCOUNT: &str = "apodect.test.near";
const CHAIN_ID: &str = "apodect-local";
const INITIAL_FUNDING: u128 = 1_000_000_000;

struct Localnet {
    data: PathBuf,
    rpc: String,
    worker: Worker<Sandbox>,
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn account_id(role: &str) -> Result<AccountId> {
    Ok(format!("{role}.{ROOT_ACCOUNT}").parse()?)
}

fn wasm(name: &str) -> Result<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../artifacts")
        .join(format!("apodect_near_{name}.wasm"));
    fs::read(&path).with_context(|| format!("read {}; run prepare first", path.display()))
}

fn number(value: &Value, field: &str) -> Result<u128> {
    value[field]
        .as_str()
        .with_context(|| format!("missing decimal field {field}"))?
        .parse()
        .with_context(|| format!("invalid decimal field {field}"))
}

fn genesis_identity(data: &Path) -> Result<Value> {
    let genesis = read_json(&data.join("node/genesis.json"))?;
    Ok(json!({
        "chain_id": genesis["chain_id"],
        "genesis_time": genesis["genesis_time"],
        "genesis_height": genesis["genesis_height"],
        "validators": genesis["validators"]
    }))
}

fn prepare(data: &Path) -> Result<()> {
    fs::create_dir_all(data)?;
    let binary = near_sandbox::install()?;
    let version = Command::new(&binary).arg("--version").output()?;
    ensure!(version.status.success(), "sandbox version probe failed");
    ensure!(
        String::from_utf8_lossy(&version.stdout).contains("release 2.14.0"),
        "local environment requires near-sandbox 2.14.0"
    );
    fs::write(data.join("sandbox.path"), format!("{}\n", binary.display()))?;
    let node = data.join("node");
    if !node.join("config.json").exists() {
        ensure!(
            !node.exists(),
            "incomplete node home; use localnet reset to recreate it"
        );
        let status = Command::new(&binary)
            .arg("--home")
            .arg(&node)
            .args([
                "init",
                "--fast",
                "--account-id",
                ROOT_ACCOUNT,
                "--chain-id",
                CHAIN_ID,
            ])
            .status()?;
        ensure!(status.success(), "sandbox initialization failed");
    }
    let genesis = read_json(&node.join("genesis.json"))?;
    ensure!(
        genesis["chain_id"] == CHAIN_ID,
        "unexpected local genesis chain ID"
    );
    ensure!(read_json(&node.join("validator_key.json"))?["account_id"] == ROOT_ACCOUNT);
    for file in ["validator_key.json", "node_key.json"] {
        fs::set_permissions(node.join(file), fs::Permissions::from_mode(0o600))?;
    }
    let mut config = read_json(&node.join("config.json"))?;
    config["rpc"]["addr"] = json!(format!("127.0.0.1:{}", std::env::var("APODECT_RPC_PORT")?));
    config["rpc"]["prometheus_addr"] = Value::Null;
    config["network"]["addr"] = json!(format!(
        "127.0.0.1:{}",
        std::env::var("APODECT_NETWORK_PORT")?
    ));
    config["network"]["boot_nodes"] = json!("");
    config["telemetry"]["endpoints"] = json!([]);
    write_json(&node.join("config.json"), &config)?;
    println!("NEAR persistent sandbox prepared: {}", node.display());
    Ok(())
}

impl Localnet {
    async fn connect(data: PathBuf, rpc: String) -> Result<Self> {
        let port: u16 = std::env::var("APODECT_RPC_PORT")?.parse()?;
        ensure!(
            port > 0 && rpc == format!("http://127.0.0.1:{port}"),
            "RPC URL must exactly match the configured loopback port"
        );
        let worker = near_workspaces::sandbox()
            .rpc_addr(&rpc)
            .validator_key(ValidatorKey::HomeDir(data.join("node")))
            .await?;
        ensure!(
            worker.status().await?.chain_id == CHAIN_ID,
            "RPC chain ID mismatch"
        );
        let expected = read_json(&data.join("node/genesis.json"))?;
        let actual = serde_json::to_value(worker.genesis_config().await?)?;
        for field in ["chain_id", "genesis_time", "genesis_height", "validators"] {
            ensure!(
                actual[field] == expected[field],
                "RPC genesis {field} mismatch"
            );
        }
        Ok(Self { data, rpc, worker })
    }

    fn key_file(&self, role: &str) -> PathBuf {
        self.data.join("keys").join(format!("{role}.json"))
    }

    fn account(&self, role: &str) -> Result<Account> {
        let path = self.key_file(role);
        ensure!(
            fs::metadata(&path)?.permissions().mode() & 0o077 == 0,
            "key file must be private: {}",
            path.display()
        );
        let account = Account::from_file(path, &self.worker)?;
        ensure!(
            account.id() == &account_id(role)?,
            "account/key role mismatch"
        );
        Ok(account)
    }

    async fn ensure_account(&self, role: &str) -> Result<Account> {
        let path = self.key_file(role);
        if !path.exists() {
            fs::create_dir_all(path.parent().unwrap())?;
            let key = SecretKey::from_random(KeyType::ED25519);
            write_json(
                &path,
                &json!({
                    "account_id": account_id(role)?,
                    "public_key": key.public_key().to_string(),
                    "private_key": key.to_string()
                }),
            )?;
        }
        let account = self.account(role)?;
        if self.worker.view_account(account.id()).await.is_err() {
            let root = Account::from_file(self.data.join("node/validator_key.json"), &self.worker)?;
            root.create_subaccount(role)
                .keys(account.secret_key().clone())
                .initial_balance(NearToken::from_near(100))
                .transact()
                .await?
                .into_result()?;
        }
        account
            .view_access_key(&account.secret_key().public_key())
            .await?;
        Ok(account)
    }

    async fn view(&self, account: &AccountId, method: &str, args: Value) -> Result<Value> {
        Ok(self
            .worker
            .view(account, method)
            .args_json(args)
            .await?
            .json()?)
    }

    async fn call(
        &self,
        actor: &Account,
        target: &AccountId,
        method: &str,
        args: Value,
        deposit: NearToken,
    ) -> Result<Value> {
        let outcome = actor
            .call(target, method)
            .args_json(args)
            .deposit(deposit)
            .max_gas()
            .transact()
            .await?
            .into_result()
            .with_context(|| format!("{target}.{method}"))?;
        Ok(outcome.json().unwrap_or(Value::Null))
    }

    async fn deploy(&self, account: &Account, artifact: &str, args: Value) -> Result<()> {
        let expected = wasm(artifact)?;
        // The SDK's default contract state is None; querying its code would fail.
        if account.view_account().await?.contract_state == Default::default() {
            account.deploy(&expected).await?.into_result()?;
        } else {
            ensure!(
                self.worker.view_code(account.id()).await? == expected,
                "{} code differs from current build; reset localnet to redeploy",
                account.id()
            );
        }
        let state = self.worker.view_state(account.id()).await?;
        if !state.contains_key(b"STATE".as_slice()) {
            self.call(
                account,
                account.id(),
                "new",
                args,
                NearToken::from_yoctonear(0),
            )
            .await?;
        }
        Ok(())
    }

    async fn deposit(
        &self,
        actor: &Account,
        project: &AccountId,
        amount: u128,
        msg: Value,
    ) -> Result<()> {
        let result = self.call(actor, &account_id("token")?, "ft_transfer_call", json!({
            "receiver_id": project, "amount": amount.to_string(), "memo": null, "msg": msg.to_string()
        }), NearToken::from_yoctonear(1)).await?;
        ensure!(
            result == amount.to_string(),
            "token receiver did not accept the complete deposit"
        );
        Ok(())
    }

    async fn bootstrap(&self) -> Result<()> {
        if self.data.join("manifest.json").exists() {
            self.check().await?;
            println!("NEAR existing deployment verified");
            return Ok(());
        }
        let owner = self.ensure_account("owner").await?;
        let authority = self.ensure_account("authority").await?;
        let agent = self.ensure_account("agent").await?;
        let token = self.ensure_account("token").await?;
        let workspace = self.ensure_account("workspace").await?;
        self.deploy(
            &token,
            "test_ft",
            json!({"owner_id": owner.id(), "total_supply": "1000000000000"}),
        )
        .await?;
        self.deploy(
            &workspace,
            "workspace",
            json!({"owner_id": owner.id(), "token_id": token.id()}),
        )
        .await?;
        let mut creation = self
            .view(workspace.id(), "get_project", json!({"project_id": 0}))
            .await?;
        if creation.is_null() {
            let result = self
                .call(
                    &owner,
                    workspace.id(),
                    "create_project",
                    json!({"authority_id": authority.id()}),
                    NearToken::from_near(10),
                )
                .await?;
            ensure!(result == true, "factory creation did not succeed");
            creation = self
                .view(workspace.id(), "get_project", json!({"project_id": 0}))
                .await?;
        }
        ensure!(
            creation["state"] == "Ready",
            "factory Project is not Ready; inspect local node before resetting"
        );
        let project: AccountId = creation["account_id"]
            .as_str()
            .context("Project address missing")?
            .parse()?;
        for account in [&project, agent.id()] {
            self.call(
                &owner,
                token.id(),
                "storage_deposit",
                json!({"account_id": account, "registration_only": true}),
                NearToken::from_millinear(10),
            )
            .await?;
        }
        let state = self.view(&project, "get_project", json!({})).await?;
        ensure!(
            state["next_task"] == 0,
            "partial bootstrap already has tasks; refusing to guess initial funding"
        );
        let free = number(&state, "free")?;
        if free == 0 {
            self.deposit(&owner, &project, INITIAL_FUNDING, json!({"kind": "fund"}))
                .await?;
        } else {
            ensure!(
                free == INITIAL_FUNDING,
                "unexpected partial bootstrap funding"
            );
        }
        let mut accounts = serde_json::Map::new();
        for role in ["owner", "authority", "agent", "token", "workspace"] {
            accounts.insert(
                role.into(),
                json!({"address": account_id(role)?, "key_file": self.key_file(role)}),
            );
        }
        let manifest = json!({
            "schema_version": 1, "chain": "near", "rpc_url": self.rpc,
            "chain_id": CHAIN_ID, "genesis": genesis_identity(&self.data)?,
            "sandbox_version": "2.14.0",
            "contracts": {"workspace": workspace.id(), "project": project, "token": token.id()},
            "accounts": accounts, "asset": {"address": token.id(), "decimals": 6, "symbol": "LOCAL_USDC"},
            "initial_funding": INITIAL_FUNDING.to_string()
        });
        write_json(&self.data.join("manifest.json"), &manifest)?;
        self.check().await?;
        println!("NEAR deployed and funded: {project}");
        Ok(())
    }

    async fn check(&self) -> Result<Value> {
        let manifest = read_json(&self.data.join("manifest.json"))?;
        ensure!(
            manifest["schema_version"] == 1
                && manifest["chain"] == "near"
                && manifest["rpc_url"] == self.rpc,
            "manifest network mismatch"
        );
        ensure!(
            manifest["chain_id"] == CHAIN_ID
                && manifest["genesis"] == genesis_identity(&self.data)?,
            "manifest genesis mismatch"
        );
        let owner = self.account("owner")?;
        let authority = self.account("authority")?;
        for role in ["owner", "authority", "agent", "token", "workspace"] {
            let account = self.account(role)?;
            ensure!(
                manifest["accounts"][role]["address"] == account.id().as_str(),
                "manifest role mismatch"
            );
            ensure!(
                manifest["accounts"][role]["key_file"]
                    == self.key_file(role).to_string_lossy().as_ref(),
                "manifest key path mismatch"
            );
            account
                .view_access_key(&account.secret_key().public_key())
                .await?;
        }
        let project: AccountId = manifest["contracts"]["project"]
            .as_str()
            .context("missing Project address")?
            .parse()?;
        let token = account_id("token")?;
        let workspace = account_id("workspace")?;
        ensure!(
            manifest["contracts"]["workspace"] == workspace.as_str()
                && manifest["contracts"]["token"] == token.as_str()
        );
        ensure!(
            manifest["asset"]["address"] == token.as_str() && manifest["asset"]["decimals"] == 6
        );
        for (address, artifact) in [
            (&project, "project"),
            (&workspace, "workspace"),
            (&token, "test_ft"),
        ] {
            ensure!(
                self.worker.view_code(address).await? == wasm(artifact)?,
                "{address} deployed code does not match current artifacts; reset to redeploy"
            );
        }
        let factory = self.view(&workspace, "get_workspace", json!({})).await?;
        ensure!(
            factory["owner_id"] == owner.id().as_str() && factory["token_id"] == token.as_str(),
            "factory roles/token mismatch"
        );
        let creation = self
            .view(&workspace, "get_project", json!({"project_id": 0}))
            .await?;
        ensure!(
            creation["state"] == "Ready" && creation["account_id"] == project.as_str(),
            "factory Project identity mismatch"
        );
        let state = self.view(&project, "get_project", json!({})).await?;
        ensure!(
            state["owner_id"] == owner.id().as_str()
                && state["authority_id"] == authority.id().as_str()
                && state["token_id"] == token.as_str(),
            "Project roles/token mismatch"
        );
        let balance: u128 = self
            .view(&token, "ft_balance_of", json!({"account_id": project}))
            .await?
            .as_str()
            .context("token balance missing")?
            .parse()?;
        ensure!(
            balance >= number(&state, "free")?,
            "Project free ledger exceeds physical token balance"
        );
        Ok(json!({
            "chain": "near",
            "project": project,
            "free": state["free"],
            "token_balance": balance.to_string(),
            "next_task": state["next_task"]
        }))
    }

    async fn smoke(&self) -> Result<()> {
        self.check().await?;
        let owner = self.account("owner")?;
        let authority = self.account("authority")?;
        let agent = self.account("agent")?;
        let project: AccountId = format!("p0.{}", account_id("workspace")?).parse()?;
        let before = self.view(&project, "get_project", json!({})).await?;
        ensure!(
            before["queue_head"] == before["queue_tail"],
            "smoke requires an empty Project queue"
        );
        self.deposit(&owner, &project, 60_000_000, json!({"kind": "fund"}))
            .await?;
        let deposit = NearToken::from_near(1);
        let task = self
            .call(
                &agent,
                &project,
                "create_task",
                json!({"amount": "60000000", "purpose": vec![1u8; 32]}),
                deposit,
            )
            .await?;
        let task_state = self
            .view(&project, "get_task", json!({"task_id": task}))
            .await?;
        self.call(
            &authority,
            &project,
            "approve_request",
            json!({"request_id": task_state["live_request"]}),
            deposit,
        )
        .await?;
        let applied = self
            .call(&authority, &project, "apply", json!({}), deposit)
            .await?;
        ensure!(
            applied["Funded"]["task_id"] == task && applied["Funded"]["amount"] == "60000000",
            "unexpected allocation result"
        );
        ensure!(
            self.call(
                &agent,
                &project,
                "withdraw",
                json!({"task_id": task, "amount": "40000000"}),
                deposit
            )
            .await?
                == true
        );
        self.deposit(
            &agent,
            &project,
            15_000_000,
            json!({"kind": "return", "task_id": task}),
        )
        .await?;
        self.call(
            &agent,
            &project,
            "release",
            json!({"task_id": task, "amount": "20000000"}),
            deposit,
        )
        .await?;
        ensure!(
            self.call(
                &agent,
                &project,
                "finalize",
                json!({"task_id": task, "report": vec![2u8; 32]}),
                deposit
            )
            .await?
                == "25000000"
        );
        let final_task = self
            .view(&project, "get_task", json!({"task_id": task}))
            .await?;
        let after = self.view(&project, "get_project", json!({})).await?;
        ensure!(
            final_task["state"] == "Completed"
                && final_task["available"] == "0"
                && final_task["pending_outflow"] == "0"
                && final_task["expense"] == "25000000"
        );
        ensure!(
            number(&after, "free")? == number(&before, "free")? + 35_000_000,
            "smoke accounting mismatch"
        );
        self.check().await?;
        let report = json!({
            "chain": "near",
            "smoke": "passed",
            "task_id": task,
            "state": "Completed",
            "issued": "60000000",
            "recovered": "35000000",
            "expense": "25000000",
            "project_free": after["free"]
        });
        write_json(&self.data.join("last-smoke.json"), &report)?;
        println!("{}", serde_json::to_string(&report)?);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let command = std::env::args()
        .nth(1)
        .context("expected prepare, bootstrap, check or smoke")?;
    let data = PathBuf::from(
        std::env::var("APODECT_LOCALNET_DIR").context("APODECT_LOCALNET_DIR is required")?,
    );
    ensure!(data.is_absolute(), "local data directory must be absolute");
    for name in ["APODECT_RPC_PORT", "APODECT_NETWORK_PORT"] {
        let raw = std::env::var(name)?;
        let port: u16 = raw.parse().with_context(|| format!("invalid {name}"))?;
        ensure!(port > 0 && raw == port.to_string(), "invalid {name}");
    }
    if command == "prepare" {
        return prepare(&data);
    }
    let local = Localnet::connect(data, std::env::var("APODECT_RPC_URL")?).await?;
    match command.as_str() {
        "bootstrap" => local.bootstrap().await,
        "check" => {
            println!("{}", local.check().await?);
            Ok(())
        }
        "smoke" => local.smoke().await,
        _ => bail!("unknown localnet command: {command}"),
    }
}
