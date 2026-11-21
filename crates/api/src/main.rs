use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use netwatch_core::scan::parse_allowlist;
use netwatch_crypto::SecretKey;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();
    let url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let key = if let Ok(path) = std::env::var("NETWATCH_KEY_FILE") {
        SecretKey::from_file(&PathBuf::from(path))?
    } else {
        SecretKey::from_hex(
            &std::env::var("NETWATCH_KEY_HEX")
                .context("NETWATCH_KEY_FILE or NETWATCH_KEY_HEX is required")?,
        )?
    };
    let allow_env = match std::env::var("NETWATCH_SCAN_ALLOW") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(error) => return Err(error.into()),
    };
    let allow = parse_allowlist(&allow_env)?;
    let bind: SocketAddr = std::env::var("NETWATCH_API_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()
        .context("invalid NETWATCH_API_BIND")?;
    if !bind.ip().is_loopback() {
        tracing::warn!("API is listening beyond localhost; use HTTPS and a trusted reverse proxy");
    }
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await?;
    let state = netwatch_api::AppState::new(pool, key, allow).await?;
    let web =
        PathBuf::from(std::env::var("NETWATCH_WEB_ROOT").unwrap_or_else(|_| "web/dist".into()));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(address = %listener.local_addr()?, "API listening");
    axum::serve(
        listener,
        netwatch_api::router(state, web).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
