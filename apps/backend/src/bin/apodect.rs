//! Operational JSON interface to the same services as the browser API. Read
//! stdin so session tokens and signed proofs do not appear in process arguments.

use std::{
    io::{self, Read},
    process::ExitCode,
};

use anyhow::{Context, Result, ensure};
use apodect_backend::actions::{Actions, ApplyRequest, CoordinatorAction, LocalAction};
use apodect_backend::{application::Application, config::Chain};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Projects,
    Inspect {
        project: String,
    },
    Index {
        project: String,
    },
    Operations {
        project: String,
    },
    RetryOperation {
        project: String,
        operation: String,
        attempt: i32,
    },
    LoginChallenge {
        chain: Chain,
        address: String,
    },
    Login {
        challenge: Uuid,
        signature: String,
        public_key: Option<String>,
    },
    LinkChallenge {
        session: String,
        chain: Chain,
        address: String,
    },
    LinkWallet {
        session: String,
        challenge: Uuid,
        signature: String,
        public_key: Option<String>,
    },
    Profile {
        session: String,
    },
    RenameProfile {
        session: String,
        name: Option<String>,
    },
    Logout {
        session: String,
    },
    TransportStatus {
        project: String,
        route: String,
    },
    FundingProposal {
        project: String,
        destination: Chain,
        request: String,
        slippage_bps: u16,
        expires: u64,
    },
    PrepareApply {
        session: String,
        project: String,
        actor: String,
        request: ApplyRequest,
    },
    PrepareCoordinator {
        session: String,
        project: String,
        actor: String,
        action: CoordinatorAction,
        storage_deposit: String,
    },
    PrepareLocal {
        session: String,
        project: String,
        actor: String,
        chain: Chain,
        action: LocalAction,
        storage_deposit: Option<String>,
        token_account: Option<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(false)
        .with_writer(io::stderr)
        .init();
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!(
            "Read one JSON command from stdin. Commands: projects, inspect, index, operations, retry_operation, transport_status, login_challenge, login, link_challenge, link_wallet, profile, rename_profile, logout, funding_proposal, prepare_apply, prepare_coordinator, prepare_local. See docs/backend.md."
        );
        return ExitCode::SUCCESS;
    }
    match run().await {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("JSON serializes")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<Value> {
    let mut bytes = Vec::new();
    io::stdin().take(1_000_001).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1_000_000, "Input exceeds one megabyte");
    let command: Command = serde_json::from_slice(&bytes).context("Invalid JSON command")?;
    let app = Application::connect().await?;
    let config = &app.indexer.config;
    let project = |key: &str| {
        config
            .projects
            .iter()
            .find(|p| config.project_key(p) == key)
            .context("Unknown Project deployment key")
    };
    let result = match command {
        Command::Projects => json!(
            config
                .projects
                .iter()
                .map(|p| json!({"key":config.project_key(p),"deployment":p}))
                .collect::<Vec<_>>()
        ),
        Command::Inspect { project: key } => {
            serde_json::to_value(app.indexer.index_project(project(&key)?).await?)?
        }
        Command::Index { project: key } => {
            let deployment = project(&key)?;
            json!({"coordinator":app.indexer.index_project(deployment).await?,"vaults":app.indexer.index_local(deployment).await?})
        }
        Command::Operations { project: key } => {
            project(&key)?;
            let values: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id', id, 'chain', chain, 'state', state, 'attempt', attempt, 'intent', intent, 'result', result, 'error', error) FROM effects WHERE project=$1 ORDER BY id")
                .bind(key).fetch_all(&app.indexer.store.pool).await?;
            json!(values)
        }
        Command::RetryOperation {
            project: key,
            operation,
            attempt,
        } => {
            project(&key)?;
            app.indexer
                .store
                .retry_effect(&key, &operation, attempt)
                .await?;
            json!({"operation":operation,"state":"planned","attempt":attempt+1})
        }
        Command::TransportStatus {
            project: key,
            route,
        } => {
            project(&key)?;
            let route = hex::encode(apodect_backend::config::word(&route)?);
            let quote: Value = sqlx::query_scalar(
                "SELECT response FROM funding_quotes WHERE project=$1 AND route=$2",
            )
            .bind(key)
            .bind(route)
            .fetch_one(&app.indexer.store.pool)
            .await?;
            app.liquidity.status(&quote).await?
        }
        Command::LoginChallenge { chain, address } => {
            serde_json::to_value(app.accounts.begin_login(chain, &address).await?)?
        }
        Command::Login {
            challenge,
            signature,
            public_key,
        } => serde_json::to_value(
            app.accounts
                .complete_login(challenge, &signature, public_key.as_deref())
                .await?,
        )?,
        Command::LinkChallenge {
            session,
            chain,
            address,
        } => serde_json::to_value(app.accounts.begin_link(&session, chain, &address).await?)?,
        Command::LinkWallet {
            session,
            challenge,
            signature,
            public_key,
        } => serde_json::to_value(
            app.accounts
                .complete_link(&session, challenge, &signature, public_key.as_deref())
                .await?,
        )?,
        Command::Profile { session } => {
            json!({"session":app.accounts.session(&session).await?,"wallets":app.accounts.wallets(&session).await?})
        }
        Command::RenameProfile { session, name } => serde_json::to_value(
            app.accounts
                .update_display_name(&session, name.as_deref())
                .await?,
        )?,
        Command::Logout { session } => {
            app.accounts.revoke_session(&session).await?;
            json!({"revoked":true})
        }
        Command::FundingProposal {
            project: key,
            destination,
            request,
            slippage_bps,
            expires,
        } => serde_json::to_value(
            app.liquidity
                .propose_apply(
                    &app.indexer,
                    project(&key)?,
                    destination,
                    &request,
                    slippage_bps,
                    expires,
                )
                .await?,
        )?,
        Command::PrepareApply {
            session,
            project,
            actor,
            request,
        } => serde_json::to_value(
            Actions::new(&app.accounts, &app.indexer)
                .prepare_apply(&session, &project, &actor, request, &app.liquidity)
                .await?,
        )?,
        Command::PrepareCoordinator {
            session,
            project,
            actor,
            action,
            storage_deposit,
        } => serde_json::to_value(
            Actions::new(&app.accounts, &app.indexer)
                .prepare_coordinator(&session, &project, &actor, action, storage_deposit.parse()?)
                .await?,
        )?,
        Command::PrepareLocal {
            session,
            project,
            actor,
            chain,
            action,
            storage_deposit,
            token_account,
        } => {
            let actions = Actions::new(&app.accounts, &app.indexer);
            let intent = match chain {
                Chain::Ethereum => {
                    ensure!(
                        storage_deposit.is_none() && token_account.is_none(),
                        "Ethereum does not use these chain-specific arguments"
                    );
                    actions
                        .prepare_ethereum(&session, &project, &actor, action)
                        .await?
                }
                Chain::Solana => {
                    ensure!(
                        storage_deposit.is_none(),
                        "Solana does not use a NEAR storage deposit"
                    );
                    actions
                        .prepare_solana(
                            &session,
                            &project,
                            &actor,
                            action,
                            token_account.as_deref(),
                        )
                        .await?
                }
                Chain::Near => {
                    ensure!(
                        token_account.is_none(),
                        "NEAR does not use a Solana token account"
                    );
                    actions
                        .prepare_near(
                            &session,
                            &project,
                            &actor,
                            action,
                            storage_deposit
                                .context("NEAR storage_deposit is required")?
                                .parse()?,
                        )
                        .await?
                }
            };
            serde_json::to_value(intent)?
        }
    };
    app.indexer.store.pool.close().await;
    Ok(result)
}
