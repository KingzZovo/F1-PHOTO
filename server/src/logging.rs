use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise tracing subscriber. Reads filter from `F1P_LOG` env (falls back
/// to `info,sqlx=warn,hyper=warn,tower_http=info`).
///
/// Idempotent: safe to call once at startup. Calling again is a no-op via
/// `try_init` semantics if a global subscriber is already set.
pub fn init() {
    let filter = EnvFilter::try_from_env("F1P_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,tower_http=info"));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_line_number(false)
                .compact(),
        )
        .try_init();
}
