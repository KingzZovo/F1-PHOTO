use anyhow::{Result, bail};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use clap::Parser;
use std::sync::Arc;
use uuid::Uuid;

use f1_photo_server::{
    api,
    auth::{JwtCodec, jwt::DEFAULT_TTL_SECONDS, password},
    bundled_pg::BundledPg,
    cli::{Cli, Command, FinetuneAction, ModelsAction},
    config::Config,
    db, finetune, inference, logging, static_assets, worker,
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
        Command::Finetune { action } => match action {
            FinetuneAction::Stats { since, project } => {
                finetune_stats(cfg, since.as_deref(), project.as_deref()).await
            }
            FinetuneAction::Apply {
                since,
                project,
                dry_run,
            } => finetune_apply(cfg, since.as_deref(), project.as_deref(), dry_run).await,
        },
    }
}

async fn serve(cfg: Config) -> Result<()> {
    // If the operator opted into the bundled PG (`F1P_USE_BUNDLED_PG=1`),
    // start it now so the rest of the boot sequence can rely on the
    // standard `F1P_DATABASE_URL` plumbing. The launched child is held in
    // `_pg` for the duration of the process and torn down on shutdown.
    let bundled = BundledPg::maybe_start()?;
    if let Some(ref pg) = bundled {
        tracing::info!(port = pg.port, data_dir = ?pg.data_dir, "bundled postgres ready");
    }

    // Re-derive Config so it picks up F1P_DATABASE_URL written by bundled PG.
    let cfg = if bundled.is_some() {
        Config::from_env()?
    } else {
        cfg
    };
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

    // Log embedded SPA stats so operators can confirm the release binary
    // shipped with the right web bundle.
    let summary = static_assets::embed_summary();
    tracing::info!(
        files = summary.file_count,
        bytes = summary.total_bytes,
        has_index = summary.has_index,
        "embedded SPA bundle"
    );

    let app = api::router_with_spa(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(bind = %bind_addr, "listening");
    let serve_result = axum::serve(listener, app).await;
    if let Some(pg) = bundled {
        pg.shutdown();
    }
    serve_result?;
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

fn parse_since(s: Option<&str>) -> Result<DateTime<Utc>> {
    match s {
        None => Ok(Utc::now() - Duration::days(30)),
        Some(raw) => {
            // accept either YYYY-MM-DD or full RFC3339
            if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
                return Ok(dt.with_timezone(&Utc));
            }
            let d = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .map_err(|e| anyhow::anyhow!("invalid --since '{raw}': {e}"))?;
            Ok(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight always valid")))
        }
    }
}

fn parse_project(s: Option<&str>) -> Result<Option<Uuid>> {
    match s {
        None => Ok(None),
        Some(raw) => Uuid::parse_str(raw)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("invalid --project UUID '{raw}': {e}")),
    }
}

async fn finetune_stats(cfg: Config, since: Option<&str>, project: Option<&str>) -> Result<()> {
    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;
    let since = parse_since(since)?;
    let project = parse_project(project)?;
    let s = finetune::stats(&pool, since, project).await?;
    println!("since               : {}", s.since);
    println!(
        "project             : {}",
        s.project.map(|u| u.to_string()).unwrap_or_else(|| "<all>".to_string())
    );
    println!("total_candidates    : {}", s.total_candidates);
    println!("already_rolled_back : {}", s.already_rolled_back);
    println!("pending             : {}", s.pending);
    println!();
    println!(
        "{:<8}  {:<36}  {:>9}  {:>11}  {:>7}  {}",
        "owner", "id", "candidate", "already", "pending", "latest_corrected_at"
    );
    println!("{}", "-".repeat(110));
    for o in &s.owners {
        println!(
            "{:<8}  {:<36}  {:>9}  {:>11}  {:>7}  {}",
            o.owner_type,
            o.owner_id,
            o.candidate_count,
            o.already_rolled_back,
            o.pending,
            o.latest_corrected_at
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| "-".to_string()),
        );
    }
    Ok(())
}

async fn finetune_apply(
    cfg: Config,
    since: Option<&str>,
    project: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;
    let since = parse_since(since)?;
    let project = parse_project(project)?;
    let r = finetune::apply(&pool, since, project, dry_run).await?;
    println!("since                  : {}", r.since);
    println!(
        "project                : {}",
        r.project.map(|u| u.to_string()).unwrap_or_else(|| "<all>".to_string())
    );
    println!("dry_run                : {}", r.dry_run);
    println!("inserted               : {}", r.inserted);
    println!("skipped_already_present: {}", r.skipped_already_present);
    println!("skipped_no_embedding   : {}", r.skipped_no_embedding);
    Ok(())
}
