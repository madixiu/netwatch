//! Device inventory queries.

use netwatch_core::model::{Device, DeviceId};
use sqlx::PgPool;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: DeviceId,
    name: String,
    mgmt_ip: String,
    poll_interval_secs: i32,
    profile: String,
    snmp_version: i16,
    enabled: bool,
}

pub async fn list_enabled(pool: &PgPool) -> Result<Vec<Device>, DeviceError> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, host(mgmt_ip) AS mgmt_ip, poll_interval_secs,
                profile, snmp_version, enabled
         FROM devices WHERE enabled = TRUE ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Device {
            id: r.id,
            name: r.name,
            mgmt_ip: r.mgmt_ip,
            poll_interval_secs: r.poll_interval_secs as u64,
            profile: r.profile,
            snmp_version: r.snmp_version,
            enabled: r.enabled,
        })
        .collect())
}

pub async fn record_poll_status(
    pool: &PgPool,
    device_id: i64,
    status: &str,
    duration_ms: Option<i32>,
    error: Option<&str>,
) -> Result<(), DeviceError> {
    sqlx::query(
        "INSERT INTO poll_status (device_id, ok, status, duration_ms, error)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(device_id)
    .bind(status == "ok")
    .bind(status)
    .bind(duration_ms)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}