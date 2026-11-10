mod support;

use anyhow::{Result, ensure};
use near_workspaces::operations::Function;
use near_workspaces::types::{Gas, NearToken};
use serde_json::json;
use support::{Fixture, fails, fails_without_deposit, number, ok, reference};

#[tokio::test]
async fn fifo_tombstones_stops_restoration_and_invoice_rotation() -> Result<()> {
    let f = Fixture::new().await?;
    f.fund(30).await?;
    let a = f.create(&f.agent, 60).await?;
    let b = f.create(&f.other, 10).await?;
    f.approve_task(a).await?;
    f.approve_task(b).await?;
    ensure!(f.apply().await?.get("Waiting").is_some());
    ensure!(number(&f.view("get_task", json!({"task_id": b})).await?, "issued") == 0);
    ok(
        &f.authority,
        &f.project,
        "mark_out_of_budget",
        json!({"task_id": a}),
    )
    .await?;
    ensure!(f.apply().await?.get("Skipped").is_some());
    f.apply().await?;
    fails(
        &f.agent,
        &f.project,
        "request_budget",
        json!({
            "task_id": a,
            "amount": "1",
            "purpose": reference(1)
        }),
    )
    .await?;

    // Retained reserve Continue has no issuance, while reclaim/restoration adds balanced counters.
    ok(&f.owner, &f.project, "force_cancel", json!({"task_id": b})).await?;
    fails(
        &f.other,
        &f.project,
        "withdraw",
        json!({
            "task_id": b,
            "amount": "1"
        }),
    )
    .await?;
    ok(&f.owner, &f.project, "continue_task", json!({"task_id": b})).await?;
    ensure!(number(&f.view("get_task", json!({"task_id": b})).await?, "issued") == 10);
    ok(&f.owner, &f.project, "force_cancel", json!({"task_id": b})).await?;
    ok(
        &f.owner,
        &f.project,
        "reclaim",
        json!({
            "task_id": b,
            "amount": "5"
        }),
    )
    .await?;
    ok(
        &f.authority,
        &f.project,
        "restore",
        json!({
            "task_id": b,
            "amount": "5",
            "purpose": reference(3)
        }),
    )
    .await?;
    f.apply().await?;
    ensure!(f.view("get_task", json!({"task_id": b})).await?["paused"] == true);
    ok(&f.owner, &f.project, "continue_task", json!({"task_id": b})).await?;

    // Register second agent before actual outgoing transfer.
    f.register(f.other.id()).await?;
    ok(
        &f.other,
        &f.project,
        "withdraw",
        json!({
            "task_id": b,
            "amount": "4"
        }),
    )
    .await?;
    let request = ok(
        &f.other,
        &f.project,
        "request_budget",
        json!({
            "task_id": b,
            "amount": "2",
            "purpose": reference(1)
        }),
    )
    .await?;
    ok(
        &f.authority,
        &f.project,
        "reject_request",
        json!({"request_id": request}),
    )
    .await?;
    fails(
        &f.other,
        &f.project,
        "withdraw",
        json!({
            "task_id": b,
            "amount": "1"
        }),
    )
    .await?;
    ok(&f.owner, &f.project, "force_cancel", json!({"task_id": b})).await?;
    fails(&f.owner, &f.project, "continue_task", json!({"task_id": b})).await?;
    let resume = ok(
        &f.other,
        &f.project,
        "request_budget",
        json!({
            "task_id": b,
            "amount": "2",
            "purpose": reference(1)
        }),
    )
    .await?;
    ok(
        &f.authority,
        &f.project,
        "approve_request",
        json!({"request_id": resume}),
    )
    .await?;
    f.apply().await?;
    let state = f.view("get_task", json!({"task_id": b})).await?;
    ensure!(state["state"] == "Active" && state["paused"] == true);
    ensure!(
        f.deposit(
            &f.other,
            &f.project,
            1,
            json!({
                "kind": "return",
                "task_id": b
            })
        )
        .await?
            == 1
    );
    ok(
        &f.other,
        &f.project,
        "release",
        json!({
            "task_id": b,
            "amount": "8"
        }),
    )
    .await?;
    ok(
        &f.other,
        &f.project,
        "finalize",
        json!({
            "task_id": b,
            "report": null
        }),
    )
    .await?;
    let invoice = ok(
        &f.authority,
        &f.project,
        "create_invoice",
        json!({
            "amount": "100",
            "reference": null
        }),
    )
    .await?;
    let previous_free = f.amount("free").await?;
    ok(
        &f.owner,
        &f.project,
        "set_authority",
        json!({"authority_id": f.agent.id()}),
    )
    .await?;
    fails(&f.authority, &f.project, "apply", json!({})).await?;
    ok(
        &f.owner,
        &f.project,
        "respond_invoice",
        json!({
            "invoice_id": invoice,
            "intended": "0",
            "reference": reference(4)
        }),
    )
    .await?;
    ensure!(f.amount("free").await? == previous_free);
    ensure!(
        f.deposit(
            &f.owner,
            &f.project,
            7,
            json!({
                "kind": "fund",
                "invoice_id": invoice
            })
        )
        .await?
            == 7
    );
    let invoice = f
        .view("get_invoice", json!({"invoice_id": invoice}))
        .await?;
    ensure!(invoice["closed"] == true && number(&invoice, "actual_funding") == 7);
    Ok(())
}

