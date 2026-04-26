//! Admin-only diagnostics endpoints.
//!
//! Currently:
//! - GET /api/admin/queue/stats: snapshot of `recognition_queue` and
//!   per-status photo counts. Useful for smoke-testing the worker and as
//!   a base for the M3 admin dashboard.
//! - GET /api/admin/models: snapshot of the loaded ONNX model registry
//!   (which slots are filled, which files / libraries are missing).

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use sqlx::Row;

use crate::api::AppState;
use crate::auth::RequireAdmin;
use crate::error::AppResult;
use crate::inference::ModelRegistryStatus;

#[derive(Debug, Serialize)]
pub struct QueueStats {
    pub queue_pending: i64,
    pub queue_locked: i64,
    pub queue_total: i64,
    pub photo_pending: i64,
    pub photo_processing: i64,
    pub photo_matched: i64,
    pub photo_unmatched: i64,
    pub photo_learning: i64,
    pub photo_failed: i64,
}

pub async fn queue_stats(
    _admin: RequireAdmin,
    State(s): State<AppState>,
) -> AppResult<(StatusCode, Json<QueueStats>)> {
    let q_row = sqlx::query(
        "SELECT \
            COUNT(*) FILTER (WHERE locked_until IS NULL OR locked_until < now())::bigint AS pending, \
            COUNT(*) FILTER (WHERE locked_until IS NOT NULL AND locked_until >= now())::bigint AS locked, \
            COUNT(*)::bigint AS total \
         FROM recognition_queue",
    )
    .fetch_one(&s.db)
    .await?;
    let queue_pending: i64 = q_row.get("pending");
    let queue_locked: i64 = q_row.get("locked");
    let queue_total: i64 = q_row.get("total");

    let p_rows = sqlx::query(
        "SELECT status::text AS status, COUNT(*)::bigint AS n FROM photos GROUP BY status",
    )
    .fetch_all(&s.db)
    .await?;
    let mut by_status = std::collections::HashMap::<String, i64>::new();
    for r in p_rows {
        let st: String = r.get("status");
        let n: i64 = r.get("n");
        by_status.insert(st, n);
    }

    let stats = QueueStats {
        queue_pending,
        queue_locked,
        queue_total,
        photo_pending: by_status.get("pending").copied().unwrap_or(0),
        photo_processing: by_status.get("processing").copied().unwrap_or(0),
        photo_matched: by_status.get("matched").copied().unwrap_or(0),
        photo_unmatched: by_status.get("unmatched").copied().unwrap_or(0),
        photo_learning: by_status.get("learning").copied().unwrap_or(0),
        photo_failed: by_status.get("failed").copied().unwrap_or(0),
    };

    Ok((StatusCode::OK, Json(stats)))
}

/// GET /api/admin/models — return a snapshot of the model registry.
///
/// Always 200 even when ORT or model files are missing; the JSON body
/// reports `ort_available=false` / per-model `loaded=false` so callers
/// (admin UI, ops scripts) can tell what's wrong without grepping logs.
pub async fn list_models(
    _admin: RequireAdmin,
    State(s): State<AppState>,
) -> AppResult<(StatusCode, Json<ModelRegistryStatus>)> {
    Ok((StatusCode::OK, Json(s.models.status())))
}
