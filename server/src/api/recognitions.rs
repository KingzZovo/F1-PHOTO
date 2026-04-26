//! Recognition results: detections + recognition_items query and manual
//! correction (M1 turn 8).
//!
//! Routes wired in `api/mod.rs`:
//! - GET   /api/projects/:pid/photos/:photo_id/detections
//! - GET   /api/projects/:pid/photos/:photo_id/recognition_items
//! - GET   /api/projects/:pid/recognition_items
//!   filters: photo_id, owner_type+owner_id, status, page, page_size
//! - GET   /api/projects/:pid/recognition_items/:id
//! - PATCH /api/projects/:pid/recognition_items/:id/correct
//!
//! Reads require ViewPerm; manual correction requires UploadPerm.
//!
//! M1 turn 8 only handles QUERY + CORRECT. Detections / recognition_items
//! are populated by the worker once turn 10 wires the real ONNX inference.
//! Until then these endpoints return empty lists; tests seed rows via SQL.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::api::AppState;
use crate::audit::Audit;
use crate::auth::{RequireProjectPerm, UploadPerm, ViewPerm};
use crate::error::{AppError, AppResult};

const CORRECTION_OWNER_TYPES: &[&str] = &["person", "tool", "device"];
const FILTER_OWNER_TYPES: &[&str] = &["person", "tool", "device", "wo_raw"];
const ALLOWED_STATUSES: &[&str] = &["matched", "learning", "unmatched", "manual_corrected"];

// =============================================================================
// DTOs
// =============================================================================

#[derive(Debug, Serialize)]
pub struct DetectionDto {
    pub id: i64,
    pub project_id: Uuid,
    pub photo_id: Uuid,
    pub target_type: String,
    pub bbox: Value,
    pub score: f32,
    pub angle: String,
    pub match_status: String,
    pub matched_owner_type: Option<String>,
    pub matched_owner_id: Option<Uuid>,
    pub matched_score: Option<f32>,
    pub created_at: DateTime<Utc>,
}

const DETECTION_COLS: &str = "id, project_id, photo_id, \
    target_type::text AS target_type, \
    bbox, score, \
    angle::text AS angle, \
    match_status::text AS match_status, \
    matched_owner_type::text AS matched_owner_type, \
    matched_owner_id, matched_score, created_at";

fn detection_from_row(r: &PgRow) -> DetectionDto {
    DetectionDto {
        id: r.get("id"),
        project_id: r.get("project_id"),
        photo_id: r.get("photo_id"),
        target_type: r.get("target_type"),
        bbox: r.get("bbox"),
        score: r.get("score"),
        angle: r.get("angle"),
        match_status: r.get("match_status"),
        matched_owner_type: r.try_get("matched_owner_type").ok(),
        matched_owner_id: r.try_get("matched_owner_id").ok(),
        matched_score: r.try_get("matched_score").ok(),
        created_at: r.get("created_at"),
    }
}

#[derive(Debug, Serialize)]
pub struct RecognitionItemDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub photo_id: Uuid,
    pub detection_id: Option<i64>,
    pub status: String,
    pub suggested_owner_type: Option<String>,
    pub suggested_owner_id: Option<Uuid>,
    pub suggested_score: Option<f32>,
    pub corrected_owner_type: Option<String>,
    pub corrected_owner_id: Option<Uuid>,
    pub corrected_by: Option<Uuid>,
    pub corrected_at: Option<DateTime<Utc>>,
    pub effective_owner_type: Option<String>,
    pub effective_owner_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const ITEM_COLS: &str = "id, project_id, photo_id, detection_id, \
    status::text AS status, \
    suggested_owner_type::text AS suggested_owner_type, \
    suggested_owner_id, suggested_score, \
    corrected_owner_type::text AS corrected_owner_type, \
    corrected_owner_id, corrected_by, corrected_at, \
    COALESCE(corrected_owner_type::text, suggested_owner_type::text) AS effective_owner_type, \
    COALESCE(corrected_owner_id, suggested_owner_id) AS effective_owner_id, \
    created_at";

fn item_from_row(r: &PgRow) -> RecognitionItemDto {
    RecognitionItemDto {
        id: r.get("id"),
        project_id: r.get("project_id"),
        photo_id: r.get("photo_id"),
        detection_id: r.try_get("detection_id").ok(),
        status: r.get("status"),
        suggested_owner_type: r.try_get("suggested_owner_type").ok(),
        suggested_owner_id: r.try_get("suggested_owner_id").ok(),
        suggested_score: r.try_get("suggested_score").ok(),
        corrected_owner_type: r.try_get("corrected_owner_type").ok(),
        corrected_owner_id: r.try_get("corrected_owner_id").ok(),
        corrected_by: r.try_get("corrected_by").ok(),
        corrected_at: r.try_get("corrected_at").ok(),
        effective_owner_type: r.try_get("effective_owner_type").ok(),
        effective_owner_id: r.try_get("effective_owner_id").ok(),
        created_at: r.get("created_at"),
    }
}

