//! Global master data: devices.
//!
//! Same shape as `tools` but with a `model` field instead of `category`.
//! See `persons.rs` for the soft-delete + admin-only-write conventions.

use crate::api::{is_unique_violation, AppState};
use crate::audit::Audit;
use crate::auth::{CurrentUser, RequireAdmin};
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
/// `Some(None)`. Used on `Option<Option<T>>` PATCH fields to tell
/// "unspecified" apart from "set to null (clear it)".
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, Clone)]
pub struct DeviceDto {
    pub id: Uuid,
    pub sn: String,
    pub name: String,
    pub model: Option<String>,
    pub photo_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub data: Vec<DeviceDto>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

type DeviceRow = (
    Uuid,
    String,
    String,
    Option<String>,
    i32,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn row_to_dto(r: DeviceRow) -> DeviceDto {
    DeviceDto {
        id: r.0,
        sn: r.1,
        name: r.2,
        model: r.3,
        photo_count: r.4,
        created_at: r.5,
        updated_at: r.6,
        deleted_at: r.7,
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    pub q: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub include_deleted: Option<bool>,
}

pub async fn list(
    user: CurrentUser,
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;
    let include_deleted = q.include_deleted.unwrap_or(false) && user.is_admin();
    let exclude_deleted = !include_deleted;

    let pattern = match q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => format!("%{}%", p),
        None => "%".to_string(),
    };

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM devices \
         WHERE (NOT $1 OR deleted_at IS NULL) \
           AND (sn ILIKE $2 OR name ILIKE $2 \
                OR coalesce(model,'') ILIKE $2)",
    )
    .bind(exclude_deleted)
    .bind(&pattern)
    .fetch_one(&s.db)
    .await?;

    let rows: Vec<DeviceRow> = sqlx::query_as(
        "SELECT id, sn, name, model, photo_count, \
                created_at, updated_at, deleted_at \
         FROM devices \
         WHERE (NOT $1 OR deleted_at IS NULL) \
           AND (sn ILIKE $2 OR name ILIKE $2 \
                OR coalesce(model,'') ILIKE $2) \
         ORDER BY created_at DESC, id DESC \
         LIMIT $3 OFFSET $4",
    )
    .bind(exclude_deleted)
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

pub async fn get_one(
    _user: CurrentUser,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<Json<DeviceDto>> {
    Ok(Json(load_dto(&s, id).await?))
}

#[derive(Debug, Deserialize)]
pub struct CreateInput {
    pub sn: String,
    pub name: String,
    pub model: Option<String>,
}

pub async fn create(
    RequireAdmin(user): RequireAdmin,
    State(s): State<AppState>,
    Json(input): Json<CreateInput>,
) -> AppResult<(StatusCode, Json<DeviceDto>)> {
    let sn = input.sn.trim().to_string();
    let name = input.name.trim().to_string();
    if sn.is_empty() || name.is_empty() {
        return Err(AppError::InvalidInput("sn and name are required".into()));
    }
    if sn.len() > 64 {
        return Err(AppError::InvalidInput("sn too long".into()));
    }
    if name.len() > 200 {
        return Err(AppError::InvalidInput("name too long".into()));
    }
    let model = input
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let row: DeviceRow = sqlx::query_as(
        "INSERT INTO devices (sn, name, model) \
         VALUES ($1, $2, $3) \
         RETURNING id, sn, name, model, photo_count, \
                   created_at, updated_at, deleted_at",
    )
    .bind(&sn)
    .bind(&name)
    .bind(model.as_deref())
    .fetch_one(&s.db)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!("sn '{sn}' already exists"))
        } else {
            AppError::Db(e)
        }
    })?;

    let dto = row_to_dto(row);

    Audit::new("device.create", "device")
        .actor(user.id)
        .target(dto.id.to_string())
        .after(json!({
            "sn": dto.sn,
            "name": dto.name,
            "model": dto.model,
        }))
        .write(&s.db)
        .await;

    Ok((StatusCode::CREATED, Json(dto)))
}

#[derive(Debug, Deserialize)]
pub struct PatchInput {
    pub sn: Option<String>,
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub model: Option<Option<String>>,
}

