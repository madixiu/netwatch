use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone, Copy, Debug)]
pub enum Granularity {
    Minute,
    Hour,
    Day,
}

impl Granularity {
    fn seconds(self) -> i64 {
        match self {
            Self::Minute => 60,
            Self::Hour => 3600,
            Self::Day => 86400,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Minute => "1m",
            Self::Hour => "1h",
            Self::Day => "1d",
        }
    }

    fn source(self) -> (&'static str, &'static str) {
        match self {
            Self::Minute => ("raw_samples", "ts"),
            Self::Hour => ("rollup_1m", "bucket"),
            Self::Day => ("rollup_1h", "bucket"),
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Minute => "rollup_1m",
            Self::Hour => "rollup_1h",
            Self::Day => "rollup_1d",
        }
    }

    fn max_span(self) -> Duration {
        match self {
            Self::Minute => Duration::days(1),
            Self::Hour => Duration::days(30),
            Self::Day => Duration::days(365),
        }
    }

    fn upstream(self) -> Option<&'static str> {
        match self {
            Self::Minute => None,
            Self::Hour => Some("1m"),
            Self::Day => Some("1h"),
        }
    }
}

fn floor_time(time: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(time.timestamp().div_euclid(seconds) * seconds, 0)
        .expect("a rounded representable UTC timestamp")
}

/// Recompute a bounded closed range and advance the exclusive watermark atomically.
pub async fn rollup(
    pool: &PgPool,
    granularity: Granularity,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let seconds = granularity.seconds();
    let safe_end = floor_time(now - Duration::seconds(30), seconds);
    let mut tx = pool.begin().await?;
    let watermark: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_rolled_up_at FROM rollup_state WHERE granularity = $1 FOR UPDATE",
    )
    .bind(granularity.key())
    .fetch_one(&mut *tx)
    .await?;
    let upstream = if let Some(key) = granularity.upstream() {
        let value: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT last_rolled_up_at FROM rollup_state WHERE granularity = $1")
                .bind(key)
                .fetch_one(&mut *tx)
                .await?;
        let Some(value) = value else { return Ok(false) };
        Some(floor_time(value, seconds))
    } else {
        None
    };
    let max_end = upstream.map_or(safe_end, |value| value.min(safe_end));
    let (source, time_col) = granularity.source();
    let start = if let Some(watermark) = watermark {
        watermark - Duration::seconds(seconds * 2)
    } else {
        let query = format!("SELECT min({time_col}) FROM {source}");
        let first: Option<DateTime<Utc>> = sqlx::query_scalar(&query).fetch_one(&mut *tx).await?;
        let Some(first) = first else { return Ok(false) };
        floor_time(first, seconds)
    };
    let progress_start = watermark.unwrap_or(start);
    let end = max_end.min(progress_start + granularity.max_span());
    if end <= start {
        return Ok(false);
    }
    write_range(&mut tx, granularity, start, end).await?;
    if end > progress_start {
        sqlx::query("UPDATE rollup_state SET last_rolled_up_at = $2, updated_at = now() WHERE granularity = $1")
            .bind(granularity.key())
            .bind(end)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(end > progress_start)
}

async fn write_range(
    tx: &mut Transaction<'_, Postgres>,
    granularity: Granularity,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let (source, time_col) = granularity.source();
    let target = granularity.target();
    let aggregate = match granularity {
        Granularity::Minute => format!(
            "min(value) AS min, max(value) AS max, avg(value) AS avg,
             (array_agg(value ORDER BY {time_col} DESC))[1] AS last,
             count(*) AS count, sum(value) AS sum_value,
             sum(delta) AS sum_delta, sum(dt_seconds) AS sum_seconds,
             count(*) FILTER (WHERE counter_reset) AS reset_count"
        ),
        _ => format!(
            "min(min) AS min, max(max) AS max, sum(sum_value) / sum(count) AS avg,
             (array_agg(last ORDER BY {time_col} DESC))[1] AS last,
             sum(count)::bigint AS count, sum(sum_value) AS sum_value,
             sum(sum_delta) AS sum_delta, sum(sum_seconds) AS sum_seconds,
             sum(reset_count)::bigint AS reset_count"
        ),
    };
    let query = format!(
        "INSERT INTO {target}
            (device_id, metric_id, interface_id, bucket, min, max, avg, last, count,
             sum_value, sum_delta, sum_seconds, reset_count, mapping_verified)
         SELECT device_id, metric_id, interface_id,
                date_bin($3 * interval '1 second', {time_col}, '1970-01-01 UTC'::timestamptz),
                {aggregate}, true
         FROM {source} WHERE {time_col} >= $1 AND {time_col} < $2
           AND (interface_id = 0 OR mapping_verified)
         GROUP BY device_id, metric_id, interface_id, 4
         ON CONFLICT (device_id, metric_id, interface_id, bucket) DO UPDATE SET
           min = EXCLUDED.min, max = EXCLUDED.max, avg = EXCLUDED.avg,
           last = EXCLUDED.last, count = EXCLUDED.count, sum_value = EXCLUDED.sum_value,
           sum_delta = EXCLUDED.sum_delta, sum_seconds = EXCLUDED.sum_seconds,
           reset_count = EXCLUDED.reset_count, mapping_verified = true"
    );
    sqlx::query(&query)
        .bind(start)
        .bind(end)
        .bind(granularity.seconds() as f64)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
