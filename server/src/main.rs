use anyhow::Result;
use std::sync::Arc;

use f1_photo_server::{api, config::Config, db, logging};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init();
    let cfg = Config::from_env()?;
    let bind_addr = cfg.bind_addr.clone();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %bind_addr,
        "starting f1-photo server"
    );

    let pool = db::connect(&cfg).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");

    let state = api::AppState {
        db: pool,
        config: Arc::new(cfg),
    };
    let app = api::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(bind = %bind_addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}
