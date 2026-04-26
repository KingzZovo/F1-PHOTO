//! Offline ONNX inference (M2 turn 9 scaffold).
//!
//! Loads the five models that drive recognition:
//! - SCRFD                       face detection
//! - ArcFace MobileFaceNet 512d  face embedding
//! - YOLOv8n                     tool / device detection
//! - DINOv2-small                generic embedding
//! - MobileNetV3 (optional)      angle classification
//!
//! Models live as ONNX files in `Config::models_dir` (default `./models/`).
//! ONNX Runtime itself is loaded dynamically (`ort` `load-dynamic` feature),
//! so the binary builds and links without `libonnxruntime.so` available. At
//! runtime, missing libraries or model files are reported via
//! `ModelRegistry::status()` and a warning log line, and inference is left
//! disabled — the worker (turn 7 skeleton, turn 10 real inference) treats
//! photos as `unmatched` in that case so the API surface stays usable.

pub mod models;
pub mod preprocess;
pub mod recall;

pub use models::{LoadedModel, ModelInfo, ModelKind, ModelRegistry, ModelRegistryStatus};
pub use recall::{Bucket, Hit, Thresholds};
