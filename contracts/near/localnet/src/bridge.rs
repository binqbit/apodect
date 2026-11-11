//! JSON bridge adapter for the local coordinator and provider runner.
use crate::{
    INITIAL_FUNDING, Localnet, account_id, genesis_identity, read_json, transactions, wasm,
    write_json,
};
use anyhow::{Context, Result, bail, ensure};
use near_workspaces::{
    AccountId,
    types::{Finality, NearToken},
};
use serde_json::{Value, json};
use std::fs;

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("missing string {key}"))
}

fn yocto(value: Option<&str>, default: u128) -> Result<NearToken> {
    Ok(NearToken::from_yoctonear(
        value.map(str::parse).transpose()?.unwrap_or(default),
    ))
}

fn funding_receipt(payload: &Value, project: &AccountId, reference: &Value) -> Result<Value> {
    if payload.is_null() {
        return Ok(Value::Null);
    }
    let payload = payload.as_str().context("invalid funding receipt")?;
    ensure!(
        payload.len() == 320 && payload.is_ascii(),
        "invalid funding receipt length"
    );
    let amount = u128::from_str_radix(&payload[192..256], 16)?;
    Ok(json!({
        "payload": payload,
        "funding_id": &payload[64..128],
        "project": project,
        "amount": amount.to_string(),
        "funding_ref": reference
    }))
}

fn validate_funding_receipt(receipt: &Value, input: &Value) -> Result<()> {
    let amount = input["amount"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| input["amount"].to_string());
    let invoice = input["invoice_id"].as_str().unwrap_or("");
    let invoice = if invoice.is_empty() {
        "0".repeat(64)
    } else {
        invoice
            .strip_prefix("0x")
            .unwrap_or(invoice)
            .to_ascii_lowercase()
    };
    ensure!(
        receipt["amount"] == amount && &receipt["payload"].as_str().unwrap()[128..192] == invoice,
        "funding reference terms changed"
    );
    Ok(())
}

