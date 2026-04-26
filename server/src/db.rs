use crate::config::Config;
use anyhow::Result;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

/// Open and pool PostgreSQL connections.
pub async fn connect(cfg: &Config) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(32)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Some(Duration::from_secs(60)))
        .connect(&cfg.database_url)
        .await?;
    Ok(pool)
}

/// Run all sqlx-migrate migrations under `./migrations`.
///
/// The migration files are embedded into the binary at build time, so
/// `cargo run` and a deployed binary share the same migration set.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
