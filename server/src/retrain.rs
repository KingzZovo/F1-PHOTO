//! Detector-retrain training-set materialiser (milestone #7a).
//!
//! Workflow:
//! 1. Operators correct false positives via
//!    `PATCH /api/projects/:pid/recognition_items/:id/correct`. The
//!    `recognition_items.corrected_owner_*` columns + the
//!    `v_training_corrections` view (added in milestone #5-skel) expose
//!    every manual-corrected sample joined with its detection bbox +
//!    photo path + photo dimensions.
//! 2. `f1photo retrain-detector prepare --min-corrections N --since DATE`
//!    reads the view, materialises a YOLO-format dataset under
//!    `<training_dir>/cycle-<unix_ts>/{images,labels,data.yaml,
//!    metadata.json}`, and stops there. The actual `yolo train` + ONNX
//!    export + shadow-eval gate land in milestones #7b / #7c.
//! 3. The dataset is single-class (id 0 = `tool`) per the #5-skel scope
//!    revision: `device` is folded into `tool` for new writes.
//!
//! Bbox conversion: the worker stores `detections.bbox` as JSON
//! `{"x1","y1","x2","y2"}` in pixel coordinates (top-left + bottom-right
//! corners). YOLO expects normalised `cx cy w h` in [0,1]. Boxes that
//! clip to <1 px on either axis after clamping to the image bounds are
//! rejected as degenerate.
//!
//! `prepare` is best-effort idempotent across reruns: each cycle gets a
//! distinct timestamp directory; reruns simply produce a new cycle. No
//! state is mutated in the database (the `model_versions` audit row is
//! written by milestone #7c at promote time, not here).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use uuid::Uuid;

