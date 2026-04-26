use anyhow::{Result, bail};
use clap::Parser;
use std::sync::Arc;

use f1_photo_server::{
    api,
    auth::{JwtCodec, jwt::DEFAULT_TTL_SECONDS, password},
    cli::{Cli, Command},
    config::Config,
    db, logging,
};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init();
    let cli = Cli::parse();
    let cfg = Config::from_env()?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(cfg).await,
        Command::BootstrapAdmin {
            username,
            password,
            full_name,
        } => bootstrap_admin(cfg, &username, &password, full_name.as_deref()).await,
    }
}

async fn serve(cfg: Config) -> Result<()> {
    let bind_addr = cfg.bind_addr.clone();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %bind_addr,
        "starting f1-photo server"
    );

    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");

    let jwt = Arc::new(JwtCodec::new(&cfg.jwt_secret, DEFAULT_TTL_SECONDS));

    let state = api::AppState {
        db: pool,
        config: Arc::new(cfg),
        jwt,
    };
    let app = api::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(bind = %bind_addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn bootstrap_admin(
    cfg: Config,
    username: &str,
    password_plain: &str,
    full_name: Option<&str>,
) -> Result<()> {
    if username.trim().is_empty() {
        bail!("--username must not be empty");
    }
    if password_plain.len() < 8 {
        bail!("--password must be at least 8 characters");
    }

    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;

    let hash = password::hash_password(password_plain)?;

    let result = sqlx::query(
        "INSERT INTO users (username, password_hash, role, full_name) \
         VALUES ($1, $2, 'admin', $3) \
         ON CONFLICT (username) DO UPDATE SET \
           password_hash = EXCLUDED.password_hash, \
           role          = 'admin', \
           full_name     = COALESCE(EXCLUDED.full_name, users.full_name), \
           disabled_at   = NULL, \
           updated_at    = now() \
         RETURNING (xmax = 0) AS inserted",
    )
    .bind(username)
    .bind(&hash)
    .bind(full_name)
    .fetch_one(&pool)
    .await?;

    let inserted: bool = sqlx::Row::try_get(&result, "inserted").unwrap_or(false);
    if inserted {
        println!("✓ created admin user '{username}'");
    } else {
        println!("✓ updated admin user '{username}' (password reset, role forced to admin)");
    }
    Ok(())
}
