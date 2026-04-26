//! Recognition queue worker.
//!
//! - turn 7 (skeleton): photo → unmatched fallback, queue retries with backoff.
//! - turn 9: ONNX [`crate::inference::ModelRegistry`] is loaded into [`AppState`]
//!   on boot; the worker can query `state.models.ready()` to choose between
//!   the real pipeline and the fallback.
//! - turn 10: preprocessing helpers ([`crate::inference::preprocess`])
//!   and pgvector recall helpers ([`crate::inference::recall`]) are wired into
//!   the worker.
//! - turn 22: GenericEmbed slot wired to a real DINOv2-small ONNX model
//!   (whole-image stand-in for tool/device).
//! - turn 23 (this turn): FaceDetect (SCRFD) + FaceEmbed (ArcFace 512d) slots
//!   wired end-to-end. The face pipeline now runs **before** the DINOv2
//!   tool/device pipeline; per-photo `Outcome` is the best bucket across all
//!   detections (face + tool).
//! - turn 23 Step B: ObjectDetect (YOLOv8n COCO) wired end-to-end. The
//!   tool/device pipeline now runs YOLOv8 first, then takes per-detection
//!   crops through DINOv2-small for re-identification against the project's
//!   tool/device gallery. When YOLOv8 finds nothing, the pipeline falls back
//!   to a single whole-image DINOv2 detection (preserves prior behaviour and
//!   keeps `recognition_items` non-empty in smoke). YOLOv8n is COCO-trained
//!   so its class labels are unrelated to F1-photo's taxonomy — see
//!   `docs/TODO-deferred.md` §1 for the fine-tune follow-up.
//!
//! Real-inference responsibilities (gated on `ready()`):
//! 1. Decode the photo file from `data_dir/photos/...`.
//! 2. SCRFD face detection + ArcFace embedding for each face crop.
//! 3. YOLOv8n tool/device detection + DINOv2-small embedding for each crop.
//!    Falls back to a single whole-image DINOv2 detection when YOLOv8
//!    proposes no boxes for the photo.
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
use crate::inference::{preprocess, recall, scrfd, yolov8, ModelKind, Thresholds};

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
/// 2. `state.models.ready() == true` → [`run_real_pipeline`]. Errors are
///    converted into a queue retry by [`record_failure`] until
///    [`MAX_ATTEMPTS`] is hit (or, when `F1P_INFERENCE_STUB_FALLBACK=1`,
///    silently downgraded to `Outcome::FallbackUnmatched` so the queue
///    keeps draining during the gradual roll-out).
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
/// Gated on [`crate::inference::ModelRegistry::ready`] being `true`
/// (libonnxruntime.so loaded AND every required `.onnx` Session built).
///
/// Current scope (turn 23 + Step B):
/// - **FaceDetect** (SCRFD) + **FaceEmbed** (ArcFace MobileFaceNet 512d)
///   are wired end-to-end. The whole image goes through SCRFD; each face
///   bbox feeds into an ArcFace 112×112 crop and produces a 512-d
///   L2-normalised embedding (NO zero padding — schema width matches).
///   Per-face `detections` row + person-gallery recall + `recognition_items`.
/// - **ObjectDetect** (YOLOv8n COCO) + **GenericEmbed** (DINOv2-small) are
///   wired end-to-end for tool/device. YOLOv8 proposes object boxes; each
///   crop runs DINOv2 to produce a CLS-token embedding (384 dims) which is
///   L2-normalised and zero-padded to 512 via [`recall::pad_to_512`]. One
///   `detections` row + `recognition_items` row per detected object, recall
///   against the tool/device gallery. When YOLOv8 finds nothing the pipeline
///   degrades to a single whole-image DINOv2 detection.
///
/// Caveat: yolov8n is COCO-trained. Its class labels do not match the
/// project's tool/device taxonomy; we use it purely as a region proposer
/// and rely on the DINOv2 embedding + per-project gallery for actual
/// re-identification. Fine-tuning a domain-specific detector is tracked in
/// `docs/TODO-deferred.md` §1.
///
/// The per-photo [`Outcome`] is the best bucket across all detections
/// (face + tool): Matched > Learning > Unmatched. With zero detections the
/// photo is flagged `Unmatched`.
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

    // Decode once and reuse for both face (640) and tool (224) sub-pipelines.
    let img = preprocess::decode_path(&full)?;
    let src_w = img.width();
    let src_h = img.height();

    let mut buckets: Vec<recall::Bucket> = Vec::new();

    // ---- face pipeline (SCRFD detect + ArcFace embed) --------------------
    let face_buckets = run_face_pipeline(state, job, &img, src_w, src_h, project_id).await?;
    buckets.extend(face_buckets);

    // ---- tool/device pipeline (YOLOv8 detect + DINOv2 per-crop embed) ----
    let tool_buckets = run_tool_pipeline(state, job, &img, src_w, src_h, project_id).await?;
    buckets.extend(tool_buckets);

    Ok(aggregate_outcome(&buckets))
}