/// Aggregate roll-up emitted by [`stats`].
#[derive(Debug, Clone, Serialize)]
pub struct RetrainStats {
    pub since: DateTime<Utc>,
    pub min_score: f64,
    pub total: i64,
    pub by_owner_type: Vec<RetrainOwnerStat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrainOwnerStat {
    pub owner_type: String,
    pub count: i64,
}

/// Outcome of [`prepare`]. Designed to be both JSON-serialisable (for an
/// eventual `/api/admin/retrain/prepare` endpoint) and pretty-printable
/// from the CLI.
#[derive(Debug, Clone, Serialize)]
pub struct RetrainPrepareReport {
    pub since: DateTime<Utc>,
    pub min_score: f64,
    pub min_corrections: i64,
    pub dry_run: bool,
    pub cycle_dir: Option<PathBuf>,
    pub eligible: i64,
    pub written: i64,
    pub skipped_no_dimensions: i64,
    pub skipped_degenerate_bbox: i64,
    pub skipped_missing_photo: i64,
    pub below_threshold: bool,
}

/// Sidecar `metadata.json` written into each cycle directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleMetadata {
    pub cycle_id: String,
    pub prepared_at: DateTime<Utc>,
    pub since: DateTime<Utc>,
    pub min_score: f64,
    pub min_corrections: i64,
    pub class_names: Vec<String>,
    pub count: usize,
    pub items: Vec<CycleItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleItem {
    pub recognition_item_id: Uuid,
    pub photo_id: Uuid,
    pub detection_id: Uuid,
    pub corrected_owner_type: String,
    pub corrected_owner_id: Uuid,
    pub corrected_at: DateTime<Utc>,
    pub detection_score: Option<f64>,
    pub photo_hash: String,
    pub photo_width: i32,
    pub photo_height: i32,
    pub bbox_pixel: BboxPixel,
    pub bbox_yolo: BboxYolo,
    pub image_filename: String,
    pub label_filename: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BboxPixel {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BboxYolo {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
}

/// Convert a pixel-corner bbox to YOLO normalised cx/cy/w/h. Returns
/// `None` if the box clips to less than 1 px on either axis after
/// clamping to image bounds, or if either image dimension is
/// non-positive.
pub fn bbox_to_yolo(b: &BboxPixel, img_w: i32, img_h: i32) -> Option<BboxYolo> {
    if img_w <= 0 || img_h <= 0 {
        return None;
    }
    let w = img_w as f64;
    let h = img_h as f64;
    let x1 = b.x1.clamp(0.0, w);
    let y1 = b.y1.clamp(0.0, h);
    let x2 = b.x2.clamp(0.0, w);
    let y2 = b.y2.clamp(0.0, h);
    let bw = (x2 - x1).max(0.0);
    let bh = (y2 - y1).max(0.0);
    if bw < 1.0 || bh < 1.0 {
        return None;
    }
    Some(BboxYolo {
        cx: (x1 + bw / 2.0) / w,
        cy: (y1 + bh / 2.0) / h,
        w: bw / w,
        h: bh / h,
    })
}

fn parse_bbox(v: &serde_json::Value) -> Option<BboxPixel> {
    let obj = v.as_object()?;
    let g = |k: &str| obj.get(k).and_then(|x| x.as_f64());
    Some(BboxPixel {
        x1: g("x1")?,
        y1: g("y1")?,
        x2: g("x2")?,
        y2: g("y2")?,
    })
}

/// Per-owner-type roll-up of correction candidates without writing
/// anything. Reads from the `v_training_corrections` view added in
/// milestone #5-skel.
pub async fn stats(pool: &PgPool, since: DateTime<Utc>, min_score: f64) -> Result<RetrainStats> {
    let rows = sqlx::query(
        "SELECT corrected_owner_type, COUNT(*) AS cnt \
         FROM v_training_corrections \
         WHERE corrected_at >= $1 AND COALESCE(detection_score, 0) >= $2 \
         GROUP BY corrected_owner_type \
         ORDER BY corrected_owner_type",
    )
    .bind(since)
    .bind(min_score)
    .fetch_all(pool)
    .await?;
    let mut total: i64 = 0;
    let mut by_owner_type = Vec::with_capacity(rows.len());
    for r in rows {
        let owner_type: String = r.try_get("corrected_owner_type")?;
        let cnt: i64 = r.try_get("cnt")?;
        total += cnt;
        by_owner_type.push(RetrainOwnerStat {
            owner_type,
            count: cnt,
        });
    }
    Ok(RetrainStats {
        since,
        min_score,
        total,
        by_owner_type,
    })
}

/// Materialise a YOLO training-set `cycle-<ts>/` under `training_dir`.
///
/// If fewer than `min_corrections` eligible rows are available, returns
/// early with `below_threshold = true` and writes nothing. This is the
/// expected outcome before operators have accumulated enough corrections.
pub async fn prepare(
    pool: &PgPool,
    data_dir: &Path,
    training_dir: &Path,
    since: DateTime<Utc>,
    min_score: f64,
    min_corrections: i64,
    dry_run: bool,
) -> Result<RetrainPrepareReport> {
    let rows = sqlx::query(
        "SELECT recognition_item_id, photo_id, detection_id, \
                corrected_owner_type, corrected_owner_id, corrected_at, \
                bbox, detection_score, photo_path, photo_hash, \
                photo_width, photo_height \
         FROM v_training_corrections \
         WHERE corrected_at >= $1 AND COALESCE(detection_score, 0) >= $2 \
         ORDER BY corrected_at",
    )
    .bind(since)
    .bind(min_score)
    .fetch_all(pool)
    .await?;
    let eligible = rows.len() as i64;
    if eligible < min_corrections {
        return Ok(RetrainPrepareReport {
            since,
            min_score,
            min_corrections,
            dry_run,
            cycle_dir: None,
            eligible,
            written: 0,
            skipped_no_dimensions: 0,
            skipped_degenerate_bbox: 0,
            skipped_missing_photo: 0,
            below_threshold: true,
        });
    }

    struct Pending {
        item: CycleItem,
        photo_rel: String,
    }
    let mut pending: Vec<Pending> = Vec::new();
    let mut skipped_no_dimensions = 0i64;
    let mut skipped_degenerate_bbox = 0i64;
    let mut skipped_missing_photo = 0i64;

    for r in &rows {
        let pw_opt: Option<i32> = r.try_get("photo_width").ok();
        let ph_opt: Option<i32> = r.try_get("photo_height").ok();
        let (pw, ph) = match (pw_opt, ph_opt) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
            _ => {
                skipped_no_dimensions += 1;
                continue;
            }
        };
        let bbox_json: serde_json::Value = r.try_get("bbox")?;
        let bp = match parse_bbox(&bbox_json) {
            Some(b) => b,
            None => {
                skipped_degenerate_bbox += 1;
                continue;
            }
        };
        let by = match bbox_to_yolo(&bp, pw, ph) {
            Some(b) => b,
            None => {
                skipped_degenerate_bbox += 1;
                continue;
            }
        };
        let photo_rel: String = r.try_get("photo_path")?;
        let abs = data_dir.join(&photo_rel);
        if !abs.is_file() {
            skipped_missing_photo += 1;
            continue;
        }
        let recognition_item_id: Uuid = r.try_get("recognition_item_id")?;
        let photo_id: Uuid = r.try_get("photo_id")?;
        let detection_id: Uuid = r.try_get("detection_id")?;
        let owner_type: String = r.try_get("corrected_owner_type")?;
        let owner_id: Uuid = r.try_get("corrected_owner_id")?;
        let corrected_at: DateTime<Utc> = r.try_get("corrected_at")?;
        let detection_score: Option<f64> = r.try_get("detection_score").ok();
        let photo_hash: String = r.try_get("photo_hash")?;
        let ext = Path::new(&photo_rel)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("jpg")
            .to_lowercase();
        let stem = recognition_item_id.to_string();
        pending.push(Pending {
            item: CycleItem {
                recognition_item_id,
                photo_id,
                detection_id,
                corrected_owner_type: owner_type,
                corrected_owner_id: owner_id,
                corrected_at,
                detection_score,
                photo_hash,
                photo_width: pw,
                photo_height: ph,
                bbox_pixel: bp,
                bbox_yolo: by,
                image_filename: format!("{stem}.{ext}"),
                label_filename: format!("{stem}.txt"),
            },
            photo_rel,
        });
    }

    if (pending.len() as i64) < min_corrections {
        return Ok(RetrainPrepareReport {
            since,
            min_score,
            min_corrections,
            dry_run,
            cycle_dir: None,
            eligible,
            written: 0,
            skipped_no_dimensions,
            skipped_degenerate_bbox,
            skipped_missing_photo,
            below_threshold: true,
        });
    }

    let cycle_id = format!("cycle-{}", Utc::now().timestamp());
    let cycle_dir = training_dir.join(&cycle_id);

    if dry_run {
        return Ok(RetrainPrepareReport {
            since,
            min_score,
            min_corrections,
            dry_run,
            cycle_dir: Some(cycle_dir),
            eligible,
            written: pending.len() as i64,
            skipped_no_dimensions,
            skipped_degenerate_bbox,
            skipped_missing_photo,
            below_threshold: false,
        });
    }

    let images_dir = cycle_dir.join("images");
    let labels_dir = cycle_dir.join("labels");
    fs::create_dir_all(&images_dir).context("create images/")?;
    fs::create_dir_all(&labels_dir).context("create labels/")?;

    let mut written: i64 = 0;
    for p in &pending {
        let src = data_dir.join(&p.photo_rel);
        let dst_img = images_dir.join(&p.item.image_filename);
        let dst_label = labels_dir.join(&p.item.label_filename);
        // Hard-link first to save disk; fall back to copy on cross-fs.
        if fs::hard_link(&src, &dst_img).is_err() {
            fs::copy(&src, &dst_img).with_context(|| {
                format!("copy photo {} -> {}", src.display(), dst_img.display())
            })?;
        }
        let label_text = format!(
            "0 {:.6} {:.6} {:.6} {:.6}\n",
            p.item.bbox_yolo.cx, p.item.bbox_yolo.cy, p.item.bbox_yolo.w, p.item.bbox_yolo.h,
        );
        fs::write(&dst_label, label_text)
            .with_context(|| format!("write label {}", dst_label.display()))?;
        written += 1;
    }

    let metadata = CycleMetadata {
        cycle_id: cycle_id.clone(),
        prepared_at: Utc::now(),
        since,
        min_score,
        min_corrections,
        class_names: vec!["tool".to_string()],
        count: pending.len(),
        items: pending.iter().map(|p| p.item.clone()).collect(),
    };
    let meta_json = serde_json::to_string_pretty(&metadata)?;
    fs::write(cycle_dir.join("metadata.json"), meta_json)?;

    let yaml = format!(
        "# Generated by f1photo retrain-detector prepare\n\
         path: {}\n\
         train: images\n\
         val: images\n\
         nc: 1\n\
         names:\n  - tool\n",
        cycle_dir.display(),
    );
    fs::write(cycle_dir.join("data.yaml"), yaml)?;

    Ok(RetrainPrepareReport {
        since,
        min_score,
        min_corrections,
        dry_run,
        cycle_dir: Some(cycle_dir),
        eligible,
        written,
        skipped_no_dimensions,
        skipped_degenerate_bbox,
        skipped_missing_photo,
        below_threshold: false,
    })
}

/// Owned parameter bundle for [`train`].
///
/// Mirrors the argparse surface of `tools/retrain_train.py` so the
/// Rust CLI subcommand `f1photo retrain-detector train` is a thin
/// wrapper that forwards strongly-typed values into the python
/// fine-tune pipeline.
#[derive(Debug, Clone)]
pub struct TrainParams {
    pub cycle_dir: PathBuf,
    pub base_weights: String,
    pub epochs: u32,
    pub imgsz: u32,
    pub export_imgsz: u32,
    pub freeze: u32,
    pub batch: u32,
    pub workers: u32,
    pub device: String,
    pub runs_dir: PathBuf,
    pub run_name: String,
    pub candidate_out: PathBuf,
    pub opset: u32,
    pub summary_out: PathBuf,
    pub python: PathBuf,
    pub script: PathBuf,
}

/// Structured JSON report written by `tools/retrain_train.py` to its
/// `--summary-out` path. Kept permissive (`#[serde(default)]` on
/// optional-ish fields) so future python additions don't break the
/// Rust deserialiser; only `status` and `output_shape` are load-bearing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrainTrainReport {
    pub status: String,
    #[serde(default)]
    pub cycle_dir: String,
    #[serde(default)]
    pub base_weights: String,
    #[serde(default)]
    pub epochs: u32,
    #[serde(default)]
    pub imgsz: u32,
    #[serde(default)]
    pub export_imgsz: u32,
    #[serde(default)]
    pub freeze: u32,
    #[serde(default)]
    pub batch: u32,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub run_dir: String,
    #[serde(default)]
    pub best_pt: String,
    #[serde(default)]
    pub onnx_export: String,
    #[serde(default)]
    pub candidate_out: String,
    #[serde(default)]
    pub candidate_size_bytes: u64,
    pub output_shape: Vec<i64>,
    #[serde(default)]
    pub train_seconds: f64,
    #[serde(default)]
    pub export_seconds: f64,
}

/// Build the argv that `tools/retrain_train.py` should be invoked with
/// (without the leading interpreter; that is `params.python`). Pure
/// function; no I/O. Exposed for unit tests so we can lock in the CLI
/// surface without spawning python.
pub fn build_train_args(params: &TrainParams) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::with_capacity(32);
    a.push(params.script.as_os_str().to_owned());
    a.push(OsString::from("--cycle-dir"));
    a.push(params.cycle_dir.as_os_str().to_owned());
    a.push(OsString::from("--base-weights"));
    a.push(OsString::from(&params.base_weights));
    a.push(OsString::from("--epochs"));
    a.push(OsString::from(params.epochs.to_string()));
    a.push(OsString::from("--imgsz"));
    a.push(OsString::from(params.imgsz.to_string()));
    a.push(OsString::from("--export-imgsz"));
    a.push(OsString::from(params.export_imgsz.to_string()));
    a.push(OsString::from("--freeze"));
    a.push(OsString::from(params.freeze.to_string()));
    a.push(OsString::from("--batch"));
    a.push(OsString::from(params.batch.to_string()));
    a.push(OsString::from("--workers"));
    a.push(OsString::from(params.workers.to_string()));
    a.push(OsString::from("--device"));
    a.push(OsString::from(&params.device));
    a.push(OsString::from("--runs-dir"));
    a.push(params.runs_dir.as_os_str().to_owned());
    a.push(OsString::from("--run-name"));
    a.push(OsString::from(&params.run_name));
    a.push(OsString::from("--candidate-out"));
    a.push(params.candidate_out.as_os_str().to_owned());
    a.push(OsString::from("--opset"));
    a.push(OsString::from(params.opset.to_string()));
    a.push(OsString::from("--summary-out"));
    a.push(params.summary_out.as_os_str().to_owned());
    a
}

