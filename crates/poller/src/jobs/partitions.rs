use anyhow::Result;
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn};

pub async fn run(pool: PgPool, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    // Ensure on startup so a poller restart past midnight is safe.
    ensure(&pool).await;
    let mut ticker = tokio::time::interval(Duration::from_secs(3600));
    ticker.tick().await; // consume immediate
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => ensure(&pool).await,
        }
    }
}

async fn ensure(pool: &PgPool) {
    match sqlx::query_scalar::<_, i32>("SELECT ensure_raw_partitions(7)")
        .fetch_one(pool)
        .await
    {
        Ok(_) => info!("ensured raw partitions for next 7 days"),
        Err(e) => warn!(error = %e, "partition ensure failed"),
    }
}
