//! Project-scoped photos: multipart upload + dedupe + recognition_queue enqueue.
//!
//! Schema (see `migrations/20260426140000_init.sql`):
//!   photos(id, project_id, work_order_id, owner_type, owner_id,
//!          hash, path, thumb_path, archive_path, annotated_path,
//!          angle, width, height, bytes, status, exif,
//!          uploaded_by, created_at, updated_at)
//!          UNIQUE(project_id, hash)
//!   recognition_queue(id, project_id, photo_id, attempts, locked_until,
//!                     last_error, created_at)
//!
//! Routes (see docs/api.md §6):
//!   POST   /api/projects/:project_id/photos                upload
//!   GET    /api/projects/:project_id/photos                view
//!   GET    /api/projects/:project_id/photos/:id            view
//!   PATCH  /api/projects/:project_id/photos/:id            upload
//!   DELETE /api/projects/:project_id/photos/:id            delete
//!   GET    /api/projects/:project_id/work_orders/:id/photos view
//!
//! Multipart fields (POST):
//!   file (required, binary)
//!   wo_id | wo_code (one of two; references a work_order in the project)
//!   owner_type (required: person|tool|device|wo_raw)
//!   owner_id (optional uuid)
//!   employee_no (optional, when owner_type=person)
//!   sn (optional, when owner_type=tool|device)
//!   angle (optional: front|side|back|unknown, default unknown)

use crate::api::{is_unique_violation, AppState};
use crate::audit::Audit;
use crate::auth::{DeletePerm, RequireProjectPerm, UploadPerm, ViewPerm};
use crate::error::{AppError, AppResult};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::path::PathBuf;
use uuid::Uuid;

const MAX_UPLOAD_MB_HARD_CAP: i64 = 1024;
const ALLOWED_OWNER_TYPES: &[&str] = &["person", "tool", "device", "wo_raw"];
const ALLOWED_ANGLES: &[&str] = &["front", "side", "back", "unknown"];
const ALLOWED_STATUS: &[&str] = &[
    "pending",
    "processing",
    "matched",
    "unmatched",
    "learning",
    "failed",
];

fn validate_owner_type(s: &str) -> AppResult<&'static str> {
    for &v in ALLOWED_OWNER_TYPES {
        if v == s {
            return Ok(v);
        }
    }
    Err(AppError::InvalidInput(format!("invalid owner_type: {s}")))
}

fn validate_angle(s: &str) -> AppResult<&'static str> {
    for &v in ALLOWED_ANGLES {
        if v == s {
            return Ok(v);
        }
    }
    Err(AppError::InvalidInput(format!("invalid angle: {s}")))
}

/// `Option<Option<T>>` PATCH helper: see persons.rs for context.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
pub struct PhotoDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub work_order_id: Option<Uuid>,
    pub owner_type: Option<String>,
    pub owner_id: Option<Uuid>,
    pub hash: String,
    pub path: String,
    pub angle: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub bytes: Option<i64>,
    pub status: String,
    pub uploaded_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn row_to_dto(row: &sqlx::postgres::PgRow) -> PhotoDto {
    PhotoDto {
        id: row.get(0),
        project_id: row.get(1),
        work_order_id: row.get(2),
        owner_type: row.get(3),
        owner_id: row.get(4),
        hash: row.get(5),
        path: row.get(6),
        angle: row.get(7),
        width: row.get(8),
        height: row.get(9),
        bytes: row.get(10),
        status: row.get(11),
        uploaded_by: row.get(12),
        created_at: row.get(13),
        updated_at: row.get(14),
    }
}

const SELECT_COLS: &str = "id, project_id, work_order_id, owner_type::text, owner_id, hash, path, \
     angle::text, width, height, bytes, status::text, uploaded_by, created_at, updated_at";

#[derive(Serialize)]
pub struct UploadResponse {
    pub id: Uuid,
    pub hash: String,
    pub status: String,
    pub deduped: bool,
    pub work_order_id: Option<Uuid>,
    pub owner_type: Option<String>,
    pub owner_id: Option<Uuid>,
    pub angle: String,
}