/// Spawn `python tools/retrain_train.py ...`, wait for it to exit, then
/// parse the JSON `--summary-out` file. Errors are bubbled with rich
/// context so operators can read stdout/stderr inherited from the
/// child process and combine that with the structured report.
///
/// This is intentionally a synchronous (`std::process::Command`)
/// implementation: training runs for minutes-to-hours and the parent
/// CLI process has nothing else to do. Callers in async contexts can
/// wrap with `tokio::task::spawn_blocking`.
pub fn train(params: &TrainParams) -> Result<RetrainTrainReport> {
    // Sanity-check the cycle directory before paying for ultralytics startup.
    let data_yaml = params.cycle_dir.join("data.yaml");
    if !data_yaml.is_file() {
        anyhow::bail!(
            "cycle-dir is not a valid YOLO cycle (missing data.yaml): {}",
            data_yaml.display()
        );
    }
    if let Some(parent) = params.summary_out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create summary-out parent {}", parent.display()))?;
        }
    }
    // Clear any stale summary so we can detect a child that exits 0
    // without writing one (e.g. crashed before reaching the writer).
    let _ = fs::remove_file(&params.summary_out);

    let argv = build_train_args(params);
    let mut cmd = StdCommand::new(&params.python);
    cmd.args(&argv);
    // Inherit stdout/stderr so operators can watch ultralytics' progress
    // bars in real time. Structured data goes to --summary-out, not stdout.
    let status = cmd.status().with_context(|| {
        format!(
            "spawn {} {}",
            params.python.display(),
            params.script.display()
        )
    })?;
    if !status.success() {
        anyhow::bail!(
            "retrain_train.py exited with status {} ({} {})",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".into()),
            params.python.display(),
            params.script.display(),
        );
    }
    if !params.summary_out.is_file() {
        anyhow::bail!(
            "retrain_train.py exited 0 but did not write summary file: {}",
            params.summary_out.display()
        );
    }
    let body = fs::read_to_string(&params.summary_out)
        .with_context(|| format!("read summary {}", params.summary_out.display()))?;
    let report: RetrainTrainReport = serde_json::from_str(&body)
        .with_context(|| format!("parse summary JSON {}", params.summary_out.display()))?;
    if report.status != "ok" {
        anyhow::bail!(
            "retrain_train.py reported status={:?} (expected \"ok\")",
            report.status
        );
    }
    if report.output_shape.len() != 3
        || report.output_shape[0] != 1
        || report.output_shape[2] != crate::inference::yolov8::NUM_ANCHORS as i64
    {
        anyhow::bail!(
            "retrain_train.py produced unexpected output_shape {:?} (expected [1, 4+nc, {}])",
            report.output_shape,
            crate::inference::yolov8::NUM_ANCHORS,
        );
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Milestone #7c-skel: candidate -> live promotion + model_versions audit row.
// ---------------------------------------------------------------------------

/// Hash + size snapshot of an ONNX file ready for promotion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateFingerprint {
    pub path: PathBuf,
    pub sha256: String,
    pub file_size: i64,
}

