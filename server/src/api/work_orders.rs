//! Project-scoped work orders.
//!
//! Schema (see `migrations/20260426140000_init.sql`):
//!   work_orders(id, project_id, code, title, status, created_by,
//!               created_at, updated_at)  UNIQUE(project_id, code)
//!
//! Status is a free-form text column with default `'open'`. We treat it as a
//! soft state machine in code:
//!   open ──► in_progress ──► done
//!   open ──► cancelled        in_progress ──► cancelled
//!   done ──► in_progress (reopen)
//!   cancelled ──► open (revert)
//!
//! Permissions (see docs/api.md §5):
//!   GET     view    POST    upload    PATCH   upload
//!   DELETE  delete  POST    upload (transition)

use crate::api::{is_unique_violation, AppState};
use crate::audit::Audit;
use crate::auth::{DeletePerm, RequireProjectPerm, UploadPerm, ViewPerm};
use crate::error::{AppError, AppResult};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

/// Deserialize a present-but-possibly-null JSON field as `Some(_)`,
/// so a missing field stays `None` while explicit `null` becomes
/// `Some(None)`.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// All valid `work_orders.status` values, in canonical order.
const ALL_STATUSES: &[&str] = &["open", "in_progress", "done", "cancelled"];

fn validate_status(s: &str) -> AppResult<&'static str> {
    for &v in ALL_STATUSES {
        if v == s {
            return Ok(v);
        }
    }
    Err(AppError::InvalidInput(format!(
        "unknown status '{s}', expected one of {ALL_STATUSES:?}"
    )))
}

/// Returns `Ok(())` if `from -> to` is a legal transition, else `Conflict`.
fn check_transition(from: &str, to: &str) -> AppResult<()> {
    if from == to {
        return Err(AppError::Conflict(format!(
            "work order is already in status '{from}'"
        )));
    }
    let allowed: &[&str] = match from {
        "open" => &["in_progress", "cancelled"],
        "in_progress" => &["done", "cancelled"],
        "done" => &["in_progress"],
        "cancelled" => &["open"],
        // Unknown legacy value — let the user only move it to 'open' to recover.
        _ => &["open"],
    };
    if allowed.contains(&to) {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "illegal transition '{from}' -> '{to}' (allowed: {allowed:?})"
        )))
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone)]
pub struct WorkOrderDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub code: String,
    pub title: Option<String>,
    pub status: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub data: Vec<WorkOrderDto>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

type WoRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    String,
    Option<Uuid>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn row_to_dto(r: WoRow) -> WorkOrderDto {
    WorkOrderDto {
        id: r.0,
        project_id: r.1,
        code: r.2,
        title: r.3,
        status: r.4,
        created_by: r.5,
        created_at: r.6,
        updated_at: r.7,
    }
}

// ---------------------------------------------------------------------------
// LIST
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    pub q: Option<String>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

/// `GET /api/projects/:project_id/work_orders` — view.
pub async fn list(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    let pid = perm.access.project_id;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;

    let pattern = match q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => format!("%{}%", p),
        None => "%".to_string(),
    };

    // Optional status filter; "" / missing means "any".
    let status_filter: Option<String> = match q.status.as_deref() {
        Some("") | None => None,
        Some(s) => Some(validate_status(s)?.to_string()),
    };

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_orders \
         WHERE project_id = $1 \
           AND ($2::text IS NULL OR status = $2) \
           AND (code ILIKE $3 OR coalesce(title,'') ILIKE $3)",
    )
    .bind(pid)
    .bind(status_filter.as_deref())
    .bind(&pattern)
    .fetch_one(&s.db)
    .await?;

    let rows: Vec<WoRow> = sqlx::query_as(
        "SELECT id, project_id, code, title, status, created_by, created_at, updated_at \
         FROM work_orders \
         WHERE project_id = $1 \
           AND ($2::text IS NULL OR status = $2) \
           AND (code ILIKE $3 OR coalesce(title,'') ILIKE $3) \
         ORDER BY created_at DESC, id DESC \
         LIMIT $4 OFFSET $5",
    )
    .bind(pid)
    .bind(status_filter.as_deref())
    .bind(&pattern)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&s.db)
    .await?;

    Ok(Json(ListResponse {
        data: rows.into_iter().map(row_to_dto).collect(),
        page,
        page_size,
        total,
    }))
}