async fn read_max_upload_mb(pool: &sqlx::PgPool) -> AppResult<i64> {
    let row: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'upload.max_mb'")
            .fetch_optional(pool)
            .await?;
    let v = row.and_then(|(v,)| v.as_i64()).unwrap_or(10);
    Ok(v.clamp(1, MAX_UPLOAD_MB_HARD_CAP))
}

async fn read_allow_auto_create_person(pool: &sqlx::PgPool) -> AppResult<bool> {
    let row: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'upload.allow_auto_create_person'")
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(v,)| v.as_bool()).unwrap_or(false))
}

fn ext_for(filename: Option<&str>, content_type: Option<&str>) -> String {
    if let Some(name) = filename {
        if let Some(idx) = name.rfind('.') {
            let cleaned: String = name[idx + 1..]
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(5)
                .collect::<String>()
                .to_ascii_lowercase();
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
    }
    match content_type {
        Some("image/jpeg") | Some("image/jpg") => "jpg".into(),
        Some("image/png") => "png".into(),
        Some("image/webp") => "webp".into(),
        Some("image/gif") => "gif".into(),
        _ => "bin".into(),
    }
}

fn map_mp_err(e: axum::extract::multipart::MultipartError) -> AppError {
    AppError::InvalidInput(format!("multipart: {e}"))
}

// ---------------------------------------------------------------------------
// POST upload
// ---------------------------------------------------------------------------

pub async fn upload(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    perm: RequireProjectPerm<UploadPerm>,
    mut multipart: Multipart,
) -> AppResult<impl IntoResponse> {
    let access = perm.into_access();
    let actor = access.user.id;

    let max_mb = read_max_upload_mb(&state.db).await?;
    let max_bytes: usize = (max_mb as usize).saturating_mul(1024 * 1024);

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut field_owner_type: Option<String> = None;
    let mut field_owner_id: Option<String> = None;
    let mut field_employee_no: Option<String> = None;
    let mut field_sn: Option<String> = None;
    let mut field_wo_id: Option<String> = None;
    let mut field_wo_code: Option<String> = None;
    let mut field_angle: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(map_mp_err)? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                content_type = field.content_type().map(|s| s.to_string());
                let bytes = field.bytes().await.map_err(map_mp_err)?;
                if bytes.len() > max_bytes {
                    return Err(AppError::TooLarge(format!(
                        "file size {} bytes exceeds limit {}MB",
                        bytes.len(),
                        max_mb
                    )));
                }
                file_bytes = Some(bytes.to_vec());
            }
            "owner_type" => field_owner_type = Some(field.text().await.map_err(map_mp_err)?),
            "owner_id" => field_owner_id = Some(field.text().await.map_err(map_mp_err)?),
            "employee_no" => field_employee_no = Some(field.text().await.map_err(map_mp_err)?),
            "sn" => field_sn = Some(field.text().await.map_err(map_mp_err)?),
            "wo_id" => field_wo_id = Some(field.text().await.map_err(map_mp_err)?),
            "wo_code" => field_wo_code = Some(field.text().await.map_err(map_mp_err)?),
            "angle" => field_angle = Some(field.text().await.map_err(map_mp_err)?),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let file_bytes = file_bytes.ok_or_else(|| AppError::InvalidInput("file is required".into()))?;
    if file_bytes.is_empty() {
        return Err(AppError::InvalidInput("file is empty".into()));
    }

    let owner_type_raw = field_owner_type
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("owner_type is required".into()))?;
    let owner_type = validate_owner_type(owner_type_raw)?;

    let angle = match field_angle.as_deref() {
        Some(a) if !a.is_empty() => validate_angle(a)?,
        _ => "unknown",
    };

    // Resolve work_order_id (optional; one of wo_id/wo_code may be supplied).
    let work_order_id: Option<Uuid> = match (field_wo_id.as_deref(), field_wo_code.as_deref()) {
        (Some(s), _) if !s.is_empty() => {
            let wo_id = Uuid::parse_str(s)
                .map_err(|_| AppError::InvalidInput("wo_id is not a valid uuid".into()))?;
            let exists: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM work_orders WHERE id = $1 AND project_id = $2")
                    .bind(wo_id)
                    .bind(project_id)
                    .fetch_optional(&state.db)
                    .await?;
            if exists.is_none() {
                return Err(AppError::NotFound("work order not found in project".into()));
            }
            Some(wo_id)
        }
        (None, Some(code)) if !code.is_empty() => {
            let row: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM work_orders WHERE project_id = $1 AND code = $2")
                    .bind(project_id)
                    .bind(code)
                    .fetch_optional(&state.db)
                    .await?;
            match row {
                Some((id,)) => Some(id),
                None => {
                    return Err(AppError::NotFound(format!(
                        "work order not found: code={code}"
                    )));
                }
            }
        }
        _ => None,
    };

    // Resolve owner_id.
    let mut owner_id: Option<Uuid> = match field_owner_id.as_deref() {
        Some(s) if !s.is_empty() => Some(
            Uuid::parse_str(s)
                .map_err(|_| AppError::InvalidInput("owner_id is not a valid uuid".into()))?,
        ),
        _ => None,
    };
    if owner_id.is_none() {
        match owner_type {
            "person" => {
                if let Some(no) = field_employee_no.as_deref().filter(|s| !s.is_empty()) {
                    let row: Option<(Uuid,)> = sqlx::query_as(
                        "SELECT id FROM persons WHERE employee_no = $1 AND deleted_at IS NULL",
                    )
                    .bind(no)
                    .fetch_optional(&state.db)
                    .await?;
                    match row {
                        Some((id,)) => owner_id = Some(id),
                        None => {
                            let allow = read_allow_auto_create_person(&state.db).await?;
                            if allow {
                                let row: (Uuid,) = sqlx::query_as(
                                    "INSERT INTO persons(employee_no, name) \
                                     VALUES ($1, '\u{5f85}\u{8865}\u{5168}') RETURNING id",
                                )
                                .bind(no)
                                .fetch_one(&state.db)
                                .await?;
                                owner_id = Some(row.0);
                                Audit::new("person.auto_create", "person")
                                    .actor(actor)
                                    .target(row.0.to_string())
                                    .after(json!({"employee_no": no, "source": "photo_upload"}))
                                    .write(&state.db)
                                    .await;
                            } else {
                                return Err(AppError::InvalidInput(format!(
                                    "unknown_employee_no: {no}"
                                )));
                            }
                        }
                    }
                }
            }
            "tool" => {
                if let Some(sn) = field_sn.as_deref().filter(|s| !s.is_empty()) {
                    let row: Option<(Uuid,)> =
                        sqlx::query_as("SELECT id FROM tools WHERE sn = $1 AND deleted_at IS NULL")
                            .bind(sn)
                            .fetch_optional(&state.db)
                            .await?;
                    match row {
                        Some((id,)) => owner_id = Some(id),
                        None => {
                            return Err(AppError::InvalidInput(format!("unknown_sn: {sn}")));
                        }
                    }
                }
            }
            "device" => {
                if let Some(sn) = field_sn.as_deref().filter(|s| !s.is_empty()) {
                    let row: Option<(Uuid,)> = sqlx::query_as(
                        "SELECT id FROM devices WHERE sn = $1 AND deleted_at IS NULL",
                    )
                    .bind(sn)
                    .fetch_optional(&state.db)
                    .await?;
                    match row {
                        Some((id,)) => owner_id = Some(id),
                        None => {
                            return Err(AppError::InvalidInput(format!("unknown_sn: {sn}")));
                        }
                    }
                }
            }
            _ => {} // wo_raw — no owner lookup
        }
    }

    // Hash + path.
    let hash = {
        let mut h = Sha256::new();
        h.update(&file_bytes);
        hex::encode(h.finalize())
    };
    let bytes_len = file_bytes.len() as i64;
    let ext = ext_for(filename.as_deref(), content_type.as_deref());

    // Dedupe pre-check: if the same (project_id, hash) already exists, return
    // the existing photo_id with deduped:true and skip file write + enqueue.
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, status::text FROM photos WHERE project_id = $1 AND hash = $2")
            .bind(project_id)
            .bind(&hash)
            .fetch_optional(&state.db)
            .await?;
    if let Some((eid, estatus)) = existing {
        return Ok((
            StatusCode::ACCEPTED,
            Json(UploadResponse {
                id: eid,
                hash,
                status: estatus,
                deduped: true,
                work_order_id,
                owner_type: Some(owner_type.to_string()),
                owner_id,
                angle: angle.to_string(),
            }),
        ));
    }

    // Write file to disk first; cheap to leave behind if INSERT loses a race.
    let prefix = &hash[..2];
    let rel = format!("photos/{}/{}/{}.{}", project_id, prefix, hash, ext);
    let abs_path: PathBuf = std::path::Path::new(&state.config.data_dir).join(&rel);
    if let Some(parent) = abs_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs_path, &file_bytes)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("write file: {e}")))?;

    // INSERT photo + enqueue (transactional).
    let mut tx = state.db.begin().await?;

    let insert_res: Result<(Uuid, String), sqlx::Error> = sqlx::query_as(
        "INSERT INTO photos (\
            project_id, work_order_id, owner_type, owner_id, hash, path, \
            angle, bytes, status, uploaded_by\
         ) VALUES (\
            $1, $2, $3::owner_type, $4, $5, $6, \
            $7::angle_kind, $8, 'pending', $9\
         ) RETURNING id, status::text",
    )
    .bind(project_id)
    .bind(work_order_id)
    .bind(owner_type)
    .bind(owner_id)
    .bind(&hash)
    .bind(&rel)
    .bind(angle)
    .bind(bytes_len)
    .bind(actor)
    .fetch_one(&mut *tx)
    .await;

    let (id, status) = match insert_res {
        Ok(v) => v,
        Err(e) if is_unique_violation(&e) => {
            // Race: another concurrent upload won. Treat as dedupe.
            tx.rollback().await.ok();
            let row: (Uuid, String) = sqlx::query_as(
                "SELECT id, status::text FROM photos WHERE project_id = $1 AND hash = $2",
            )
            .bind(project_id)
            .bind(&hash)
            .fetch_one(&state.db)
            .await?;
            return Ok((
                StatusCode::ACCEPTED,
                Json(UploadResponse {
                    id: row.0,
                    hash,
                    status: row.1,
                    deduped: true,
                    work_order_id,
                    owner_type: Some(owner_type.to_string()),
                    owner_id,
                    angle: angle.to_string(),
                }),
            ));
        }
        Err(e) => return Err(AppError::Db(e)),
    };

    sqlx::query("INSERT INTO recognition_queue (project_id, photo_id) VALUES ($1, $2)")
        .bind(project_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // Best-effort NOTIFY for the (yet-unbuilt) worker (turn 7).
    let _ = sqlx::query("SELECT pg_notify('recognition_queue', $1)")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await;

    tx.commit().await?;

    Audit::new("photo.upload", "photo")
        .actor(actor)
        .project(project_id)
        .target(id.to_string())
        .after(json!({
            "hash": hash,
            "path": rel,
            "bytes": bytes_len,
            "owner_type": owner_type,
            "owner_id": owner_id,
            "work_order_id": work_order_id,
            "angle": angle,
        }))
        .write(&state.db)
        .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(UploadResponse {
            id,
            hash,
            status,
            deduped: false,
            work_order_id,
            owner_type: Some(owner_type.to_string()),
            owner_id,
            angle: angle.to_string(),
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET list / GET single
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListQuery {
    pub wo_id: Option<Uuid>,
    pub owner_type: Option<String>,
    pub owner_id: Option<Uuid>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub data: Vec<PhotoDto>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

pub async fn list(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    perm: RequireProjectPerm<ViewPerm>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    let _ = perm;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;

    if let Some(ref ot) = q.owner_type {
        validate_owner_type(ot)?;
    }
    if let Some(ref s) = q.status {
        if !ALLOWED_STATUS.contains(&s.as_str()) {
            return Err(AppError::InvalidInput(format!("invalid status: {s}")));
        }
    }

    let mut sql = format!("SELECT {} FROM photos WHERE project_id = $1", SELECT_COLS);
    let mut n = 1;
    if q.wo_id.is_some() {
        n += 1;
        sql.push_str(&format!(" AND work_order_id = ${}", n));
    }
    if q.owner_type.is_some() {
        n += 1;
        sql.push_str(&format!(" AND owner_type = ${}::owner_type", n));
    }
    if q.owner_id.is_some() {
        n += 1;
        sql.push_str(&format!(" AND owner_id = ${}", n));
    }
    if q.status.is_some() {
        n += 1;
        sql.push_str(&format!(" AND status = ${}::photo_status", n));
    }
    sql.push_str(&format!(
        " ORDER BY created_at DESC OFFSET ${} LIMIT ${}",
        n + 1,
        n + 2
    ));

    let mut qb = sqlx::query(&sql).bind(project_id);
    if let Some(v) = q.wo_id {
        qb = qb.bind(v);
    }
    if let Some(ref v) = q.owner_type {
        qb = qb.bind(v.clone());
    }
    if let Some(v) = q.owner_id {
        qb = qb.bind(v);
    }
    if let Some(ref v) = q.status {
        qb = qb.bind(v.clone());
    }
    qb = qb.bind(offset).bind(page_size);
    let rows = qb.fetch_all(&state.db).await?;
    let data: Vec<PhotoDto> = rows.iter().map(row_to_dto).collect();

    let mut sql_c = String::from("SELECT COUNT(*) FROM photos WHERE project_id = $1");
    let mut cn = 1;
    if q.wo_id.is_some() {
        cn += 1;
        sql_c.push_str(&format!(" AND work_order_id = ${}", cn));
    }
    if q.owner_type.is_some() {
        cn += 1;
        sql_c.push_str(&format!(" AND owner_type = ${}::owner_type", cn));
    }
    if q.owner_id.is_some() {
        cn += 1;
        sql_c.push_str(&format!(" AND owner_id = ${}", cn));
    }
    if q.status.is_some() {
        cn += 1;
        sql_c.push_str(&format!(" AND status = ${}::photo_status", cn));
    }
    let mut cb = sqlx::query_as::<_, (i64,)>(&sql_c).bind(project_id);
    if let Some(v) = q.wo_id {
        cb = cb.bind(v);
    }
    if let Some(ref v) = q.owner_type {
        cb = cb.bind(v.clone());
    }
    if let Some(v) = q.owner_id {
        cb = cb.bind(v);
    }
    if let Some(ref v) = q.status {
        cb = cb.bind(v.clone());
    }
    let (total,): (i64,) = cb.fetch_one(&state.db).await?;

    Ok(Json(ListResponse {
        data,
        page,
        page_size,
        total,
    }))
}

pub async fn get_one(
    State(state): State<AppState>,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    perm: RequireProjectPerm<ViewPerm>,
) -> AppResult<Json<PhotoDto>> {
    let _ = perm;
    let sql = format!(
        "SELECT {} FROM photos WHERE id = $1 AND project_id = $2",
        SELECT_COLS
    );
    let row = sqlx::query(&sql)
        .bind(id)
        .bind(project_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("photo not found: {id}")))?;
    Ok(Json(row_to_dto(&row)))
}

pub async fn list_by_work_order(
    State(state): State<AppState>,
    Path((project_id, wo_id)): Path<(Uuid, Uuid)>,
    perm: RequireProjectPerm<ViewPerm>,
) -> AppResult<Json<Vec<PhotoDto>>> {
    let _ = perm;
    // Verify the work order belongs to this project (404 otherwise).
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM work_orders WHERE id = $1 AND project_id = $2")
            .bind(wo_id)
            .bind(project_id)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound(format!("work order not found: {wo_id}")));
    }
    let sql = format!(
        "SELECT {} FROM photos WHERE project_id = $1 AND work_order_id = $2 ORDER BY created_at DESC",
        SELECT_COLS
    );
    let rows = sqlx::query(&sql)
        .bind(project_id)
        .bind(wo_id)
        .fetch_all(&state.db)
        .await?;
    let data: Vec<PhotoDto> = rows.iter().map(row_to_dto).collect();
    Ok(Json(data))
}

// ---------------------------------------------------------------------------
// PATCH (angle / owner_type / owner_id)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchBody {
    #[serde(default, deserialize_with = "deserialize_some")]
    pub angle: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub owner_type: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub owner_id: Option<Option<Uuid>>,
}

