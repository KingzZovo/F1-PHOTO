//! Identity recall (M2 turn 10).
//!
//! Given a 512-d embedding produced by ArcFace (face) or DINOv2-small (object),
//! find the closest match in `identity_embeddings` filtered by `owner_type`.
//!
//! pgvector exposes the `<=>` operator for cosine **distance** in `[0, 2]`.
//! We convert it to a similarity score in `[-1, 1]` (typically `[0, 1]` for
//! normalized embeddings) by `score = 1 - distance`. The thresholds in
//! [`Thresholds`] are interpreted on this similarity space:
//!
//! - `score >= match_lower` → matched
//! - `low_lower <= score < match_lower` → learning (needs human review)
//! - `score < low_lower` → unmatched
//! - `score >= augment_upper` → in addition to matched, augment the gallery
//!
//! Project scoping: identity records (`persons` / `tools` / `devices`) are
//! workspace-global in the v3 schema, so we don't filter by `project_id` on
//! recall — but we do filter out soft-deleted owners via per-target joins,
//! and we record `source_project` on augmented embeddings so the future
//! finetune flow can re-train per-project if needed.

use anyhow::Result;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Threshold trio shared by face and object recall.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub low_lower: f32,
    pub match_lower: f32,
    pub augment_upper: f32,
}

impl Thresholds {
    /// Project defaults from the long-term instructions.
    pub const DEFAULT: Self = Self {
        low_lower: 0.50,
        match_lower: 0.62,
        augment_upper: 0.95,
    };
}

/// Recall outcome bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Matched,
    Learning,
    Unmatched,
}

/// Single recall result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub owner_type: String, // 'person' | 'tool' | 'device'
    pub owner_id: Uuid,
    pub score: f32,
}

impl Hit {
    pub fn bucket(&self, t: Thresholds) -> Bucket {
        if self.score >= t.match_lower {
            Bucket::Matched
        } else if self.score >= t.low_lower {
            Bucket::Learning
        } else {
            Bucket::Unmatched
        }
    }
}

/// Encode a 512-d f32 embedding as the pgvector text literal `[v1,v2,...]`.
/// We don't use the `pgvector` crate dependency to keep the dep tree small;
/// the text form is portable and round-trip safe.
pub fn encode_vector(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        // pgvector accepts standard f32 text; clamp NaN/Inf defensively.
        let safe = if x.is_finite() { *x } else { 0.0 };
        // Use compact decimal; precision 7 is plenty for fp32 cosine.
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{:.7}", safe);
    }
    s.push(']');
    s
}

/// Recall the top-1 identity for a face embedding from the `person` gallery.
/// Returns `None` if there are zero candidates in the project's gallery.
pub async fn top1_face(pool: &PgPool, embedding: &[f32]) -> Result<Option<Hit>> {
    top1_for_owner(pool, "person", embedding).await
}

/// Recall the top-1 identity from `tool`+`device` galleries (objects).
pub async fn top1_object(pool: &PgPool, embedding: &[f32]) -> Result<Option<Hit>> {
    let v = encode_vector(embedding);
    let row = sqlx::query(
        "SELECT ie.owner_type::text AS owner_type, ie.owner_id, \
                1.0 - (ie.embedding <=> $1::vector) AS score \
         FROM identity_embeddings ie \
         LEFT JOIN tools   t ON ie.owner_type = 'tool'   AND t.id = ie.owner_id \
         LEFT JOIN devices d ON ie.owner_type = 'device' AND d.id = ie.owner_id \
         WHERE ie.owner_type IN ('tool','device') \
           AND (ie.owner_type <> 'tool'   OR (t.id IS NOT NULL AND t.deleted_at IS NULL)) \
           AND (ie.owner_type <> 'device' OR (d.id IS NOT NULL AND d.deleted_at IS NULL)) \
         ORDER BY ie.embedding <=> $1::vector \
         LIMIT 1",
    )
    .bind(&v)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Hit {
        owner_type: r.get::<String, _>("owner_type"),
        owner_id: r.get::<Uuid, _>("owner_id"),
        score: row_f32(&r, "score"),
    }))
}

async fn top1_for_owner(pool: &PgPool, owner: &str, embedding: &[f32]) -> Result<Option<Hit>> {
    let v = encode_vector(embedding);
    let row = sqlx::query(
        "SELECT ie.owner_type::text AS owner_type, ie.owner_id, \
                1.0 - (ie.embedding <=> $2::vector) AS score \
         FROM identity_embeddings ie \
         JOIN persons p ON p.id = ie.owner_id \
         WHERE ie.owner_type::text = $1 \
           AND p.deleted_at IS NULL \
         ORDER BY ie.embedding <=> $2::vector \
         LIMIT 1",
    )
    .bind(owner)
    .bind(&v)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Hit {
        owner_type: r.get::<String, _>("owner_type"),
        owner_id: r.get::<Uuid, _>("owner_id"),
        score: row_f32(&r, "score"),
    }))
}

/// pgvector returns the `<=>` distance as `double precision`. sqlx maps that
/// to `f64`. Cast to `f32` defensively.
fn row_f32(row: &sqlx::postgres::PgRow, col: &str) -> f32 {
    row.try_get::<f64, _>(col)
        .map(|d| d as f32)
        .or_else(|_| row.try_get::<f32, _>(col))
        .unwrap_or(0.0)
}

/// Insert an `incremental` augment row into `identity_embeddings`.
/// Used when a matched score >= [`Thresholds::augment_upper`] so the gallery
/// adapts to angle/lighting drift over time.
pub async fn augment(
    pool: &PgPool,
    owner_type: &str,
    owner_id: Uuid,
    embedding: &[f32],
    source_photo: Uuid,
    source_project: Uuid,
) -> Result<()> {
    let v = encode_vector(embedding);
    sqlx::query(
        "INSERT INTO identity_embeddings \
         (owner_type, owner_id, embedding, source, source_photo, source_project) \
         VALUES ($1::owner_type, $2, $3::vector, 'incremental'::embedding_source, $4, $5)",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(&v)
    .bind(source_photo)
    .bind(source_project)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_round_trip_smoke() {
        let v = encode_vector(&[0.0, 1.0, -1.0, 0.5, f32::NAN, f32::INFINITY]);
        assert!(v.starts_with('['));
        assert!(v.ends_with(']'));
        // NaN / Inf must be sanitised to 0
        assert!(v.contains("0.0000000"));
        assert!(!v.contains("NaN"));
        assert!(!v.contains("inf"));
    }

    #[test]
    fn bucket_thresholds() {
        let t = Thresholds::DEFAULT;
        let mk = |s: f32| Hit {
            owner_type: "person".into(),
            owner_id: Uuid::nil(),
            score: s,
        };
        assert_eq!(mk(0.95).bucket(t), Bucket::Matched);
        assert_eq!(mk(0.62).bucket(t), Bucket::Matched);
        assert_eq!(mk(0.61).bucket(t), Bucket::Learning);
        assert_eq!(mk(0.50).bucket(t), Bucket::Learning);
        assert_eq!(mk(0.49).bucket(t), Bucket::Unmatched);
        assert_eq!(mk(0.0).bucket(t), Bucket::Unmatched);
    }
}
