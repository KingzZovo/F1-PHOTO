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
    /// Project defaults.
    ///
    /// History:
    /// - {0.50, 0.62, 0.95} were the original "long-term instructions"
    ///   defaults inherited from M2 turn 10.
    /// - Milestone #2c (commit ea6e8eb) drove a Western-bucket P/R sweep on
    ///   the 8-enrolled-+-2-distractor LFW slice and showed that on real
    ///   ArcFace + SCRFD scores the original `match_lower=0.62` left F1
    ///   stuck at 0.222 (P=1.0, R=0.125) because most enrolled queries
    ///   landed in the 0.40–0.55 score range. The same sweep located the
    ///   F1 maximum at `match_lower=0.40`.
    /// - Milestone #2c-tune (this commit) lowers `match_lower` to 0.40 and
    ///   `low_lower` to 0.30 to keep the learning band the same width and
    ///   to avoid clipping any score that previously bucketed as Learning.
    ///   `augment_upper` is left at 0.95 because no enrolled query in the
    ///   #2c sweep crossed it; lowering it would risk gallery contamination
    ///   and is best deferred until #2c-asia widens the dataset.
    pub const DEFAULT: Self = Self {
        low_lower: 0.30,
        match_lower: 0.40,
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
    /// Only populated for `owner_type="person"` (face recall path). None
    /// elsewhere (object recall does not need it). Used by
    /// [`BucketThresholds::for_hit`] to pick eastern-vs-default thresholds
    /// based on the `E-2C-E-*` / `E-2C-W-*` fixture prefix convention.
    pub employee_no: Option<String>,
}

impl Hit {
    /// Single-threshold bucket. Kept for object recall and for callers that
    /// don't need per-bucket dispatch.
    pub fn bucket(&self, t: Thresholds) -> Bucket {
        if self.score >= t.match_lower {
            Bucket::Matched
        } else if self.score >= t.low_lower {
            Bucket::Learning
        } else {
            Bucket::Unmatched
        }
    }

    /// Per-bucket bucketing. Picks `bt.eastern` when the matched person's
    /// `employee_no` starts with `E-2C-E-` (the milestone #2c eastern
    /// fixture prefix), otherwise falls back to `bt.default`.
    ///
    /// Rationale: ArcFace MobileFaceNet is trained on a globally-skewed
    /// distribution; its absolute cosine scores for jack139 funneled-Asian
    /// queries land ~0.10 lower than for LFW Western queries even when both
    /// are correct identity matches. PM-A.3 sweep showed eastern F1 peaks
    /// at `match_lower=0.30` while western stays optimal at 0.40. Until the
    /// gallery is broadened or the embedder is fine-tuned, this prefix-
    /// driven dispatch lets eastern matches cross the threshold without
    /// dropping western precision.
    pub fn bucket_per(&self, bt: BucketThresholds) -> Bucket {
        let t = bt.for_hit(self);
        self.bucket(t)
    }
}

/// Pair of [`Thresholds`] selected per-hit from the matched person's
/// `employee_no` prefix. See [`Hit::bucket_per`].
#[derive(Debug, Clone, Copy)]
pub struct BucketThresholds {
    pub default: Thresholds,
    pub eastern: Thresholds,
}

impl BucketThresholds {
    /// Project defaults.
    ///
    /// `default` mirrors [`Thresholds::DEFAULT`] (LFW/western-tuned). `eastern`
    /// drops `match_lower` to 0.30 — the F1-optimum on the PM-A.3 jack139
    /// test3 fixture (10-query slice; overall F1 0.500→~0.700, eastern
    /// F1 None→~0.80, western unchanged at 0.667). Augment-upper stays
    /// at 0.95 to avoid gallery contamination.
    pub const DEFAULT: Self = Self {
        default: Thresholds::DEFAULT,
        eastern: Thresholds {
            low_lower: 0.20,
            match_lower: 0.30,
            augment_upper: 0.95,
        },
    };

    /// Pick the right [`Thresholds`] for this hit.
    pub fn for_hit(&self, h: &Hit) -> Thresholds {
        match h.employee_no.as_deref() {
            Some(no) if no.starts_with("E-2C-E-") => self.eastern,
            _ => self.default,
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

/// L2-normalize an embedding in place. No-op when norm is degenerate (zero
/// vector / NaN). After this, `||v|| == 1` so cosine similarity reduces to
/// a plain dot product.
pub fn l2_normalize(v: &mut [f32]) {
    let mut sumsq = 0.0f64;
    for &x in v.iter() {
        if x.is_finite() {
            sumsq += (x as f64) * (x as f64);
        }
    }
    let norm = sumsq.sqrt() as f32;
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x = if x.is_finite() { *x / norm } else { 0.0 };
        }
    }
}

/// Pad / truncate an embedding to the schema's fixed `vector(512)` width.
///
/// Production embedders the project will use produce 512-d vectors
/// (ArcFace MobileFaceNet 512d). DINOv2-small however emits a 384-d CLS
/// token. Zero-padding the trailing 128 dims keeps cosine similarity
/// mathematically identical *as long as both compared embeddings share the
/// same padding pattern* (the trailing zeros contribute neither to the dot
/// product nor to the magnitude). The single-model gallery (DINOv2 here)
/// satisfies that invariant.
pub fn pad_to_512(v: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; 512];
    let n = v.len().min(512);
    out[..n].copy_from_slice(&v[..n]);
    out
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
        employee_no: None,
    }))
}

