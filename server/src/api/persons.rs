//! Global master data: persons.
//!
//! All writes are admin-only (RequireAdmin). Reads are open to any logged-in
//! user — members need to be able to look up names / employee numbers when
//! viewing photos that reference a person. `deleted_at` is a soft-delete
//! marker; deleted rows are hidden from list/get unless an admin explicitly
//! sets `?include_deleted=1`.

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

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone)]
pub struct PersonDto {
    pub id: Uuid,
    pub employee_no: String,
    pub name: String,
    pub department: Option<String>,
    pub phone: Option<String>,
    pub photo_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub data: Vec<PersonDto>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

type PersonRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    i32,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn row_to_dto(r: PersonRow) -> PersonDto {
    PersonDto {
        id: r.0,
        employee_no: r.1,
        name: r.2,
        department: r.3,
        phone: r.4,
        photo_count: r.5,
        created_at: r.6,
        updated_at: r.7,
        deleted_at: r.8,
    }
}

// ---------------------------------------------------------------------------
// LIST
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    pub q: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub include_deleted: Option<bool>,
}

/// `GET /api/persons` — any logged-in user.
pub async fn list(
    user: CurrentUser,
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ListResponse>> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;
    // Only admins may peek at soft-deleted rows.
    let include_deleted = q.include_deleted.unwrap_or(false) && user.is_admin();
    let exclude_deleted = !include_deleted;

    let pattern = match q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => format!("%{}%", p),
        None => "%".to_string(),
    };

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM persons \
         WHERE (NOT $1 OR deleted_at IS NULL) \
           AND (employee_no ILIKE $2 OR name ILIKE $2 \
                OR coalesce(department,'') ILIKE $2 \
                OR coalesce(phone,'') ILIKE $2)",
    )
    .bind(exclude_deleted)
    .bind(&pattern)
    .fetch_one(&s.db)
    .await?;

    let rows: Vec<PersonRow> = sqlx::query_as(
        "SELECT id, employee_no, name, department, phone, photo_count, \
                created_at, updated_at, deleted_at \
         FROM persons \
         WHERE (NOT $1 OR deleted_at IS NULL) \
           AND (employee_no ILIKE $2 OR name ILIKE $2 \
                OR coalesce(department,'') ILIKE $2 \
                OR coalesce(phone,'') ILIKE $2) \
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

// ---------------------------------------------------------------------------
// GET ONE
// ---------------------------------------------------------------------------

/// `GET /api/persons/:id` — any logged-in user.
pub async fn get_one(
    _user: CurrentUser,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<Json<PersonDto>> {
    let dto = load_dto(&s, id).await?;
    Ok(Json(dto))
}

// ---------------------------------------------------------------------------
// CREATE
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateInput {
    pub employee_no: String,
    pub name: String,
    pub department: Option<String>,
    pub phone: Option<String>,
}

/// `POST /api/persons` — admin only.
pub async fn create(
    RequireAdmin(user): RequireAdmin,
    State(s): State<AppState>,
    Json(input): Json<CreateInput>,
) -> AppResult<(StatusCode, Json<PersonDto>)> {
    let employee_no = input.employee_no.trim().to_string();
    let name = input.name.trim().to_string();
    if employee_no.is_empty() || name.is_empty() {
        return Err(AppError::InvalidInput(
            "employee_no and name are required".into(),
        ));
    }
    if employee_no.len() > 64 {
        return Err(AppError::InvalidInput("employee_no too long".into()));
    }
    if name.len() > 200 {
        return Err(AppError::InvalidInput("name too long".into()));
    }
    let department = input
        .department
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let phone = input
        .phone
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let row: PersonRow = sqlx::query_as(
        "INSERT INTO persons (employee_no, name, department, phone) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, employee_no, name, department, phone, photo_count, \
                   created_at, updated_at, deleted_at",
    )
    .bind(&employee_no)
    .bind(&name)
    .bind(department.as_deref())
    .bind(phone.as_deref())
    .fetch_one(&s.db)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!("employee_no '{employee_no}' already exists"))
        } else {
            AppError::Db(e)
        }
    })?;

    let dto = row_to_dto(row);

    Audit::new("person.create", "person")
        .actor(user.id)
        .target(dto.id.to_string())
        .after(json!({
            "employee_no": dto.employee_no,
            "name": dto.name,
            "department": dto.department,
            "phone": dto.phone,
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
    pub employee_no: Option<String>,
    pub name: Option<String>,
    /// Use `Some(None)` (JSON `null`) to clear the field; omit to keep.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub department: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub phone: Option<Option<String>>,
}

/// `PATCH /api/persons/:id` — admin only.
pub async fn patch(
    RequireAdmin(user): RequireAdmin,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
    Json(input): Json<PatchInput>,
) -> AppResult<Json<PersonDto>> {
    let before = load_dto(&s, id).await?;

    let employee_no = match input.employee_no {
        Some(v) => {
            let v = v.trim().to_string();
            if v.is_empty() {
                return Err(AppError::InvalidInput(
                    "employee_no must not be empty".into(),
                ));
            }
            if v.len() > 64 {
                return Err(AppError::InvalidInput("employee_no too long".into()));
            }
            v
        }
        None => before.employee_no.clone(),
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
    let department = match input.department {
        Some(opt) => opt.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        None => before.department.clone(),
    };
    let phone = match input.phone {
        Some(opt) => opt.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        None => before.phone.clone(),
    };

    sqlx::query(
        "UPDATE persons \
         SET employee_no=$1, name=$2, department=$3, phone=$4, updated_at=now() \
         WHERE id=$5",
    )
    .bind(&employee_no)
    .bind(&name)
    .bind(department.as_deref())
    .bind(phone.as_deref())
    .bind(id)
    .execute(&s.db)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!("employee_no '{employee_no}' already exists"))
        } else {
            AppError::Db(e)
        }
    })?;

    let after = load_dto(&s, id).await?;

    Audit::new("person.update", "person")
        .actor(user.id)
        .target(id.to_string())
        .before(json!({
            "employee_no": before.employee_no,
            "name": before.name,
            "department": before.department,
            "phone": before.phone,
        }))
        .after(json!({
            "employee_no": after.employee_no,
            "name": after.name,
            "department": after.department,
            "phone": after.phone,
        }))
        .write(&s.db)
        .await;

    Ok(Json(after))
}