/// Best-bucket aggregation across all detections in a job.
fn aggregate_outcome(buckets: &[recall::Bucket]) -> Outcome {
    if buckets.iter().any(|b| matches!(b, recall::Bucket::Matched)) {
        Outcome::Matched
    } else if buckets
        .iter()
        .any(|b| matches!(b, recall::Bucket::Learning))
    {
        Outcome::Learning
    } else {
        Outcome::Unmatched
    }
}

/// SCRFD face detection → ArcFace face embedding → person-gallery recall.
/// Persists one `detections` row per face plus a `recognition_items` row.
/// Returns one [`recall::Bucket`] per detected face.
async fn run_face_pipeline(
    state: &AppState,
    job: &Job,
    img: &image::DynamicImage,
    src_w: u32,
    src_h: u32,
    project_id: Uuid,
) -> Result<Vec<recall::Bucket>> {
    let pool = &state.db;
    let det_session = state
        .models
        .get(ModelKind::FaceDetect)
        .ok_or_else(|| anyhow!("FaceDetect session not loaded"))?;
    let emb_session = state
        .models
        .get(ModelKind::FaceEmbed)
        .ok_or_else(|| anyhow!("FaceEmbed session not loaded"))?;

    // SCRFD preprocess: letterbox to a fixed 640×640 + Norm::Scrfd.
    let (canvas, lb) = preprocess::letterbox(img, scrfd::INPUT_SIZE);
    let nchw = preprocess::to_nchw(&canvas, preprocess::Norm::Scrfd);
    let dims = nchw.shape().to_vec();
    let data: Vec<f32> = nchw.iter().copied().collect();
    let input_tensor = ort::value::Tensor::from_array((dims, data))?;
    // Positional inputs work regardless of the model's input name (SCRFD
    // det_500m exports it as `input.1`).
    let det_outputs = det_session.run(ort::inputs![input_tensor]?)?;

    // Pull the 9 SCRFD outputs by their declared name (in session order):
    // The buffalo_s `det_500m.onnx` exports outputs grouped by *head* then
    // by stride, not by level. Confirmed live from the model registry:
    //   ["443", "468", "493",   // scores @ s8, s16, s32   (12800, 3200, 800)
    //    "446", "471", "496",   // bboxes @ s8, s16, s32   (4 each)
    //    "449", "474", "499"]   // kps    @ s8, s16, s32   (10 each)
    let out_names: Vec<String> = det_session.outputs.iter().map(|o| o.name.clone()).collect();
    if out_names.len() != 9 {
        return Err(anyhow!(
            "SCRFD expected 9 outputs, got {}: {:?}",
            out_names.len(),
            out_names
        ));
    }
    let mut flat: Vec<Vec<f32>> = Vec::with_capacity(9);
    for name in &out_names {
        let view = det_outputs[name.as_str()].try_extract_tensor::<f32>()?;
        flat.push(view.iter().copied().collect());
    }

    let dets = scrfd::decode_outputs(
        [&flat[0], &flat[1], &flat[2]],
        [&flat[3], &flat[4], &flat[5]],
        [&flat[6], &flat[7], &flat[8]],
        lb,
        src_w,
        src_h,
    )?;

    tracing::info!(
        job_id = job.id,
        photo_id = %job.photo_id,
        face_count = dets.len(),
        "SCRFD detected faces"
    );

    let thresholds = Thresholds::DEFAULT;
    let mut buckets: Vec<recall::Bucket> = Vec::with_capacity(dets.len());

    for det in &dets {
        // ArcFace per-face: 112×112 tight crop + Norm::ArcFace.
        let nchw = preprocess::crop_to_nchw(img, det.bbox, 112, preprocess::Norm::ArcFace);
        let dims = nchw.shape().to_vec();
        let data: Vec<f32> = nchw.iter().copied().collect();
        let face_input = ort::value::Tensor::from_array((dims, data))?;
        let emb_outputs = emb_session.run(ort::inputs![face_input]?)?;

        let emb_out_name = emb_session
            .outputs
            .first()
            .ok_or_else(|| anyhow!("FaceEmbed has no outputs"))?
            .name
            .clone();
        let view = emb_outputs[emb_out_name.as_str()].try_extract_tensor::<f32>()?;
        // ArcFace MobileFaceNet output is (1, 512). Iterate flat (one face).
        let mut emb: Vec<f32> = view.iter().copied().collect();
        if emb.is_empty() {
            return Err(anyhow!("ArcFace produced an empty embedding"));
        }
        recall::l2_normalize(&mut emb);

        // Production face_embed.onnx (ArcFace) is 512-d; do NOT pad. Defensive
        // fallback only fires if the slot is replaced with a different
        // architecture later — surface it as a warning so it's noticed.
        if emb.len() != 512 {
            tracing::warn!(
                emb_dim = emb.len(),
                "ArcFace embedding is not 512-d; adapting to schema width via pad_to_512"
            );
            emb = recall::pad_to_512(&emb);
        }

        // Persist detection row with `target_type='face'`.
        let bbox_json = serde_json::json!({
            "x1": det.bbox.0,
            "y1": det.bbox.1,
            "x2": det.bbox.2,
            "y2": det.bbox.3,
            "kps": det.kps.iter().map(|(x, y)| serde_json::json!([*x, *y])).collect::<Vec<_>>(),
            "source": "scrfd",
        });
        let v_pg = recall::encode_vector(&emb);
        let detection_id: i64 = sqlx::query_scalar(
            "INSERT INTO detections \
             (project_id, photo_id, target_type, bbox, score, embedding, match_status) \
             VALUES ($1, $2, 'face'::detect_target, $3, $4, $5::vector, 'unmatched'::match_status) \
             RETURNING id",
        )
        .bind(project_id)
        .bind(job.photo_id)
        .bind(&bbox_json)
        .bind(det.score)
        .bind(&v_pg)
        .fetch_one(pool)
        .await?;

        // pgvector cosine recall against the person gallery.
        let hit = recall::top1_face(pool, &emb).await?;
        let (bucket, owner_type, owner_id, score) = match &hit {
            Some(h) => (
                h.bucket(thresholds),
                Some(h.owner_type.clone()),
                Some(h.owner_id),
                Some(h.score),
            ),
            None => (recall::Bucket::Unmatched, None, None, None),
        };
        let status_str = match bucket {
            recall::Bucket::Matched => "matched",
            recall::Bucket::Learning => "learning",
            recall::Bucket::Unmatched => "unmatched",
        };

        sqlx::query(
            "UPDATE detections SET \
                 match_status = $1::match_status, \
                 matched_owner_type = $2::owner_type, \
                 matched_owner_id = $3, \
                 matched_score = $4 \
             WHERE id = $5",
        )
        .bind(status_str)
        .bind(owner_type.as_deref())
        .bind(owner_id)
        .bind(score)
        .bind(detection_id)
        .execute(pool)
        .await?;

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
        .bind(owner_type.as_deref())
        .bind(owner_id)
        .bind(score)
        .execute(pool)
        .await?;

        if let (Some(h), recall::Bucket::Matched) = (&hit, bucket) {
            if h.score >= thresholds.augment_upper {
                recall::augment(
                    pool,
                    &h.owner_type,
                    h.owner_id,
                    &emb,
                    job.photo_id,
                    project_id,
                )
                .await?;
            }
        }
        buckets.push(bucket);
    }

    Ok(buckets)
}

