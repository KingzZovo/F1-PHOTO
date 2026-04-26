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
use crate::inference::{preprocess, recall, ModelKind, Thresholds};

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
/// Gated on [`ModelRegistry::ready`] being `true` (libonnxruntime.so loaded
/// AND every required `.onnx` Session built).
///
/// Current scope (turn 22): the **GenericEmbed** slot is wired to a real
/// DINOv2-small ONNX model. The whole image is run through the embedder,
/// producing a CLS-token embedding (384 dims) which is L2-normalised and
/// zero-padded to the schema's `vector(512)` width. A single `detections`
/// row is written (`target_type='tool'`, bbox = full image, score = 1.0
/// = synthetic confidence — there is no proper detection step yet),
/// followed by a pgvector cosine recall against the tool/device gallery,
/// a `recognition_items` row, and (when the score crosses the augment
/// threshold) an incremental gallery augment.
///
/// The face-detect / face-embed / object-detect slots are NOT yet wired
/// here — see `docs/TODO-deferred.md` §1. Until they are, faces and
/// individual tools/devices are NOT separately localised; the per-photo
/// outcome is whatever the whole-image DINOv2 embedding nearest-neighbours
/// to in the existing gallery (typically `Unmatched` on a fresh install).
async fn run_real_pipeline(state: &AppState, job: &Job) -> Result<Outcome> {
    let pool = &state.db;

    // Resolve photo storage path. `photos.path` is stored as a relative path
    // (e.g. `photos/<project_id>/<prefix>/<hash>.jpg`); the data root lives
    // in `Config::data_dir`.
    let photo_row = sqlx::query("SELECT path, project_id FROM photos WHERE id = $1")
        .bind(job.photo_id)
        .fetch_one(pool)
        .await?;
    let rel: String = photo_row.get("path");
    let project_id: Uuid = photo_row.get("project_id");
    let full = std::path::Path::new(&state.config.data_dir).join(&rel);

    // ---- DINOv2 generic embedding on the whole image ----------------------
    // 224x224 letterbox + ImageNet mean/std, per `preprocess::Norm::ImageNet`.
    let (nchw, _lb, (src_w, src_h)) =
        preprocess::decode_letterbox_nchw(&full, 224, preprocess::Norm::ImageNet)?;

    let session = state
        .models
        .get(ModelKind::GenericEmbed)
        .ok_or_else(|| anyhow!("GenericEmbed session not loaded"))?;

    // ort 2.0.0-rc.9 internally bundles ndarray 0.16, but this crate's
    // `preprocess` module uses ndarray 0.15 (`Array4<f32>` is therefore a
    // *different* type from ort's perspective and not assignable to its
    // `IntoValueTensor`). Side-step the version skew by feeding ort the
    // `(shape, Vec<T>)` form, which is one of the supported input shapes
    // and is independent of the ndarray version.
    let dims = nchw.shape().to_vec(); // [1, 3, 224, 224]
    let data: Vec<f32> = nchw.iter().copied().collect(); // row-major NCHW
    let pixel_values = ort::value::Tensor::from_array((dims, data))?;
    let outputs = session.run(ort::inputs! { "pixel_values" => pixel_values }?)?;

    // DINOv2-small output: shape (B=1, 257, 384). Token 0 is the CLS embedding.
    let raw_view = outputs["last_hidden_state"].try_extract_tensor::<f32>()?;
    let shape = raw_view.shape().to_vec();
    if shape.len() != 3 || shape[2] == 0 || shape[1] == 0 {
        return Err(anyhow!(
            "DINOv2 output has unexpected shape {:?}; expected (B, T, D)",
            shape
        ));
    }
    let dim = shape[2];
    let mut emb: Vec<f32> = (0..dim).map(|i| raw_view[[0, 0, i]]).collect();
    recall::l2_normalize(&mut emb);
    let emb_512 = recall::pad_to_512(&emb);

    // ---- detection row (synthetic single full-image detection) ------------
    let bbox = serde_json::json!({
        "x1": 0,
        "y1": 0,
        "x2": src_w,
        "y2": src_h,
        "source": "dinov2_whole_image",
    });
    let v_pg = recall::encode_vector(&emb_512);
    let detection_id: i64 = sqlx::query_scalar(
        "INSERT INTO detections \
         (project_id, photo_id, target_type, bbox, score, embedding, match_status) \
         VALUES ($1, $2, 'tool'::detect_target, $3, $4, $5::vector, 'unmatched'::match_status) \
         RETURNING id",
    )
    .bind(project_id)
    .bind(job.photo_id)
    .bind(&bbox)
    .bind(1.0_f32)
    .bind(&v_pg)
    .fetch_one(pool)
    .await?;

    // ---- pgvector cosine recall against the tool/device gallery -----------
    let hit = recall::top1_object(pool, &emb_512).await?;
    let thresholds = Thresholds::DEFAULT;
    let (bucket, suggested_owner_type, suggested_owner_id, suggested_score) = match &hit {
        Some(h) => {
            let b = h.bucket(thresholds);
            (
                b,
                Some(h.owner_type.clone()),
                Some(h.owner_id),
                Some(h.score),
            )
        }
        None => (recall::Bucket::Unmatched, None, None, None),
    };
    let status_str = match bucket {
        recall::Bucket::Matched => "matched",
        recall::Bucket::Learning => "learning",
        recall::Bucket::Unmatched => "unmatched",
    };

    // Update detection with recall result.
    sqlx::query(
        "UPDATE detections SET \
             match_status = $1::match_status, \
             matched_owner_type = $2::owner_type, \
             matched_owner_id = $3, \
             matched_score = $4 \
         WHERE id = $5",
    )
    .bind(status_str)
    .bind(suggested_owner_type.as_deref())
    .bind(suggested_owner_id)
    .bind(suggested_score)
    .bind(detection_id)
    .execute(pool)
    .await?;

    // Insert the recognition_items row that surfaces this detection in the UI.
    sqlx::query(
        "INSERT INTO recognition_items \
         (project_id, photo_id, detection_id, status, \
          suggested_owner_type, suggested_owner_id, suggested_score) \
         VALUES ($1, $2, $3, $4::match_status, $5::owner_type, $6, $7)",
    )
    .bind(project_id)
    .bind(job.photo_id)
    .bind(detection_id)
    .bind(status_str)
    .bind(suggested_owner_type.as_deref())
    .bind(suggested_owner_id)
    .bind(suggested_score)
    .execute(pool)
    .await?;

    // Augment the gallery on a strong match so it adapts over time.
    if let (Some(h), recall::Bucket::Matched) = (&hit, bucket) {
        if h.score >= thresholds.augment_upper {
            recall::augment(
                pool,
                &h.owner_type,
                h.owner_id,
                &emb_512,
                job.photo_id,
                project_id,
            )
            .await?;
        }
    }

    Ok(match bucket {
        recall::Bucket::Matched => Outcome::Matched,
        recall::Bucket::Learning => Outcome::Learning,
        recall::Bucket::Unmatched => Outcome::Unmatched,
    })
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
