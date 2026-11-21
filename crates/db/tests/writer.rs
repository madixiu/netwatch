//! Integration tests. Require `DATABASE_URL` and a DB with migrations applied.
//! Run: `DATABASE_URL=postgres://... cargo test -p netwatch-db -- --ignored`

use chrono::Utc;
use netwatch_core::model::InterfaceRef;
use netwatch_core::model::SampleRow;
use netwatch_db::writer::{copy_samples, load_metric_ids, upsert_metric_defs};
use sqlx::PgPool;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

#[tokio::test]
#[ignore]
async fn copy_inserts_rows_and_partition_exists() {
    let pool = pool().await;
    // Seed a metric so the metric_id is valid (FK-less but logical).
    let def = netwatch_core::model::MetricDef {
        name: "test_counter",
        unit: "count",
        kind: netwatch_core::model::MetricKind::Counter,
    };
    upsert_metric_defs(&pool, &[def]).await.unwrap();
    let ids = load_metric_ids(&pool).await.unwrap();
    let metric_id = *ids.get("test_counter").unwrap();

    // Ensure today's partition exists (migration creates +7 days).
    sqlx::query("SELECT ensure_raw_partitions(7)")
        .execute(&pool)
        .await
        .unwrap();

    let now = Utc::now();
    let rows: Vec<SampleRow> = (0..100)
        .map(|i| SampleRow {
            device_id: 1,
            metric_id,
            interface_id: 0,
            ts: now + chrono::Duration::microseconds(i),
            value: i as f64,
            delta: None,
            dt_seconds: None,
            counter_reset: false,
            mapping_verified: true,
        })
        .collect();

    let n = copy_samples(&pool, &rows).await.unwrap();
    assert_eq!(n, 100);

    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM raw_samples WHERE device_id = 1 AND metric_id = $1")
            .bind(metric_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 100);

    // Cleanup.
    sqlx::query("DELETE FROM raw_samples WHERE device_id = 1 AND metric_id = $1")
        .bind(metric_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn interface_reconciliation_preserves_identity_and_history() {
    let pool = pool().await;
    let device: i64 = sqlx::query_scalar(
        "INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version)
         VALUES ('interface-test', '192.0.2.123', 'test', 'fake', 2) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let port = |if_index, name: &str| InterfaceRef {
        if_index,
        name: name.into(),
        speed_bps: Some(1_000_000_000),
    };
    let first =
        netwatch_db::interfaces::reconcile(&pool, device, &[port(1, "old"), port(2, "gone")])
            .await
            .unwrap();
    let second =
        netwatch_db::interfaces::reconcile(&pool, device, &[port(1, "renamed"), port(3, "new")])
            .await
            .unwrap();
    assert_eq!(first[&1], second[&1]);
    let removed: bool =
        sqlx::query_scalar("SELECT removed_at IS NOT NULL FROM interfaces WHERE id = $1")
            .bind(first[&2])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(removed);
    assert!(netwatch_db::interfaces::reconcile(&pool, device, &[])
        .await
        .is_err());
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM interfaces WHERE device_id = $1 AND removed_at IS NULL",
    )
    .bind(device)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 2);
    assert!(netwatch_db::interfaces::reconcile(
        &pool,
        device,
        &[port(1, "must roll back"), port(u32::MAX, "invalid")]
    )
    .await
    .is_err());
    let name: String = sqlx::query_scalar("SELECT name FROM interfaces WHERE id = $1")
        .bind(first[&1])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "renamed");
    let restored =
        netwatch_db::interfaces::reconcile(&pool, device, &[port(1, "renamed"), port(2, "back")])
            .await
            .unwrap();
    assert_eq!(first[&2], restored[&2]);
    let state: (String, bool) =
        sqlx::query_as("SELECT name, removed_at IS NULL FROM interfaces WHERE id = $1")
            .bind(restored[&2])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("back".into(), true));
    sqlx::query("DELETE FROM devices WHERE id = $1")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
}
