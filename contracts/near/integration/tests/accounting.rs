mod support;

use anyhow::{Result, ensure};
use near_workspaces::types::NearToken;
use serde_json::json;
use support::{Fixture, fails, fails_without_deposit, number, ok, reference, tx};

#[tokio::test]
async fn actual_ft_accounting_completion_roles_and_refund_rejection() -> Result<()> {
    let f = Fixture::new().await?;
    f.fund(100).await?;
    let task = f.create(&f.agent, 60).await?;
    fails(
        &f.owner,
        &f.project,
        "approve_request",
        json!({"request_id": 0}),
    )
    .await?;
    fails(
        &f.other,
        &f.project,
        "request_budget",
        json!({
            "task_id": task,
            "amount": "1",
            "purpose": reference(1)
        }),
    )
    .await?;
    f.approve_task(task).await?;
    ensure!(f.amount("free").await? == 100);
    f.apply().await?;
    ensure!(f.amount("free").await? == 40);
    fails(
        &f.other,
        &f.project,
        "withdraw",
        json!({
            "task_id": task,
            "amount": "1"
        }),
    )
    .await?;
    let withdrawal = tx(
        &f.agent,
        &f.project,
        "withdraw",
        json!({
            "task_id": task,
            "amount": "40"
        }),
    )
    .await?
    .into_result()?;
    ensure!(withdrawal.json::<bool>()?);
    ensure!(
        withdrawal
            .logs()
            .iter()
            .any(|log| log.contains("outflow_accepted") && log.contains("operation_id"))
    );
    ensure!(f.balance(f.agent.id()).await? == 40);
    ensure!(f.balance(&f.project).await? == 60);
    fails(&f.owner, &f.project, "cancel", json!({"task_id": task})).await?;

    // Give the agent unrelated tokens so an excessive return reaches the Project receiver.
    f.owner
        .call(f.token.id(), "ft_transfer")
        .args_json(json!({
            "receiver_id": f.agent.id(),
            "amount": "2",
            "memo": null
        }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;
    ensure!(
        f.deposit(
            &f.agent,
            &f.project,
            41,
            json!({
                "kind": "return",
                "task_id": task
            })
        )
        .await?
            == 0
    );
    ensure!(
        f.deposit(
            &f.agent,
            &f.project,
            15,
            json!({
                "kind": "return",
                "task_id": task
            })
        )
        .await?
            == 15
    );
    let state = f.view("get_task", json!({"task_id": task})).await?;
    ensure!(number(&state, "available") == 20 && number(&state, "external_outstanding") == 25);
    fails(
        &f.agent,
        &f.project,
        "finalize",
        json!({
            "task_id": task,
            "report": null
        }),
    )
    .await?;
    ok(
        &f.agent,
        &f.project,
        "release",
        json!({
            "task_id": task,
            "amount": "20"
        }),
    )
    .await?;
    let expense = ok(
        &f.agent,
        &f.project,
        "finalize",
        json!({
            "task_id": task,
            "report": reference(2)
        }),
    )
    .await?;
    ensure!(expense == "25" && f.amount("free").await? == 75 && f.balance(&f.project).await? == 75);
    fails(
        &f.agent,
        &f.project,
        "finalize",
        json!({
            "task_id": task,
            "report": null
        }),
    )
    .await?;
    ensure!(
        f.deposit(
            &f.agent,
            &f.project,
            1,
            json!({
                "kind": "return",
                "task_id": task
            })
        )
        .await?
            == 0
    );
    fails_without_deposit(
        &f.other,
        &f.project,
        "ft_on_transfer",
        json!({
            "sender_id": f.owner.id(),
            "amount": "100",
            "msg": "{\"kind\":\"fund\"}"
        }),
        "WRONG_TOKEN",
    )
    .await?;

    // Unattributed plain token transfers never create ledger funding.
    f.owner
        .call(f.token.id(), "ft_transfer")
        .args_json(json!({
            "receiver_id": f.project,
            "amount": "7",
            "memo": null
        }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;
    ensure!(f.amount("free").await? == 75 && f.balance(&f.project).await? == 82);
    ok(
        &f.owner,
        &f.project,
        "withdraw_free",
        json!({"amount": "75"}),
    )
    .await?;
    ensure!(f.amount("free").await? == 0 && f.balance(&f.project).await? == 7);
    Ok(())
}

#[tokio::test]
async fn input_boundaries_initial_replacement_cancellation_and_live_completion() -> Result<()> {
    let f = Fixture::new().await?;
    f.fund(100).await?;
    fails(
        &f.agent,
        &f.project,
        "create_task",
        json!({
            "amount": "0",
            "purpose": reference(1)
        }),
    )
    .await?;
    fails(
        &f.agent,
        &f.project,
        "create_task",
        json!({
            "amount": "1",
            "purpose": reference(0)
        }),
    )
    .await?;
    fails(
        &f.agent,
        &f.project,
        "create_task",
        json!({
            "amount": "340282366920938463463374607431768211456",
            "purpose": reference(1)
        }),
    )
    .await?;
    ensure!(f.view("get_project", json!({})).await?["next_task"] == 0);
    let first = f.create(&f.agent, 10).await?;
    fails(
        &f.agent,
        &f.project,
        "request_budget",
        json!({
            "task_id": first,
            "amount": "1",
            "purpose": reference(1)
        }),
    )
    .await?;
    fails(
        &f.agent,
        &f.project,
        "finalize",
        json!({
            "task_id": first,
            "report": null
        }),
    )
    .await?;
    ok(
        &f.authority,
        &f.project,
        "reject_request",
        json!({"request_id": 0}),
    )
    .await?;
    let replacement = ok(
        &f.agent,
        &f.project,
        "request_budget",
        json!({
            "task_id": first,
            "amount": "10",
            "purpose": reference(2)
        }),
    )
    .await?;
    ok(
        &f.authority,
        &f.project,
        "approve_request",
        json!({"request_id": replacement}),
    )
    .await?;
    f.apply().await?;
    ok(&f.owner, &f.project, "cancel", json!({"task_id": first})).await?;
    ensure!(f.amount("free").await? == 100);
    ensure!(f.view("get_task", json!({"task_id": first})).await?["state"] == "Canceled");
    fails(
        &f.agent,
        &f.project,
        "withdraw",
        json!({
            "task_id": first,
            "amount": "1"
        }),
    )
    .await?;

    let task = f.create(&f.agent, 20).await?;
    f.approve_task(task).await?;
    f.apply().await?;

    // A funded-but-unused task cannot obtain another initial request.
    fails(
        &f.agent,
        &f.project,
        "request_budget",
        json!({
            "task_id": task,
            "amount": "1",
            "purpose": reference(1)
        }),
    )
    .await?;
    ok(
        &f.agent,
        &f.project,
        "withdraw",
        json!({
            "task_id": task,
            "amount": "5"
        }),
    )
    .await?;
    let supplement = ok(
        &f.agent,
        &f.project,
        "request_budget",
        json!({
            "task_id": task,
            "amount": "5",
            "purpose": reference(3)
        }),
    )
    .await?;
    ok(
        &f.authority,
        &f.project,
        "approve_request",
        json!({"request_id": supplement}),
    )
    .await?;
    ok(
        &f.owner,
        &f.project,
        "set_authority",
        json!({"authority_id": f.other.id()}),
    )
    .await?;
    fails(&f.authority, &f.project, "apply", json!({})).await?;
    ok(&f.other, &f.project, "apply", json!({})).await?;
    let state = f.view("get_task", json!({"task_id": task})).await?;
    ensure!(
        number(&state, "issued") == 25
            && number(&state, "available") == 20
            && state["started"] == true
    );
    let queued = ok(
        &f.agent,
        &f.project,
        "request_budget",
        json!({
            "task_id": task,
            "amount": "5",
            "purpose": reference(4)
        }),
    )
    .await?;
    ok(
        &f.other,
        &f.project,
        "approve_request",
        json!({"request_id": queued}),
    )
    .await?;
    ok(
        &f.agent,
        &f.project,
        "release",
        json!({
            "task_id": task,
            "amount": "20"
        }),
    )
    .await?;
    let expense = ok(
        &f.agent,
        &f.project,
        "finalize",
        json!({
            "task_id": task,
            "report": null
        }),
    )
    .await?;
    ensure!(expense == "5");
    ensure!(f.view("get_request", json!({"request_id": queued})).await?["state"] == "Removed");
    ensure!(
        ok(&f.other, &f.project, "apply", json!({}))
            .await?
            .get("Skipped")
            .is_some()
    );
    ensure!(
        number(
            &f.view("get_task", json!({"task_id": task})).await?,
            "issued"
        ) == 25
    );
    ensure!(f.amount("free").await? == 95 && f.balance(&f.project).await? == 95);
    fails(
        &f.owner,
        &f.project,
        "withdraw_free",
        json!({"amount": "96"}),
    )
    .await?;
    ensure!(f.amount("free").await? == 95);
    Ok(())
}
