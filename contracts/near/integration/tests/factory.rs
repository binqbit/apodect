mod support;

use anyhow::{Result, ensure};
use near_workspaces::AccountId;
use near_workspaces::types::{AccountDetailsPatch, NearToken};
use serde_json::{Value, json};
use support::{Fixture, fails, number, reference, wasm};

#[tokio::test]
async fn factory_isolation_failed_creation_refund_and_storage_prepayment() -> Result<()> {
    let f = Fixture::new().await?;
    let state_before = f.view("get_project", json!({})).await?;
    let underpaid = f
        .agent
        .call(&f.project, "create_task")
        .args_json(json!({
            "amount": "10",
            "purpose": reference(1)
        }))
        .max_gas()
        .transact()
        .await?;
    ensure!(underpaid.is_failure());
    let state_after = f.view("get_project", json!({})).await?;
    ensure!(state_before["next_task"] == state_after["next_task"]);
    f.fund(100).await?;

    // Sandbox setup injects an existing future child; the factory then executes a real failed create receipt.
    let occupied: AccountId = format!("p1.{}", f.factory.id()).parse()?;
    f.worker
        .patch(&occupied)
        .account(AccountDetailsPatch::default().balance(NearToken::from_near(10)))
        .transact()
        .await?;
    let result = f
        .owner
        .call(f.factory.id(), "create_project")
        .args_json(json!({"authority_id": f.authority.id()}))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    ensure!(!result.json::<bool>()?);
    let failed: Value = f
        .factory
        .view("get_project")
        .args_json(json!({"project_id": 1}))
        .await?
        .json()?;
    ensure!(failed["state"] == "Failed" && number(&failed, "refund_credit") > 0);
    ensure!(f.worker.view_account(&occupied).await?.balance == NearToken::from_near(10));
    f.owner
        .call(f.factory.id(), "claim_failed_creation")
        .args_json(json!({"project_id": 1}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let failed: Value = f
        .factory
        .view("get_project")
        .args_json(json!({"project_id": 1}))
        .await?
        .json()?;
    ensure!(number(&failed, "refund_credit") == 0 && failed["refund_pending"] == false);
    ensure!(
        f.owner
            .call(f.factory.id(), "claim_failed_creation")
            .args_json(json!({"project_id": 1}))
            .max_gas()
            .transact()
            .await?
            .is_failure()
    );
    f.owner
        .call(f.factory.id(), "create_project")
        .args_json(json!({"authority_id": f.authority.id()}))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let second: Value = f
        .factory
        .view("get_project")
        .args_json(json!({"project_id": 2}))
        .await?
        .json()?;
    ensure!(second["state"] == "Ready");
    let second: AccountId = second["account_id"].as_str().unwrap().parse()?;
    ensure!(second != f.project);
    f.register(&second).await?;
    ensure!(
        f.deposit(&f.owner, &second, 20, json!({"kind": "fund"}))
            .await?
            == 20
    );
    let second_view: Value = f
        .worker
        .view(&second, "get_project")
        .args_json(json!({}))
        .await?
        .json()?;
    ensure!(number(&second_view, "free") == 20 && f.amount("free").await? == 100);
    ensure!(f.balance(&second).await? == 20 && f.balance(&f.project).await? == 100);

    // A task ID from another Project is not recognized here.
    fails(
        &f.agent,
        &second,
        "withdraw",
        json!({
            "task_id": 0,
            "amount": "1"
        }),
    )
    .await?;
    Ok(())
}

fn uleb(bytes: &[u8], cursor: &mut usize) -> usize {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= usize::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return value;
        }
    }
    panic!("invalid test Wasm LEB");
}

fn remove_initializer_export(bytes: &mut [u8]) {
    let mut cursor = 8;
    while cursor < bytes.len() {
        let section = bytes[cursor];
        cursor += 1;
        let length = uleb(bytes, &mut cursor);
        let end = cursor + length;
        if section == 7 {
            let count = uleb(bytes, &mut cursor);
            for _ in 0..count {
                let length = uleb(bytes, &mut cursor);
                let start = cursor;
                cursor += length;
                let is_initializer = &bytes[start..cursor] == b"new";
                cursor += 1; // export kind
                uleb(bytes, &mut cursor); // export index
                if is_initializer {
                    bytes[start..start + length].copy_from_slice(b"old");
                    return;
                }
            }
        }
        cursor = end;
    }
    panic!("Project initializer export not found");
}

#[tokio::test]
async fn failed_initializer_rolls_back_child_funding_and_refunds_exactly_once() -> Result<()> {
    let project = wasm("project")?;
    let mut factory_code = wasm("workspace")?;
    let position = factory_code
        .windows(project.len())
        .position(|bytes| bytes == project)
        .expect("embedded Project code not found");
    remove_initializer_export(&mut factory_code[position..position + project.len()]);
    let worker = near_workspaces::sandbox().await?;
    let owner = worker.dev_create_account().await?;
    let authority = worker.dev_create_account().await?;
    let factory = worker.dev_deploy(&factory_code).await?;
    factory
        .call("new")
        .args_json(json!({
            "owner_id": owner.id(),
            "token_id": "usdc.test.near"
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let creation = owner
        .call(factory.id(), "create_project")
        .args_json(json!({"authority_id": authority.id()}))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    ensure!(!creation.json::<bool>()?);
    ensure!(
        format!("{creation:?}").contains("MethodNotFound"),
        "unexpected failure: {creation:?}"
    );
    let record: Value = factory
        .view("get_project")
        .args_json(json!({"project_id": 0}))
        .await?
        .json()?;
    ensure!(record["state"] == "Failed");
    let child = record["account_id"].as_str().unwrap().parse()?;
    ensure!(
        worker.view_account(&child).await.is_err(),
        "failed atomic creation left a funded child"
    );
    let refund = record["refund_credit"].as_str().unwrap().parse::<u128>()?;
    ensure!(
        refund
            == record["initial_balance"]
                .as_str()
                .unwrap()
                .parse::<u128>()?
    );
    let before = owner.view_account().await?.balance.as_yoctonear();
    owner
        .call(factory.id(), "claim_failed_creation")
        .args_json(json!({"project_id": 0}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let after = owner.view_account().await?.balance.as_yoctonear();
    ensure!(after > before && after - before <= refund);
    let record: Value = factory
        .view("get_project")
        .args_json(json!({"project_id": 0}))
        .await?
        .json()?;
    ensure!(record["refund_credit"] == "0" && record["refund_pending"] == false);
    ensure!(
        owner
            .call(factory.id(), "claim_failed_creation")
            .args_json(json!({"project_id": 0}))
            .max_gas()
            .transact()
            .await?
            .is_failure()
    );
    let workspace: Value = factory.view("get_workspace").await?.json()?;
    ensure!(workspace["next_project"] == 1);
    Ok(())
}
