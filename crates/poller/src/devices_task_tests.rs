use super::*;
use async_trait::async_trait;
use netwatch_snmp::{SnmpError, VarBind};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CounterClient(AtomicUsize);

struct MemoryClient(AtomicUsize);

#[async_trait]
impl SnmpClient for MemoryClient {
    async fn get(&self, oid: &Oid) -> std::result::Result<VarBind, SnmpError> {
        Ok((oid.clone(), SnmpValue::TimeTicks(100)))
    }

    async fn walk(&self, root: &Oid) -> std::result::Result<Vec<VarBind>, SnmpError> {
        let value = match root.to_string().as_str() {
            "1.3.6.1.2.1.25.2.3.1.2" => {
                SnmpValue::ObjectIdentifier(Oid::parse("1.3.6.1.2.1.25.2.1.2").unwrap())
            }
            "1.3.6.1.2.1.25.2.3.1.5" => SnmpValue::Integer(100),
            "1.3.6.1.2.1.25.2.3.1.6" => SnmpValue::Integer(25),
            _ => return Ok(Vec::new()),
        };
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![(root.child(1), value)])
    }
}

#[tokio::test]
async fn polls_optional_memory_columns_once_per_cycle() {
    let concrete = Arc::new(MemoryClient(AtomicUsize::new(0)));
    let client: Arc<dyn SnmpClient> = concrete.clone();
    let device = Device {
        id: 1,
        name: "test".into(),
        mgmt_ip: "192.0.2.1".into(),
        poll_interval_secs: 60,
        profile: "cisco_ios".into(),
        snmp_version: 2,
        enabled: true,
    };
    let mut state = PollState {
        interfaces: vec![],
        interface_ids: HashMap::new(),
        counters: CounterTracker::default(),
        buffer: Buffer::new(10, 10, Duration::from_secs(60)),
    };
    let metric_ids = HashMap::from([("memory_used_pct".to_string(), 1)]);
    let outcome = poll_once(
        &device,
        &netwatch_profiles::cisco_ios::CiscoIos,
        &client,
        &metric_ids,
        &mut state,
    )
    .await
    .unwrap();
    assert_eq!(concrete.0.load(Ordering::SeqCst), 3);
    assert_eq!(outcome.samples, 1);
    assert_eq!(state.buffer.len(), 1);
}

#[async_trait]
impl SnmpClient for CounterClient {
    async fn get(&self, oid: &Oid) -> std::result::Result<VarBind, SnmpError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok((oid.clone(), SnmpValue::TimeTicks(10_000)))
    }

    async fn walk(&self, root: &Oid) -> std::result::Result<Vec<VarBind>, SnmpError> {
        let poll = self.0.load(Ordering::SeqCst);
        let value = match root.to_string().as_str() {
            "1.3.6.1.2.1.31.1.1.1.19" if poll == 4 => return Ok(Vec::new()),
            "1.3.6.1.2.1.31.1.1.1.19" => SnmpValue::TimeTicks(if poll >= 3 { 100 } else { 0 }),
            "1.3.6.1.2.1.31.1.1.1.6" => SnmpValue::Counter64((1_u64 << 60) + poll as u64 * 7),
            _ => return Ok(Vec::new()),
        };
        Ok(vec![(root.child(1), value)])
    }
}

#[tokio::test]
#[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn stores_exact_counter_deltas_and_discontinuities() {
    let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version)
         VALUES ('counter-test', '192.0.2.124', 'test', 'cisco_ios', 2) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let interfaces = vec![InterfaceRef {
        if_index: 1,
        name: "test".into(),
        speed_bps: Some(1_000_000_000),
    }];
    let interface_ids = netwatch_db::interfaces::reconcile(&pool, id, &interfaces)
        .await
        .unwrap();
    let profile = netwatch_profiles::cisco_ios::CiscoIos;
    netwatch_db::writer::upsert_metric_defs(&pool, profile.metrics())
        .await
        .unwrap();
    let metric_ids = netwatch_db::writer::load_metric_ids(&pool).await.unwrap();
    let device = Device {
        id,
        name: "counter-test".into(),
        mgmt_ip: "192.0.2.124".into(),
        poll_interval_secs: 1,
        profile: "cisco_ios".into(),
        snmp_version: 2,
        enabled: true,
    };
    let client: Arc<dyn SnmpClient> = Arc::new(CounterClient(AtomicUsize::new(0)));
    let mut state = PollState {
        interfaces,
        interface_ids,
        counters: CounterTracker::default(),
        buffer: Buffer::new(100, 100, Duration::from_secs(60)),
    };
    for _ in 0..6 {
        poll_once(&device, &profile, &client, &metric_ids, &mut state)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    state
        .buffer
        .flush(&mut pool.acquire().await.unwrap())
        .await
        .unwrap();
    let rows: Vec<(Option<f64>, Option<f64>, bool)> = sqlx::query_as(
        "SELECT delta, dt_seconds, counter_reset FROM raw_samples
         WHERE device_id = $1 AND metric_id = $2 ORDER BY ts",
    )
    .bind(id)
    .bind(metric_ids["if_hc_in_octets"])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0], (None, None, false));
    assert_eq!(rows[1].0, Some(7.0));
    assert!(rows[1].1.unwrap() > 0.0);
    assert_eq!(rows[2], (None, None, true));
    assert_eq!(rows[3], (None, None, false));
    assert_eq!(rows[4], (None, None, false));
    assert_eq!(rows[5].0, Some(7.0));
    assert!(rows[5].1.unwrap() > 0.0);
    let utilization: Vec<f64> = sqlx::query_scalar(
        "SELECT value FROM raw_samples WHERE device_id = $1 AND metric_id = $2 ORDER BY ts",
    )
    .bind(id)
    .bind(metric_ids["if_in_util_pct"])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(utilization.len(), 2);
    let expected = 7.0 / rows[1].1.unwrap() * 8.0 / 1_000_000_000.0 * 100.0;
    assert!((utilization[0] - expected).abs() < 1e-10);
    let invalid = sqlx::query(
        "INSERT INTO raw_samples (device_id, metric_id, ts, value, dt_seconds)
         VALUES ($1, $2, now() + interval '1 second', 1, 1)",
    )
    .bind(id)
    .bind(metric_ids["if_hc_in_octets"])
    .execute(&pool)
    .await;
    assert!(
        invalid.is_err(),
        "elapsed time without a delta must be rejected"
    );
    sqlx::query("DELETE FROM raw_samples WHERE device_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM devices WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
}