/// Hash a file fully into hex sha256 + size. Errors if the file is empty
/// (a 0-byte ONNX is never a valid promotion candidate).
pub fn fingerprint_file(path: &Path) -> Result<CandidateFingerprint> {
    let bytes = fs::read(path).with_context(|| format!("read candidate {}", path.display()))?;
    if bytes.is_empty() {
        anyhow::bail!("candidate file is empty: {}", path.display());
    }
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let sha256 = format!("{:x}", hasher.finalize());
    let file_size = bytes.len() as i64;
    Ok(CandidateFingerprint {
        path: path.to_path_buf(),
        sha256,
        file_size,
    })
}

/// Filename used to archive the *previous* live model when a new cycle K
/// is being promoted. Example: `object_detect.v3.onnx` for the model that
/// was live as cycle 3 just before cycle 4 was promoted.
pub fn history_filename(kind: &str, prev_cycle: i64) -> String {
    format!("{}.v{}.onnx", kind, prev_cycle.max(0))
}

/// Inputs to [`plan_promote`] / [`execute_filesystem_promote`].
#[derive(Debug, Clone)]
pub struct PromoteParams {
    pub candidate: PathBuf,
    pub models_dir: PathBuf,
    pub kind: String,
    pub cycle_dir: Option<PathBuf>,
    pub notes: Option<String>,
    pub dry_run: bool,
}

