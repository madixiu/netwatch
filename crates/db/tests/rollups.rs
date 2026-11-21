use chrono::{TimeZone, Utc};
use netwatch_core::model::SampleRow;
use netwatch_db::{
    rollups::{rollup, Granularity},
    writer::copy_samples,
};
use sqlx::PgPool;

type Aggregate = (i64, f64, Option<f64>, Option<f64>, i64);

#[tokio::test]
#[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn rollups_are_weighted_idempotent_and_revisit_late_data() {
    let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let base = Utc.with_ymd_and_hms(2025, 1, 2, 1, 0, 0).unwrap();
    sqlx::query("SELECT create_raw_partition($1)")
        .bind(base.date_naive())
        .execute(&pool)
        .await
        .unwrap();
    let row = |offset, value, delta, seconds, reset| SampleRow {
        device_id: 555,
        metric_id: 999,
        interface_id: 3,
        ts: base + chrono::Duration::seconds(offset),
        value,
        delta,
        dt_seconds: seconds,
        counter_reset: reset,
        mapping_verified: true,
    };
    copy_samples(
        &pool,
        &[
            row(0, 10.0, None, None, false),
            row(30, 20.0, Some(10.0), Some(30.0), false),
            row(60, 30.0, Some(30.0), Some(60.0), false),
        ],
    )
    .await
    .unwrap();
    let mut legacy = row(15, 9999.0, Some(999.0), Some(1.0), false);
    legacy.mapping_verified = false;
    copy_samples(&pool, &[legacy]).await.unwrap();
    let first = base + chrono::Duration::seconds(150);
    assert!(rollup(&pool, Granularity::Minute, first).await.unwrap());
    copy_samples(&pool, &[row(90, 40.0, None, None, true)])
        .await
        .unwrap();
    assert!(!rollup(&pool, Granularity::Minute, first).await.unwrap());
    let minutes: Vec<Aggregate> = sqlx::query_as(
        "SELECT count, avg, sum_delta, sum_seconds, reset_count FROM rollup_1m
         WHERE device_id = 555 ORDER BY bucket",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        minutes,
        vec![
            (2, 15.0, Some(10.0), Some(30.0), 0),
            (2, 35.0, Some(30.0), Some(60.0), 1)
        ]
    );

    let later = base + chrono::Duration::days(1) + chrono::Duration::hours(2);
    while rollup(&pool, Granularity::Minute, later).await.unwrap() {}
    while rollup(&pool, Granularity::Hour, later).await.unwrap() {}
    while rollup(&pool, Granularity::Day, later).await.unwrap() {}
    for table in ["rollup_1h", "rollup_1d"] {
        let query = format!("SELECT count, avg, sum_delta, sum_seconds, reset_count FROM {table} WHERE device_id = 555");
        let rows: Vec<Aggregate> = sqlx::query_as(&query).fetch_all(&pool).await.unwrap();
        assert_eq!(rows, vec![(4, 25.0, Some(40.0), Some(90.0), 1)]);
    }
}
