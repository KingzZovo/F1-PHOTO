//! Model registry: discovers, loads, and exposes ONNX sessions.
//!
//! Loading is best-effort. The registry always returns a populated `models`
//! list whose entries describe what would have been loaded; entries whose
//! `loaded` flag is false either had no file on disk or failed to load.
//! `ort_available` is false when ONNX Runtime itself could not be
//! initialised (typically: `libonnxruntime.so` not found via
//! `ORT_DYLIB_PATH` / system loader).

use std::path::{Path, PathBuf};

use ort::session::{Session, builder::GraphOptimizationLevel};

use anyhow::Result;
use serde::Serialize;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// SCRFD face detector.
    FaceDetect,
    /// ArcFace MobileFaceNet 512d face embedder.
    FaceEmbed,
    /// YOLOv8n tool / device detector.
    ObjectDetect,
    /// DINOv2-small generic embedder (used for tool/device match + augment).
    GenericEmbed,
    /// MobileNetV3 angle classifier (front/side/back). Optional.
    AngleClassify,
}

impl ModelKind {
    pub const ALL: &'static [ModelKind] = &[
        ModelKind::FaceDetect,
        ModelKind::FaceEmbed,
        ModelKind::ObjectDetect,
        ModelKind::GenericEmbed,
        ModelKind::AngleClassify,
    ];

    pub fn file_name(self) -> &'static str {
        match self {
            ModelKind::FaceDetect => "face_detect.onnx",
            ModelKind::FaceEmbed => "face_embed.onnx",
            ModelKind::ObjectDetect => "object_detect.onnx",
            ModelKind::GenericEmbed => "generic_embed.onnx",
            ModelKind::AngleClassify => "angle_classify.onnx",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ModelKind::FaceDetect => "SCRFD face detection",
            ModelKind::FaceEmbed => "ArcFace MobileFaceNet 512d face embedding",
            ModelKind::ObjectDetect => "YOLOv8n tool/device detection",
            ModelKind::GenericEmbed => "DINOv2-small generic embedding",
            ModelKind::AngleClassify => "MobileNetV3 angle classification (optional)",
        }
    }

    /// Optional models do not block `ready()`.
    pub fn optional(self) -> bool {
        matches!(self, ModelKind::AngleClassify)
    }
}

/// Public, JSON-serialisable description of one model slot.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub kind: ModelKind,
    pub description: &'static str,
    pub file_name: &'static str,
    pub path: String,
    pub optional: bool,
    pub file_present: bool,
    pub file_bytes: Option<u64>,
    pub loaded: bool,
    pub error: Option<String>,
    pub input_names: Vec<String>,
    pub output_names: Vec<String>,
}

/// One slot in the registry: metadata plus an optional live `Session`.
pub struct LoadedModel {
    pub info: ModelInfo,
    pub session: Option<Session>,
}

/// JSON view of the whole registry.
#[derive(Debug, Clone, Serialize)]
pub struct ModelRegistryStatus {
    pub models_dir: String,
    pub ort_available: bool,
    pub ort_init_error: Option<String>,
    pub intra_threads: usize,
    pub ready: bool,
    pub models: Vec<ModelInfo>,
}

pub struct ModelRegistry {
    pub models_dir: PathBuf,
    pub ort_available: bool,
    pub ort_init_error: Option<String>,
    pub intra_threads: usize,
    pub models: Vec<LoadedModel>,
}

