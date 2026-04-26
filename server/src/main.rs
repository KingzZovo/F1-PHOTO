use anyhow::{Result, bail};
use clap::Parser;
use std::sync::Arc;

use f1_photo_server::{
    api,
    auth::{JwtCodec, jwt::DEFAULT_TTL_SECONDS, password},
    cli::{Cli, Command, ModelsAction},
    config::Config,
    db, inference, logging, worker,
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
        Command::Models { action } => match action {
            ModelsAction::Check => models_check(cfg),
        },
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

    // Load ONNX model registry up-front so /api/admin/models and the
    // worker can both observe a stable snapshot. Loading is best-effort:
    // missing libonnxruntime / model files are reflected in the status
    // and inference is left disabled.
    let registry = inference::ModelRegistry::load(
        cfg.models_dir.clone(),
        cfg.inference_intra_threads,
    );
    let status = registry.status();
    let loaded_count = status.models.iter().filter(|m| m.loaded).count();
    let missing_required: Vec<&str> = status
        .models
        .iter()
        .filter(|m| !m.optional && !m.loaded)
        .map(|m| m.file_name)
        .collect();
    tracing::info!(
        models_dir = %status.models_dir,
        ort_available = status.ort_available,
        ready = status.ready,
        loaded = loaded_count,
        total = status.models.len(),
        missing_required = ?missing_required,
        "inference model registry initialised"
    );
    let models = Arc::new(registry);

    let state = api::AppState {
        db: pool,
        config: Arc::new(cfg),
        jwt,
        models,
    };
    worker::spawn(state.clone());
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

/// `f1photo models check` — probe the configured `models_dir` and print a
/// human-readable summary. Always exits 0 (even when ORT is missing) so it
/// can be safely wired into ops smoke checks.
fn models_check(cfg: Config) -> Result<()> {
    let registry = inference::ModelRegistry::load(
        cfg.models_dir.clone(),
        cfg.inference_intra_threads,
    );
    let status = registry.status();

    println!("models_dir       : {}", status.models_dir);
    println!("intra_threads    : {}", status.intra_threads);
    println!(
        "ort_available    : {}{}",
        status.ort_available,
        match &status.ort_init_error {
            Some(e) => format!("  (error: {e})"),
            None => String::new(),
        }
    );
    println!("ready            : {}", status.ready);
    println!();
    println!(
        "{:<14}  {:<28}  {:<8}  {:<8}  {:<10}  {}",
        "kind", "file", "present", "loaded", "optional", "bytes"
    );
    println!("{}", "-".repeat(96));
    for m in &status.models {
        let kind = format!("{:?}", m.kind);
        let bytes = m
            .file_bytes
            .map(|b| b.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<14}  {:<28}  {:<8}  {:<8}  {:<10}  {}",
            kind,
            m.file_name,
            m.file_present,
            m.loaded,
            m.optional,
            bytes,
        );
        if let Some(err) = &m.error {
            println!("  ! error: {err}");
        }
    }
    Ok(())
}
