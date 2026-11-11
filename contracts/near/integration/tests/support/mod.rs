#![allow(dead_code)]
//! Shared sandbox deployment, transaction and view helpers.
use anyhow::{Context, Result, ensure};
use near_workspaces::network::Sandbox;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, AccountId, Contract, Worker};
use serde_json::{Value, json};

pub(crate) struct Fixture {
    pub(crate) worker: Worker<Sandbox>,
    pub(crate) owner: Account,
    pub(crate) authority: Account,
    pub(crate) agent: Account,
    pub(crate) other: Account,
    pub(crate) token: Contract,
    pub(crate) factory: Contract,
    pub(crate) project: AccountId,
}

pub(crate) fn wasm(name: &str) -> Result<Vec<u8>> {
    let root = std::path::PathBuf::from(
        std::env::var_os("APODECT_NEAR_ARTIFACT_DIR")
            .context("APODECT_NEAR_ARTIFACT_DIR is required; run scripts/check.sh")?,
    );
    ensure!(
        root.is_absolute(),
        "APODECT_NEAR_ARTIFACT_DIR must be absolute"
    );
    let path = root.join(format!("apodect_near_{name}.wasm"));
    std::fs::read(&path).with_context(|| format!("read {}; run scripts/check.sh", path.display()))
}

pub(crate) async fn tx(
    actor: &Account,
    contract: &AccountId,
    method: &str,
    args: Value,
) -> Result<ExecutionFinalResult> {
    Ok(actor
        .call(contract, method)
        .args_json(args)
        .deposit(NearToken::from_near(1))
        .max_gas()
        .transact()
        .await?)
}

pub(crate) async fn ok(
    actor: &Account,
    contract: &AccountId,
    method: &str,
    args: Value,
) -> Result<Value> {
    let result = tx(actor, contract, method, args).await?.into_result()?;
    Ok(result.json().unwrap_or(Value::Null))
}

pub(crate) async fn fails(
    actor: &Account,
    contract: &AccountId,
    method: &str,
    args: Value,
) -> Result<()> {
    ensure!(
        tx(actor, contract, method, args).await?.is_failure(),
        "{method} unexpectedly succeeded"
    );
    Ok(())
}

pub(crate) async fn fails_without_deposit(
    actor: &Account,
    contract: &AccountId,
    method: &str,
    args: Value,
    reason: &str,
) -> Result<()> {
    let outcome = actor
        .call(contract, method)
        .args_json(args)
        .max_gas()
        .transact()
        .await?;
    ensure!(outcome.is_failure(), "{method} unexpectedly succeeded");
    ensure!(
        format!("{outcome:?}").contains(reason),
        "unexpected {method} failure: {outcome:?}"
    );
    Ok(())
}

impl Fixture {
    pub(crate) async fn new() -> Result<Self> {
        let worker = near_workspaces::sandbox().await?;
        let owner = worker.dev_create_account().await?;
        let authority = worker.dev_create_account().await?;
        let agent = worker.dev_create_account().await?;
        let other = worker.dev_create_account().await?;

        let token = worker.dev_deploy(&wasm("test_ft")?).await?;
        token
            .call("new")
            .args_json(json!({
                "owner_id": owner.id(),
                "total_supply": "1000000000"
            }))
            .max_gas()
            .transact()
            .await?
            .into_result()
            .context("initialize test FT")?;

        let factory = worker.dev_deploy(&wasm("workspace")?).await?;
        factory
            .call("new")
            .args_json(json!({
                "owner_id": owner.id(),
                "token_id": token.id()
            }))
            .max_gas()
            .transact()
            .await?
            .into_result()
            .context("initialize Workspace")?;

        let creation = owner
            .call(factory.id(), "create_project")
            .args_json(json!({"authority_id": authority.id()}))
            .deposit(NearToken::from_near(10))
            .max_gas()
            .transact()
            .await?
            .into_result()
            .with_context(|| format!("create Project under {}", factory.id()))?;
        ensure!(
            creation.json::<bool>()?,
            "Project creation failed: {creation:?}"
        );

        let record: Value = factory
            .view("get_project")
            .args_json(json!({"project_id": 0}))
            .await?
            .json()?;
        let project: AccountId = record["account_id"].as_str().unwrap().parse()?;

        let fixture = Self {
            worker,
            owner,
            authority,
            agent,
            other,
            token,
            factory,
            project,
        };

        fixture.register(&fixture.project).await?;
        fixture.register(fixture.agent.id()).await?;
        Ok(fixture)
    }

    pub(crate) async fn register(&self, account_id: &AccountId) -> Result<()> {
        self.owner
            .call(self.token.id(), "storage_deposit")
            .args_json(json!({
                "account_id": account_id,
                "registration_only": true
            }))
            .deposit(NearToken::from_millinear(10))
            .transact()
            .await?
            .into_result()?;
        Ok(())
    }

    pub(crate) async fn view(&self, method: &str, args: Value) -> Result<Value> {
        Ok(self
            .worker
            .view(&self.project, method)
            .args_json(args)
            .await?
            .json()?)
    }

    pub(crate) async fn deposit(
        &self,
        actor: &Account,
        project: &AccountId,
        amount: u128,
        msg: Value,
    ) -> Result<u128> {
        let outcome = actor
            .call(self.token.id(), "ft_transfer_call")
            .args_json(json!({
                "receiver_id": project,
                "amount": amount.to_string(),
                "memo": null,
                "msg": msg.to_string()
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        Ok(outcome.json::<String>()?.parse()?)
    }

    pub(crate) async fn fund(&self, amount: u128) -> Result<()> {
        ensure!(
            self.deposit(&self.owner, &self.project, amount, json!({"kind": "fund"}))
                .await?
                == amount
        );
        Ok(())
    }

    pub(crate) async fn create(&self, actor: &Account, amount: u128) -> Result<u64> {
        Ok(ok(
            actor,
            &self.project,
            "create_task",
            json!({
                "amount": amount.to_string(),
                "purpose": reference(1)
            }),
        )
        .await?
        .as_u64()
        .unwrap())
    }

    pub(crate) async fn approve_task(&self, task_id: u64) -> Result<()> {
        let task = self.view("get_task", json!({"task_id": task_id})).await?;
        ok(
            &self.authority,
            &self.project,
            "approve_request",
            json!({"request_id": task["live_request"]}),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn apply(&self) -> Result<Value> {
        ok(&self.authority, &self.project, "apply", json!({})).await
    }

    pub(crate) async fn amount(&self, field: &str) -> Result<u128> {
        Ok(number(&self.view("get_project", json!({})).await?, field))
    }

    pub(crate) async fn balance(&self, account: &AccountId) -> Result<u128> {
        Ok(self
            .token
            .view("ft_balance_of")
            .args_json(json!({"account_id": account}))
            .await?
            .json::<String>()?
            .parse()?)
    }
}

pub(crate) fn reference(value: u8) -> Vec<u8> {
    vec![value; 32]
}

pub(crate) fn number(value: &Value, field: &str) -> u128 {
    value[field].as_str().unwrap().parse().unwrap()
}
