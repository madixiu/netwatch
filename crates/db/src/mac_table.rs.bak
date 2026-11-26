//! MAC addresses learned per switch port. Each collection replaces the device's rows.
 
use sqlx::{PgPool, Postgres, QueryBuilder};
use std::collections::HashSet;
 
/// Replace all MAC rows for one device. `rows` holds (interface_id, mac, vlan).
pub async fn replace_for_device(
    pool: &PgPool,
    device_id: i64,
    rows: &[(i64, String, i32)],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM mac_table WHERE device_id = $1")
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
 
    // A MAC can only appear once per device (primary key).
    let mut seen = HashSet::new();
    let unique: Vec<&(i64, String, i32)> = rows
        .iter()
        .filter(|(_, mac, _)| seen.insert(mac.clone()))
        .collect();
 
    for chunk in unique.chunks(500) {
        let mut query: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO mac_table (device_id, interface_id, mac, vlan, last_seen) ",
        );
        query.push_values(chunk, |mut row, (interface_id, mac, vlan)| {
            row.push_bind(device_id)
                .push_bind(*interface_id)
                .push_bind(mac.clone())
                .push_bind(*vlan)
                .push("now()");
        });
        query.build().execute(&mut *tx).await?;
    }
 
    tx.commit().await?;
    Ok(())
}
