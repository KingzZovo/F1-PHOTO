use crate::api::AppState;
use crate::auth::{password, CurrentUser};
use crate::error::{AppError, AppResult};
use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserDto {
    pub id: Uuid,
    pub username: String,
    pub role: String,
    pub full_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_at: DateTime<Utc>,
    pub user: UserDto,
}

/// `POST /api/auth/login`
///
/// Trades a username + password for a JWT access token. Returns 401 with the
/// generic `UNAUTHORIZED` code on bad credentials or a disabled account; we
/// deliberately do not distinguish "unknown user" from "wrong password".
pub async fn login(
    State(state): State<AppState>,
    Json(input): Json<LoginInput>,
) -> AppResult<Json<LoginResponse>> {
    if input.username.trim().is_empty() || input.password.is_empty() {
        return Err(AppError::InvalidInput(
            "username and password are required".into(),
        ));
    }

    let row: Option<(
        Uuid,
        String,
        String,
        String,
        Option<String>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, username, password_hash, role::text, full_name, disabled_at \
         FROM users WHERE username = $1",
    )
    .bind(&input.username)
    .fetch_optional(&state.db)
    .await?;

    let (id, username, password_hash, role, full_name, disabled_at) =
        row.ok_or(AppError::Unauthorized)?;

    if disabled_at.is_some() {
        return Err(AppError::Unauthorized);
    }
    if !password::verify_password(&input.password, &password_hash) {
        return Err(AppError::Unauthorized);
    }

    let token = state
        .jwt
        .issue(id, &username, &role)
        .map_err(AppError::Internal)?;
    let expires_at = Utc::now() + Duration::seconds(state.jwt.ttl_seconds());

    Ok(Json(LoginResponse {
        access_token: token,
        token_type: "Bearer",
        expires_at,
        user: UserDto {
            id,
            username,
            role,
            full_name,
        },
    }))
}

/// `POST /api/auth/logout`
///
/// Stateless JWT — the server simply tells the client to drop its token.
/// A revocation list / refresh-token table can be added later without
/// changing the public contract.
pub async fn logout(_user: CurrentUser) -> StatusCode {
    StatusCode::NO_CONTENT
}

/// `GET /api/auth/me`
pub async fn me(user: CurrentUser, State(state): State<AppState>) -> AppResult<Json<UserDto>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT full_name FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?;
    let full_name = row.and_then(|(f,)| f);
    Ok(Json(UserDto {
        id: user.id,
        username: user.username,
        role: user.role,
        full_name,
    }))
}
