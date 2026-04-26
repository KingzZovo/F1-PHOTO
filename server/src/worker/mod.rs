//! Recognition queue worker (M1 turn 7 — skeleton).
//!
//! Responsibilities:
//! 1. LISTEN on the Postgres `recognition_queue` channel for wake-up signals.
//! 2. Drain the `recognition_queue` table using `FOR UPDATE SKIP LOCKED`.
//! 3. Run the per-photo pipeline (currently a stub: `pending` -> `processing`
//!    -> `unmatched`, since no models are wired up yet).
//! 4. Track `attempts` / `last_error`, exponential backoff via `locked_until`,
//!    and mark the photo `failed` after [`MAX_ATTEMPTS`] tries.
//!
//! `moka` LRU caches are constructed up-front and threaded through; they are
//! placeholders for the embedding caches that turn 10 (real inference) will
//! actually populate.
//!
//! The whole worker is started with [`spawn`] from `main.rs::serve`. It runs as
//! a single-task drainer for now; horizontal concurrency can be added later.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use moka::future::Cache;
use sqlx::{PgPool, Row, postgres::PgListener};
use tokio::time::sleep;
use uuid::Uuid;

use crate::api::AppState;

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

/// Caches available to the recognition pipeline. Currently unused by the
/// skeleton; turn 10 will read/write these to skip redundant model invocations.
#[derive(Clone)]
pub struct WorkerCaches {
    /// person_id -> 512d face embedding (placeholder type).
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
///
/// Errors during startup or the main loop are logged but never propagated; the
/// API server keeps running even if the worker stops. (A future iteration may
/// expose a healthz field for worker liveness.)
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
    tracing::info!("recognition worker listening on 'recognition_queue'");

    loop {
        // Drain anything currently due.
        loop {
            match claim_one(&pool).await? {
                Some(job) => {
                    let job_id = job.id;
                    let photo_id = job.photo_id;
                    if let Err(e) = process_job(&pool, &job).await {
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
                None => break,
            }
        }

        // Wait for either a NOTIFY or the idle poll tick.
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

/// Atomically pick the oldest non-locked row, bump `attempts`, and lease it
/// to this worker for [`LOCK_LEASE`].
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
/// SKELETON: real inference (SCRFD / ArcFace / YOLOv8n / DINOv2-small) lands
/// in M2 turn 10. For now we just shepherd the photo through the state
/// machine `pending` -> `processing` -> `unmatched` and remove the queue row.
async fn process_job(pool: &PgPool, job: &Job) -> Result<()> {
    sqlx::query(
        "UPDATE photos SET status = 'processing'::photo_status, updated_at = now() \
         WHERE id = $1",
    )
    .bind(job.photo_id)
    .execute(pool)
    .await?;

    // Simulate a tiny bit of work so concurrent uploads can interleave.
    sleep(Duration::from_millis(20)).await;

    sqlx::query(
        "UPDATE photos SET status = 'unmatched'::photo_status, updated_at = now() \
         WHERE id = $1",
    )
    .bind(job.photo_id)
    .execute(pool)
    .await?;

    sqlx::query("DELETE FROM recognition_queue WHERE id = $1")
        .bind(job.id)
        .execute(pool)
        .await?;

    tracing::debug!(
        job_id = job.id,
        photo_id = %job.photo_id,
        project_id = %job.project_id,
        "job processed (skeleton: status=unmatched)"
    );
    Ok(())
}

async fn record_failure(pool: &PgPool, job: &Job, msg: &str) -> Result<()> {
    if job.attempts >= MAX_ATTEMPTS {
        // Give up: mark photo failed and drop the queue row.
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

    // Exponential-ish backoff: 5s, 20s, 45s, 80s.
    let backoff_secs: i32 = 5 * (job.attempts as i32).pow(2).max(1);
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