// ---------------------------------------------------------------------------
// SOFT DELETE
// ---------------------------------------------------------------------------

/// `DELETE /api/persons/:id` — admin only, soft delete via `deleted_at`.
pub async fn soft_delete(
    RequireAdmin(user): RequireAdmin,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<StatusCode> {
    let row = sqlx::query(
        "UPDATE persons SET deleted_at=now(), updated_at=now() \
         WHERE id=$1 AND deleted_at IS NULL RETURNING id",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    if row.is_none() {
        let exists: Option<(bool,)> =
            sqlx::query_as("SELECT (deleted_at IS NOT NULL) FROM persons WHERE id=$1")
                .bind(id)
                .fetch_optional(&s.db)
                .await?;
        return match exists {
            None => Err(AppError::NotFound("person".into())),
            Some((true,)) => Err(AppError::Conflict("person already deleted".into())),
            Some((false,)) => Err(AppError::Db(sqlx::Error::RowNotFound)),
        };
    }

    Audit::new("person.delete", "person")
        .actor(user.id)
        .target(id.to_string())
        .write(&s.db)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// RESTORE
// ---------------------------------------------------------------------------

/// `POST /api/persons/:id/restore` — admin only.
pub async fn restore(
    RequireAdmin(user): RequireAdmin,
    Path(id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<Json<PersonDto>> {
    let row = sqlx::query(
        "UPDATE persons SET deleted_at=NULL, updated_at=now() \
         WHERE id=$1 AND deleted_at IS NOT NULL RETURNING id",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    if row.is_none() {
        let exists: Option<(bool,)> =
            sqlx::query_as("SELECT (deleted_at IS NOT NULL) FROM persons WHERE id=$1")
                .bind(id)
                .fetch_optional(&s.db)
                .await?;
        return match exists {
            None => Err(AppError::NotFound("person".into())),
            Some((false,)) => Err(AppError::Conflict("person is not deleted".into())),
            Some((true,)) => Err(AppError::Db(sqlx::Error::RowNotFound)),
        };
    }

    Audit::new("person.restore", "person")
        .actor(user.id)
        .target(id.to_string())
        .write(&s.db)
        .await;

    Ok(Json(load_dto(&s, id).await?))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn load_dto(s: &AppState, id: Uuid) -> AppResult<PersonDto> {
    let row: Option<PersonRow> = sqlx::query_as(
        "SELECT id, employee_no, name, department, phone, photo_count, \
                created_at, updated_at, deleted_at \
         FROM persons WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    let r = row.ok_or_else(|| AppError::NotFound("person".into()))?;
    Ok(row_to_dto(r))
}
