//! Real sandbox NEP-141 custody, keyless route receivers and authenticated command execution.
use anyhow::{Context, Result, ensure};
use near_workspaces::{Account, AccountId, Worker, network::Sandbox, types::NearToken};
use serde_json::{Value, json};

fn wasm(name: &str) -> Result<Vec<u8>> {
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
fn word(n: u64) -> String {
    format!("{n:064x}")
}
async fn call(
    actor: &Account,
    target: &AccountId,
    method: &str,
    args: Value,
    deposit: NearToken,
) -> Result<Value> {
    Ok(actor
        .call(target, method)
        .args_json(args)
        .deposit(deposit)
        .max_gas()
        .transact()
        .await?
        .into_result()?
        .json()
        .unwrap_or(Value::Null))
}
async fn view(
    worker: &Worker<Sandbox>,
    target: &AccountId,
    method: &str,
    args: Value,
) -> Result<Value> {
    Ok(worker.view(target, method).args_json(args).await?.json()?)
}
async fn execute(owner: &Account, project: &AccountId, name: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(include_str!(
        "../../project/tests/fixtures/coordinated-signatures.json"
    ))?;
    let mut args = v[name].clone();
    args["recipient_id"] = if name == "dispatch" {
        json!("sink.test.near")
    } else {
        Value::Null
    };
    let mut deposit = NearToken::from_millinear(100).as_yoctonear();
    if matches!(name, "delivery" | "refund") {
        let deployment: String = owner
            .view(project, "route_deployment_deposit")
            .args_json(json!({}))
            .await?
            .json()?;
        deposit += deployment.parse::<u128>()?;
    }
    call(
        owner,
        project,
        "execute_command",
        args,
        NearToken::from_yoctonear(deposit),
    )
    .await
}
async fn transfer(
    owner: &Account,
    token: &AccountId,
    receiver: &AccountId,
    amount: u64,
) -> Result<()> {
    call(
        owner,
        token,
        "ft_transfer",
        json!({"receiver_id": receiver, "amount": amount.to_string(), "memo": null}),
        NearToken::from_yoctonear(1),
    )
    .await?;
    Ok(())
}

async fn funding_storage_recovers_after_native_topup(
    worker: &Worker<Sandbox>,
    owner: &Account,
    other: &Account,
    token: &AccountId,
    project: &AccountId,
) -> Result<()> {
    let before = view(worker, project, "get_project", json!({})).await?;
    let bridge_before = view(worker, project, "get_bridge", json!({})).await?;
    let funding_id = word(bridge_before["next_funding"].as_u64().unwrap());
    let reference = "cd".repeat(32);
    let message = json!({"kind": "fund", "funding_ref": reference}).to_string();
    let owner_before = view(
        worker,
        token,
        "ft_balance_of",
        json!({"account_id": owner.id()}),
    )
    .await?;
    let project_before = view(
        worker,
        token,
        "ft_balance_of",
        json!({"account_id": project}),
    )
    .await?;
    let other_before = view(
        worker,
        token,
        "ft_balance_of",
        json!({"account_id": other.id()}),
    )
    .await?;
    // The pinned sandbox and near-sdk 5.30.1 use this native storage byte price.
    let byte_cost = 10_000_000_000_000_000_000u128;
    let account = worker.view_account(project).await?;
    let minimum_balance = NearToken::from_yoctonear(u128::from(account.storage_usage) * byte_cost);
    // Isolated sandbox setup exhausts only the spare native storage reserve.
    // The actual FT transfer, receiver failure and resolver refund execute normally.
    worker
        .patch(project)
        .account_from_current(move |mut account| {
            account.balance = minimum_balance;
            account.into()
        })
        .transact()
        .await?;
    for payer in [other, owner] {
        let used = call(
            payer,
            token,
            "ft_transfer_call",
            json!({"receiver_id": project, "amount": "1", "memo": null, "msg": message}),
            NearToken::from_yoctonear(1),
        )
        .await?;
        ensure!(
            used == "0",
            "unfunded or unauthorized receipt consumed tokens"
        );
        ensure!(view(worker, project, "get_project", json!({})).await? == before);
        ensure!(view(worker, project, "get_bridge", json!({})).await? == bridge_before);
        ensure!(
            view(
                worker,
                project,
                "get_funding",
                json!({"funding_id": funding_id})
            )
            .await?
            .is_null()
        );
        ensure!(
            view(
                worker,
                project,
                "get_funding_by_reference",
                json!({"funding_ref": reference})
            )
            .await?
            .is_null()
        );
    }
    for (account, expected) in [
        (owner.id(), owner_before.clone()),
        (other.id(), other_before),
        (project, project_before.clone()),
    ] {
        ensure!(
            view(
                worker,
                token,
                "ft_balance_of",
                json!({"account_id": account})
            )
            .await?
                == expected
        );
    }
    owner
        .transfer_near(project, NearToken::from_near(1))
        .await?
        .into_result()?;
    ensure!(
        call(
            owner,
            token,
            "ft_transfer_call",
            json!({"receiver_id": project, "amount": "1", "memo": null, "msg": message}),
            NearToken::from_yoctonear(1),
        )
        .await?
            == "1"
    );
    let after = view(worker, project, "get_project", json!({})).await?;
    ensure!(
        after["free"].as_str().unwrap().parse::<u128>()?
            == before["free"].as_str().unwrap().parse::<u128>()? + 1
    );
    ensure!(
        view(
            worker,
            token,
            "ft_balance_of",
            json!({"account_id": project})
        )
        .await?
        .as_str()
        .unwrap()
        .parse::<u128>()?
            == project_before.as_str().unwrap().parse::<u128>()? + 1
    );
    ensure!(
        view(
            worker,
            token,
            "ft_balance_of",
            json!({"account_id": owner.id()})
        )
        .await?
        .as_str()
        .unwrap()
        .parse::<u128>()?
            == owner_before.as_str().unwrap().parse::<u128>()? - 1
    );
    ensure!(
        view(
            worker,
            project,
            "get_funding_by_reference",
            json!({"funding_ref": reference})
        )
        .await?
            == view(
                worker,
                project,
                "get_funding",
                json!({"funding_id": funding_id})
            )
            .await?
    );
    ensure!(
        view(worker, project, "get_bridge", json!({})).await?["next_funding"]
            == bridge_before["next_funding"].as_u64().unwrap() + 1
    );
    Ok(())
}

#[tokio::test]
async fn plain_ft_partial_delivery_dispatch_and_late_refund_are_attributed_once() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;
    let owner = worker.dev_create_account().await?;
    let authority = worker.dev_create_account().await?;
    let sink = root
        .create_subaccount("sink")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;
    let token = worker.dev_deploy(&wasm("test_ft")?).await?;
    call(
        token.as_account(),
        token.id(),
        "new",
        json!({"owner_id": owner.id(), "total_supply": "1000000"}),
        NearToken::from_yoctonear(0),
    )
    .await?;
    let w = root
        .create_subaccount("w")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    w.deploy(&wasm("workspace")?).await?.into_result()?;
    call(
        &w,
        w.id(),
        "new",
        json!({"owner_id": owner.id(), "token_id": token.id()}),
        NearToken::from_yoctonear(0),
    )
    .await?;
    ensure!(
        call(
            &owner,
            w.id(),
            "create_project",
            json!({"authority_id": authority.id()}),
            NearToken::from_near(15)
        )
        .await?
            == true
    );
    let project: AccountId = "p0.w.test.near".parse()?;
    for id in [&project, sink.id()] {
        call(
            &owner,
            token.id(),
            "storage_deposit",
            json!({"account_id": id, "registration_only": true}),
            NearToken::from_millinear(10),
        )
        .await?;
    }
    let v: Value = serde_json::from_str(include_str!(
        "../../project/tests/fixtures/coordinated-signatures.json"
    ))?;
    call(
        &owner,
        &project,
        "bind_coordinator",
        json!({"domain": "11".repeat(32), "key": v["public_key"]}),
        NearToken::from_near(1),
    )
    .await?;
    let funding_message = json!({"kind": "fund", "funding_ref": "ab".repeat(32)}).to_string();
    let fund = call(
        &owner,
        token.id(),
        "ft_transfer_call",
        json!({"receiver_id": project, "amount": "100", "memo": null, "msg": funding_message}),
        NearToken::from_yoctonear(1),
    )
    .await?;
    ensure!(fund == "100");
    let duplicate = call(
        &owner,
        token.id(),
        "ft_transfer_call",
        json!({"receiver_id": project, "amount": "100", "memo": null, "msg": funding_message}),
        NearToken::from_yoctonear(1),
    )
    .await?;
    ensure!(
        duplicate == "0",
        "duplicate owner intent must refund through the FT resolver"
    );
    ensure!(
        view(
            &worker,
            token.id(),
            "ft_balance_of",
            json!({"account_id": owner.id()})
        )
        .await?
            == "999900"
    );
    ensure!(
        view(
            &worker,
            &project,
            "get_funding_by_reference",
            json!({"funding_ref": "ab".repeat(32)})
        )
        .await?
            == view(
                &worker,
                &project,
                "get_funding",
                json!({"funding_id": word(1)})
            )
            .await?
    );
    ensure!(
        view(
            &worker,
            &project,
            "get_funding",
            json!({"funding_id": word(1)})
        )
        .await?
        .as_str()
        .unwrap()
        .len()
            == 320
    );
    execute(&owner, &project, "reserve").await?;
    execute(&owner, &project, "delivery").await?;
    execute(&owner, &project, "refund").await?;
    let delivery: AccountId = view(
        &worker,
        &project,
        "route_receiver",
        json!({"route": "22".repeat(32)}),
    )
    .await?
    .as_str()
    .unwrap()
    .parse()?;
    let refund: AccountId = view(
        &worker,
        &project,
        "route_receiver",
        json!({"route": "44".repeat(32)}),
    )
    .await?
    .as_str()
    .unwrap()
    .parse()?;
    ensure!(
        worker.view_access_keys(&delivery).await?.is_empty(),
        "route escrow has an access key"
    );
    // A registered refund maximum is not evidence of outgoing input. Tokens
    // arriving early must stay in escrow until Dispatch is actually confirmed.
    transfer(&owner, token.id(), &refund, 7).await?;
    ensure!(
        call(
            &owner,
            &refund,
            "collect",
            json!({}),
            NearToken::from_yoctonear(1)
        )
        .await?
            == "0"
    );
    ensure!(view(&worker, &project, "get_project", json!({})).await?["free"] == "40");
    let before_dispatch = view(
        &worker,
        &project,
        "get_route",
        json!({"route": "44".repeat(32)}),
    )
    .await?;
    ensure!(before_dispatch["received"] == "0" && before_dispatch["dispatched"] == "0");
    ensure!(
        view(
            &worker,
            token.id(),
            "ft_balance_of",
            json!({"account_id": refund})
        )
        .await?
            == "7"
    );
    transfer(&owner, token.id(), &delivery, 15).await?;
    ensure!(
        call(
            &owner,
            &delivery,
            "collect",
            json!({}),
            NearToken::from_yoctonear(1)
        )
        .await?
            == "0"
    );
    ensure!(
        view(
            &worker,
            token.id(),
            "ft_balance_of",
            json!({"account_id": delivery})
        )
        .await?
            == "15"
    );
    ensure!(view(&worker, &project, "get_plan", json!({"plan": word(1)})).await?["held"] == "60");
    transfer(&owner, token.id(), &delivery, 25).await?;
    ensure!(
        call(
            &owner,
            &delivery,
            "collect",
            json!({}),
            NearToken::from_yoctonear(1)
        )
        .await?
            == "40"
    );
    ensure!(view(&worker, &project, "get_plan", json!({"plan": word(1)})).await?["held"] == "100");
    execute(&owner, &project, "dispatch").await?;
    execute(&owner, &project, "dispatch").await?;
    ensure!(
        view(
            &worker,
            token.id(),
            "ft_balance_of",
            json!({"account_id": sink.id()})
        )
        .await?
            == "20"
    );
    call(
        &owner,
        &refund,
        "collect",
        json!({}),
        NearToken::from_yoctonear(1),
    )
    .await?;
    execute(&owner, &project, "close_transport").await?;
    ensure!(view(&worker, &project, "get_project", json!({})).await?["free"] == "127");
    transfer(&sink, token.id(), &refund, 13).await?;
    call(
        &owner,
        &refund,
        "collect",
        json!({}),
        NearToken::from_yoctonear(1),
    )
    .await?;
    ensure!(view(&worker, &project, "get_project", json!({})).await?["free"] == "140");
    transfer(&owner, token.id(), &refund, 2).await?;
    ensure!(
        call(
            &owner,
            &refund,
            "collect",
            json!({}),
            NearToken::from_yoctonear(1)
        )
        .await?
            == "0"
    );
    ensure!(view(&worker, &project, "get_project", json!({})).await?["free"] == "140");
    let rejected = call(
        &owner,
        token.id(),
        "ft_transfer_call",
        json!({
            "receiver_id": project,
            "amount": "1",
            "memo": null,
            "msg": json!({
                "kind": "refund",
                "route": "44".repeat(32)
            }).to_string()
        }),
        NearToken::from_yoctonear(1),
    )
    .await?;
    ensure!(
        rejected == "0",
        "owner bypassed route sender authentication"
    );
    funding_storage_recovers_after_native_topup(&worker, &owner, &sink, token.id(), &project)
        .await?;
    Ok(())
}

