use anyhow::Result;
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::watch;
use tracing::warn;

pub async fn run(
    pool: PgPool,
    version: &'static str,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => {
                if let Err(e) = sqlx::query(
                    "INSERT INTO poller_heartbeat (name, ts, version) VALUES ('default', now(), $1)
                     ON CONFLICT (name) DO UPDATE SET ts = EXCLUDED.ts, version = EXCLUDED.version",
                ).bind(version).execute(&pool).await {
                    warn!(error = %e, "heartbeat write failed");
                }
            }
        }
    }
}
