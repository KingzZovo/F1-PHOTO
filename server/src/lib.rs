//! F1-Photo Rust backend library crate.
//!
//! `main.rs` is a thin binary that wires `Config` -> `db::connect` -> `db::migrate`
//! -> `api::router`. All real logic lives here so unit tests can exercise it.

pub mod api;
pub mod audit;
pub mod auth;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
pub mod logging;
pub mod worker;
