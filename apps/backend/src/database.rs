use std::{env, fs, time::Duration};

use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tokio::time::timeout;

pub async fn connect() -> Result<PgPool, &'static str> {
    let host = env::var("PGHOST").map_err(|_| "PGHOST is required")?;
    let port = env::var("PGPORT").map_err(|_| "PGPORT is required")?;
    let database = env::var("PGDATABASE").map_err(|_| "PGDATABASE is required")?;
    let user = env::var("PGUSER").map_err(|_| "PGUSER is required")?;
    if host.is_empty() || database.is_empty() || user.is_empty() {
        return Err("PGHOST, PGDATABASE and PGUSER must not be empty");
    }
    if !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("PGPORT must be an integer between 1 and 65535");
    }
    let port: u16 = port
        .parse()
        .ok()
        .filter(|port| *port > 0)
        .ok_or("PGPORT must be an integer between 1 and 65535")?;

    let password = match (env::var_os("PGPASSWORD"), env::var_os("PGPASSWORD_FILE")) {
        (Some(password), None) => password
            .into_string()
            .map_err(|_| "PGPASSWORD must be valid UTF-8")?,
        (None, Some(path)) => fs::read_to_string(path)
            .map_err(|_| "Cannot read PGPASSWORD_FILE")?
            .trim_end_matches('\n')
            .to_owned(),
        _ => return Err("Set exactly one of PGPASSWORD and PGPASSWORD_FILE"),
    };
    if password.is_empty() {
        return Err("PostgreSQL password must not be empty");
    }

    let options = PgConnectOptions::new_without_pgpass()
        .host(&host)
        .port(port)
        .database(&database)
        .username(&user)
        .password(&password)
        .options([("statement_timeout", "5000")]);
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
        .map_err(|_| "PostgreSQL startup connection check failed")?;

    match timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1").execute(&pool),
    )
    .await
    {
        Ok(Ok(_)) => {
            tracing::info!("PostgreSQL connection verified");
            Ok(pool)
        }
        _ => {
            pool.close().await;
            Err("PostgreSQL startup connection check failed")
        }
    }
}