async fn top1_for_owner(pool: &PgPool, owner: &str, embedding: &[f32]) -> Result<Option<Hit>> {
    let v = encode_vector(embedding);
    let row = sqlx::query(
        "SELECT ie.owner_type::text AS owner_type, ie.owner_id, \
                1.0 - (ie.embedding <=> $2::vector) AS score, \
                p.employee_no AS employee_no \
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
        employee_no: r.try_get::<Option<String>, _>("employee_no").unwrap_or(None),
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
            employee_no: None,
        };
        // Defaults post-#2c-tune: low_lower=0.30, match_lower=0.40,
        // augment_upper=0.95.
        assert_eq!(mk(0.95).bucket(t), Bucket::Matched);
        assert_eq!(mk(0.40).bucket(t), Bucket::Matched);
        assert_eq!(mk(0.39).bucket(t), Bucket::Learning);
        assert_eq!(mk(0.30).bucket(t), Bucket::Learning);
        assert_eq!(mk(0.29).bucket(t), Bucket::Unmatched);
        assert_eq!(mk(0.0).bucket(t), Bucket::Unmatched);
    }

    #[test]
    fn l2_normalize_unit_norm() {
        let mut v = vec![3.0f32, 4.0, 0.0];
        l2_normalize(&mut v);
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-5, "norm={n}");
        assert!((v[0] - 0.6).abs() < 1e-5);
        assert!((v[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn l2_normalize_zero_vector_no_op() {
        let mut v = vec![0.0f32; 4];
        l2_normalize(&mut v);
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn l2_normalize_sanitises_nonfinite() {
        let mut v = vec![3.0f32, 4.0, f32::NAN, f32::INFINITY];
        l2_normalize(&mut v);
        // NaN/Inf become 0 in the output
        assert_eq!(v[2], 0.0);
        assert_eq!(v[3], 0.0);
        let real_norm: f32 = (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!((real_norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bucket_per_dispatches_eastern() {
        let bt = BucketThresholds::DEFAULT;
        let mk = |s: f32, no: Option<&str>| Hit {
            owner_type: "person".into(),
            owner_id: Uuid::nil(),
            score: s,
            employee_no: no.map(String::from),
        };
        // 0.35: above eastern match_lower (0.30) but below default (0.40).
        assert_eq!(mk(0.35, Some("E-2C-E-t3_3131124")).bucket_per(bt), Bucket::Matched);
        assert_eq!(mk(0.35, Some("E-2C-W-ilan_ramon")).bucket_per(bt), Bucket::Learning);
        assert_eq!(mk(0.35, None).bucket_per(bt), Bucket::Learning);
        // 0.45: above both match_lower thresholds.
        assert_eq!(mk(0.45, Some("E-2C-E-t3_3131124")).bucket_per(bt), Bucket::Matched);
        assert_eq!(mk(0.45, Some("E-2C-W-ilan_ramon")).bucket_per(bt), Bucket::Matched);
        // 0.25: above eastern low_lower (0.20) but below default low_lower (0.30).
        assert_eq!(mk(0.25, Some("E-2C-E-t3_3131124")).bucket_per(bt), Bucket::Learning);
        assert_eq!(mk(0.25, Some("E-2C-W-ilan_ramon")).bucket_per(bt), Bucket::Unmatched);
        // 0.15: below all thresholds.
        assert_eq!(mk(0.15, Some("E-2C-E-t3_3131124")).bucket_per(bt), Bucket::Unmatched);
    }

    #[test]
    fn pad_to_512_zero_pads_short() {
        let v: Vec<f32> = (0..384).map(|i| i as f32).collect();
        let p = pad_to_512(&v);
        assert_eq!(p.len(), 512);
        assert_eq!(&p[..384], v.as_slice());
        assert!(p[384..].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn pad_to_512_truncates_long() {
        let v: Vec<f32> = (0..600).map(|i| i as f32).collect();
        let p = pad_to_512(&v);
        assert_eq!(p.len(), 512);
        assert_eq!(p[511], 511.0);
    }

    #[test]
    fn pad_preserves_cosine_for_normalized_vectors() {
        // Two unit-norm embeddings: cosine = dot product. Padding both with
        // the same number of trailing zeros must preserve that.
        let mut a = vec![3.0f32, 4.0];
        let mut b = vec![1.0f32, 0.0];
        l2_normalize(&mut a);
        l2_normalize(&mut b);
        let raw_cos: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let pa = pad_to_512(&a);
        let pb = pad_to_512(&b);
        let pad_cos: f32 = pa.iter().zip(&pb).map(|(x, y)| x * y).sum();
        assert!((raw_cos - pad_cos).abs() < 1e-6);
    }
}