#[tokio::test]
async fn failed_and_concurrent_outflows_preserve_pause_and_callbacks_are_private() -> Result<()> {
    let f = Fixture::new().await?;
    f.fund(100).await?;

    // Unregistered recipient makes the actual FT receipt fail; callback restores once.
    let failed = f.create(&f.other, 10).await?;
    f.approve_task(failed).await?;
    f.apply().await?;
    let result = ok(
        &f.other,
        &f.project,
        "withdraw",
        json!({
            "task_id": failed,
            "amount": "10"
        }),
    )
    .await?;
    ensure!(result == false);
    let state = f.view("get_task", json!({"task_id": failed})).await?;
    ensure!(
        number(&state, "available") == 10
            && number(&state, "pending_outflow") == 0
            && state["started"] == true
    );
    fails(&f.owner, &f.project, "cancel", json!({"task_id": failed})).await?;
    fails_without_deposit(
        &f.other,
        &f.project,
        "settle_outflow",
        json!({"operation_id": 0}),
        "private",
    )
    .await?;
    ensure!(
        number(
            &f.view("get_task", json!({"task_id": failed})).await?,
            "available"
        ) == 10
    );

    // One atomic local receipt admits two separate transfers and pauses before either FT callback.
    let task = f.create(&f.owner, 60).await?;
    f.approve_task(task).await?;
    f.apply().await?;

    // Neither completion nor Continue may consume an unresolved transfer. Each failed
    // action batch also rolls back the earlier admission and its scheduled FT receipt.
    let completion = f
        .owner
        .batch(&f.project)
        .call(
            Function::new("withdraw")
                .args_json(json!({"task_id": task, "amount": "60"}))
                .deposit(NearToken::from_near(1))
                .gas(Gas::from_tgas(110)),
        )
        .call(
            Function::new("finalize")
                .args_json(json!({"task_id": task, "report": null}))
                .gas(Gas::from_tgas(30)),
        )
        .transact()
        .await?;
    ensure!(completion.is_failure());
    ensure!(format!("{completion:?}").contains("TASK_FUNDS_NOT_SETTLED"));

    let continuation = f
        .owner
        .batch(&f.project)
        .call(
            Function::new("withdraw")
                .args_json(json!({"task_id": task, "amount": "20"}))
                .deposit(NearToken::from_near(1))
                .gas(Gas::from_tgas(110)),
        )
        .call(
            Function::new("force_cancel")
                .args_json(json!({"task_id": task}))
                .gas(Gas::from_tgas(30)),
        )
        .call(
            Function::new("continue_task")
                .args_json(json!({"task_id": task}))
                .gas(Gas::from_tgas(30)),
        )
        .transact()
        .await?;
    ensure!(continuation.is_failure());
    ensure!(format!("{continuation:?}").contains("CONFIRMED_RESERVE_REQUIRED"));
    let unchanged = f.view("get_task", json!({"task_id": task})).await?;
    ensure!(unchanged["started"] == false && unchanged["paused"] == false);
    ensure!(number(&unchanged, "available") == 60 && number(&unchanged, "pending_outflow") == 0);
    ensure!(
        f.view("get_operation", json!({"operation_id": 1}))
            .await?
            .is_null()
    );
    ensure!(f.balance(&f.project).await? == 100);

    let result = f
        .owner
        .batch(&f.project)
        .call(
            Function::new("withdraw")
                .args_json(json!({
                    "task_id": task,
                    "amount": "20"
                }))
                .deposit(NearToken::from_near(1))
                .gas(Gas::from_tgas(110)),
        )
        .call(
            Function::new("withdraw")
                .args_json(json!({
                    "task_id": task,
                    "amount": "15"
                }))
                .deposit(NearToken::from_near(1))
                .gas(Gas::from_tgas(110)),
        )
        .call(
            Function::new("force_cancel")
                .args_json(json!({"task_id": task}))
                .gas(Gas::from_tgas(30)),
        )
        .transact()
        .await?
        .into_result()?;
    ensure!(
        result
            .logs()
            .iter()
            .filter(|line| line.contains("outflow_settled"))
            .count()
            == 2
    );
    let state = f.view("get_task", json!({"task_id": task})).await?;
    ensure!(
        state["paused"] == true
            && state["started"] == true
            && number(&state, "available") == 25
            && number(&state, "pending_outflow") == 0
            && number(&state, "external_outstanding") == 35
    );
    ensure!(f.view("get_operation", json!({"operation_id": 1})).await?["state"] == "Succeeded");
    ensure!(f.view("get_operation", json!({"operation_id": 2})).await?["state"] == "Succeeded");
    ensure!(f.balance(&f.project).await? == 65);
    fails(
        &f.owner,
        &f.project,
        "withdraw",
        json!({
            "task_id": task,
            "amount": "1"
        }),
    )
    .await?;
    ok(
        &f.owner,
        &f.project,
        "reclaim",
        json!({
            "task_id": task,
            "amount": "25"
        }),
    )
    .await?;
    ensure!(f.amount("free").await? == 55);
    Ok(())
}
