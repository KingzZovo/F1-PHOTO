use crate::api::AppState;
use crate::error::AppError;
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts, RawPathParams},
    http::{header::AUTHORIZATION, request::Parts},
};
use chrono::{DateTime, Utc};
use std::marker::PhantomData;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CurrentUser
// ---------------------------------------------------------------------------

/// Authenticated user resolved from a `Bearer` token.
///
/// Used as an axum extractor: any handler that takes `CurrentUser` will
/// short-circuit with 401 if the token is missing, malformed, expired, or
/// belongs to a disabled user.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: Uuid,
    pub username: String,
    pub role: String,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);

        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthorized)?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or(AppError::Unauthorized)?;

        let claims = app.jwt.verify(token).map_err(|_| AppError::Unauthorized)?;

        // Re-confirm the user still exists and is not disabled.
        let row: Option<(String, String, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT username, role::text, disabled_at FROM users WHERE id = $1",
        )
        .bind(claims.sub)
        .fetch_optional(&app.db)
        .await?;

        let (username, role, disabled_at) = row.ok_or(AppError::Unauthorized)?;
        if disabled_at.is_some() {
            return Err(AppError::Unauthorized);
        }

        Ok(CurrentUser {
            id: claims.sub,
            username,
            role,
        })
    }
}

// ---------------------------------------------------------------------------
// RequireAdmin
// ---------------------------------------------------------------------------

/// Extractor that requires the caller to be an admin user.
pub struct RequireAdmin(pub CurrentUser);

#[async_trait]
impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if !user.is_admin() {
            return Err(AppError::Forbidden("admin role required".into()));
        }
        Ok(RequireAdmin(user))
    }
}

// ---------------------------------------------------------------------------
// Per-project permissions
// ---------------------------------------------------------------------------

/// Member permission flags as stored in `project_members` (admin gets all
/// flags set automatically).
#[derive(Debug, Clone, Copy)]
pub struct MemberPerms {
    pub can_view: bool,
    pub can_upload: bool,
    pub can_delete: bool,
    pub can_manage: bool,
}

impl MemberPerms {
    pub const FULL: MemberPerms = MemberPerms {
        can_view: true,
        can_upload: true,
        can_delete: true,
        can_manage: true,
    };
}

/// Resolved access for one user against one project.
#[derive(Debug, Clone)]
pub struct ProjectAccess {
    pub user: CurrentUser,
    pub project_id: Uuid,
    pub perms: MemberPerms,
    pub is_admin: bool,
    /// True if the project is archived. Handlers may disallow writes on
    /// archived projects but reads remain allowed.
    pub archived: bool,
}

/// Marker trait selecting which permission flag is required.
pub trait Perm: Send + Sync + 'static {
    const NAME: &'static str;
    fn check(p: &MemberPerms) -> bool;
}

#[derive(Debug)]
pub struct ViewPerm;
#[derive(Debug)]
pub struct UploadPerm;
#[derive(Debug)]
pub struct DeletePerm;
#[derive(Debug)]
pub struct ManagePerm;

impl Perm for ViewPerm {
    const NAME: &'static str = "view";
    fn check(p: &MemberPerms) -> bool {
        p.can_view
    }
}
impl Perm for UploadPerm {
    const NAME: &'static str = "upload";
    fn check(p: &MemberPerms) -> bool {
        p.can_upload
    }
}
impl Perm for DeletePerm {
    const NAME: &'static str = "delete";
    fn check(p: &MemberPerms) -> bool {
        p.can_delete
    }
}
impl Perm for ManagePerm {
    const NAME: &'static str = "manage";
    fn check(p: &MemberPerms) -> bool {
        p.can_manage
    }
}

/// Extractor: requires the caller to have permission `P` on the project
/// identified by the `{project_id}` path param.
///
/// Resolution rules:
///   * No `Authorization` header           → 401 `UNAUTHORIZED`
///   * Project does not exist               → 404 `NOT_FOUND`
///   * Caller is admin                      → granted, with `MemberPerms::FULL`
///   * Caller is a member with flag `P`     → granted, with their actual perms
///   * Otherwise                            → 403 `PROJECT_FORBIDDEN`
pub struct RequireProjectPerm<P: Perm> {
    pub access: ProjectAccess,
    _phantom: PhantomData<fn() -> P>,
}

impl<P: Perm> RequireProjectPerm<P> {
    pub fn into_access(self) -> ProjectAccess {
        self.access
    }
}

#[async_trait]
impl<S, P> FromRequestParts<S> for RequireProjectPerm<P>
where
    S: Send + Sync,
    AppState: FromRef<S>,
    P: Perm,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        let project_id = extract_project_id(parts, state).await?;
        let app = AppState::from_ref(state);

        // Project existence + archived flag.
        let proj: Option<(Option<DateTime<Utc>>,)> =
            sqlx::query_as("SELECT archived_at FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_optional(&app.db)
                .await?;
        let (archived_at,) = proj.ok_or_else(|| AppError::NotFound("project".into()))?;
        let archived = archived_at.is_some();

        let is_admin = user.is_admin();
        let perms = if is_admin {
            MemberPerms::FULL
        } else {
            let row: Option<(bool, bool, bool, bool)> = sqlx::query_as(
                "SELECT can_view, can_upload, can_delete, can_manage \
                 FROM project_members WHERE project_id = $1 AND user_id = $2",
            )
            .bind(project_id)
            .bind(user.id)
            .fetch_optional(&app.db)
            .await?;
            let r = row.ok_or(AppError::ProjectForbidden)?;
            MemberPerms {
                can_view: r.0,
                can_upload: r.1,
                can_delete: r.2,
                can_manage: r.3,
            }
        };

        if !P::check(&perms) {
            return Err(AppError::ProjectForbidden);
        }

        Ok(Self {
            access: ProjectAccess {
                user,
                project_id,
                perms,
                is_admin,
                archived,
            },
            _phantom: PhantomData,
        })
    }
}

async fn extract_project_id<S>(parts: &mut Parts, state: &S) -> Result<Uuid, AppError>
where
    S: Send + Sync,
{
    let raw = RawPathParams::from_request_parts(parts, state)
        .await
        .map_err(|_| AppError::InvalidInput("project_id missing in path".into()))?;
    let pid_str = raw
        .iter()
        .find(|(k, _)| *k == "project_id")
        .map(|(_, v)| v)
        .ok_or_else(|| AppError::InvalidInput("project_id missing in path".into()))?;
    Uuid::parse_str(pid_str)
        .map_err(|_| AppError::InvalidInput("project_id is not a valid UUID".into()))
}
