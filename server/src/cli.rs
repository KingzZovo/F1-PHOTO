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
}
