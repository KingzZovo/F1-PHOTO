pub mod auth;
pub mod health;

use crate::auth::JwtCodec;
use crate::config::Config;
use axum::{
    Router,
    routing::{get, post},
};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

/// Shared application state injected into every handler via `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
    pub jwt: Arc<JwtCodec>,
}

/// Build the top-level axum router.
///
/// Current surface:
///   - `/healthz`, `/readyz`         (unauthenticated health probes)
///   - `/api/auth/login`             (no auth)
///   - `/api/auth/logout`            (auth required)
///   - `/api/auth/me`                (auth required)
///
/// Projects, master data, photos and admin endpoints are added in
/// subsequent commits.
pub fn router(state: AppState) -> Router {
    let api_auth = Router::new()
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/me", get(auth::me));

    let api = Router::new().nest("/auth", api_auth);

    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .nest("/api", api)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