// ---------------------------------------------------------------------------
// GET ONE
// ---------------------------------------------------------------------------

/// `GET /api/projects/:project_id/work_orders/:id` — view.
pub async fn get_one(
    perm: RequireProjectPerm<ViewPerm>,
    Path((_pid, id)): Path<(Uuid, Uuid)>,
    State(s): State<AppState>,
) -> AppResult<Json<WorkOrderDto>> {
    let dto = load_dto(&s, perm.access.project_id, id).await?;
    Ok(Json(dto))
}

// ---------------------------------------------------------------------------
// CREATE
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateInput {
    pub code: String,
    pub title: Option<String>,
    /// Optional initial status; defaults to `"open"`.
    pub status: Option<String>,
}

/// `POST /api/projects/:project_id/work_orders` — upload.
pub async fn create(
    perm: RequireProjectPerm<UploadPerm>,
    State(s): State<AppState>,
    Json(input): Json<CreateInput>,
) -> AppResult<(StatusCode, Json<WorkOrderDto>)> {
    let pid = perm.access.project_id;
    let actor = perm.access.user.id;

    let code = input.code.trim().to_string();
    if code.is_empty() {
        return Err(AppError::InvalidInput("code is required".into()));
    }
    if code.len() > 64 {
        return Err(AppError::InvalidInput("code too long".into()));
    }
    let title = input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(ref t) = title {
        if t.len() > 200 {
            return Err(AppError::InvalidInput("title too long".into()));
        }
    }
    let status: &str = match input.status.as_deref() {
        Some("") | None => "open",
        Some(s) => validate_status(s)?,
    };

    let row: WoRow = sqlx::query_as(
        "INSERT INTO work_orders (project_id, code, title, status, created_by) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, project_id, code, title, status, created_by, created_at, updated_at",
    )
    .bind(pid)
    .bind(&code)
    .bind(title.as_deref())
    .bind(status)
    .bind(actor)
    .fetch_one(&s.db)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!(
                "work order code '{code}' already exists in project"
            ))
        } else {
            AppError::Db(e)
        }
    })?;

    let dto = row_to_dto(row);

    Audit::new("work_order.create", "work_order")
        .actor(actor)
        .project(pid)
        .target(dto.id.to_string())
        .after(json!({
            "code": dto.code,
            "title": dto.title,
            "status": dto.status,
        }))
        .write(&s.db)
        .await;

    Ok((StatusCode::CREATED, Json(dto)))
}

// ---------------------------------------------------------------------------
// PATCH
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PatchInput {
    pub code: Option<String>,
    /// Use `Some(None)` (JSON `null`) to clear the field; omit to keep.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub title: Option<Option<String>>,
    /// Status updates via PATCH must obey the state machine. Use the dedicated
    /// `:transition` endpoint for clarity, but PATCH is also accepted.
    pub status: Option<String>,
}

