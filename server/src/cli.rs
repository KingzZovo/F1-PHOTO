//! CLI surface for the `f1photo` binary.
//!
//! Default subcommand is `serve` (running the HTTP server). Operational
//! subcommands such as `bootstrap-admin`, `models check`, and `finetune`
//! are kept in the same binary so a single deployment artefact can both
//! run and be administered.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "f1photo",
    version,
    about = "F1-Photo backend server + admin CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the HTTP server (default if no subcommand is supplied).
    Serve,

    /// Create or upsert an administrator user.
    ///
    /// On conflict (existing username) the password and full name are
    /// updated and the role is forced to `admin`. Used to seed the very
    /// first admin during deployment, or to reset an admin's password.
    BootstrapAdmin {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        full_name: Option<String>,
    },

    /// ONNX model registry maintenance.
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },

    /// Roll manually-corrected recognition samples back into
    /// `identity_embeddings` so the recognition gallery learns from human
    /// feedback. Designed to be run monthly via cron.
    Finetune {
        #[command(subcommand)]
        action: FinetuneAction,
    },

    /// Detector retraining loop (active learning, milestone #7).
    ///
    /// Reads operator corrections via the `v_training_corrections` view
    /// (added in milestone #5-skel) and materialises a YOLO-format
    /// training cycle on disk. Actual `yolo train` + ONNX export +
    /// shadow-eval gate land in milestones #7b / #7c.
    RetrainDetector {
        #[command(subcommand)]
        action: RetrainDetectorAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModelsAction {
    /// Probe the configured `models_dir`, try to load every model, and
    /// print a human-readable summary. Exits 0 even when ORT is not
    /// installed so it's safe to run on hosts without `libonnxruntime.so`.
    Check,
}

#[derive(Subcommand, Debug)]
pub enum FinetuneAction {
    /// Print per-owner candidate stats without writing anything.
    Stats {
        /// Only consider corrections from this date onwards (YYYY-MM-DD).
        /// Defaults to 30 days ago.
        #[arg(long)]
        since: Option<String>,
        /// Restrict to a single project UUID. Defaults to all projects.
        #[arg(long)]
        project: Option<String>,
    },
    /// Roll eligible embeddings into `identity_embeddings` (idempotent).
    Apply {
        /// Only consider corrections from this date onwards (YYYY-MM-DD).
        /// Defaults to 30 days ago.
        #[arg(long)]
        since: Option<String>,
        /// Restrict to a single project UUID. Defaults to all projects.
        #[arg(long)]
        project: Option<String>,
        /// Print what would be inserted but do not write.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RetrainDetectorAction {
    /// Print per-owner-type correction-candidate counts without writing.
    Stats {
        /// Only consider corrections from this date onwards (YYYY-MM-DD).
        /// Defaults to 30 days ago.
        #[arg(long)]
        since: Option<String>,
        /// Minimum `detections.score` floor for inclusion. Defaults 0.5.
        #[arg(long, default_value_t = 0.5)]
        min_score: f64,
    },
    /// Materialise a YOLO training cycle under `<training_dir>/cycle-<ts>/`.
    Prepare {
        /// Only consider corrections from this date onwards (YYYY-MM-DD).
        /// Defaults to 30 days ago.
        #[arg(long)]
        since: Option<String>,
        /// Minimum `detections.score` floor for inclusion. Defaults 0.5.
        #[arg(long, default_value_t = 0.5)]
        min_score: f64,
        /// Refuse to write a cycle until at least this many eligible
        /// corrections are available. Defaults 50.
        #[arg(long, default_value_t = 50)]
        min_corrections: i64,
        /// Override the training directory. Defaults to
        /// `$F1P_TRAINING_DIR` if set, otherwise `<data_dir>/training`.
        #[arg(long)]
        training_dir: Option<String>,
        /// Plan the cycle and count eligible items but do not write.
        #[arg(long)]
        dry_run: bool,
    },
    /// Run YOLOv8 fine-tune + ONNX export against an existing cycle.
    ///
    /// Shells out to `tools/retrain_train.py` (an ultralytics + onnxruntime
    /// wrapper) and writes a `candidate.onnx` whose shape `[1, 4+nc, 8400]`
    /// is validated to match the server-side shape-tolerant decoder added
    /// in milestone #7b-prep. Promotion of the candidate into the live
    /// model registry lands in milestone #7c.
    Train {
        /// Cycle directory previously produced by `prepare`. Must
        /// contain `data.yaml` + `images/` + `labels/`.
        #[arg(long)]
        cycle_dir: String,
        /// Base weights to fine-tune from. Defaults to `yolov8n.pt`,
        /// which `ultralytics` auto-downloads on first use.
        #[arg(long, default_value = "yolov8n.pt")]
        base_weights: String,
        /// Number of training epochs.
        #[arg(long, default_value_t = 50)]
        epochs: u32,
        /// Training image size. Lower values train faster but the
        /// exported anchor count must match the server (which assumes
        /// 640×640 → 8400 anchors); see `--export-imgsz`.
        #[arg(long, default_value_t = 640)]
        imgsz: u32,
        /// ONNX export image size. Must remain 640 unless the server
        /// `NUM_ANCHORS` constant is updated in lockstep.
        #[arg(long, default_value_t = 640)]
        export_imgsz: u32,
        /// Number of layers to freeze. Defaults 10 (backbone only).
        #[arg(long, default_value_t = 10)]
        freeze: u32,
        /// Mini-batch size.
        #[arg(long, default_value_t = 16)]
        batch: u32,
        /// Dataloader workers.
        #[arg(long, default_value_t = 4)]
        workers: u32,
        /// Compute device passed to ultralytics (`cpu`, `0`, `0,1`, ...).
        #[arg(long, default_value = "cpu")]
        device: String,
        /// Override the training-cycle root (used to derive default
        /// `--runs-dir` and `--candidate-out`).
        #[arg(long)]
        training_dir: Option<String>,
        /// Where ultralytics writes its `runs/<run_name>/` artefacts.
        /// Defaults to `<training_dir>/runs`.
        #[arg(long)]
        runs_dir: Option<String>,
        /// ultralytics run-name. Defaults to the cycle directory's
        /// basename.
        #[arg(long)]
        run_name: Option<String>,
        /// Final candidate ONNX path. Defaults to
        /// `<training_dir>/<run_name>.candidate.onnx`.
        #[arg(long)]
        candidate_out: Option<String>,
        /// ONNX opset version.
        #[arg(long, default_value_t = 12)]
        opset: u32,
        /// Override the python interpreter (default: `$F1P_PYTHON`
        /// or `python3`).
        #[arg(long)]
        python: Option<String>,
        /// Override the path to `retrain_train.py` (default:
        /// `$F1P_RETRAIN_SCRIPT` or `tools/retrain_train.py` next to
        /// the binary).
        #[arg(long)]
        script: Option<String>,
    },
    /// Promote a candidate ONNX into the live model registry.
    ///
    /// Atomically renames `--candidate` over `<models_dir>/object_detect.onnx`
    /// (archiving the previous version under `<models_dir>/history/`) and
    /// records an audit row in `model_versions` (cycle, sha256, file_size,
    /// corrections_consumed, eval_deltas, promoted_at, promoted_by, notes).
    ///
    /// `#7c-skel` ships unconditional promotion (no shadow-eval gate); the
    /// gate (#2-tool fixture + #2c-tune face fixture deltas, fail-closed on
    /// regressions) lands in `#7c-eval`. Use `--dry-run` to preview the
    /// plan without touching the filesystem or the database.
    Promote {
        /// Candidate ONNX path. Must be an existing non-empty file,
        /// typically the `--candidate-out` of a previous `train` run.
        #[arg(long)]
        candidate: String,
        /// Optional cycle directory; if it contains `metadata.json`,
        /// `corrections_consumed` is populated from its `count` field.
        #[arg(long)]
        cycle_dir: Option<String>,
        /// Override the models directory. Defaults to the config
        /// `models_dir` (= `$F1P_MODELS_DIR` or `<cwd>/models`).
        #[arg(long)]
        models_dir: Option<String>,
        /// Override the kind. Defaults to `object_detect`. Reserved for
        /// future face / generic-embed retrain pipelines.
        #[arg(long, default_value = "object_detect")]
        kind: String,
        /// Optional free-form note recorded in `model_versions.notes`.
        #[arg(long)]
        notes: Option<String>,
        /// Plan the promote and report sha256 / file size / cycle number
        /// without renaming files or inserting a `model_versions` row.
        #[arg(long)]
        dry_run: bool,
    },
}
