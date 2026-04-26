//! Finetune / sample roll-back utility (M2 turn 11).
//!
//! Workflow:
//! 1. Operators correct false positives / missed detections through
//!    `PATCH /api/projects/:pid/recognition_items/:id/correct`. That sets
//!    `recognition_items.status = 'manual_corrected'` and stamps
//!    `corrected_owner_type / corrected_owner_id / corrected_by /
//!    corrected_at`.
//! 2. Each manually-corrected item already has a `detection_id` whose row
//!    in `detections` carries the 512-d embedding the worker computed at
//!    recognition time.
//! 3. Once a month (cron) we want to roll those embeddings back into
//!    `identity_embeddings` so the gallery learns from human feedback. The
//!    new rows are tagged `source = 'manual'` (vs the worker's automatic
//!    `incremental` augmentations) and reference the source photo +
//!    project for traceability.
//!
//! This module is the engine; [`crate::cli`] exposes it as
//! `f1photo finetune stats` (dry-run summary) and `f1photo finetune apply`
//! (idempotent insert).
//!
//! Idempotency: a row is considered already rolled back when an
//! `identity_embeddings` row exists with matching `owner_type`, `owner_id`,
//! `source_photo` and `source = 'manual'`. Re-running `apply` is safe.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Per-owner roll-up of finetune candidates.
#[derive(Debug, Clone, Serialize)]
pub struct OwnerStat {
    pub owner_type: String,
    pub owner_id: Uuid,
    pub candidate_count: i64,
    pub already_rolled_back: i64,
    pub pending: i64,
    pub latest_corrected_at: Option<DateTime<Utc>>,
}

/// Output of [`stats`]. Designed to be both JSON-serialisable (for an
/// eventual `/api/admin/finetune/stats` endpoint) and pretty-printable from
/// the CLI.
#[derive(Debug, Clone, Serialize)]
pub struct FinetuneStats {
    pub since: DateTime<Utc>,
    pub project: Option<Uuid>,
    pub total_candidates: i64,
    pub already_rolled_back: i64,
    pub pending: i64,
    pub owners: Vec<OwnerStat>,
}

/// Output of [`apply`].
#[derive(Debug, Clone, Serialize)]
pub struct FinetuneApplyReport {
    pub since: DateTime<Utc>,
    pub project: Option<Uuid>,
    pub dry_run: bool,
    pub inserted: i64,
    pub skipped_already_present: i64,
    pub skipped_no_embedding: i64,
}

/// Compute per-owner candidate stats without writing anything.
pub async fn stats(
    pool: &PgPool,
    since: DateTime<Utc>,
    project: Option<Uuid>,
) -> Result<FinetuneStats> {
    // candidate set: manual_corrected items in window with non-null corrected owner
    // already_rolled_back: at least one manual identity_embeddings row with same owner + source_photo
    let rows = sqlx::query(
        "SELECT \
             ri.corrected_owner_type::text AS owner_type, \
             ri.corrected_owner_id        AS owner_id, \
             COUNT(*) AS candidate_count, \
             COUNT(*) FILTER (WHERE EXISTS ( \
                 SELECT 1 FROM identity_embeddings ie \
                 WHERE ie.owner_type = ri.corrected_owner_type \
                   AND ie.owner_id   = ri.corrected_owner_id \
                   AND ie.source_photo = ri.photo_id \
                   AND ie.source = 'manual'::embedding_source \
             )) AS already_rolled_back, \
             MAX(ri.corrected_at) AS latest_corrected_at \
         FROM recognition_items ri \
         WHERE ri.status = 'manual_corrected'::match_status \
           AND ri.corrected_owner_type IS NOT NULL \
           AND ri.corrected_owner_id   IS NOT NULL \
           AND ri.corrected_at IS NOT NULL \
           AND ri.corrected_at >= $1 \
           AND ($2::uuid IS NULL OR ri.project_id = $2) \
         GROUP BY ri.corrected_owner_type, ri.corrected_owner_id \
         ORDER BY MAX(ri.corrected_at) DESC NULLS LAST",
    )
    .bind(since)
    .bind(project)
    .fetch_all(pool)
    .await?;

    let mut owners = Vec::with_capacity(rows.len());
    let mut total: i64 = 0;
    let mut total_done: i64 = 0;
    for r in rows {
        let owner_type: String = r.try_get("owner_type")?;
        let owner_id: Uuid = r.try_get("owner_id")?;
        let candidate_count: i64 = r.try_get("candidate_count")?;
        let already_rolled_back: i64 = r.try_get("already_rolled_back")?;
        let latest_corrected_at: Option<DateTime<Utc>> = r.try_get("latest_corrected_at").ok();
        let pending = candidate_count - already_rolled_back;
        total += candidate_count;
        total_done += already_rolled_back;
        owners.push(OwnerStat {
            owner_type,
            owner_id,
            candidate_count,
            already_rolled_back,
            pending,
            latest_corrected_at,
        });
    }

    Ok(FinetuneStats {
        since,
        project,
        total_candidates: total,
        already_rolled_back: total_done,
        pending: total - total_done,
        owners,
    })
}

