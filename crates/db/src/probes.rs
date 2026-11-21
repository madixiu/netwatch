use sqlx::PgPool;

pub async fn record(
    pool: &PgPool,
    device_id: i64,
    protocol: &str,
    status: &str,
    latency_ms: Option<f64>,
    detail: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO device_probe_results (device_id, protocol, status, latency_ms, detail) VALUES ($1, $2, $3, $4, $5)")
        .bind(device_id).bind(protocol).bind(status).bind(latency_ms).bind(detail)
        .execute(pool).await?;
    Ok(())
}
