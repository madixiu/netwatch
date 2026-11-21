use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use netwatch_db::rollups::{rollup, Granularity};
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::error;

pub async fn run(pool: PgPool, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => {
                for granularity in [Granularity::Minute, Granularity::Hour, Granularity::Day] {
                    if let Err(error) = rollup(&pool, granularity, Utc::now()).await {
                        error!(?granularity, %error, "rollup failed; watermark was not advanced");
                        break;
                    }
                }
            }
        }
    }
}
