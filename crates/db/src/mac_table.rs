//! MAC addresses learned per switch port.

use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
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
    insert_rows(&mut tx, device_id, rows).await?;
    tx.commit().await?;
    Ok(())
}

/// Replace only the VLANs that were read successfully this cycle.
///
/// - Rows for VLANs no longer on the switch are removed, and so are rows without a VLAN.
/// - Rows for `refreshed` VLANs are replaced by `rows`.
/// - Rows for VLANs that failed this cycle are kept as they were.
///
/// A MAC is unique per device (primary key). A fresh entry for a MAC replaces any older
/// row for that MAC, wherever it was kept from.
pub async fn replace_vlans(
    pool: &PgPool,
    device_id: i64,
    refreshed: &[i32],
    discovered: &[i32],
    rows: &[(i64, String, i32)],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "DELETE FROM mac_table WHERE device_id = $1 AND (vlan IS NULL OR NOT (vlan = ANY($2)))",
    )
    .bind(device_id)
    .bind(discovered.to_vec())
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM mac_table WHERE device_id = $1 AND vlan = ANY($2)")
        .bind(device_id)
        .bind(refreshed.to_vec())
        .execute(&mut *tx)
        .await?;

    let macs: Vec<String> = rows.iter().map(|(_, mac, _)| mac.clone()).collect();
    sqlx::query("DELETE FROM mac_table WHERE device_id = $1 AND mac = ANY($2)")
        .bind(device_id)
        .bind(macs)
        .execute(&mut *tx)
        .await?;

    insert_rows(&mut tx, device_id, rows).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_rows(
    tx: &mut Transaction<'_, Postgres>,
    device_id: i64,
    rows: &[(i64, String, i32)],
) -> Result<(), sqlx::Error> {
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
        query.build().execute(&mut **tx).await?;
    }
    Ok(())
}

/// Replace the set of addresses owned by a monitored device, so other switches can show its name.
pub async fn replace_device_macs(
    pool: &PgPool,
    device_id: i64,
    macs: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM device_macs WHERE device_id = $1")
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO device_macs (device_id, mac) SELECT $1, UNNEST($2::text[]) ON CONFLICT DO NOTHING",
    )
    .bind(device_id)
    .bind(macs.to_vec())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
