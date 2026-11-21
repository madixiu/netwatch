//! Batched writes to `raw_samples` via `COPY ... FROM STDIN`.
//!
//! Uses sqlx's `PgCopyIn` text format. The text format is not the fastest
//! possible (binary is) but is trivially debuggable: you can copy a line
//! into psql by hand and see it insert. At our row rates (5–10k/min) the
//! difference is noise.

use sqlx::{pool::PoolConnection, PgConnection, PgPool, Postgres};

use netwatch_core::model::SampleRow;

#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Write a batch of samples via COPY. All-or-nothing per call: on error no
/// rows are committed (Postgres COPY aborts the transaction).
pub async fn copy_samples(pool: &PgPool, rows: &[SampleRow]) -> Result<usize, WriterError> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut conn = pool.acquire().await?;
    copy_samples_on(&mut conn, rows).await
}

/// Inner form that takes a connection — used by the buffer so it can hold a
/// single connection for the duration of a flush burst.
pub async fn copy_samples_on(
    conn: &mut PgConnection,
    rows: &[SampleRow],
) -> Result<usize, WriterError> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut copy = conn
        .copy_in_raw(
            "COPY raw_samples (device_id, metric_id, interface_id, ts, value, delta, dt_seconds, counter_reset, mapping_verified) FROM STDIN",
        )
        .await?;

    for row in rows {
        let line = row.to_copy_line();
        copy.send(line.as_bytes()).await?;
    }
    copy.finish().await?;
    Ok(rows.len())
}

/// Upsert metric definitions into `metrics_catalog`. Called once at poller
/// startup for every profile's `metrics()`. Idempotent.
pub async fn upsert_metric_defs(
    pool: &PgPool,
    defs: &[netwatch_core::model::MetricDef],
) -> Result<(), WriterError> {
    for d in defs {
        let kind = match d.kind {
            netwatch_core::model::MetricKind::Counter => "counter",
            netwatch_core::model::MetricKind::Gauge => "gauge",
        };
        sqlx::query(
            "INSERT INTO metrics_catalog (name, unit, kind)
             VALUES ($1, $2, $3)
             ON CONFLICT (name) DO UPDATE
                SET unit = EXCLUDED.unit, kind = EXCLUDED.kind",
        )
        .bind(d.name)
        .bind(d.unit)
        .bind(kind)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Fetch `name -> id` for every metric in the catalog. Used by the poller to
/// resolve metric names to ids before parsing responses.
pub async fn load_metric_ids(
    pool: &PgPool,
) -> Result<std::collections::HashMap<String, i32>, WriterError> {
    let rows = sqlx::query_as::<Postgres, (String, i32)>("SELECT name, id FROM metrics_catalog")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Grab a pooled connection explicitly. The buffer holds one across a flush
/// burst so a slow DB doesn't cause connection churn on every poll.
pub async fn acquire(pool: &PgPool) -> Result<PoolConnection<Postgres>, WriterError> {
    Ok(pool.acquire().await?)
}
