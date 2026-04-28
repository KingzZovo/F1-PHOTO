use anyhow::{bail, Result};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

use f1_photo_server::{
    api,
    auth::{jwt::DEFAULT_TTL_SECONDS, password, JwtCodec},
    bundled_pg::BundledPg,
    cli::{Cli, Command, FinetuneAction, ModelsAction, RetrainDetectorAction},
    config::Config,
    db, finetune, inference, logging, retrain, static_assets, worker,
};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init();
    let cli = Cli::parse();

    // If the operator opted into the bundled PG (`F1P_USE_BUNDLED_PG=1`),
    // start it BEFORE Config::from_env() so that F1P_DATABASE_URL is set
    // by the time Config validates env. The handle is held for the lifetime
    // of `main` so the watchdog stays attached and shutdown still runs.
    let bundled = BundledPg::maybe_start()?;
    if let Some(ref pg) = bundled {
        tracing::info!(
            port = pg.port,
            data_dir = ?pg.data_dir,
            "bundled postgres ready"
        );
    }

    let cfg = Config::from_env()?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(cfg, bundled).await,
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
        Command::RetrainDetector { action } => match action {
            RetrainDetectorAction::Stats { since, min_score } => {
                retrain_stats(cfg, since.as_deref(), min_score).await
            }
            RetrainDetectorAction::Prepare {
                since,
                min_score,
                min_corrections,
                training_dir,
                dry_run,
            } => {
                retrain_prepare(
                    cfg,
                    since.as_deref(),
                    min_score,
                    min_corrections,
                    training_dir.as_deref(),
                    dry_run,
                )
                .await
            }
            RetrainDetectorAction::Train {
                cycle_dir,
                base_weights,
                epochs,
                imgsz,
                export_imgsz,
                freeze,
                batch,
                workers,
                device,
                training_dir,
                runs_dir,
                run_name,
                candidate_out,
                opset,
                python,
                script,
            } => {
                retrain_train(
                    cfg,
                    &cycle_dir,
                    &base_weights,
                    epochs,
                    imgsz,
                    export_imgsz,
                    freeze,
                    batch,
                    workers,
                    &device,
                    training_dir.as_deref(),
                    runs_dir.as_deref(),
                    run_name.as_deref(),
                    candidate_out.as_deref(),
                    opset,
                    python.as_deref(),
                    script.as_deref(),
                )
                .await
            }
        },
    }
}

async fn serve(cfg: Config, bundled: Option<BundledPg>) -> Result<()> {
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
    let registry =
        inference::ModelRegistry::load(cfg.models_dir.clone(), cfg.inference_intra_threads);
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
    let registry =
        inference::ModelRegistry::load(cfg.models_dir.clone(), cfg.inference_intra_threads);
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
        "{:<14}  {:<28}  {:<8}  {:<8}  {:<10}  bytes",
        "kind", "file", "present", "loaded", "optional"
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
            kind, m.file_name, m.file_present, m.loaded, m.optional, bytes,
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
        s.project
            .map(|u| u.to_string())
            .unwrap_or_else(|| "<all>".to_string())
    );
    println!("total_candidates    : {}", s.total_candidates);
    println!("already_rolled_back : {}", s.already_rolled_back);
    println!("pending             : {}", s.pending);
    println!();
    println!(
        "{:<8}  {:<36}  {:>9}  {:>11}  {:>7}  latest_corrected_at",
        "owner", "id", "candidate", "already", "pending"
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
        r.project
            .map(|u| u.to_string())
            .unwrap_or_else(|| "<all>".to_string())
    );
    println!("dry_run                : {}", r.dry_run);
    println!("inserted               : {}", r.inserted);
    println!("skipped_already_present: {}", r.skipped_already_present);
    println!("skipped_no_embedding   : {}", r.skipped_no_embedding);
    Ok(())
}

/// Resolve the on-disk training-cycle root.
///
/// Precedence: `--training-dir` flag > `F1P_TRAINING_DIR` env > `<data_dir>/training`.
fn resolve_training_dir(cfg: &Config, override_dir: Option<&str>) -> PathBuf {
    if let Some(s) = override_dir {
        return PathBuf::from(s);
    }
    if let Ok(s) = std::env::var("F1P_TRAINING_DIR") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    Path::new(&cfg.data_dir).join("training")
}

async fn retrain_stats(cfg: Config, since: Option<&str>, min_score: f64) -> Result<()> {
    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;
    let since = parse_since(since)?;
    let s = retrain::stats(&pool, since, min_score).await?;
    println!("since               : {}", s.since);
    println!("min_score           : {}", s.min_score);
    println!("total               : {}", s.total);
    println!();
    println!("{:<14}  {:>9}", "owner_type", "count");
    println!("{}", "-".repeat(28));
    for o in &s.by_owner_type {
        println!("{:<14}  {:>9}", o.owner_type, o.count);
    }
    Ok(())
}

