//! CLI surface for the `f1photo` binary.
//!
//! Default subcommand is `serve` (running the HTTP server). Operational
//! subcommands such as `bootstrap-admin` and `models check` are kept in the
//! same binary so a single deployment artefact can both run and be
//! administered.

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
}

#[derive(Subcommand, Debug)]
pub enum ModelsAction {
    /// Probe the configured `models_dir`, try to load every model, and
    /// print a human-readable summary. Exits 0 even when ORT is not
    /// installed so it's safe to run on hosts without `libonnxruntime.so`.
    Check,
}