/// Pre-flight plan describing what a promotion is about to do, computed
/// without mutating the filesystem (apart from reading the candidate to
/// hash it). `dry_run=true` runs stop here; otherwise this plan is fed
/// into [`execute_filesystem_promote`] + the audit-row insert.
#[derive(Debug, Clone, Serialize)]
pub struct PromotePlan {
    pub kind: String,
    /// 1-based cycle number of the *new* live model. Computed as
    /// `prior_promotions + 1` so cycle K is the K-th promotion of this
    /// kind. The previous live model (if any) is archived as cycle K-1.
    pub cycle: i64,
    pub candidate: PathBuf,
    pub candidate_sha256: String,
    pub candidate_size: i64,
    /// Final live path: `<models_dir>/<kind>.onnx`.
    pub target: PathBuf,
    /// `<models_dir>/history/<kind>.v<cycle-1>.onnx`. None when there is
    /// no current live model to archive (first-ever promotion).
    pub history_archive: Option<PathBuf>,
    /// True when `target` already exists pre-promotion.
    pub previous_target_existed: bool,
    /// Pulled from `<cycle_dir>/metadata.json` when available.
    pub corrections_consumed: Option<i64>,
    pub notes: Option<String>,
    pub dry_run: bool,
}

/// Read `<cycle_dir>/metadata.json` (written by `prepare`) and return its
/// `count` field as `corrections_consumed`. Returns `Ok(None)` when the
/// file is absent (operator may promote a hand-crafted candidate).
pub fn read_corrections_consumed(cycle_dir: &Path) -> Result<Option<i64>> {
    let p = cycle_dir.join("metadata.json");
    if !p.is_file() {
        return Ok(None);
    }
    let body =
        fs::read_to_string(&p).with_context(|| format!("read cycle metadata {}", p.display()))?;
    let meta: CycleMetadata = serde_json::from_str(&body)
        .with_context(|| format!("parse cycle metadata {}", p.display()))?;
    Ok(Some(meta.count as i64))
}

/// Compute the promotion plan. Caller passes the count of pre-existing
/// `model_versions` rows of this kind so that K = prior + 1 is decided
/// without coupling this pure function to a DB pool. The candidate file
/// is hashed here.
pub fn plan_promote(params: &PromoteParams, prior_promotions: i64) -> Result<PromotePlan> {
    if params.kind.is_empty() {
        anyhow::bail!("kind must be non-empty");
    }
    if !params.candidate.is_file() {
        anyhow::bail!(
            "candidate is not a regular file: {}",
            params.candidate.display()
        );
    }
    let fp = fingerprint_file(&params.candidate)?;
    let target = params.models_dir.join(format!("{}.onnx", params.kind));
    let previous_target_existed = target.is_file();
    let cycle = prior_promotions.saturating_add(1);
    let history_archive = if previous_target_existed {
        Some(
            params
                .models_dir
                .join("history")
                .join(history_filename(&params.kind, prior_promotions)),
        )
    } else {
        None
    };
    let corrections_consumed = match params.cycle_dir.as_deref() {
        Some(d) => read_corrections_consumed(d)?,
        None => None,
    };
    Ok(PromotePlan {
        kind: params.kind.clone(),
        cycle,
        candidate: fp.path,
        candidate_sha256: fp.sha256,
        candidate_size: fp.file_size,
        target,
        history_archive,
        previous_target_existed,
        corrections_consumed,
        notes: params.notes.clone(),
        dry_run: params.dry_run,
    })
}

/// Apply the plan: archive any current live model under
/// `<models_dir>/history/`, then move the candidate into place.
/// Caller is responsible for ensuring `plan.dry_run == false` first.
pub fn execute_filesystem_promote(plan: &PromotePlan) -> Result<()> {
    if plan.dry_run {
        anyhow::bail!("refusing to execute filesystem promote on dry-run plan");
    }
    if let Some(history_path) = plan.history_archive.as_ref() {
        let history_dir = history_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("history archive has no parent"))?;
        fs::create_dir_all(history_dir)
            .with_context(|| format!("mkdir -p {}", history_dir.display()))?;
        rename_or_copy(&plan.target, history_path).with_context(|| {
            format!(
                "archive {} -> {}",
                plan.target.display(),
                history_path.display()
            )
        })?;
    }
    if let Some(target_dir) = plan.target.parent() {
        fs::create_dir_all(target_dir)
            .with_context(|| format!("mkdir -p {}", target_dir.display()))?;
    }
    rename_or_copy(&plan.candidate, &plan.target).with_context(|| {
        format!(
            "promote {} -> {}",
            plan.candidate.display(),
            plan.target.display()
        )
    })?;
    Ok(())
}

