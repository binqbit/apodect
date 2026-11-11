use std::{
    io,
    process::ExitCode,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use apodect_backend::{
    accounts::Accounts,
    config::{Config, ProjectConfig},
    database,
    events::Events,
    indexing::Indexer,
    relay::Relay,
    storage::Store,
};
use tokio::signal::unix::{SignalKind, signal};

const HEALTH_FILE: &str = "/tmp/apodect-worker-health";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(false)
        .with_writer(io::stderr)
        .init();
    let result = if std::env::args().nth(1).as_deref() == Some("--health-check") {
        health_check()
    } else {
        run().await
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "Worker stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    if std::fs::exists(HEALTH_FILE)? {
        std::fs::remove_file(HEALTH_FILE)?;
    }
    let config = Config::from_env()?;
    apodect_backend::deployment::verify_public(&config).await?;
    let projects = config.projects.clone();
    let pool = database::connect().await.map_err(anyhow::Error::msg)?;
    let store = Store::new(pool.clone());
    store.migrate().await?;
    let indexer = Indexer::connect(config, store).await?;
    let mut relay = Relay::connect(indexer).await?;
    let events = Events::from_env().await?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let shutdown = async move {
        tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
    };
    tokio::pin!(shutdown);
    // Cancellation is safe at every await: transaction bytes precede broadcasts,
    // and event deletion follows broker acknowledgement.
    tokio::select! {
        result = synchronize(&mut relay, &projects, &events) => result?,
        _ = &mut shutdown => tracing::info!("Worker shutdown requested"),
    }
    drop(relay);
    if std::fs::exists(HEALTH_FILE)? {
        std::fs::remove_file(HEALTH_FILE)?;
    }
    pool.close().await;
    Ok(())
}

async fn synchronize(relay: &mut Relay, projects: &[ProjectConfig], events: &Events) -> Result<()> {
    Accounts::purge_expired(&relay.indexer.store.pool).await?;
    let mut cleanup = Instant::now();
    loop {
        relay.verify_ownership().await?;
        let mut completed = true;
        for project in projects {
            if let Err(error) = relay.reconcile_project(project).await {
                tracing::error!(project = project.project_id, %error, "Project synchronization failed; original effects retained");
                completed = false;
            }
        }
        if let Err(error) = events.publish_pending(&relay.indexer.store).await {
            tracing::error!(%error, "Outbox delivery paused; committed events retained");
            completed = false;
        }
        if cleanup.elapsed() >= Duration::from_secs(60) {
            Accounts::purge_expired(&relay.indexer.store.pool).await?;
            cleanup = Instant::now();
        }
        if completed {
            heartbeat()?;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn heartbeat() -> Result<()> {
    let temporary = format!("{HEALTH_FILE}.tmp");
    std::fs::write(&temporary, now()?.to_string())?;
    std::fs::rename(temporary, HEALTH_FILE)?;
    Ok(())
}

fn health_check() -> Result<()> {
    let last: u64 = std::fs::read_to_string(HEALTH_FILE)
        .context("Worker has not completed initialization")?
        .parse()?;
    ensure!(
        now()?.saturating_sub(last) < 300,
        "Worker has not completed a successful synchronization cycle for five minutes"
    );
    Ok(())
}