#[tokio::test]
async fn coordinator_signs_its_own_immutable_command_through_yielded_provider_callback()
-> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;
    let owner = worker.dev_create_account().await?;
    let authority = worker.dev_create_account().await?;
    let provider = worker.dev_create_account().await?;
    let token = worker.dev_deploy(&wasm("test_ft")?).await?;
    call(
        token.as_account(),
        token.id(),
        "new",
        json!({"owner_id": owner.id(), "total_supply": "1000000"}),
        NearToken::from_yoctonear(0),
    )
    .await?;
    let w = root
        .create_subaccount("w")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    w.deploy(&wasm("workspace")?).await?.into_result()?;
    call(
        &w,
        w.id(),
        "new",
        json!({"owner_id": owner.id(), "token_id": token.id()}),
        NearToken::from_yoctonear(0),
    )
    .await?;
    ensure!(
        call(
            &owner,
            w.id(),
            "create_project",
            json!({"authority_id": authority.id()}),
            NearToken::from_near(15)
        )
        .await?
            == true
    );
    let project: AccountId = "p0.w.test.near".parse()?;
    call(
        &owner,
        token.id(),
        "storage_deposit",
        json!({"account_id": project, "registration_only": true}),
        NearToken::from_millinear(10),
    )
    .await?;
    let v: Value = serde_json::from_str(include_str!(
        "../../project/tests/fixtures/coordinated-signatures.json"
    ))?;
    let coordinator = worker.dev_deploy(&wasm("coordinator")?).await?;
    let mpc = worker.dev_deploy(&wasm("mock_mpc")?).await?;
    call(
        coordinator.as_account(),
        coordinator.id(),
        "new",
        json!({
            "owner_id": owner.id(),
            "authority_id": authority.id(),
            "project_id": word(50),
            "mpc_id": mpc.id(),
            "sign_key": v["public_key"],
            "read_key": v["public_key"],
            "sign_path": "test"
        }),
        NearToken::from_yoctonear(0),
    )
    .await?;
    call(
        mpc.as_account(),
        mpc.id(),
        "new",
        json!({
            "coordinator_id": coordinator.id(),
            "responder_id": provider.id(),
            "sign_key": v["public_key"],
            "read_key": v["public_key"],
            "sign_path": "test"
        }),
        NearToken::from_yoctonear(0),
    )
    .await?;
    call(
        &owner,
        coordinator.id(),
        "register_domain",
        json!({
            "domain": "11".repeat(32),
            "chain": "near",
            "vault": project,
            "asset": token.id(),
            "owner_recipient": word(1),
            "owner_identity": word(1),
            "program": null
        }),
        NearToken::from_near(1),
    )
    .await?;
    call(
        &owner,
        &project,
        "bind_coordinator",
        json!({"domain": "11".repeat(32), "key": v["public_key"]}),
        NearToken::from_near(1),
    )
    .await?;
    call(
        &owner,
        token.id(),
        "ft_transfer_call",
        json!({"receiver_id": project, "amount": "100", "memo": null, "msg": "{\"kind\":\"fund\"}"}),
        NearToken::from_yoctonear(1),
    )
    .await?;
    call(
        &owner,
        &project,
        "create_task",
        json!({"amount": "60", "purpose": vec![1;32]}),
        NearToken::from_near(1),
    )
    .await?;
    call(
        &owner,
        &project,
        "checkpoint",
        json!({
            "observation": word(1),
            "task": 0,
            "request": 0,
            "plan": null,
            "route": null,
            "operation": null
        }),
        NearToken::from_near(1),
    )
    .await?;
    call(
        &owner,
        coordinator.id(),
        "observe_near",
        json!({"domain": "11".repeat(32), "observation": word(1)}),
        NearToken::from_near(1),
    )
    .await?;
    call(
        &authority,
        coordinator.id(),
        "approve",
        json!({"domain": "11".repeat(32), "request": word(0)}),
        NearToken::from_near(1),
    )
    .await?;
    call(
        &authority,
        coordinator.id(),
        "apply",
        json!({
            "domain": "11".repeat(32),
            "request": word(0),
            "local_amount": 60,
            "routes": [],
            "expires": 2000000000
        }),
        NearToken::from_near(1),
    )
    .await?;
    let command = view(
        &worker,
        coordinator.id(),
        "get_command",
        json!({"operation": word(1)}),
    )
    .await?;
    ensure!(command["command"] == v["reserve"]["command"]);
    let tx = owner
        .call(coordinator.id(), "request_signature")
        .args_json(json!({"operation": word(1)}))
        .deposit(NearToken::from_near(1))
        .max_gas()
        .transact_async()
        .await?;
    let mut pending = Value::Null;
    for _ in 0..60 {
        pending = view(&worker, mpc.id(), "get_pending", json!({"request_id": 0})).await?;
        if !pending.is_null() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    ensure!(pending["kind"] == "sign");
    ensure!(pending["digest"] == command["digest"]);
    let recovery = v["reserve"]["recovery_id"].as_u64().unwrap();
    let sig = v["reserve"]["signature"].as_str().unwrap();
    let response = json!({
        "scheme": "Secp256k1",
        "big_r": {
            "affine_point": format!("{:02x}{}", 2 + recovery, &sig[..64])
        },
        "s": {"scalar": &sig[64..]},
        "recovery_id": recovery
    });
    ensure!(
        owner
            .call(mpc.id(), "respond")
            .args_json(json!({"request_id": 0, "response": response}))
            .max_gas()
            .transact()
            .await?
            .is_failure(),
        "unauthorized provider response accepted"
    );
    call(
        &provider,
        mpc.id(),
        "respond",
        json!({"request_id": 0, "response": response}),
        NearToken::from_yoctonear(0),
    )
    .await?;
    let mut signed = Value::Null;
    for _ in 0..60 {
        signed = view(
            &worker,
            coordinator.id(),
            "get_command",
            json!({"operation": word(1)}),
        )
        .await?;
        if !signed["signature"].is_null() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    ensure!(
        signed["signature"] == response,
        "signature callback not stored: {signed}; tx {}",
        tx.hash()
    );
    execute(&owner, &project, "reserve").await?;
    ensure!(view(&worker, &project, "get_plan", json!({"plan": word(1)})).await?["held"] == "60");
    ensure!(
        owner
            .call(coordinator.id(), "on_near")
            .args_json(json!({"evidence_id": 0}))
            .max_gas()
            .transact()
            .await?
            .is_failure(),
        "public caller forged private evidence callback"
    );
    Ok(())
}