pub async fn patch(
    RequireAdmin(user): RequireAdmin,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
    Json(input): Json<PatchInput>,
) -> AppResult<Json<DeviceDto>> {
    let before = load_dto(&s, id).await?;

    let sn = match input.sn {
        Some(v) => {
            let v = v.trim().to_string();
            if v.is_empty() {
                return Err(AppError::InvalidInput("sn must not be empty".into()));
            }
            if v.len() > 64 {
                return Err(AppError::InvalidInput("sn too long".into()));
            }
            v
        }
        None => before.sn.clone(),
    };
    let name = match input.name {
        Some(v) => {
            let v = v.trim().to_string();
            if v.is_empty() {
                return Err(AppError::InvalidInput("name must not be empty".into()));
            }
            if v.len() > 200 {
                return Err(AppError::InvalidInput("name too long".into()));
            }
            v
        }
        None => before.name.clone(),
    };
    let model = match input.model {
        Some(opt) => opt.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        None => before.model.clone(),
    };

    sqlx::query("UPDATE devices SET sn=$1, name=$2, model=$3, updated_at=now() WHERE id=$4")
        .bind(&sn)
        .bind(&name)
        .bind(model.as_deref())
        .bind(id)
        .execute(&s.db)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict(format!("sn '{sn}' already exists"))
            } else {
                AppError::Db(e)
            }
        })?;

    let after = load_dto(&s, id).await?;

    Audit::new("device.update", "device")
        .actor(user.id)
        .target(id.to_string())
        .before(json!({
            "sn": before.sn,
            "name": before.name,
            "model": before.model,
        }))
        .after(json!({
            "sn": after.sn,
            "name": after.name,
            "model": after.model,
        }))
        .write(&s.db)
        .await;

    Ok(Json(after))
}

pub async fn soft_delete(
    RequireAdmin(user): RequireAdmin,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<StatusCode> {
    let row = sqlx::query(
        "UPDATE devices SET deleted_at=now(), updated_at=now() \
         WHERE id=$1 AND deleted_at IS NULL RETURNING id",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    if row.is_none() {
        let exists: Option<(bool,)> =
            sqlx::query_as("SELECT (deleted_at IS NOT NULL) FROM devices WHERE id=$1")
                .bind(id)
                .fetch_optional(&s.db)
                .await?;
        return match exists {
            None => Err(AppError::NotFound("device".into())),
            Some((true,)) => Err(AppError::Conflict("device already deleted".into())),
            Some((false,)) => Err(AppError::Db(sqlx::Error::RowNotFound)),
        };
    }

    Audit::new("device.delete", "device")
        .actor(user.id)
        .target(id.to_string())
        .write(&s.db)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore(
    RequireAdmin(user): RequireAdmin,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<Json<DeviceDto>> {
    let row = sqlx::query(
        "UPDATE devices SET deleted_at=NULL, updated_at=now() \
         WHERE id=$1 AND deleted_at IS NOT NULL RETURNING id",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    if row.is_none() {
        let exists: Option<(bool,)> =
            sqlx::query_as("SELECT (deleted_at IS NOT NULL) FROM devices WHERE id=$1")
                .bind(id)
                .fetch_optional(&s.db)
                .await?;
        return match exists {
            None => Err(AppError::NotFound("device".into())),
            Some((false,)) => Err(AppError::Conflict("device is not deleted".into())),
            Some((true,)) => Err(AppError::Db(sqlx::Error::RowNotFound)),
        };
    }

    Audit::new("device.restore", "device")
        .actor(user.id)
        .target(id.to_string())
        .write(&s.db)
        .await;

    Ok(Json(load_dto(&s, id).await?))
}

async fn load_dto(s: &AppState, id: Uuid) -> AppResult<DeviceDto> {
    let row: Option<DeviceRow> = sqlx::query_as(
        "SELECT id, sn, name, model, photo_count, \
                created_at, updated_at, deleted_at \
         FROM devices WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    let r = row.ok_or_else(|| AppError::NotFound("device".into()))?;
    Ok(row_to_dto(r))
}
