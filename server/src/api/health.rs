use crate::api::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

/// Process is alive. No DB check.
pub async fn healthz() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Process is ready to serve traffic. Verifies a DB roundtrip; once worker
/// pool / model loading is added, this also asserts those are warm.
pub async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match sqlx::query("SELECT 1").fetch_one(&state.db).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "status": "ready", "db": "ok" })),
        ),
        Err(e) => {
            tracing::error!(error = ?e, "readyz: db check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "not_ready", "db": "error" })),
            )
        }
    }
}