/// YOLOv8n object detection → DINOv2-small per-crop embedding →
/// tool/device-gallery recall.
///
/// For each YOLOv8 detection, this function persists one `detections` row
/// (`target_type='tool'`) with the per-crop DINOv2 embedding plus a matching
/// `recognition_items` row, and returns the recall bucket. When YOLOv8
/// proposes zero boxes the function falls back to a single whole-image
/// DINOv2 detection (`source="dinov2_whole_image_fallback"`) so that the
/// pipeline still produces a `detections`/`recognition_items` pair per
/// photo — important both for smoke evidence and so an Unmatched bucket
/// still propagates into the per-photo aggregate.
///
/// yolov8n is COCO-trained: its class labels are unrelated to F1-photo's
/// `tools`/`devices` taxonomy. We use it strictly as a region proposer; the
/// actual re-identification happens via DINOv2 + per-project gallery cosine
/// recall. See `docs/TODO-deferred.md` §1.
async fn run_tool_pipeline(
    state: &AppState,
    job: &Job,
    img: &image::DynamicImage,
    src_w: u32,
    src_h: u32,
    project_id: Uuid,
) -> Result<Vec<recall::Bucket>> {
    let det_session = state
        .models
        .get(ModelKind::ObjectDetect)
        .ok_or_else(|| anyhow!("ObjectDetect session not loaded"))?;
    let emb_session = state
        .models
        .get(ModelKind::GenericEmbed)
        .ok_or_else(|| anyhow!("GenericEmbed session not loaded"))?;

    // YOLOv8 preprocess: letterbox to 640x640 + Norm::Unit (px / 255).
    let (canvas, lb) = preprocess::letterbox(img, yolov8::INPUT_SIZE);
    let nchw = preprocess::to_nchw(&canvas, preprocess::Norm::Unit);
    let dims = nchw.shape().to_vec();
    let data: Vec<f32> = nchw.iter().copied().collect();
    let yolo_input = ort::value::Tensor::from_array((dims, data))?;
    let yolo_outputs = det_session.run(ort::inputs![yolo_input]?)?;

    // YOLOv8 has a single output named (by ultralytics export) `output0`.
    // Fall back to the first declared output if a future export renames it.
    let out_name: String = det_session
        .outputs
        .first()
        .ok_or_else(|| anyhow!("ObjectDetect has no outputs"))?
        .name
        .clone();
    let view = yolo_outputs[out_name.as_str()].try_extract_tensor::<f32>()?;
    let flat: Vec<f32> = view.iter().copied().collect();
    let dets = yolov8::decode_outputs(
        &flat,
        lb,
        src_w,
        src_h,
        yolov8::DEFAULT_CONF,
        yolov8::DEFAULT_IOU,
    )?;

    tracing::info!(
        job_id = job.id,
        photo_id = %job.photo_id,
        object_count = dets.len(),
        "YOLOv8 detected objects"
    );

    let thresholds = Thresholds::DEFAULT;
    let mut buckets: Vec<recall::Bucket> = Vec::new();

    if dets.is_empty() {
        // No proposals: fall back to a whole-image DINOv2 embedding so the
        // pipeline still emits a detections/recognition_items pair.
        let bucket = embed_and_persist_object(
            state,
            job,
            project_id,
            (0.0, 0.0, src_w as f32, src_h as f32),
            1.0,
            "dinov2_whole_image_fallback",
            None,
            img,
            emb_session,
            thresholds,
        )
        .await?;
        buckets.push(bucket);
        return Ok(buckets);
    }

    for det in &dets {
        let bucket = embed_and_persist_object(
            state,
            job,
            project_id,
            det.bbox,
            det.score,
            "yolov8",
            Some(det.class_id),
            img,
            emb_session,
            thresholds,
        )
        .await?;
        buckets.push(bucket);
    }
    Ok(buckets)
}

