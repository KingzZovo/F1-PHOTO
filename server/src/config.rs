use anyhow::{bail, Context, Result};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub jwt_secret: String,
    pub data_dir: String,
    pub max_upload_mb: u64,
    /// Where ONNX model files (`face_detect.onnx`, etc.) are looked up.
    /// Override with `F1P_MODELS_DIR`. Defaults to `./models/`.
    pub models_dir: PathBuf,
    /// Per-session intra-op thread count for ONNX Runtime. Override with
    /// `F1P_INFERENCE_THREADS`. Defaults to 4 (CPU INT8 on the 10C/20T host).
    pub inference_intra_threads: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Load .env if present (best-effort, ignore errors silently for prod).
        let _ = load_dotenv(".env");

        let bind_addr = env::var("F1P_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
        let database_url = env::var("F1P_DATABASE_URL").context("F1P_DATABASE_URL must be set")?;
        let jwt_secret =
            env::var("F1P_JWT_SECRET").context("F1P_JWT_SECRET must be set (32+ chars)")?;
        if jwt_secret.len() < 32 {
            bail!("F1P_JWT_SECRET must be at least 32 characters");
        }
        let data_dir = env::var("F1P_DATA_DIR").unwrap_or_else(|_| "./data".into());
        let max_upload_mb = env::var("F1P_MAX_UPLOAD_MB")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let models_dir = env::var("F1P_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./models"));
        let inference_intra_threads = env::var("F1P_INFERENCE_THREADS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(4);

        Ok(Self {
            bind_addr,
            database_url,
            jwt_secret,
            data_dir,
            max_upload_mb,
            models_dir,
            inference_intra_threads,
        })
    }
}

fn load_dotenv(path: &str) -> std::io::Result<()> {
    use std::io::BufRead;
    let f = std::fs::File::open(path)?;
    let r = std::io::BufReader::new(f);
    for line in r.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if env::var_os(k).is_none() {
                // SAFETY: we run this once at startup before threads are spawned.
                unsafe { env::set_var(k, v) };
            }
        }
    }
    Ok(())
}
