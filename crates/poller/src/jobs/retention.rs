use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn};

pub const RAW_RETENTION_DAYS: i32 = 14; // review §14 open decision

async fn prune(pool: &PgPool) -> Result<Option<i32>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let watermark: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_rolled_up_at FROM rollup_state WHERE granularity = '1m' FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    let Some(watermark) = watermark else {
        sqlx::query(
            "DELETE FROM device_probe_results WHERE observed_at < now() - interval '90 days'",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(None);
    };
    let safe_day = (watermark - chrono::Duration::minutes(2)).date_naive();
    let db_day: chrono::NaiveDate = sqlx::query_scalar("SELECT CURRENT_DATE")
        .fetch_one(&mut *tx)
        .await?;
    let required_days = (db_day - safe_day).num_days().max(0);
    let days = i32::try_from(required_days)
        .unwrap_or(i32::MAX)
        .max(RAW_RETENTION_DAYS);
    let dropped: i32 = sqlx::query_scalar("SELECT drop_old_raw_partitions($1)")
        .bind(days)
        .fetch_one(&mut *tx)
        .await?;
    let hourly: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_rolled_up_at FROM rollup_state WHERE granularity = '1h' FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    if let Some(hourly) = hourly {
        sqlx::query(
            "DELETE FROM rollup_1m WHERE bucket < LEAST(now() - interval '90 days', $1::timestamptz - interval '2 hours')",
        ).bind(hourly).execute(&mut *tx).await?;
    } else {
        warn!("minute rollup retention paused until hourly rollups have a watermark");
    }
    let daily: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_rolled_up_at FROM rollup_state WHERE granularity = '1d' FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    if let Some(daily) = daily {
        sqlx::query(
            "DELETE FROM rollup_1h WHERE bucket < LEAST(now() - interval '400 days', $1::timestamptz - interval '2 days')",
        ).bind(daily).execute(&mut *tx).await?;
    } else {
        warn!("hourly rollup retention paused until daily rollups have a watermark");
    }
    sqlx::query("DELETE FROM device_probe_results WHERE observed_at < now() - interval '90 days'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(dropped))
}

pub async fn run(pool: PgPool, mut shutdown: watch::Receiver<bool>) -> Result<()> {
    // Nightly at ~03:00 — simple 24h tick; phase 1 doesn't need wall-clock scheduling.
    let mut ticker = tokio::time::interval(Duration::from_secs(24 * 3600));
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => {
                match prune(&pool).await {
                    Ok(Some(n)) => info!(dropped = n, "retention pass"),
                    Ok(None) => warn!("raw retention paused until minute rollups have a watermark"),
                    Err(e) => warn!(error = %e, "retention failed"),
                }
            }

        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
    async fn retention_never_deletes_unrolled_raw_data() {
        let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let day: chrono::NaiveDate = sqlx::query_scalar("SELECT CURRENT_DATE - 20")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("SELECT create_raw_partition($1)")
            .bind(day)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO raw_samples (device_id, metric_id, ts, value) VALUES (777, 999, $1::date + interval '12 hours', 1)")
            .bind(day).execute(&pool).await.unwrap();
        for (table, age) in [("rollup_1m", 100), ("rollup_1h", 500)] {
            let query = format!("INSERT INTO {table} (device_id, metric_id, bucket, min, max, avg, last, count, sum_value, mapping_verified) VALUES (777, 999, now() - $1 * interval '1 day', 1, 1, 1, 1, 1, 1, true)");
            sqlx::query(&query)
                .bind(age as f64)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE rollup_state SET last_rolled_up_at = NULL")
            .execute(&pool)
            .await
            .unwrap();
        assert!(prune(&pool).await.unwrap().is_none());
        sqlx::query("UPDATE rollup_state SET last_rolled_up_at = now() - interval '30 days' WHERE granularity = '1m'").execute(&pool).await.unwrap();
        prune(&pool).await.unwrap();
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM raw_samples WHERE device_id = 777)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(present);
        sqlx::query("UPDATE rollup_state SET last_rolled_up_at = now() WHERE granularity = '1m'")
            .execute(&pool)
            .await
            .unwrap();
        prune(&pool).await.unwrap();
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM raw_samples WHERE device_id = 777)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!present);
        for table in ["rollup_1m", "rollup_1h"] {
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE device_id = 777)");
            assert!(sqlx::query_scalar::<_, bool>(&query)
                .fetch_one(&pool)
                .await
                .unwrap());
        }
        sqlx::query(
            "UPDATE rollup_state SET last_rolled_up_at = now() WHERE granularity IN ('1h', '1d')",
        )
        .execute(&pool)
        .await
        .unwrap();
        prune(&pool).await.unwrap();
        for table in ["rollup_1m", "rollup_1h"] {
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE device_id = 777)");
            assert!(!sqlx::query_scalar::<_, bool>(&query)
                .fetch_one(&pool)
                .await
                .unwrap());
        }
    }
}