// =============================================================================
// Helpers
// =============================================================================

async fn ensure_photo_in_project(
    db: &sqlx::PgPool,
    project_id: Uuid,
    photo_id: Uuid,
) -> AppResult<()> {
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM photos WHERE id = $1 AND project_id = $2")
            .bind(photo_id)
            .bind(project_id)
            .fetch_optional(db)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("photo not found".into()));
    }
    Ok(())
}

// =============================================================================
// GET /api/projects/:pid/photos/:photo_id/detections
// =============================================================================

pub async fn list_detections_for_photo(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
    Path((project_id, photo_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<Vec<DetectionDto>>> {
    let _ = perm;
    ensure_photo_in_project(&s.db, project_id, photo_id).await?;
    let sql = format!(
        "SELECT {DETECTION_COLS} FROM detections \
         WHERE photo_id = $1 AND project_id = $2 \
         ORDER BY id"
    );
    let rows = sqlx::query(&sql)
        .bind(photo_id)
        .bind(project_id)
        .fetch_all(&s.db)
        .await?;
    Ok(Json(rows.iter().map(detection_from_row).collect()))
}

// =============================================================================
// GET /api/projects/:pid/photos/:photo_id/recognition_items
// =============================================================================

pub async fn list_items_for_photo(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
    Path((project_id, photo_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<Vec<RecognitionItemDto>>> {
    let _ = perm;
    ensure_photo_in_project(&s.db, project_id, photo_id).await?;
    let sql = format!(
        "SELECT {ITEM_COLS} FROM recognition_items \
         WHERE photo_id = $1 AND project_id = $2 \
         ORDER BY created_at, id"
    );
    let rows = sqlx::query(&sql)
        .bind(photo_id)
        .bind(project_id)
        .fetch_all(&s.db)
        .await?;
    Ok(Json(rows.iter().map(item_from_row).collect()))
}

// =============================================================================
// GET /api/projects/:pid/recognition_items  (paginated, filterable)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListItemsQuery {
    pub photo_id: Option<Uuid>,
    pub owner_type: Option<String>,
    pub owner_id: Option<Uuid>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListItemsResponse {
    pub data: Vec<RecognitionItemDto>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

pub async fn list_items(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(q): Query<ListItemsQuery>,
) -> AppResult<Json<ListItemsResponse>> {
    let _ = perm;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 200);
    let offset = (page - 1) * page_size;

    if let Some(st) = &q.status {
        if !ALLOWED_STATUSES.contains(&st.as_str()) {
            return Err(AppError::InvalidInput(format!("invalid status '{st}'")));
        }
    }
    if let Some(t) = &q.owner_type {
        if !FILTER_OWNER_TYPES.contains(&t.as_str()) {
            return Err(AppError::InvalidInput(format!("invalid owner_type '{t}'")));
        }
    }
    if q.owner_id.is_some() && q.owner_type.is_none() {
        return Err(AppError::InvalidInput(
            "owner_id requires owner_type".into(),
        ));
    }

    // count
    let mut count_qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT COUNT(*)::bigint AS n FROM recognition_items WHERE project_id = ",
    );
    count_qb.push_bind(project_id);
    push_filters(&mut count_qb, &q);
    let total: i64 = count_qb.build().fetch_one(&s.db).await?.get("n");

    // data
    let mut data_qb: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "SELECT {ITEM_COLS} FROM recognition_items WHERE project_id = "
    ));
    data_qb.push_bind(project_id);
    push_filters(&mut data_qb, &q);
    data_qb
        .push(" ORDER BY created_at DESC, id LIMIT ")
        .push_bind(page_size)
        .push(" OFFSET ")
        .push_bind(offset);
    let rows = data_qb.build().fetch_all(&s.db).await?;
    let data: Vec<RecognitionItemDto> = rows.iter().map(item_from_row).collect();

    Ok(Json(ListItemsResponse {
        data,
        page,
        page_size,
        total,
    }))
}

fn push_filters(qb: &mut QueryBuilder<'_, Postgres>, q: &ListItemsQuery) {
    if let Some(p) = q.photo_id {
        qb.push(" AND photo_id = ").push_bind(p);
    }
    if let Some(st) = q.status.as_ref() {
        qb.push(" AND status = ")
            .push_bind(st.clone())
            .push("::match_status");
    }
    if let Some(t) = q.owner_type.as_ref() {
        qb.push(" AND COALESCE(corrected_owner_type::text, suggested_owner_type::text) = ")
            .push_bind(t.clone());
        if let Some(oid) = q.owner_id {
            qb.push(" AND COALESCE(corrected_owner_id, suggested_owner_id) = ")
                .push_bind(oid);
        }
    }
}

// =============================================================================
// GET /api/projects/:pid/recognition_items/:id
// =============================================================================

pub async fn get_item(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
    Path((project_id, item_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<RecognitionItemDto>> {
    let _ = perm;
    let sql = format!(
        "SELECT {ITEM_COLS} FROM recognition_items \
         WHERE id = $1 AND project_id = $2"
    );
    let row = sqlx::query(&sql)
        .bind(item_id)
        .bind(project_id)
        .fetch_optional(&s.db)
        .await?;
    let row = row.ok_or_else(|| AppError::NotFound("recognition_item not found".into()))?;
    Ok(Json(item_from_row(&row)))
}

// =============================================================================
// PATCH /api/projects/:pid/recognition_items/:id/correct
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CorrectInput {
    /// `Some("person"|"tool"|"device")` together with `owner_id` sets a manual
    /// correction. Both `None` clears the correction. Mixing one of the two is
    /// rejected with `INVALID_INPUT`.
    pub owner_type: Option<String>,
    pub owner_id: Option<Uuid>,
}

pub async fn correct_item(
    perm: RequireProjectPerm<UploadPerm>,
    State(s): State<AppState>,
    Path((project_id, item_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<CorrectInput>,
) -> AppResult<(StatusCode, Json<RecognitionItemDto>)> {
    let access = perm.into_access();
    let user_id = access.user.id;

    // Decide intent: clear vs set.
    enum Intent {
        Clear,
        Set(String, Uuid),
    }
    let intent = match (input.owner_type.as_deref(), input.owner_id) {
        (None, None) => Intent::Clear,
        (Some(""), None) => Intent::Clear,
        (Some(t), Some(oid)) if !t.is_empty() => Intent::Set(t.to_string(), oid),
        _ => {
            return Err(AppError::InvalidInput(
                "owner_type and owner_id must be provided together; omit both to clear".into(),
            ));
        }
    };

    if let Intent::Set(t, _) = &intent {
        if !CORRECTION_OWNER_TYPES.contains(&t.as_str()) {
            return Err(AppError::InvalidInput(format!(
                "invalid owner_type '{t}' (allowed: person|tool|device)"
            )));
        }
    }

    // Load existing.
    let sel_sql = format!(
        "SELECT {ITEM_COLS} FROM recognition_items \
         WHERE id = $1 AND project_id = $2"
    );
    let row = sqlx::query(&sel_sql)
        .bind(item_id)
        .bind(project_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| AppError::NotFound("recognition_item not found".into()))?;
    let before = item_from_row(&row);

    // Verify owner exists for the requested type.
    if let Intent::Set(t, oid) = &intent {
        let table = match t.as_str() {
            "person" => "persons",
            "tool" => "tools",
            "device" => "devices",
            _ => unreachable!(),
        };
        let exists: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*)::bigint FROM {table} WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(*oid)
        .fetch_one(&s.db)
        .await?;
        if exists == 0 {
            return Err(AppError::NotFound(format!("{t} owner not found")));
        }
    }

    let updated = match intent {
        Intent::Clear => {
            let upd_sql = format!(
                "UPDATE recognition_items SET \
                    corrected_owner_type = NULL, \
                    corrected_owner_id   = NULL, \
                    corrected_by         = NULL, \
                    corrected_at         = NULL, \
                    status = COALESCE( \
                        CASE WHEN suggested_owner_id IS NOT NULL THEN 'matched'::match_status END, \
                        'unmatched'::match_status) \
                 WHERE id = $1 AND project_id = $2 \
                 RETURNING {ITEM_COLS}"
            );
            sqlx::query(&upd_sql)
                .bind(item_id)
                .bind(project_id)
                .fetch_one(&s.db)
                .await?
        }
        Intent::Set(t, oid) => {
            let upd_sql = format!(
                "UPDATE recognition_items SET \
                    corrected_owner_type = $1::owner_type, \
                    corrected_owner_id   = $2, \
                    corrected_by         = $3, \
                    corrected_at         = now(), \
                    status               = 'manual_corrected'::match_status \
                 WHERE id = $4 AND project_id = $5 \
                 RETURNING {ITEM_COLS}"
            );
            sqlx::query(&upd_sql)
                .bind(t)
                .bind(oid)
                .bind(user_id)
                .bind(item_id)
                .bind(project_id)
                .fetch_one(&s.db)
                .await?
        }
    };

    let after = item_from_row(&updated);

    Audit::new("recognition_item.correct", "recognition_item")
        .actor(user_id)
        .project(project_id)
        .target(item_id.to_string())
        .before(serde_json::to_value(&before).unwrap_or(json!({})))
        .after(serde_json::to_value(&after).unwrap_or(json!({})))
        .write(&s.db)
        .await;

    Ok((StatusCode::OK, Json(after)))
}