impl Localnet {
    pub(super) async fn bridge(&self, input: Value) -> Result<Value> {
        let action = string(&input, "action")?;
        if action == "bootstrap" {
            return self.bridge_bootstrap(&input).await;
        }
        if action == "view" {
            return self
                .view(
                    &string(&input, "contract")?.parse()?,
                    string(&input, "method")?,
                    input["args"].clone(),
                )
                .await;
        }
        if action == "call" {
            let role = string(&input, "role")?;
            ensure!(
                ["owner", "authority", "agent", "provider", "lp"].contains(&role),
                "unsupported role"
            );
            let actor = self.account(role)?;
            let contract: AccountId = string(&input, "contract")?.parse()?;
            let method = string(&input, "method")?;
            let args = input["args"].clone();
            let deposit = yocto(input["deposit"].as_str(), 0)?;
            if input["wait"] == false {
                let hash = self
                    .submit(
                        &actor,
                        &contract,
                        vec![transactions::function_call(method, args, deposit)?],
                    )
                    .await?;
                return Ok(json!({
                    "tx_hash": hash,
                    "signer_id": actor.id(),
                    "pending": true
                }));
            }
            return self.call(&actor, &contract, method, args, deposit).await;
        }
        let manifest = read_json(&self.data.join("bridge.json"))?;
        let project: AccountId = string(&manifest, "project")?.parse()?;
        match action {
            "fund" | "read_funding" => {
                if action == "read_funding" || input["funding_ref"].is_string() {
                    let reference = string(&input, "funding_ref")?;
                    let prior = self
                        .view(
                            &project,
                            "get_funding_by_reference",
                            json!({"funding_ref": reference}),
                        )
                        .await?;
                    if action == "read_funding" || !prior.is_null() {
                        let receipt = funding_receipt(&prior, &project, &input["funding_ref"])?;
                        if action == "fund" {
                            validate_funding_receipt(&receipt, &input)?;
                        }
                        return Ok(receipt);
                    }
                }
                let amount = input["amount"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| input["amount"].to_string());
                let parsed: u64 = amount.parse().context("invalid funding amount")?;
                ensure!(parsed > 0, "funding amount must be positive");
                let token: AccountId = string(&manifest, "asset")?.parse()?;
                let message = json!({
                    "kind": "fund",
                    "global_invoice": input["invoice_id"],
                    "funding_ref": input["funding_ref"]
                });
                let owner = self.account("owner")?;
                let outcome = self
                    .call_outcome(
                        &owner,
                        &token,
                        "ft_transfer_call",
                        json!({
                            "receiver_id": project,
                            "amount": amount,
                            "memo": null,
                            "msg": message.to_string()
                        }),
                        NearToken::from_yoctonear(1),
                    )
                    .await?;
                let used = transactions::value(&outcome)?
                    .as_str()
                    .context("invalid FT result")?
                    .to_owned();
                if used == "0" && input["funding_ref"].is_string() {
                    let prior = self
                        .view(
                            &project,
                            "get_funding_by_reference",
                            json!({"funding_ref": input["funding_ref"]}),
                        )
                        .await?;
                    ensure!(!prior.is_null(), "Project rejected owner funding");
                    let receipt = funding_receipt(&prior, &project, &input["funding_ref"])?;
                    validate_funding_receipt(&receipt, &input)?;
                    return Ok(receipt);
                }
                ensure!(used == amount, "Project did not accept owner funding");
                let receipt = std::iter::once(&outcome.transaction_outcome)
                    .chain(outcome.receipts_outcome.iter())
                    .flat_map(|receipt| receipt.outcome.logs.iter())
                    .filter_map(|line| {
                        let raw = line.strip_prefix("EVENT_JSON:")?;
                        let event: Value = serde_json::from_str(raw).ok()?;
                        (event["standard"] == "apodect" && event["event"] == "funding_recorded")
                            .then(|| event["data"][0].clone())
                    })
                    .next()
                    .context("actual funding transaction emitted no immutable receipt")?;
                Ok(json!({
                    "payload": receipt["payload"],
                    "funding_id": receipt["funding_id"],
                    "project": project,
                    "amount": amount,
                    "funding_ref": input["funding_ref"]
                }))
            }
            "bind" => {
                self.call(
                    &self.account("owner")?,
                    &project,
                    "bind_coordinator",
                    json!({
                        "domain": input["domain"],
                        "key": manifest["sign_key"]
                    }),
                    NearToken::from_near(1),
                )
                .await
            }

            "route_address" => {
                self.view(&project, "route_receiver", json!({"route": input["route"]}))
                    .await
            }
            "execute" => {
                self.call(
                    &self.account("owner")?,
                    &project,
                    "execute_command",
                    json!({
                        "command": input["command"],
                        "signature": input["signature"],
                        "recovery_id": input["recovery_id"],
                        "recipient_id": input["recipient_id"]
                    }),
                    NearToken::from_near(5),
                )
                .await
            }

            "checkpoint" => {
                let payload = self
                    .call(
                        &self.account("owner")?,
                        &project,
                        "checkpoint",
                        json!({
                            "observation": input["observation"],
                            "task": input["task"],
                            "request": input["request"],
                            "plan": input["plan"],
                            "route": input["route"],
                            "operation": input["operation"]
                        }),
                        NearToken::from_near(1),
                    )
                    .await?;
                Ok(json!({
                    "payload": payload,
                    "observation": input["observation"],
                    "project": project
                }))
            }
            "route_deposit" => {
                let receiver = self
                    .view(&project, "route_receiver", json!({"route": input["route"]}))
                    .await?;
                let token: AccountId = string(&manifest, "asset")?.parse()?;
                let actor = self.account("lp")?;
                let amount = input["amount"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| input["amount"].to_string());
                let outcome = self
                    .call_outcome(
                        &actor,
                        &token,
                        "ft_transfer",
                        json!({
                            "receiver_id": receiver,
                            "amount": amount,
                            "memo": null
                        }),
                        NearToken::from_yoctonear(1),
                    )
                    .await?;
                transactions::value(&outcome)?;
                // NEP-141 transfer returns no value. Report the finalized native
                // transaction so a successful provider effect is never JSON null.
                Ok(json!({
                    "tx_hash": outcome.transaction.hash,
                    "signer_id": outcome.transaction.signer_id,
                    "finality": "FINAL",
                    "asset": token,
                    "receiver_id": receiver,
                    "amount": amount,
                }))
            }
            "collect" => {
                let receiver = self
                    .view(&project, "route_receiver", json!({"route": input["route"]}))
                    .await?;
                self.call(
                    &self.account("owner")?,
                    &receiver
                        .as_str()
                        .context("route receiver missing")?
                        .parse()?,
                    "collect",
                    json!({}),
                    NearToken::from_yoctonear(1),
                )
                .await
            }
            "agent" => {
                self.call(
                    &self.account("agent")?,
                    &project,
                    string(&input, "method")?,
                    input["args"].clone(),
                    yocto(
                        input["deposit"].as_str(),
                        NearToken::from_near(1).as_yoctonear(),
                    )?,
                )
                .await
            }

            _ => bail!("unsupported NEAR bridge action {action}"),
        }
    }

