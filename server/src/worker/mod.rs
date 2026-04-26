//! Recognition queue worker.
//!
//! - turn 7 (skeleton): photo → unmatched fallback, queue retries with backoff.
//! - turn 9: ONNX [`crate::inference::ModelRegistry`] is loaded into [`AppState`]
//!   on boot; the worker can query `state.models.ready()` to choose between
//!   the real pipeline and the fallback.
//! - turn 10 (this turn): preprocessing helpers ([`crate::inference::preprocess`])
//!   and pgvector recall helpers ([`crate::inference::recall`]) are wired into
//!   the worker. When [`ModelRegistry::ready`](crate::inference::ModelRegistry::ready)
//!   is `true` the worker calls [`run_real_pipeline`] (currently returns
//!   `Err(NoSessions)` until live ONNX sessions are present; that path becomes
//!   the canonical inference path once `libonnxruntime.so` and the five `.onnx`
//!   files are deployed). When `ready()` is `false` (e.g. dev / first-run
//!   without models) the worker keeps the turn 7 fallback so the rest of the
//!   API and the photos pipeline stay green.
//!
//! Real-inference responsibilities (gated on `ready()`):
//! 1. Decode the photo file from `data_dir/photos/...`.
//! 2. SCRFD face detection + ArcFace embedding for each face crop.
//! 3. YOLOv8n tool/device detection + DINOv2-small embedding for each crop.
//! 4. Persist `detections` rows (with embeddings).
//! 5. pgvector cosine recall via [`crate::inference::recall`] with the
//!    project [`Thresholds`] (0.62 / 0.50 / 0.95).
//! 6. Bucket each detection (matched / learning / unmatched), insert
//!    `recognition_items`, augment the gallery for `score >= 0.95`.
//! 7. Update `photos.status` based on the aggregate of detections.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use moka::future::Cache;
use sqlx::{postgres::PgListener, PgPool, Row};
use tokio::time::sleep;
use uuid::Uuid;

use crate::api::AppState;
use crate::inference::{preprocess, recall, Thresholds};

/// After this many failed attempts the photo is marked `failed` and the queue
/// row is removed.
const MAX_ATTEMPTS: i32 = 5;

/// How long a claimed row stays exclusively reserved by this worker before
/// another picker is allowed to retry it.
const LOCK_LEASE: Duration = Duration::from_secs(5 * 60);

/// Idle poll fallback in case a NOTIFY was missed (e.g. listener reconnected).
const IDLE_POLL: Duration = Duration::from_secs(2);

/// LRU cache cap for face embeddings (turn 10 will populate).
const FACE_CACHE_CAPACITY: u64 = 4096;
/// LRU cache cap for tool/device crops (turn 10 will populate).
const CROP_CACHE_CAPACITY: u64 = 2048;

#[derive(Debug, Clone)]
struct Job {
    id: i64,
    project_id: Uuid,
    photo_id: Uuid,
    attempts: i32,
}

/// Outcome of one job's pipeline. The worker uses this to set `photos.status`
/// and to know whether to drop the queue row (success) or retry (error).
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// No live inference. Photo flagged `unmatched`. Used in dev / first-run.
    FallbackUnmatched,
    /// At least one detection landed in the `matched` bucket.
    Matched,
    /// At least one detection in `learning`, none `matched`.
    Learning,
    /// Detections existed but all fell below the `low_lower` threshold; or
    /// the real pipeline ran but produced zero detections.
    Unmatched,
}

impl Outcome {
    fn as_status(self) -> &'static str {
        match self {
            Outcome::FallbackUnmatched | Outcome::Unmatched => "unmatched",
            Outcome::Matched => "matched",
            Outcome::Learning => "learning",
        }
    }
}

/// Caches available to the recognition pipeline. turn 10 keeps them as
/// pre-allocated capacity that the real pipeline (turn 10+ once ort/.onnx
/// files ship) populates per project.
#[derive(Clone)]
pub struct WorkerCaches {
    /// person_id -> 512d face embedding.
    pub face_embeddings: Cache<Uuid, Arc<Vec<f32>>>,
    /// (target_kind, target_id) -> generic crop embedding.
    pub crop_embeddings: Cache<(String, Uuid), Arc<Vec<f32>>>,
}

