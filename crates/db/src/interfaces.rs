use std::collections::HashMap;

use netwatch_core::model::InterfaceRef;
use sqlx::PgPool;

/// Atomically reconcile a complete, nonempty interface snapshot.
pub async fn reconcile(
    pool: &PgPool,
    device_id: i64,
    interfaces: &[InterfaceRef],
) -> Result<HashMap<u32, i64>, sqlx::Error> {
    if interfaces.is_empty() {
        return Err(sqlx::Error::Protocol(
            "refusing to reconcile an empty interface snapshot".into(),
        ));
    }

    let mut tx = pool.begin().await?;
    let mut ids = HashMap::with_capacity(interfaces.len());
    let mut indexes = Vec::with_capacity(interfaces.len());
    for interface in interfaces {
        let index = i32::try_from(interface.if_index)
            .map_err(|_| sqlx::Error::Protocol("invalid interface index".into()))?;
        let speed = interface
            .speed_bps
            .map(i64::try_from)
            .transpose()
            .map_err(|_| sqlx::Error::Protocol("invalid interface speed".into()))?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO interfaces (device_id, if_index, name, speed_bps)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (device_id, if_index) DO UPDATE
             SET name = EXCLUDED.name, speed_bps = EXCLUDED.speed_bps, removed_at = NULL
             RETURNING id",
        )
        .bind(device_id)
        .bind(index)
        .bind(&interface.name)
        .bind(speed)
        .fetch_one(&mut *tx)
        .await?;
        indexes.push(index);
        ids.insert(interface.if_index, id);
    }

    sqlx::query(
        "UPDATE interfaces SET removed_at = now()
         WHERE device_id = $1 AND removed_at IS NULL AND NOT (if_index = ANY($2))",
    )
    .bind(device_id)
    .bind(&indexes)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ids)
}