    async fn bridge_bootstrap(&self, input: &Value) -> Result<Value> {
        let path = self.data.join("bridge.json");
        let manifest = read_json(&self.data.join("manifest.json"))?;
        let project: AccountId = string(&manifest["contracts"], "project")?.parse()?;
        if path.exists() {
            let old = read_json(&path)?;
            ensure!(
                old["project"] == project.as_str(),
                "bridge Project differs from native deployment"
            );
            for key in ["sign_key", "read_key", "project_id"] {
                ensure!(
                    old[key] == input[key],
                    "existing bridge {key} differs; isolated reset required"
                );
            }
            for (key, artifact) in [
                ("coordinator_id", "coordinator"),
                ("mpc_id", "mock_mpc"),
                ("project", "project"),
            ] {
                ensure!(
                    self.worker
                        .view_code(&string(&old, key)?.parse()?)
                        .finality(Finality::Final)
                        .await?
                        == wasm(artifact)?,
                    "existing bridge code differs; isolated reset required"
                );
            }
            return Ok(old);
        }
        ensure!(
            !self.data.join("bridge-bootstrap.pending").exists(),
            "partial bridge bootstrap exists; inspect or reset isolated environment"
        );
        for key in ["sign_key", "read_key"] {
            let value = string(input, key)?.trim_start_matches("0x");
            ensure!(
                value.len() == 128 && value.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid {key}"
            );
        }
        ensure!(
            string(input, "project_id")?.trim_start_matches("0x").len() == 64,
            "invalid project_id"
        );
        let state = self.view(&project, "get_project", json!({})).await?;
        ensure!(
            state["next_task"] == 0
                && state["next_request"] == 0
                && state["next_invoice"] == 0
                && state["next_operation"] == 0
                && state["pending_owner_outflow"] == "0",
            "local Project was already used; refusing to bind existing activity"
        );
        fs::write(self.data.join("bridge-bootstrap.pending"), b"in-progress\n")?;
        let owner = self.account("owner")?;
        let authority = self.account("authority")?;
        let agent = self.account("agent")?;
        let provider = self.ensure_account("provider").await?;
        let lp = self.ensure_account("lp").await?;
        let coordinator = self.ensure_account("coordinator").await?;
        let mpc = self.ensure_account("mpc").await?;
        let sign_path = "apodect/project/v1";
        self.deploy(
            &coordinator,
            "coordinator",
            json!({
                "owner_id": owner.id(),
                "authority_id": authority.id(),
                "project_id": input["project_id"],
                "mpc_id": mpc.id(),
                "sign_key": input["sign_key"],
                "read_key": input["read_key"],
                "sign_path": sign_path
            }),
        )
        .await?;
        self.deploy(
            &mpc,
            "mock_mpc",
            json!({
                "coordinator_id": coordinator.id(),
                "responder_id": provider.id(),
                "sign_key": input["sign_key"],
                "read_key": input["read_key"],
                "sign_path": sign_path
            }),
        )
        .await?;
        let token = account_id("token")?;
        for id in [lp.id(), provider.id()] {
            self.call(
                &owner,
                &token,
                "storage_deposit",
                json!({
                    "account_id": id,
                    "registration_only": true
                }),
                NearToken::from_millinear(10),
            )
            .await?;
        }
        self.call(
            &owner,
            &token,
            "ft_transfer",
            json!({
                "receiver_id": lp.id(),
                "amount": "100000000000",
                "memo": "finite local provider inventory"
            }),
            NearToken::from_yoctonear(1),
        )
        .await?;
        let mut accounts = serde_json::Map::new();
        for role in ["owner", "authority", "agent", "provider", "lp"] {
            accounts.insert(
                role.into(),
                json!({
                    "address": account_id(role)?,
                    "key_file": self.key_file(role)
                }),
            );
        }
        let result = json!({
            "schema_version": 1,
            "chain": "near",
            "rpc_url": self.rpc,
            "coordinator_id": coordinator.id(),
            "mpc_id": mpc.id(),
            "project": project,
            "vault": project,
            "asset": token,
            "owner": owner.id(),
            "authority": authority.id(),
            "agent": agent.id(),
            "provider": provider.id(),
            "lp": lp.id(),
            "accounts": accounts,
            "genesis": genesis_identity(&self.data)?,
            "project_id": input["project_id"],
            "sign_key": input["sign_key"],
            "read_key": input["read_key"],
            "sign_path": sign_path,
            "initial_funding": INITIAL_FUNDING,
            "lp_inventory": 100000000000u64
        });
        write_json(&path, &result)?;
        fs::remove_file(self.data.join("bridge-bootstrap.pending"))?;
        Ok(result)
    }
}