impl ModelRegistry {
    /// Tries to initialise ORT and load every model file present in
    /// `models_dir`. Always returns a populated registry; missing files /
    /// missing libonnxruntime are reflected in the per-model `loaded` flag
    /// and the top-level `ort_available` flag.
    pub fn load(models_dir: impl Into<PathBuf>, intra_threads: usize) -> Self {
        let models_dir: PathBuf = models_dir.into();
        let intra_threads = intra_threads.max(1);
        info!(
            models_dir = %models_dir.display(),
            intra_threads,
            "loading inference models"
        );

        // `ort::init().commit()` PANICS (rather than returning Err) when
        // `libonnxruntime.so` cannot be loaded under the `load-dynamic`
        // feature, so we catch the unwind to keep the server starting.
        let init_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ort::init().with_name("f1photo").commit()
        }));
        let (ort_available, ort_init_error) = match init_result {
            Ok(Ok(_)) => {
                info!("ONNX Runtime initialised successfully");
                (true, None)
            }
            Ok(Err(e)) => {
                let msg = format!("{e}");
                warn!(
                    error = %msg,
                    "ONNX Runtime not available; inference will be disabled. \
                     Set ORT_DYLIB_PATH to libonnxruntime.so to enable."
                );
                (false, Some(msg))
            }
            Err(panic) => {
                let msg = panic_message(&panic);
                warn!(
                    error = %msg,
                    "ONNX Runtime panicked while loading; inference will be disabled. \
                     Set ORT_DYLIB_PATH to libonnxruntime.so to enable."
                );
                (false, Some(msg))
            }
        };

        let mut models: Vec<LoadedModel> = Vec::with_capacity(ModelKind::ALL.len());
        for &kind in ModelKind::ALL {
            let path = models_dir.join(kind.file_name());
            let file_present = path.exists();
            let file_bytes = std::fs::metadata(&path).ok().map(|m| m.len());

            let mut info = ModelInfo {
                kind,
                description: kind.description(),
                file_name: kind.file_name(),
                path: path.to_string_lossy().to_string(),
                optional: kind.optional(),
                file_present,
                file_bytes,
                loaded: false,
                error: None,
                input_names: Vec::new(),
                output_names: Vec::new(),
            };

            let session = if !ort_available {
                None
            } else if !file_present {
                if !kind.optional() {
                    warn!(
                        ?kind,
                        path = %info.path,
                        "required model file missing"
                    );
                }
                None
            } else {
                match Self::try_load(&path, intra_threads) {
                    Ok(s) => {
                        info.input_names = s.inputs.iter().map(|i| i.name.clone()).collect();
                        info.output_names = s.outputs.iter().map(|o| o.name.clone()).collect();
                        info.loaded = true;
                        info!(
                            ?kind,
                            path = %info.path,
                            file_bytes = file_bytes.unwrap_or(0),
                            inputs = ?info.input_names,
                            outputs = ?info.output_names,
                            "model loaded"
                        );
                        Some(s)
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        warn!(?kind, error = %msg, "failed to load model");
                        info.error = Some(msg);
                        None
                    }
                }
            };

            models.push(LoadedModel { info, session });
        }

        Self {
            models_dir,
            ort_available,
            ort_init_error,
            intra_threads,
            models,
        }
    }

    fn try_load(path: &Path, intra_threads: usize) -> Result<Session> {
        let s = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(intra_threads)?
            .commit_from_file(path)?;
        Ok(s)
    }

    /// `true` when every required (non-optional) model loaded successfully.
    pub fn ready(&self) -> bool {
        self.ort_available
            && self
                .models
                .iter()
                .all(|m| m.info.optional || m.info.loaded)
    }

    pub fn status(&self) -> ModelRegistryStatus {
        ModelRegistryStatus {
            models_dir: self.models_dir.to_string_lossy().to_string(),
            ort_available: self.ort_available,
            ort_init_error: self.ort_init_error.clone(),
            intra_threads: self.intra_threads,
            ready: self.ready(),
            models: self.models.iter().map(|m| m.info.clone()).collect(),
        }
    }

    pub fn get(&self, kind: ModelKind) -> Option<&Session> {
        self.models
            .iter()
            .find(|m| m.info.kind == kind)
            .and_then(|m| m.session.as_ref())
    }
}

/// Best-effort recovery of a panic payload's `&str` / `String` message.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic payload>".to_string()
    }
}
