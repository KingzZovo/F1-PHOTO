//! Platform-level settings: a single key/value table seeded by the init
//! migration.
//!
//! Schema:
//!   settings(key text PK, value jsonb, updated_at timestamptz, updated_by uuid)
//!
//! Reads are open to any logged-in user; writes are admin-only.
//!
//! Keys are restricted to a fixed allowlist so the API can validate types and
//! reject typos. Add new keys here when the feature lands.

use crate::api::AppState;
use crate::audit::Audit;
use crate::auth::{CurrentUser, RequireAdmin};
use crate::error::{AppError, AppResult};
use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Allowlist + type validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// Any JSON string.
    String,
    /// JSON boolean.
    Bool,
    /// JSON number, must be a float in `[lo, hi]`.
    Float { lo: f64, hi: f64 },
    /// JSON number, must be an integer in `[lo, hi]`.
    Int { lo: i64, hi: i64 },
}

/// Allowed setting keys + the JSON shape we accept for each.
fn allowlist() -> &'static [(&'static str, Kind)] {
    &[
        ("platform.name", Kind::String),
        ("match.threshold", Kind::Float { lo: 0.0, hi: 1.0 }),
        ("match.low_threshold", Kind::Float { lo: 0.0, hi: 1.0 }),
        ("match.augment_upper", Kind::Float { lo: 0.0, hi: 1.0 }),
        ("upload.max_mb", Kind::Int { lo: 1, hi: 1024 }),
        ("upload.allow_auto_create_person", Kind::Bool),
        ("recognition.face.enabled", Kind::Bool),
        ("recognition.tool.enabled", Kind::Bool),
        ("recognition.device.enabled", Kind::Bool),
        ("recognition.angle.enabled", Kind::Bool),
    ]
}

fn lookup_kind(key: &str) -> AppResult<Kind> {
    for &(k, kind) in allowlist() {
        if k == key {
            return Ok(kind);
        }
    }
    Err(AppError::InvalidInput(format!(
        "unknown settings key '{key}'"
    )))
}

/// Validate `value` matches `kind`; returns the normalised JSON value to
/// store (same shape, but numbers are normalised so e.g. integer JSON values
/// passed where a Float is expected become floats).
fn validate_value(key: &str, kind: Kind, value: &Value) -> AppResult<Value> {
    match kind {
        Kind::String => match value {
            Value::String(s) => {
                if s.len() > 200 {
                    return Err(AppError::InvalidInput(format!(
                        "{key}: string too long (max 200)"
                    )));
                }
                Ok(Value::String(s.clone()))
            }
            _ => Err(AppError::InvalidInput(format!("{key}: expected string"))),
        },
        Kind::Bool => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            _ => Err(AppError::InvalidInput(format!("{key}: expected boolean"))),
        },
        Kind::Float { lo, hi } => match value.as_f64() {
            Some(f) if f.is_finite() && f >= lo && f <= hi => Ok(json!(f)),
            _ => Err(AppError::InvalidInput(format!(
                "{key}: expected number in [{lo}, {hi}]"
            ))),
        },
        Kind::Int { lo, hi } => match value.as_i64() {
            Some(n) if n >= lo && n <= hi => Ok(json!(n)),
            _ => Err(AppError::InvalidInput(format!(
                "{key}: expected integer in [{lo}, {hi}]"
            ))),
        },
    }
}

// ---------------------------------------------------------------------------
// Response shape
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SettingsResponse {
    /// Map of key -> JSON value, sorted alphabetically by key.
    pub values: BTreeMap<String, Value>,
    pub updated_at: Option<DateTime<Utc>>,
}

async fn load_all(s: &AppState) -> AppResult<SettingsResponse> {
    let rows: Vec<(String, Value, DateTime<Utc>)> =
        sqlx::query_as("SELECT key, value, updated_at FROM settings ORDER BY key")
            .fetch_all(&s.db)
            .await?;

    let mut values = BTreeMap::new();
    let mut latest: Option<DateTime<Utc>> = None;
    for (k, v, ts) in rows {
        values.insert(k, v);
        latest = Some(latest.map_or(ts, |cur| cur.max(ts)));
    }
    Ok(SettingsResponse {
        values,
        updated_at: latest,
    })
}

// ---------------------------------------------------------------------------
// GET / PATCH
// ---------------------------------------------------------------------------

/// `GET /api/settings` — any logged-in user.
pub async fn get_all(
    _user: CurrentUser,
    State(s): State<AppState>,
) -> AppResult<Json<SettingsResponse>> {
    Ok(Json(load_all(&s).await?))
}

/// `PATCH /api/settings` — admin only.
///
/// Body is a flat JSON object whose keys are settings keys (allowlist) and
/// whose values are the new JSON values. Unknown keys are rejected. Type
/// mismatches are rejected. Every change writes one `audit_log` row
/// (`settings.update`).
pub async fn patch_all(
    RequireAdmin(user): RequireAdmin,
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> AppResult<Json<SettingsResponse>> {
    let map: &Map<String, Value> = body.as_object().ok_or_else(|| {
        AppError::InvalidInput("body must be a JSON object of key -> value".into())
    })?;
    if map.is_empty() {
        return Err(AppError::InvalidInput("body must not be empty".into()));
    }

    // Validate every key/value before any write so PATCH is atomic-ish.
    let mut updates: Vec<(&str, Value)> = Vec::with_capacity(map.len());
    for (k, v) in map.iter() {
        let kind = lookup_kind(k)?;
        let normalised = validate_value(k, kind, v)?;
        // Find the &'static str for the canonical key so we can store it.
        let canonical = allowlist()
            .iter()
            .find(|(allowed, _)| *allowed == k.as_str())
            .map(|(allowed, _)| *allowed)
            .expect("key was just validated by lookup_kind");
        updates.push((canonical, normalised));
    }

    let before = load_all(&s).await?;

    let mut tx = s.db.begin().await?;
    for (k, v) in &updates {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at, updated_by) \
             VALUES ($1, $2, now(), $3) \
             ON CONFLICT (key) DO UPDATE \
               SET value      = EXCLUDED.value, \
                   updated_at = now(), \
                   updated_by = EXCLUDED.updated_by",
        )
        .bind(*k)
        .bind(v)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let after = load_all(&s).await?;

    // Build a compact diff for the audit row.
    let mut diff_before = Map::new();
    let mut diff_after = Map::new();
    for (k, v) in &updates {
        diff_before.insert(
            (*k).to_string(),
            before.values.get(*k).cloned().unwrap_or(Value::Null),
        );
        diff_after.insert((*k).to_string(), v.clone());
    }

    let target = updates
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(",");

    write_audit(
        &s,
        user.id,
        target,
        Value::Object(diff_before),
        Value::Object(diff_after),
    )
    .await;

    Ok(Json(after))
}

async fn write_audit(s: &AppState, actor: Uuid, target: String, before: Value, after: Value) {
    Audit::new("settings.update", "settings")
        .actor(actor)
        .target(target)
        .before(before)
        .after(after)
        .write(&s.db)
        .await;
}