async fn retrain_prepare(
    cfg: Config,
    since: Option<&str>,
    min_score: f64,
    min_corrections: i64,
    training_dir_override: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;
    let since = parse_since(since)?;
    let training_dir = resolve_training_dir(&cfg, training_dir_override);
    let data_dir = PathBuf::from(&cfg.data_dir);
    let r = retrain::prepare(
        &pool,
        &data_dir,
        &training_dir,
        since,
        min_score,
        min_corrections,
        dry_run,
    )
    .await?;
    println!("since                 : {}", r.since);
    println!("min_score             : {}", r.min_score);
    println!("min_corrections       : {}", r.min_corrections);
    println!("dry_run               : {}", r.dry_run);
    println!("training_dir          : {}", training_dir.display());
    println!("eligible              : {}", r.eligible);
    println!("written               : {}", r.written);
    println!("below_threshold       : {}", r.below_threshold);
    println!("skipped_no_dimensions : {}", r.skipped_no_dimensions);
    println!("skipped_degenerate_bbox: {}", r.skipped_degenerate_bbox);
    println!("skipped_missing_photo : {}", r.skipped_missing_photo);
    println!(
        "cycle_dir             : {}",
        r.cycle_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".to_string())
    );
    if r.below_threshold {
        println!();
        println!(
            "NOTE: eligible ({}) below min_corrections ({}); no cycle written.",
            r.eligible, r.min_corrections
        );
    }
    Ok(())
}

/// Resolve the python interpreter to use for `tools/retrain_train.py`.
/// Precedence: explicit override > `$F1P_PYTHON` env > `python3` on PATH.
fn resolve_python(override_path: Option<&str>) -> PathBuf {
    if let Some(s) = override_path {
        return PathBuf::from(s);
    }
    if let Ok(s) = std::env::var("F1P_PYTHON") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    PathBuf::from("python3")
}

/// Resolve `tools/retrain_train.py`. Precedence: explicit override >
/// `$F1P_RETRAIN_SCRIPT` env > `<binary_dir>/../tools/retrain_train.py`
/// (deploy layout: `payload/f1photo` next to `tools/`) > literal
/// `tools/retrain_train.py` (development layout, cwd at repo root).
fn resolve_retrain_script(override_path: Option<&str>) -> PathBuf {
    if let Some(s) = override_path {
        return PathBuf::from(s);
    }
    if let Ok(s) = std::env::var("F1P_RETRAIN_SCRIPT") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["../tools/retrain_train.py", "tools/retrain_train.py"] {
                let p = dir.join(rel);
                if p.is_file() {
                    return p;
                }
            }
        }
    }
    PathBuf::from("tools/retrain_train.py")
}

#[allow(clippy::too_many_arguments)]
async fn retrain_train(
    cfg: Config,
    cycle_dir: &str,
    base_weights: &str,
    epochs: u32,
    imgsz: u32,
    export_imgsz: u32,
    freeze: u32,
    batch: u32,
    workers: u32,
    device: &str,
    training_dir_override: Option<&str>,
    runs_dir_override: Option<&str>,
    run_name_override: Option<&str>,
    candidate_out_override: Option<&str>,
    opset: u32,
    python_override: Option<&str>,
    script_override: Option<&str>,
) -> Result<()> {
    let cycle_dir_pb = PathBuf::from(cycle_dir);
    if !cycle_dir_pb.is_dir() {
        bail!("cycle-dir is not a directory: {}", cycle_dir_pb.display());
    }
    let training_dir = resolve_training_dir(&cfg, training_dir_override);
    let run_name = run_name_override
        .map(|s| s.to_string())
        .or_else(|| {
            cycle_dir_pb
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "train".to_string());
    let runs_dir = match runs_dir_override {
        Some(s) => PathBuf::from(s),
        None => training_dir.join("runs"),
    };
    let candidate_out = match candidate_out_override {
        Some(s) => PathBuf::from(s),
        None => training_dir.join(format!("{run_name}.candidate.onnx")),
    };
    let summary_out = training_dir.join(format!("{run_name}.summary.json"));
    let python = resolve_python(python_override);
    let script = resolve_retrain_script(script_override);

    println!("cycle_dir       : {}", cycle_dir_pb.display());
    println!("training_dir    : {}", training_dir.display());
    println!("runs_dir        : {}", runs_dir.display());
    println!("run_name        : {}", run_name);
    println!("candidate_out   : {}", candidate_out.display());
    println!("summary_out     : {}", summary_out.display());
    println!("python          : {}", python.display());
    println!("script          : {}", script.display());
    println!("epochs          : {}", epochs);
    println!("imgsz / export  : {} / {}", imgsz, export_imgsz);
    println!("freeze / batch  : {} / {}", freeze, batch);
    println!("workers / device: {} / {}", workers, device);
    println!("opset           : {}", opset);
    println!();

    let params = retrain::TrainParams {
        cycle_dir: cycle_dir_pb,
        base_weights: base_weights.to_string(),
        epochs,
        imgsz,
        export_imgsz,
        freeze,
        batch,
        workers,
        device: device.to_string(),
        runs_dir,
        run_name,
        candidate_out,
        opset,
        summary_out,
        python,
        script,
    };

    // `retrain::train` shells out to a long-running python process and
    // does blocking I/O; offload it from the tokio runtime so the runtime
    // is free even though the CLI has nothing else to do.
    let report = tokio::task::spawn_blocking(move || retrain::train(&params)).await??;

    println!("status               : {}", report.status);
    println!("output_shape         : {:?}", report.output_shape);
    println!("candidate_size_bytes : {}", report.candidate_size_bytes);
    println!("candidate_out        : {}", report.candidate_out);
    println!("best_pt              : {}", report.best_pt);
    println!("train_seconds        : {:.1}", report.train_seconds);
    println!("export_seconds       : {:.1}", report.export_seconds);
    Ok(())
}
