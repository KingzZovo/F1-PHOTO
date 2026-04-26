//! Audit log helper.
//!
//! Every mutation that touches projects, members, master data, settings or
//! recognition outcomes should leave an `audit_log` row. Build one with
//! `Audit::new(action, target_type)` plus a few chained setters, then call
//! `.write(&pool).await?`.
//!
//! Failures here are logged but **never** propagated back to the caller — we
//! never want a failed audit insert to roll back the actual business write.
//! Callers can still get strict behaviour by using `try_write` instead.

use serde_json::Value;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Default)]
pub struct Audit<'a> {
    user_id: Option<Uuid>,
    project_id: Option<Uuid>,
    action: &'a str,
    target_type: &'a str,
    target_id: Option<String>,
    before: Option<Value>,
    after: Option<Value>,
}

impl<'a> Audit<'a> {
    pub fn new(action: &'a str, target_type: &'a str) -> Self {
        Self {
            action,
            target_type,
            ..Default::default()
        }
    }

    pub fn actor(mut self, user_id: Uuid) -> Self {
        self.user_id = Some(user_id);
        self
    }
    pub fn project(mut self, project_id: Uuid) -> Self {
        self.project_id = Some(project_id);
        self
    }
    pub fn target(mut self, id: impl Into<String>) -> Self {
        self.target_id = Some(id.into());
        self
    }
    pub fn before(mut self, v: Value) -> Self {
        self.before = Some(v);
        self
    }
    pub fn after(mut self, v: Value) -> Self {
        self.after = Some(v);
        self
    }

    /// Write the row, returning the sqlx error to the caller.
    pub async fn try_write<'e, E: PgExecutor<'e>>(self, exec: E) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO audit_log (user_id, project_id, action, target_type, target_id, before, after) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(self.user_id)
        .bind(self.project_id)
        .bind(self.action)
        .bind(self.target_type)
        .bind(self.target_id)
        .bind(self.before)
        .bind(self.after)
        .execute(exec)
        .await?;
        Ok(())
    }

    /// Best-effort write — logs and swallows errors. Recommended for use
    /// after a successful business write so the caller's commit isn't held
    /// up by audit-log issues.
    pub async fn write<'e, E: PgExecutor<'e>>(self, exec: E) {
        let action = self.action.to_string();
        let target_type = self.target_type.to_string();
        if let Err(e) = self.try_write(exec).await {
            tracing::error!(error = ?e, action, target_type, "audit_log insert failed");
        }
    }
}