pub async fn patch(
    State(state): State<AppState>,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    perm: RequireProjectPerm<UploadPerm>,
    Json(body): Json<PatchBody>,
) -> AppResult<Json<PhotoDto>> {
    let access = perm.into_access();
    let actor = access.user.id;

    let before: Option<(Option<String>, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT owner_type::text, owner_id, angle::text \
         FROM photos WHERE id = $1 AND project_id = $2",
    )
    .bind(id)
    .bind(project_id)
    .fetch_optional(&state.db)
    .await?;
    let (b_owner_type, b_owner_id, b_angle) =
        before.ok_or_else(|| AppError::NotFound(format!("photo not found: {id}")))?;

    // Validate.
    let new_angle: Option<String> = match body.angle {
        Some(Some(s)) => Some(validate_angle(&s)?.to_string()),
        Some(None) => Some("unknown".to_string()),
        None => None,
    };
    let new_owner_type: Option<Option<String>> = match body.owner_type {
        Some(Some(s)) => Some(Some(validate_owner_type(&s)?.to_string())),
        Some(None) => Some(None),
        None => None,
    };
    let new_owner_id: Option<Option<Uuid>> = body.owner_id;

    if new_angle.is_none() && new_owner_type.is_none() && new_owner_id.is_none() {
        // No-op: just return current.
        let sql = format!(
            "SELECT {} FROM photos WHERE id = $1 AND project_id = $2",
            SELECT_COLS
        );
        let row = sqlx::query(&sql)
            .bind(id)
            .bind(project_id)
            .fetch_one(&state.db)
            .await?;
        return Ok(Json(row_to_dto(&row)));
    }

    let mut tx = state.db.begin().await?;
    if let Some(a) = &new_angle {
        sqlx::query("UPDATE photos SET angle = $1::angle_kind, updated_at = now() WHERE id = $2")
            .bind(a)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(ref ot) = new_owner_type {
        sqlx::query(
            "UPDATE photos SET owner_type = $1::owner_type, updated_at = now() WHERE id = $2",
        )
        .bind(ot.as_deref())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(oid) = new_owner_id {
        sqlx::query("UPDATE photos SET owner_id = $1, updated_at = now() WHERE id = $2")
            .bind(oid)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let sql = format!(
        "SELECT {} FROM photos WHERE id = $1 AND project_id = $2",
        SELECT_COLS
    );
    let row = sqlx::query(&sql)
        .bind(id)
        .bind(project_id)
        .fetch_one(&state.db)
        .await?;
    let dto = row_to_dto(&row);

    Audit::new("photo.update", "photo")
        .actor(actor)
        .project(project_id)
        .target(id.to_string())
        .before(json!({
            "owner_type": b_owner_type,
            "owner_id": b_owner_id,
            "angle": b_angle,
        }))
        .after(json!({
            "owner_type": dto.owner_type,
            "owner_id": dto.owner_id,
            "angle": dto.angle,
        }))
        .write(&state.db)
        .await;

    Ok(Json(dto))
}

// ---------------------------------------------------------------------------
// DELETE
// ---------------------------------------------------------------------------

pub async fn delete_one(
    State(state): State<AppState>,
    Path((project_id, id)): Path<(Uuid, Uuid)>,
    perm: RequireProjectPerm<DeletePerm>,
) -> AppResult<StatusCode> {
    let access = perm.into_access();
    let actor = access.user.id;

    let before: Option<(String, String)> =
        sqlx::query_as("SELECT path, hash FROM photos WHERE id = $1 AND project_id = $2")
            .bind(id)
            .bind(project_id)
            .fetch_optional(&state.db)
            .await?;
    let (path_rel, hash) =
        before.ok_or_else(|| AppError::NotFound(format!("photo not found: {id}")))?;

    // CASCADE handles detections / recognition_items / recognition_queue.
    let r = sqlx::query("DELETE FROM photos WHERE id = $1 AND project_id = $2")
        .bind(id)
        .bind(project_id)
        .execute(&state.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("photo not found: {id}")));
    }

    let abs = std::path::Path::new(&state.config.data_dir).join(&path_rel);
    let _ = tokio::fs::remove_file(&abs).await;

    Audit::new("photo.delete", "photo")
        .actor(actor)
        .project(project_id)
        .target(id.to_string())
        .before(json!({"hash": hash, "path": path_rel}))
        .write(&state.db)
        .await;

    Ok(StatusCode::NO_CONTENT)
}
