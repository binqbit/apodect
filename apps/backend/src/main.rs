use std::{io, process::ExitCode};

use actix_web::{App, HttpServer, web};
use apodect_backend::application::Application;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info};

#[actix_web::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(false)
        .with_writer(io::stdout)
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(%error, "Backend startup or server failure");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> io::Result<()> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let shutdown = async move {
        tokio::select! {
            _ = interrupt.recv() => {},
            _ = terminate.recv() => {},
        }
    };

    let application = web::Data::new(Application::connect().await.map_err(io::Error::other)?);
    let pool = application.indexer.store.pool.clone();
    let server = HttpServer::new(move || App::new().app_data(application.clone()))
        .shutdown_signal(shutdown)
        .shutdown_timeout(10)
        .bind(("0.0.0.0", 3001));

    let result = match server {
        Ok(server) => {
            info!(address = "0.0.0.0:3001", "HTTP server starting");
            server.run().await
        }
        Err(error) => Err(error),
    };

    pool.close().await;
    info!("PostgreSQL pool closed");
    result
}