/// Roll manual-corrected detection embeddings back into
/// `identity_embeddings` with `source = 'manual'`. Idempotent.
pub async fn apply(
    pool: &PgPool,
    since: DateTime<Utc>,
    project: Option<Uuid>,
    dry_run: bool,
) -> Result<FinetuneApplyReport> {
    // Inspect candidate set first to drive the report counters.
    let rows = sqlx::query(
        "SELECT \
             ri.id AS ri_id, \
             ri.project_id, \
             ri.photo_id, \
             ri.corrected_owner_type::text AS owner_type, \
             ri.corrected_owner_id AS owner_id, \
             d.embedding IS NOT NULL AS has_embedding, \
             EXISTS ( \
                 SELECT 1 FROM identity_embeddings ie \
                 WHERE ie.owner_type = ri.corrected_owner_type \
                   AND ie.owner_id   = ri.corrected_owner_id \
                   AND ie.source_photo = ri.photo_id \
                   AND ie.source = 'manual'::embedding_source \
             ) AS already_present \
         FROM recognition_items ri \
         JOIN detections d ON d.id = ri.detection_id \
         WHERE ri.status = 'manual_corrected'::match_status \
           AND ri.corrected_owner_type IS NOT NULL \
           AND ri.corrected_owner_id   IS NOT NULL \
           AND ri.corrected_at IS NOT NULL \
           AND ri.corrected_at >= $1 \
           AND ($2::uuid IS NULL OR ri.project_id = $2)",
    )
    .bind(since)
    .bind(project)
    .fetch_all(pool)
    .await?;

    let mut inserted: i64 = 0;
    let mut skipped_already_present: i64 = 0;
    let mut skipped_no_embedding: i64 = 0;

    for r in &rows {
        let already: bool = r.try_get("already_present")?;
        let has_emb: bool = r.try_get("has_embedding")?;
        if already {
            skipped_already_present += 1;
            continue;
        }
        if !has_emb {
            skipped_no_embedding += 1;
            continue;
        }
        inserted += 1;
    }

    if !dry_run && inserted > 0 {
        // Bulk insert the actually-eligible rows in a single statement so
        // the read-then-write window stays small. The same `WHERE NOT
        // EXISTS` guards inside the INSERT make it safe even if a parallel
        // run sneaks a row in between the SELECT above and this INSERT.
        let res = sqlx::query(
            "INSERT INTO identity_embeddings \
                 (owner_type, owner_id, embedding, source, source_photo, source_project) \
             SELECT ri.corrected_owner_type, ri.corrected_owner_id, d.embedding, \
                    'manual'::embedding_source, ri.photo_id, ri.project_id \
             FROM recognition_items ri \
             JOIN detections d ON d.id = ri.detection_id \
             WHERE ri.status = 'manual_corrected'::match_status \
               AND ri.corrected_owner_type IS NOT NULL \
               AND ri.corrected_owner_id   IS NOT NULL \
               AND ri.corrected_at IS NOT NULL \
               AND ri.corrected_at >= $1 \
               AND ($2::uuid IS NULL OR ri.project_id = $2) \
               AND d.embedding IS NOT NULL \
               AND NOT EXISTS ( \
                   SELECT 1 FROM identity_embeddings ie \
                   WHERE ie.owner_type = ri.corrected_owner_type \
                     AND ie.owner_id   = ri.corrected_owner_id \
                     AND ie.source_photo = ri.photo_id \
                     AND ie.source = 'manual'::embedding_source \
               )",
        )
        .bind(since)
        .bind(project)
        .execute(pool)
        .await?;
        // The actual rows-affected may differ from our pre-count in the
        // (rare) presence of concurrent writers; trust the DB's count.
        inserted = res.rows_affected() as i64;
    } else if dry_run {
        // leave inserted as the projected count
    }

    Ok(FinetuneApplyReport {
        since,
        project,
        dry_run,
        inserted,
        skipped_already_present,
        skipped_no_embedding,
    })
}
