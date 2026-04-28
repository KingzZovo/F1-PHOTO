//! F1-Photo Rust backend library crate.
//!
//! `main.rs` is a thin binary that wires `Config` -> `db::connect` -> `db::migrate`
//! -> `api::router`. All real logic lives here so unit tests can exercise it.

#![allow(clippy::type_complexity)]

pub mod api;
pub mod audit;
pub mod auth;
pub mod bundled_pg;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
pub mod finetune;
pub mod inference;
pub mod logging;
pub mod retrain;
pub mod static_assets;
pub mod worker;
