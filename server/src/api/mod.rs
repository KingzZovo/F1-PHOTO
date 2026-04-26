pub mod health;

use crate::config::Config;
use axum::{Router, routing::get};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

/// Shared application state injected into every handler via `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
}

/// Build the top-level axum router.
///
/// M1 only ships `/healthz` and `/readyz`. Auth, projects, master data and
/// upload routers are added in subsequent commits and merged here.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
