use crate::api::AppState;
use crate::error::AppError;
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{header::AUTHORIZATION, request::Parts},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

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

        // Re-confirm the user still exists and is not disabled. Cheap;
        // we already paid for one DB roundtrip per protected request via
        // anything else the handler does.
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

/// Extractor that requires the caller to be an admin user.
///
/// Inner `CurrentUser` is exposed so handlers can still reach username/id.
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