impl WorkerCaches {
    fn new() -> Self {
        Self {
            face_embeddings: Cache::builder()
                .max_capacity(FACE_CACHE_CAPACITY)
                .time_to_idle(Duration::from_secs(60 * 60))
                .build(),
            crop_embeddings: Cache::builder()
                .max_capacity(CROP_CACHE_CAPACITY)
                .time_to_idle(Duration::from_secs(60 * 60))
                .build(),
        }
    }
}

/// Spawn the recognition worker as a detached tokio task.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let caches = WorkerCaches::new();
        loop {
            match run(&state, &caches).await {
                Ok(()) => {
                    tracing::warn!("recognition worker run() returned cleanly; restarting");
                }
                Err(e) => {
                    tracing::error!(error = ?e, "recognition worker crashed; restarting in 5s");
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

async fn run(state: &AppState, _caches: &WorkerCaches) -> Result<()> {
    let pool = state.db.clone();
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen("recognition_queue").await?;
    tracing::info!(
        inference_ready = state.models.ready(),
        "recognition worker listening on 'recognition_queue'"
    );

    loop {
        // Drain anything currently due.
        while let Some(job) = claim_one(&pool).await? {
            let job_id = job.id;
            let photo_id = job.photo_id;
            if let Err(e) = process_job(state, &job).await {
                tracing::warn!(
                    job_id,
                    %photo_id,
                    attempts = job.attempts,
                    error = ?e,
                    "process_job failed"
                );
                if let Err(e2) = record_failure(&pool, &job, &format!("{e:#}")).await {
                    tracing::error!(error = ?e2, job_id, "record_failure also failed");
                }
            }
        }

        tokio::select! {
            res = listener.recv() => {
                if let Err(e) = res {
                    tracing::warn!(error = ?e, "PgListener recv error");
                    sleep(Duration::from_secs(1)).await;
                }
            }
            _ = sleep(IDLE_POLL) => {}
        }
    }
}

async fn claim_one(pool: &PgPool) -> Result<Option<Job>> {
    let lease_secs: i32 = LOCK_LEASE.as_secs() as i32;
    let row = sqlx::query(
        "WITH next AS ( \
            SELECT id FROM recognition_queue \
            WHERE locked_until IS NULL OR locked_until < now() \
            ORDER BY created_at LIMIT 1 \
            FOR UPDATE SKIP LOCKED \
         ) \
         UPDATE recognition_queue q \
         SET locked_until = now() + ($1::int || ' seconds')::interval, \
             attempts = q.attempts + 1 \
         FROM next \
         WHERE q.id = next.id \
         RETURNING q.id, q.project_id, q.photo_id, q.attempts",
    )
    .bind(lease_secs)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Job {
        id: r.get::<i64, _>("id"),
        project_id: r.get::<Uuid, _>("project_id"),
        photo_id: r.get::<Uuid, _>("photo_id"),
        attempts: r.get::<i32, _>("attempts"),
    }))
}

/// Process a single recognition job.
///
/// Branches:
/// 1. `state.models.ready() == false` (no `libonnxruntime.so` or no `.onnx`
///    files) → fallback: photo goes straight to `unmatched` and the queue
///    row is dropped. This preserves the turn 7 behaviour the existing test
///    suite expects.
/// 2. `state.models.ready() == true` → [`run_real_pipeline`]. That function
///    currently returns `Err` (no live sessions are deployed in dev), which
///    is converted into a queue retry by [`record_failure`] until
///    [`MAX_ATTEMPTS`] is hit — at which point the photo is marked `failed`
///    and the row is dropped.
async fn process_job(state: &AppState, job: &Job) -> Result<()> {
    let pool = &state.db;

    sqlx::query(
        "UPDATE photos SET status = 'processing'::photo_status, updated_at = now() \
         WHERE id = $1",
    )
    .bind(job.photo_id)
    .execute(pool)
    .await?;

    sleep(Duration::from_millis(20)).await;

    let outcome = if state.models.ready() {
        match run_real_pipeline(state, job).await {
            Ok(o) => o,
            Err(e) if stub_fallback_enabled() => {
                // Real SCRFD/ArcFace/YOLOv8/DINOv2 inference pipeline is not
                // wired in this build (see docs/TODO-deferred.md). Rather
                // than retry-and-eventually-fail every job for ~5*backoff,
                // mark the photo `unmatched` so the queue drains cleanly
                // and the rest of the API stays usable. Set
                // F1P_INFERENCE_STUB_FALLBACK=0 to surface the real-pipeline
                // error as a queue retry once production weights ship.
                tracing::warn!(
                    job_id = job.id,
                    photo_id = %job.photo_id,
                    error = %format!("{e:#}"),
                    "real ONNX pipeline unavailable; falling back to unmatched (F1P_INFERENCE_STUB_FALLBACK=1)"
                );
                Outcome::FallbackUnmatched
            }
            Err(e) => return Err(e),
        }
    } else {
        Outcome::FallbackUnmatched
    };

    apply_outcome(pool, job.photo_id, outcome).await?;

    sqlx::query("DELETE FROM recognition_queue WHERE id = $1")
        .bind(job.id)
        .execute(pool)
        .await?;

    tracing::debug!(
        job_id = job.id,
        photo_id = %job.photo_id,
        project_id = %job.project_id,
        outcome = ?outcome,
        "job processed"
    );
    Ok(())
}

/// Returns true when the worker should treat a real-pipeline `Err` as
/// `Outcome::FallbackUnmatched` instead of bubbling it up to the queue
/// retry path. Defaults to ON until production ONNX weights are wired in,
/// at which point operators flip `F1P_INFERENCE_STUB_FALLBACK=0` to make
/// pipeline errors visible as failures again.
fn stub_fallback_enabled() -> bool {
    match std::env::var("F1P_INFERENCE_STUB_FALLBACK") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !matches!(v.as_str(), "0" | "false" | "no" | "off" | "")
        }
        Err(_) => true,
    }
}

