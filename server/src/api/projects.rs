use crate::api::AppState;
use crate::audit::Audit;
use crate::auth::{CurrentUser, ManagePerm, RequireAdmin, RequireProjectPerm, ViewPerm};
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

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ProjectDto {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub icon: Option<String>,
    pub description: Option<String>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct MemberDto {
    pub user_id: Uuid,
    pub username: String,
    pub full_name: Option<String>,
    pub role: String,
    pub can_view: bool,
    pub can_upload: bool,
    pub can_delete: bool,
    pub can_manage: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct MyPermsDto {
    pub is_admin: bool,
    pub archived: bool,
    pub can_view: bool,
    pub can_upload: bool,
    pub can_delete: bool,
    pub can_manage: bool,
}

// ---------------------------------------------------------------------------
// LIST / CREATE / GET / PATCH / DELETE projects
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// `active` (default) | `archived` | `all`
    #[serde(default)]
    pub archived: Option<String>,
}

/// `GET /api/projects`
///
/// Admins see every project; non-admin users see only the projects they
/// belong to via `project_members`.
pub async fn list_projects(
    user: CurrentUser,
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<ProjectDto>>> {
    let mode = q.archived.as_deref().unwrap_or("active");
    let arch_pred = match mode {
        "all" => "TRUE",
        "archived" => "archived_at IS NOT NULL",
        "active" | "" => "archived_at IS NULL",
        other => return Err(AppError::InvalidInput(format!("unknown archived={other}"))),
    };

    let rows: Vec<(
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = if user.is_admin() {
        sqlx::query_as(&format!(
            "SELECT id, code, name, icon, description, archived_at, created_at, updated_at \
             FROM projects WHERE {arch_pred} ORDER BY created_at"
        ))
        .fetch_all(&s.db)
        .await?
    } else {
        sqlx::query_as(&format!(
            "SELECT p.id, p.code, p.name, p.icon, p.description, p.archived_at, p.created_at, p.updated_at \
             FROM projects p JOIN project_members m ON m.project_id = p.id \
             WHERE m.user_id = $1 AND {arch_pred} ORDER BY p.created_at"
        ))
        .bind(user.id)
        .fetch_all(&s.db)
        .await?
    };

    Ok(Json(
        rows.into_iter()
            .map(|r| ProjectDto {
                id: r.0,
                code: r.1,
                name: r.2,
                icon: r.3,
                description: r.4,
                archived_at: r.5,
                created_at: r.6,
                updated_at: r.7,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateInput {
    pub code: String,
    pub name: String,
    pub icon: Option<String>,
    pub description: Option<String>,
}

/// `POST /api/projects` (admin only)
pub async fn create_project(
    RequireAdmin(user): RequireAdmin,
    State(s): State<AppState>,
    Json(input): Json<CreateInput>,
) -> AppResult<(StatusCode, Json<ProjectDto>)> {
    let code = input.code.trim();
    let name = input.name.trim();
    if code.is_empty() || name.is_empty() {
        return Err(AppError::InvalidInput("code and name are required".into()));
    }
    if code.len() > 64 || name.len() > 200 {
        return Err(AppError::InvalidInput("code/name too long".into()));
    }

    let row: Result<
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<DateTime<Utc>>,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
        sqlx::Error,
    > = sqlx::query_as(
        "INSERT INTO projects (code, name, icon, description, created_by) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, code, name, icon, description, archived_at, created_at, updated_at",
    )
    .bind(code)
    .bind(name)
    .bind(input.icon.as_deref())
    .bind(input.description.as_deref())
    .bind(user.id)
    .fetch_one(&s.db)
    .await;

    let row = row.map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!("project code '{code}' already exists"))
        } else {
            AppError::Db(e)
        }
    })?;

    let dto = ProjectDto {
        id: row.0,
        code: row.1.clone(),
        name: row.2.clone(),
        icon: row.3.clone(),
        description: row.4.clone(),
        archived_at: row.5,
        created_at: row.6,
        updated_at: row.7,
    };

    Audit::new("project.create", "project")
        .actor(user.id)
        .project(dto.id)
        .target(dto.id.to_string())
        .after(json!({"code": dto.code, "name": dto.name, "icon": dto.icon, "description": dto.description}))
        .write(&s.db)
        .await;

    Ok((StatusCode::CREATED, Json(dto)))
}

/// `GET /api/projects/{project_id}` (view perm)
pub async fn get_project(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
) -> AppResult<Json<ProjectDto>> {
    load_project_dto(&s, perm.access.project_id).await.map(Json)
}

/// `GET /api/projects/{project_id}/me` (view perm)
///
/// Useful for the frontend to know what buttons to render. Returns the
/// caller's effective permissions on this project.
pub async fn get_my_perms(perm: RequireProjectPerm<ViewPerm>) -> Json<MyPermsDto> {
    let a = perm.access;
    Json(MyPermsDto {
        is_admin: a.is_admin,
        archived: a.archived,
        can_view: a.perms.can_view,
        can_upload: a.perms.can_upload,
        can_delete: a.perms.can_delete,
        can_manage: a.perms.can_manage,
    })
}

#[derive(Debug, Deserialize)]
pub struct PatchInput {
    pub name: Option<String>,
    pub icon: Option<Option<String>>,
    pub description: Option<Option<String>>,
}

/// `PATCH /api/projects/{project_id}` (manage perm or admin)
pub async fn patch_project(
    perm: RequireProjectPerm<ManagePerm>,
    State(s): State<AppState>,
    Json(input): Json<PatchInput>,
) -> AppResult<Json<ProjectDto>> {
    let pid = perm.access.project_id;
    let before = load_project_dto(&s, pid).await?;

    let mut name = before.name.clone();
    if let Some(n) = input.name {
        let n = n.trim().to_string();
        if n.is_empty() {
            return Err(AppError::InvalidInput("name must not be empty".into()));
        }
        if n.len() > 200 {
            return Err(AppError::InvalidInput("name too long".into()));
        }
        name = n;
    }
    let icon = input.icon.unwrap_or(before.icon.clone());
    let description = input.description.unwrap_or(before.description.clone());

    sqlx::query(
        "UPDATE projects SET name=$1, icon=$2, description=$3, updated_at=now() WHERE id=$4",
    )
    .bind(&name)
    .bind(icon.as_deref())
    .bind(description.as_deref())
    .bind(pid)
    .execute(&s.db)
    .await?;

    let after = load_project_dto(&s, pid).await?;

    Audit::new("project.update", "project")
        .actor(perm.access.user.id)
        .project(pid)
        .target(pid.to_string())
        .before(
            json!({"name": before.name, "icon": before.icon, "description": before.description}),
        )
        .after(json!({"name": after.name, "icon": after.icon, "description": after.description}))
        .write(&s.db)
        .await;

    Ok(Json(after))
}

/// `DELETE /api/projects/{project_id}` (admin only) — soft-delete by archiving.
pub async fn archive_project(
    RequireAdmin(user): RequireAdmin,
    Path(project_id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<StatusCode> {
    if project_id == default_project_id() {
        return Err(AppError::Conflict(
            "the default project cannot be archived".into(),
        ));
    }

    let row = sqlx::query("UPDATE projects SET archived_at=now(), updated_at=now() WHERE id=$1 AND archived_at IS NULL RETURNING id")
        .bind(project_id)
        .fetch_optional(&s.db)
        .await?;
    if row.is_none() {
        // Either it doesn't exist or it is already archived.
        let exists: Option<(bool,)> =
            sqlx::query_as("SELECT (archived_at IS NOT NULL) FROM projects WHERE id=$1")
                .bind(project_id)
                .fetch_optional(&s.db)
                .await?;
        return match exists {
            None => Err(AppError::NotFound("project".into())),
            Some((true,)) => Err(AppError::Conflict("project already archived".into())),
            Some((false,)) => Err(AppError::Db(sqlx::Error::RowNotFound)),
        };
    }

    Audit::new("project.archive", "project")
        .actor(user.id)
        .project(project_id)
        .target(project_id.to_string())
        .write(&s.db)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/projects/{project_id}/unarchive` (admin only)
pub async fn unarchive_project(
    RequireAdmin(user): RequireAdmin,
    Path(project_id): Path<Uuid>,
    State(s): State<AppState>,
) -> AppResult<Json<ProjectDto>> {
    let row = sqlx::query("UPDATE projects SET archived_at=NULL, updated_at=now() WHERE id=$1 AND archived_at IS NOT NULL RETURNING id")
        .bind(project_id)
        .fetch_optional(&s.db)
        .await?;
    if row.is_none() {
        let exists: Option<(bool,)> =
            sqlx::query_as("SELECT (archived_at IS NOT NULL) FROM projects WHERE id=$1")
                .bind(project_id)
                .fetch_optional(&s.db)
                .await?;
        return match exists {
            None => Err(AppError::NotFound("project".into())),
            Some((false,)) => Err(AppError::Conflict("project is not archived".into())),
            Some((true,)) => Err(AppError::Db(sqlx::Error::RowNotFound)),
        };
    }

    Audit::new("project.unarchive", "project")
        .actor(user.id)
        .project(project_id)
        .target(project_id.to_string())
        .write(&s.db)
        .await;

    Ok(Json(load_project_dto(&s, project_id).await?))
}

// ---------------------------------------------------------------------------
// MEMBERS
// ---------------------------------------------------------------------------

/// `GET /api/projects/{project_id}/members` (view perm)
pub async fn list_members(
    perm: RequireProjectPerm<ViewPerm>,
    State(s): State<AppState>,
) -> AppResult<Json<Vec<MemberDto>>> {
    let rows: Vec<(
        Uuid,
        String,
        Option<String>,
        String,
        bool,
        bool,
        bool,
        bool,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT m.user_id, u.username, u.full_name, u.role::text, \
                m.can_view, m.can_upload, m.can_delete, m.can_manage, m.created_at \
         FROM project_members m JOIN users u ON u.id = m.user_id \
         WHERE m.project_id = $1 \
         ORDER BY u.username",
    )
    .bind(perm.access.project_id)
    .fetch_all(&s.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| MemberDto {
                user_id: r.0,
                username: r.1,
                full_name: r.2,
                role: r.3,
                can_view: r.4,
                can_upload: r.5,
                can_delete: r.6,
                can_manage: r.7,
                created_at: r.8,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct AddMemberInput {
    pub user_id: Uuid,
    #[serde(default = "default_true")]
    pub can_view: bool,
    #[serde(default)]
    pub can_upload: bool,
    #[serde(default)]
    pub can_delete: bool,
    #[serde(default)]
    pub can_manage: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /api/projects/{project_id}/members` (manage perm or admin)
pub async fn add_member(
    perm: RequireProjectPerm<ManagePerm>,
    State(s): State<AppState>,
    Json(input): Json<AddMemberInput>,
) -> AppResult<(StatusCode, Json<MemberDto>)> {
    let pid = perm.access.project_id;
    let actor_id = perm.access.user.id;

    // Look up user (must exist and be enabled).
    let user_row: Option<(String, Option<String>, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT username, full_name, role::text, disabled_at FROM users WHERE id = $1",
    )
    .bind(input.user_id)
    .fetch_optional(&s.db)
    .await?;
    let (username, full_name, role, disabled_at) =
        user_row.ok_or_else(|| AppError::NotFound("user".into()))?;
    if disabled_at.is_some() {
        return Err(AppError::Conflict("user is disabled".into()));
    }

    let inserted: Option<(DateTime<Utc>,)> = sqlx::query_as(
        "INSERT INTO project_members (project_id, user_id, can_view, can_upload, can_delete, can_manage) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (project_id, user_id) DO NOTHING \
         RETURNING created_at",
    )
    .bind(pid)
    .bind(input.user_id)
    .bind(input.can_view)
    .bind(input.can_upload)
    .bind(input.can_delete)
    .bind(input.can_manage)
    .fetch_optional(&s.db)
    .await?;

    let created_at = match inserted {
        Some((ts,)) => ts,
        None => return Err(AppError::Conflict("user is already a member".into())),
    };

    Audit::new("project.member.add", "project_member")
        .actor(actor_id)
        .project(pid)
        .target(format!("{pid}:{}", input.user_id))
        .after(json!({
            "user_id": input.user_id,
            "can_view": input.can_view,
            "can_upload": input.can_upload,
            "can_delete": input.can_delete,
            "can_manage": input.can_manage,
        }))
        .write(&s.db)
        .await;

    Ok((
        StatusCode::CREATED,
        Json(MemberDto {
            user_id: input.user_id,
            username,
            full_name,
            role,
            can_view: input.can_view,
            can_upload: input.can_upload,
            can_delete: input.can_delete,
            can_manage: input.can_manage,
            created_at,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct PatchMemberInput {
    pub can_view: Option<bool>,
    pub can_upload: Option<bool>,
    pub can_delete: Option<bool>,
    pub can_manage: Option<bool>,
}

/// `PATCH /api/projects/{project_id}/members/{user_id}` (manage perm or admin)
pub async fn patch_member(
    perm: RequireProjectPerm<ManagePerm>,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
    State(s): State<AppState>,
    Json(input): Json<PatchMemberInput>,
) -> AppResult<Json<MemberDto>> {
    debug_assert_eq!(project_id, perm.access.project_id);
    let actor_id = perm.access.user.id;

    let before: Option<(bool, bool, bool, bool)> = sqlx::query_as(
        "SELECT can_view, can_upload, can_delete, can_manage FROM project_members \
         WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?;
    let before = before.ok_or_else(|| AppError::NotFound("member".into()))?;

    let new_view = input.can_view.unwrap_or(before.0);
    let new_upload = input.can_upload.unwrap_or(before.1);
    let new_delete = input.can_delete.unwrap_or(before.2);
    let new_manage = input.can_manage.unwrap_or(before.3);

    sqlx::query(
        "UPDATE project_members SET can_view=$1, can_upload=$2, can_delete=$3, can_manage=$4, updated_at=now() \
         WHERE project_id=$5 AND user_id=$6",
    )
    .bind(new_view)
    .bind(new_upload)
    .bind(new_delete)
    .bind(new_manage)
    .bind(project_id)
    .bind(user_id)
    .execute(&s.db)
    .await?;

    Audit::new("project.member.update", "project_member")
        .actor(actor_id)
        .project(project_id)
        .target(format!("{project_id}:{user_id}"))
        .before(json!({"can_view": before.0, "can_upload": before.1, "can_delete": before.2, "can_manage": before.3}))
        .after(json!({"can_view": new_view, "can_upload": new_upload, "can_delete": new_delete, "can_manage": new_manage}))
        .write(&s.db)
        .await;

    // Re-fetch with user info for the response.
    let row: (
        Uuid,
        String,
        Option<String>,
        String,
        bool,
        bool,
        bool,
        bool,
        DateTime<Utc>,
    ) = sqlx::query_as(
        "SELECT m.user_id, u.username, u.full_name, u.role::text, \
                    m.can_view, m.can_upload, m.can_delete, m.can_manage, m.created_at \
             FROM project_members m JOIN users u ON u.id = m.user_id \
             WHERE m.project_id=$1 AND m.user_id=$2",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_one(&s.db)
    .await?;

    Ok(Json(MemberDto {
        user_id: row.0,
        username: row.1,
        full_name: row.2,
        role: row.3,
        can_view: row.4,
        can_upload: row.5,
        can_delete: row.6,
        can_manage: row.7,
        created_at: row.8,
    }))
}

/// `DELETE /api/projects/{project_id}/members/{user_id}` (manage perm or admin)
pub async fn remove_member(
    perm: RequireProjectPerm<ManagePerm>,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
    State(s): State<AppState>,
) -> AppResult<StatusCode> {
    debug_assert_eq!(project_id, perm.access.project_id);
    let actor_id = perm.access.user.id;

    let res = sqlx::query("DELETE FROM project_members WHERE project_id=$1 AND user_id=$2")
        .bind(project_id)
        .bind(user_id)
        .execute(&s.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("member".into()));
    }

    Audit::new("project.member.remove", "project_member")
        .actor(actor_id)
        .project(project_id)
        .target(format!("{project_id}:{user_id}"))
        .write(&s.db)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn load_project_dto(s: &AppState, id: Uuid) -> AppResult<ProjectDto> {
    let row: Option<(
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, code, name, icon, description, archived_at, created_at, updated_at \
         FROM projects WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    let r = row.ok_or_else(|| AppError::NotFound("project".into()))?;
    Ok(ProjectDto {
        id: r.0,
        code: r.1,
        name: r.2,
        icon: r.3,
        description: r.4,
        archived_at: r.5,
        created_at: r.6,
        updated_at: r.7,
    })
}

fn default_project_id() -> Uuid {
    // Stable UUID for the seeded "default" project (see migration 20260426140000).
    Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("valid uuid")
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        // Postgres unique_violation = 23505
        db_err.code().as_deref() == Some("23505")
    } else {
        false
    }
}