/// Helper: run DINOv2-small over a 224x224 letterbox of `bbox`, write a
/// `detections` row + `recognition_items` row, and return the recall bucket.
///
/// Used by the YOLOv8 per-detection branch and by the no-detection fallback
/// of [`run_tool_pipeline`].
#[allow(clippy::too_many_arguments)]
async fn embed_and_persist_object(
    state: &AppState,
    job: &Job,
    project_id: Uuid,
    bbox: (f32, f32, f32, f32),
    det_score: f32,
    source: &str,
    class_id: Option<usize>,
    img: &image::DynamicImage,
    emb_session: &ort::session::Session,
    thresholds: Thresholds,
) -> Result<recall::Bucket> {
    let pool = &state.db;

    // 224x224 DINOv2 input + ImageNet mean/std over the bbox crop.
    let nchw = preprocess::crop_to_nchw(img, bbox, 224, preprocess::Norm::ImageNet);
    let dims = nchw.shape().to_vec();
    let data: Vec<f32> = nchw.iter().copied().collect();
    let pixel_values = ort::value::Tensor::from_array((dims, data))?;
    let outputs = emb_session.run(ort::inputs![pixel_values]?)?;

    // DINOv2-small output: (B=1, 257, 384). Token 0 = CLS embedding.
    let raw_view = outputs["last_hidden_state"].try_extract_tensor::<f32>()?;
    let shape = raw_view.shape().to_vec();
    if shape.len() != 3 || shape[1] == 0 || shape[2] == 0 {
        return Err(anyhow!(
            "DINOv2 output has unexpected shape {:?}; expected (B, T, D)",
            shape
        ));
    }
    let dim = shape[2];
    let mut emb: Vec<f32> = (0..dim).map(|i| raw_view[[0, 0, i]]).collect();
    recall::l2_normalize(&mut emb);
    let emb_512 = recall::pad_to_512(&emb);

    let mut bbox_json = serde_json::json!({
        "x1": bbox.0,
        "y1": bbox.1,
        "x2": bbox.2,
        "y2": bbox.3,
        "source": source,
    });
    if let Some(cid) = class_id {
        bbox_json["class_id"] = serde_json::json!(cid);
    }

    let v_pg = recall::encode_vector(&emb_512);
    let detection_id: i64 = sqlx::query_scalar(
        "INSERT INTO detections \
         (project_id, photo_id, target_type, bbox, score, embedding, match_status) \
         VALUES ($1, $2, 'tool'::detect_target, $3, $4, $5::vector, 'unmatched'::match_status) \
         RETURNING id",
    )
    .bind(project_id)
    .bind(job.photo_id)
    .bind(&bbox_json)
    .bind(det_score)
    .bind(&v_pg)
    .fetch_one(pool)
    .await?;

    let hit = recall::top1_object(pool, &emb_512).await?;
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

    Ok(bucket)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::recall::Bucket;

    #[test]
    fn aggregate_outcome_empty_is_unmatched() {
        assert!(matches!(aggregate_outcome(&[]), Outcome::Unmatched));
    }

    #[test]
    fn aggregate_outcome_prefers_matched() {
        let out = aggregate_outcome(&[Bucket::Unmatched, Bucket::Learning, Bucket::Matched]);
        assert!(matches!(out, Outcome::Matched));
    }

    #[test]
    fn aggregate_outcome_learning_when_no_match() {
        let out = aggregate_outcome(&[Bucket::Unmatched, Bucket::Learning, Bucket::Unmatched]);
        assert!(matches!(out, Outcome::Learning));
    }

    #[test]
    fn aggregate_outcome_unmatched_when_all_unmatched() {
        let out = aggregate_outcome(&[Bucket::Unmatched, Bucket::Unmatched]);
        assert!(matches!(out, Outcome::Unmatched));
    }
}
