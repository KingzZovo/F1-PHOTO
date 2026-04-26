pub mod auth;
pub mod devices;
pub mod health;
pub mod persons;
pub mod projects;
pub mod tools;

use crate::auth::JwtCodec;
use crate::config::Config;
use axum::{
    Router,
    routing::{get, patch, post},
};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

/// Shared application state injected into every handler via `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
    pub jwt: Arc<JwtCodec>,
}

/// Returns true if the sqlx error is a Postgres `unique_violation` (23505).
///
/// Shared by handlers that translate INSERT / UPDATE conflicts into 409
/// `CONFLICT` (e.g. duplicate `projects.code`, `persons.employee_no`,
/// `tools.sn`, `devices.sn`).
pub(crate) fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        db_err.code().as_deref() == Some("23505")
    } else {
        false
    }
}

/// Build the top-level axum router.
///
/// Note: this project is on axum 0.7, which uses the colon-prefix path
/// parameter syntax (`/:project_id`). The `{project_id}` brace syntax
/// belongs to axum 0.8 and is treated as a literal segment in 0.7.
pub fn router(state: AppState) -> Router {
    Router::new()
        // Liveness / readiness.
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        // Auth.
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        // Projects: list + create.
        .route(
            "/api/projects",
            get(projects::list_projects).post(projects::create_project),
        )
        // Projects: single resource.
        .route(
            "/api/projects/:project_id",
            get(projects::get_project)
                .patch(projects::patch_project)
                .delete(projects::archive_project),
        )
        .route(
            "/api/projects/:project_id/me",
            get(projects::get_my_perms),
        )
        .route(
            "/api/projects/:project_id/unarchive",
            post(projects::unarchive_project),
        )
        // Members.
        .route(
            "/api/projects/:project_id/members",
            get(projects::list_members).post(projects::add_member),
        )
        .route(
            "/api/projects/:project_id/members/:user_id",
            patch(projects::patch_member).delete(projects::remove_member),
        )
        // Master data: persons.
        .route(
            "/api/persons",
            get(persons::list).post(persons::create),
        )
        .route(
            "/api/persons/:id",
            get(persons::get_one)
                .patch(persons::patch)
                .delete(persons::soft_delete),
        )
        .route(
            "/api/persons/:id/restore",
            post(persons::restore),
        )
        // Master data: tools.
        .route(
            "/api/tools",
            get(tools::list).post(tools::create),
        )
        .route(
            "/api/tools/:id",
            get(tools::get_one)
                .patch(tools::patch)
                .delete(tools::soft_delete),
        )
        .route(
            "/api/tools/:id/restore",
            post(tools::restore),
        )
        // Master data: devices.
        .route(
            "/api/devices",
            get(devices::list).post(devices::create),
        )
        .route(
            "/api/devices/:id",
            get(devices::get_one)
                .patch(devices::patch)
                .delete(devices::soft_delete),
        )
        .route(
            "/api/devices/:id/restore",
            post(devices::restore),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