/// `fs::rename` first (atomic on the same filesystem), with a copy + remove
/// fallback when the source and destination are on different filesystems
/// (e.g. training_dir mounted separately from models_dir). The fallback is
/// not atomic but the candidate has already been hashed and the audit row
/// records the sha256, so a partial failure is detectable post-hoc.
fn rename_or_copy(src: &Path, dst: &Path) -> Result<()> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    fs::remove_file(src).with_context(|| format!("remove source {}", src.display()))?;
    Ok(())
}

/// Count of prior promotions of this kind. The new cycle number is
/// `count_promotions(...) + 1`.
pub async fn count_promotions(pool: &PgPool, kind: &str) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*)::bigint AS n FROM model_versions WHERE kind = $1")
        .bind(kind)
        .fetch_one(pool)
        .await
        .with_context(|| format!("count model_versions kind={}", kind))?;
    let n: i64 = row.try_get("n")?;
    Ok(n)
}

/// Insert the audit row for a freshly-promoted model. `eval_deltas` is
/// `None` for #7c-skel (the shadow-eval gate lands in #7c-eval); a
/// `notes`-only row is fine.
#[allow(clippy::too_many_arguments)]
pub async fn record_promotion(
    pool: &PgPool,
    kind: &str,
    sha256: &str,
    file_size: i64,
    corrections_consumed: Option<i64>,
    eval_deltas: Option<serde_json::Value>,
    promoted_by: Option<Uuid>,
    notes: Option<&str>,
) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO model_versions \
         (kind, sha256, file_size, corrections_consumed, eval_deltas, promoted_by, notes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(kind)
    .bind(sha256)
    .bind(file_size)
    .bind(corrections_consumed)
    .bind(eval_deltas)
    .bind(promoted_by)
    .bind(notes)
    .fetch_one(pool)
    .await
    .with_context(|| format!("insert model_versions kind={}", kind))?;
    let id: i64 = row.try_get("id")?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bbox_to_yolo_basic() {
        let b = BboxPixel {
            x1: 100.0,
            y1: 200.0,
            x2: 300.0,
            y2: 500.0,
        };
        let y = bbox_to_yolo(&b, 1000, 1000).expect("non-degenerate");
        // x mid = 100 + (300-100)/2 = 200 -> 200/1000 = 0.2
        assert!((y.cx - 0.2).abs() < 1e-9);
        assert!((y.cy - 0.35).abs() < 1e-9);
        assert!((y.w - 0.2).abs() < 1e-9);
        assert!((y.h - 0.3).abs() < 1e-9);
    }

    #[test]
    fn bbox_to_yolo_clamps_outside_bounds() {
        let b = BboxPixel {
            x1: -10.0,
            y1: -5.0,
            x2: 1100.0,
            y2: 1050.0,
        };
        let y = bbox_to_yolo(&b, 1000, 1000).expect("clamps to full image");
        assert!((y.cx - 0.5).abs() < 1e-9);
        assert!((y.cy - 0.5).abs() < 1e-9);
        assert!((y.w - 1.0).abs() < 1e-9);
        assert!((y.h - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bbox_to_yolo_rejects_degenerate() {
        let tight = BboxPixel {
            x1: 100.0,
            y1: 100.0,
            x2: 100.5,
            y2: 100.5,
        };
        assert!(bbox_to_yolo(&tight, 1000, 1000).is_none());
        let inverted = BboxPixel {
            x1: 200.0,
            y1: 200.0,
            x2: 100.0,
            y2: 100.0,
        };
        assert!(bbox_to_yolo(&inverted, 1000, 1000).is_none());
    }

    #[test]
    fn bbox_to_yolo_rejects_zero_image() {
        let b = BboxPixel {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 10.0,
        };
        assert!(bbox_to_yolo(&b, 0, 1000).is_none());
        assert!(bbox_to_yolo(&b, 1000, 0).is_none());
    }

    #[test]
    fn parse_bbox_pixel_corners() {
        let v = json!({"x1": 10.0, "y1": 20.0, "x2": 30.0, "y2": 40.0});
        let b = parse_bbox(&v).expect("well-formed");
        assert_eq!(b.x1, 10.0);
        assert_eq!(b.y1, 20.0);
        assert_eq!(b.x2, 30.0);
        assert_eq!(b.y2, 40.0);
    }

    #[test]
    fn parse_bbox_rejects_missing_field() {
        let v = json!({"x1": 10.0, "y1": 20.0, "x2": 30.0});
        assert!(parse_bbox(&v).is_none());
    }

    #[test]
    fn parse_bbox_rejects_non_object() {
        let v = json!([1, 2, 3, 4]);
        assert!(parse_bbox(&v).is_none());
    }

    fn sample_train_params() -> TrainParams {
        TrainParams {
            cycle_dir: PathBuf::from("/tmp/cycle-1"),
            base_weights: "yolov8n.pt".into(),
            epochs: 50,
            imgsz: 640,
            export_imgsz: 640,
            freeze: 10,
            batch: 16,
            workers: 4,
            device: "cpu".into(),
            runs_dir: PathBuf::from("/tmp/cycle-1/runs"),
            run_name: "cycle-1".into(),
            candidate_out: PathBuf::from("/tmp/cycle-1.candidate.onnx"),
            opset: 12,
            summary_out: PathBuf::from("/tmp/cycle-1.summary.json"),
            python: PathBuf::from("/usr/bin/python3"),
            script: PathBuf::from("/opt/f1/tools/retrain_train.py"),
        }
    }

    #[test]
    fn build_train_args_includes_all_flags() {
        let p = sample_train_params();
        let args = build_train_args(&p);
        let as_strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        // First positional must be the script path itself.
        assert_eq!(as_strs[0], "/opt/f1/tools/retrain_train.py");
        // Required flags must all appear in pairs.
        for flag in [
            "--cycle-dir",
            "--base-weights",
            "--epochs",
            "--imgsz",
            "--export-imgsz",
            "--freeze",
            "--batch",
            "--workers",
            "--device",
            "--runs-dir",
            "--run-name",
            "--candidate-out",
            "--opset",
            "--summary-out",
        ] {
            assert!(
                as_strs.iter().any(|s| s == flag),
                "missing flag {flag} in {:?}",
                as_strs
            );
        }
        // Critical pair: export-imgsz value follows its flag and equals 640.
        let idx = as_strs.iter().position(|s| s == "--export-imgsz").unwrap();
        assert_eq!(as_strs[idx + 1], "640");
    }

    #[test]
    fn build_train_args_overrides_train_imgsz_independent_of_export() {
        let mut p = sample_train_params();
        p.imgsz = 320;
        p.export_imgsz = 640;
        let args = build_train_args(&p);
        let as_strs: Vec<String> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let i = as_strs.iter().position(|s| s == "--imgsz").unwrap();
        assert_eq!(as_strs[i + 1], "320");
        let j = as_strs.iter().position(|s| s == "--export-imgsz").unwrap();
        assert_eq!(as_strs[j + 1], "640");
    }

    #[test]
    fn retrain_train_report_parses_python_output() {
        // Sample of the JSON `tools/retrain_train.py` writes to --summary-out.
        let body = r#"{
            "status": "ok",
            "cycle_dir": "/tmp/cycle-1",
            "base_weights": "yolov8n.pt",
            "epochs": 1,
            "imgsz": 320,
            "export_imgsz": 640,
            "freeze": 10,
            "batch": 2,
            "device": "cpu",
            "run_dir": "/tmp/cycle-1/runs/smoke",
            "best_pt": "/tmp/cycle-1/runs/smoke/weights/best.pt",
            "onnx_export": "/tmp/cycle-1/runs/smoke/weights/best.onnx",
            "candidate_out": "/tmp/cycle-1.candidate.onnx",
            "candidate_size_bytes": 12238381,
            "output_shape": [1, 5, 8400],
            "train_seconds": 3.4,
            "export_seconds": 1.2
        }"#;
        let r: RetrainTrainReport = serde_json::from_str(body).expect("parses");
        assert_eq!(r.status, "ok");
        assert_eq!(r.output_shape, vec![1, 5, 8400]);
        assert_eq!(r.candidate_size_bytes, 12_238_381);
    }

    #[test]
    fn retrain_train_report_tolerates_extra_fields() {
        // Future python additions must not break the Rust deserialiser.
        let body = r#"{
            "status": "ok",
            "output_shape": [1, 5, 8400],
            "future_field_we_dont_know_about": "hello"
        }"#;
        let r: RetrainTrainReport = serde_json::from_str(body).expect("parses");
        assert_eq!(r.status, "ok");
        assert_eq!(r.output_shape, vec![1, 5, 8400]);
        // Unknown/missing fields default to zero/empty.
        assert_eq!(r.candidate_size_bytes, 0);
        assert_eq!(r.train_seconds, 0.0);
    }

    // -----------------------------------------------------------------
    // #7c-skel promote helpers.
    // -----------------------------------------------------------------

    fn unique_tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "f1p-promote-{}-{}-{}",
            tag,
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir_all(&p).expect("mkdir tmpdir");
        p
    }

    #[test]
    fn fingerprint_file_basic() {
        let dir = unique_tmpdir("fp");
        let p = dir.join("candidate.onnx");
        fs::write(&p, b"hello world").unwrap();
        let fp = fingerprint_file(&p).expect("fingerprint");
        // sha256 of "hello world" is well-known.
        assert_eq!(
            fp.sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(fp.file_size, 11);
        assert_eq!(fp.path, p);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fingerprint_file_rejects_empty() {
        let dir = unique_tmpdir("fp-empty");
        let p = dir.join("empty.onnx");
        fs::write(&p, b"").unwrap();
        let err = fingerprint_file(&p).expect_err("empty file rejected");
        let msg = format!("{:#}", err);
        assert!(msg.contains("empty"), "got: {}", msg);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_filename_format() {
        assert_eq!(
            history_filename("object_detect", 0),
            "object_detect.v0.onnx"
        );
        assert_eq!(
            history_filename("object_detect", 3),
            "object_detect.v3.onnx"
        );
        // Negative inputs (defensive) collapse to v0 rather than panicking.
        assert_eq!(
            history_filename("object_detect", -1),
            "object_detect.v0.onnx"
        );
    }

    #[test]
    fn plan_promote_first_promotion_has_no_history() {
        let dir = unique_tmpdir("plan-first");
        let candidate = dir.join("cand.onnx");
        fs::write(&candidate, b"some-onnx-bytes").unwrap();
        let models_dir = dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        let params = PromoteParams {
            candidate: candidate.clone(),
            models_dir: models_dir.clone(),
            kind: "object_detect".into(),
            cycle_dir: None,
            notes: None,
            dry_run: true,
        };
        let plan = plan_promote(&params, 0).expect("plan");
        assert_eq!(plan.cycle, 1);
        assert!(!plan.previous_target_existed);
        assert!(plan.history_archive.is_none());
        assert_eq!(plan.target, models_dir.join("object_detect.onnx"));
        assert_eq!(plan.candidate, candidate);
        assert_eq!(plan.candidate_size, 15);
        assert!(plan.dry_run);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_promote_archives_existing_target_under_history() {
        let dir = unique_tmpdir("plan-arch");
        let candidate = dir.join("cand.onnx");
        fs::write(&candidate, b"new-onnx").unwrap();
        let models_dir = dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        // Pretend cycles 1, 2, 3 have been promoted; cycle 3 is currently live.
        fs::write(models_dir.join("object_detect.onnx"), b"old-onnx").unwrap();
        let params = PromoteParams {
            candidate,
            models_dir: models_dir.clone(),
            kind: "object_detect".into(),
            cycle_dir: None,
            notes: Some("smoke".into()),
            dry_run: false,
        };
        let plan = plan_promote(&params, 3).expect("plan");
        assert_eq!(plan.cycle, 4); // K = prior + 1
        assert!(plan.previous_target_existed);
        assert_eq!(
            plan.history_archive,
            Some(models_dir.join("history").join("object_detect.v3.onnx"))
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_promote_reads_corrections_from_cycle_metadata() {
        let dir = unique_tmpdir("plan-meta");
        let candidate = dir.join("cand.onnx");
        fs::write(&candidate, b"x").unwrap();
        let models_dir = dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        let cycle_dir = dir.join("cycle-1");
        fs::create_dir_all(&cycle_dir).unwrap();
        let meta = json!({
            "cycle_id": "cycle-1",
            "prepared_at": "2026-04-28T00:00:00Z",
            "since": "2026-04-01T00:00:00Z",
            "min_score": 0.5,
            "min_corrections": 50,
            "class_names": ["tool"],
            "count": 73,
            "items": []
        });
        fs::write(
            cycle_dir.join("metadata.json"),
            serde_json::to_string_pretty(&meta).unwrap(),
        )
        .unwrap();
        let params = PromoteParams {
            candidate,
            models_dir,
            kind: "object_detect".into(),
            cycle_dir: Some(cycle_dir),
            notes: None,
            dry_run: true,
        };
        let plan = plan_promote(&params, 0).expect("plan");
        assert_eq!(plan.corrections_consumed, Some(73));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn execute_filesystem_promote_archives_then_renames() {
        let dir = unique_tmpdir("exec");
        let candidate = dir.join("cand.onnx");
        fs::write(&candidate, b"NEW").unwrap();
        let models_dir = dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        fs::write(models_dir.join("object_detect.onnx"), b"OLD").unwrap();
        let params = PromoteParams {
            candidate: candidate.clone(),
            models_dir: models_dir.clone(),
            kind: "object_detect".into(),
            cycle_dir: None,
            notes: None,
            dry_run: false,
        };
        let plan = plan_promote(&params, 2).expect("plan");
        execute_filesystem_promote(&plan).expect("execute");
        assert!(!candidate.exists(), "candidate consumed");
        assert_eq!(
            fs::read(models_dir.join("object_detect.onnx")).unwrap(),
            b"NEW"
        );
        let archive = models_dir.join("history").join("object_detect.v2.onnx");
        assert_eq!(fs::read(&archive).unwrap(), b"OLD");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn execute_filesystem_promote_refuses_dry_run_plan() {
        let dir = unique_tmpdir("dry");
        let candidate = dir.join("cand.onnx");
        fs::write(&candidate, b"x").unwrap();
        let models_dir = dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        let params = PromoteParams {
            candidate,
            models_dir,
            kind: "object_detect".into(),
            cycle_dir: None,
            notes: None,
            dry_run: true,
        };
        let plan = plan_promote(&params, 0).expect("plan");
        let err = execute_filesystem_promote(&plan).expect_err("refuses");
        let msg = format!("{:#}", err);
        assert!(msg.contains("dry-run"), "got: {}", msg);
        fs::remove_dir_all(&dir).ok();
    }
}