/// `PATCH /api/projects/:project_id/work_orders/:id` — upload.
pub async fn patch(
    perm: RequireProjectPerm<UploadPerm>,
    Path((_pid, id)): Path<(Uuid, Uuid)>,
    State(s): State<AppState>,
    Json(input): Json<PatchInput>,
) -> AppResult<Json<WorkOrderDto>> {
    let pid = perm.access.project_id;
    let actor = perm.access.user.id;

    let before = load_dto(&s, pid, id).await?;

    let code = match input.code {
        Some(v) => {
            let v = v.trim().to_string();
            if v.is_empty() {
                return Err(AppError::InvalidInput("code must not be empty".into()));
            }
            if v.len() > 64 {
                return Err(AppError::InvalidInput("code too long".into()));
            }
            v
        }
        None => before.code.clone(),
    };
    let title = match input.title {
        Some(opt) => {
            let v = opt.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            if let Some(ref t) = v {
                if t.len() > 200 {
                    return Err(AppError::InvalidInput("title too long".into()));
                }
            }
            v
        }
        None => before.title.clone(),
    };
    let status = match input.status.as_deref() {
        Some(s) => {
            let canonical = validate_status(s)?;
            if canonical != before.status {
                check_transition(&before.status, canonical)?;
            }
            canonical.to_string()
        }
        None => before.status.clone(),
    };

    sqlx::query(
        "UPDATE work_orders \
         SET code=$1, title=$2, status=$3, updated_at=now() \
         WHERE id=$4 AND project_id=$5",
    )
    .bind(&code)
    .bind(title.as_deref())
    .bind(&status)
    .bind(id)
    .bind(pid)
    .execute(&s.db)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!(
                "work order code '{code}' already exists in project"
            ))
        } else {
            AppError::Db(e)
        }
    })?;

    let after = load_dto(&s, pid, id).await?;

    Audit::new("work_order.update", "work_order")
        .actor(actor)
        .project(pid)
        .target(id.to_string())
        .before(json!({
            "code": before.code,
            "title": before.title,
            "status": before.status,
        }))
        .after(json!({
            "code": after.code,
            "title": after.title,
            "status": after.status,
        }))
        .write(&s.db)
        .await;

    Ok(Json(after))
}

// ---------------------------------------------------------------------------
// TRANSITION (explicit state-machine endpoint)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TransitionInput {
    pub status: String,
}

/// `POST /api/projects/:project_id/work_orders/:id/transition` — upload.
pub async fn transition(
    perm: RequireProjectPerm<UploadPerm>,
    Path((_pid, id)): Path<(Uuid, Uuid)>,
    State(s): State<AppState>,
    Json(input): Json<TransitionInput>,
) -> AppResult<Json<WorkOrderDto>> {
    let pid = perm.access.project_id;
    let actor = perm.access.user.id;

    let before = load_dto(&s, pid, id).await?;
    let to = validate_status(input.status.trim())?;
    check_transition(&before.status, to)?;

    sqlx::query(
        "UPDATE work_orders SET status=$1, updated_at=now() \
         WHERE id=$2 AND project_id=$3",
    )
    .bind(to)
    .bind(id)
    .bind(pid)
    .execute(&s.db)
    .await?;

    let after = load_dto(&s, pid, id).await?;

    Audit::new("work_order.transition", "work_order")
        .actor(actor)
        .project(pid)
        .target(id.to_string())
        .before(json!({ "status": before.status }))
        .after(json!({ "status": after.status }))
        .write(&s.db)
        .await;

    Ok(Json(after))
}

// ---------------------------------------------------------------------------
// DELETE (hard delete; work_orders has no soft-delete column)
// ---------------------------------------------------------------------------

/// `DELETE /api/projects/:project_id/work_orders/:id` — delete.
pub async fn hard_delete(
    perm: RequireProjectPerm<DeletePerm>,
    Path((_pid, id)): Path<(Uuid, Uuid)>,
    State(s): State<AppState>,
) -> AppResult<StatusCode> {
    let pid = perm.access.project_id;
    let actor = perm.access.user.id;

    let before = load_dto(&s, pid, id).await?;

    let res = sqlx::query("DELETE FROM work_orders WHERE id=$1 AND project_id=$2")
        .bind(id)
        .bind(pid)
        .execute(&s.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("work_order".into()));
    }

    Audit::new("work_order.delete", "work_order")
        .actor(actor)
        .project(pid)
        .target(id.to_string())
        .before(json!({
            "code": before.code,
            "title": before.title,
            "status": before.status,
        }))
        .write(&s.db)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn load_dto(s: &AppState, project_id: Uuid, id: Uuid) -> AppResult<WorkOrderDto> {
    let row: Option<WoRow> = sqlx::query_as(
        "SELECT id, project_id, code, title, status, created_by, created_at, updated_at \
         FROM work_orders WHERE id=$1 AND project_id=$2",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&s.db)
    .await?;
    let r = row.ok_or_else(|| AppError::NotFound("work_order".into()))?;
    Ok(row_to_dto(r))
}