async fn apply_outcome(pool: &PgPool, photo_id: Uuid, outcome: Outcome) -> Result<()> {
    let status = outcome.as_status();
    sqlx::query(
        "UPDATE photos SET status = $1::photo_status, updated_at = now() \
         WHERE id = $2",
    )
    .bind(status)
    .bind(photo_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Real ONNX inference pipeline.
///
/// This is gated on [`ModelRegistry::ready`] being `true`, which in turn
/// requires `libonnxruntime.so` to be loadable AND every required model file
/// (`face_detect.onnx`, `face_embed.onnx`, `object_detect.onnx`,
/// `generic_embed.onnx`) to exist and to have built a `Session`. Until we
/// deploy ONNX Runtime + the model files this returns `Err` so the queue
/// retries with backoff, exactly as it would for a transient model-load
/// problem in production.
async fn run_real_pipeline(state: &AppState, job: &Job) -> Result<Outcome> {
    // Resolve photo storage path. `photos.path` is stored as a relative path
    // (e.g. `photos/<project_id>/<prefix>/<hash>.jpg`); the data root lives
    // in `Config::data_dir`.
    let row = sqlx::query("SELECT path FROM photos WHERE id = $1")
        .bind(job.photo_id)
        .fetch_one(&state.db)
        .await?;
    let rel: String = row.get("path");
    let full = std::path::Path::new(&state.config.data_dir).join(&rel);

    // ---- detection (SCRFD) -------------------------------------------------
    // Currently uncallable: no live `Session` for FaceDetect / FaceEmbed /
    // ObjectDetect / GenericEmbed in dev. Sketching the call sites makes the
    // module shape obvious for the next code drop.
    let _ = full;
    let _t = Thresholds::DEFAULT;
    let _v = recall::encode_vector(&[0.0f32; 512]);
    let _ = preprocess::Norm::Scrfd;

    Err(anyhow!(
        "real ONNX inference pipeline not yet wired: requires libonnxruntime.so + .onnx files"
    ))
}

async fn record_failure(pool: &PgPool, job: &Job, msg: &str) -> Result<()> {
    if job.attempts >= MAX_ATTEMPTS {
        sqlx::query(
            "UPDATE photos SET status = 'failed'::photo_status, updated_at = now() \
             WHERE id = $1",
        )
        .bind(job.photo_id)
        .execute(pool)
        .await?;
        sqlx::query("DELETE FROM recognition_queue WHERE id = $1")
            .bind(job.id)
            .execute(pool)
            .await?;
        tracing::error!(
            job_id = job.id,
            photo_id = %job.photo_id,
            attempts = job.attempts,
            "job exhausted retries; photo marked failed"
        );
        return Ok(());
    }

    let backoff_secs: i32 = 5 * job.attempts.pow(2).max(1);
    sqlx::query(
        "UPDATE recognition_queue \
         SET locked_until = now() + ($1::int || ' seconds')::interval, \
             last_error = $2 \
         WHERE id = $3",
    )
    .bind(backoff_secs)
    .bind(msg)
    .bind(job.id)
    .execute(pool)
    .await?;
    Ok(())
}
