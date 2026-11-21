use std::{net::SocketAddr, path::PathBuf};

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{Request, StatusCode},
    Router,
};
use netwatch_core::scan::parse_allowlist;
use netwatch_crypto::SecretKey;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn monitoring_endpoints_require_auth_and_report_rates_and_status() {
    use chrono::{Duration, SecondsFormat, Utc};
    use netwatch_core::model::{MetricDef, MetricKind, SampleRow};

    let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
    let hash = tokio::task::spawn_blocking(|| {
        netwatch_crypto::password::hash(b"test password for monitoring")
    })
    .await
    .unwrap()
    .unwrap();
    sqlx::query("INSERT INTO local_users (username, password_hash, role) VALUES ('monitor-viewer', $1, 'viewer')")
        .bind(hash).execute(&pool).await.unwrap();
    let device: i64 = sqlx::query_scalar(
        "INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version)
         VALUES ('monitor-test', '192.0.2.125', 'test', 'cisco_ios', 2) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let removed_interface: i64 = sqlx::query_scalar(
        "INSERT INTO interfaces (device_id, if_index, name, removed_at) VALUES ($1, 2, 'old port', now()) RETURNING id",
    )
    .bind(device)
    .fetch_one(&pool)
    .await
    .unwrap();
    let interface: i64 = sqlx::query_scalar(
        "INSERT INTO interfaces (device_id, if_index, name) VALUES ($1, 1, 'port 1') RETURNING id",
    )
    .bind(device)
    .fetch_one(&pool)
    .await
    .unwrap();
    netwatch_db::devices::record_poll_status(
        &pool,
        device,
        "partial",
        Some(200),
        Some("test problem"),
    )
    .await
    .unwrap();
    netwatch_db::writer::upsert_metric_defs(
        &pool,
        &[
            MetricDef {
                name: "if_hc_in_octets",
                unit: "bytes",
                kind: MetricKind::Counter,
            },
            MetricDef {
                name: "cpu_5min_pct",
                unit: "%",
                kind: MetricKind::Gauge,
            },
            MetricDef {
                name: "if_in_errors",
                unit: "packets",
                kind: MetricKind::Counter,
            },
            MetricDef {
                name: "if_oper_status",
                unit: "state",
                kind: MetricKind::Gauge,
            },
            MetricDef {
                name: "if_in_util_pct",
                unit: "%",
                kind: MetricKind::Gauge,
            },
        ],
    )
    .await
    .unwrap();
    let ids = netwatch_db::writer::load_metric_ids(&pool).await.unwrap();
    let now = Utc::now();
    let rows = [
        SampleRow {
            device_id: device,
            metric_id: ids["if_hc_in_octets"],
            interface_id: interface,
            ts: now - Duration::minutes(1),
            value: 1000.0,
            delta: Some(200.0),
            dt_seconds: Some(2.0),
            counter_reset: false,
            mapping_verified: true,
        },
        SampleRow {
            device_id: device,
            metric_id: ids["if_in_errors"],
            interface_id: interface,
            ts: now - Duration::minutes(1),
            value: 100.0,
            delta: Some(10.0),
            dt_seconds: Some(2.0),
            counter_reset: false,
            mapping_verified: true,
        },
        SampleRow {
            device_id: device,
            metric_id: ids["if_oper_status"],
            interface_id: interface,
            ts: now - Duration::minutes(1),
            value: 2.0,
            delta: None,
            dt_seconds: None,
            counter_reset: false,
            mapping_verified: true,
        },
        SampleRow {
            device_id: device,
            metric_id: ids["if_in_util_pct"],
            interface_id: interface,
            ts: now - Duration::minutes(1),
            value: 12.5,
            delta: None,
            dt_seconds: None,
            counter_reset: false,
            mapping_verified: true,
        },
        SampleRow {
            device_id: device,
            metric_id: ids["cpu_5min_pct"],
            interface_id: 0,
            ts: now - Duration::minutes(1),
            value: 42.0,
            delta: None,
            dt_seconds: None,
            counter_reset: false,
            mapping_verified: true,
        },
    ];
    netwatch_db::writer::copy_samples(&pool, &rows)
        .await
        .unwrap();
    netwatch_db::probes::record(&pool, device, "icmp", "ok", Some(0.5), None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO hardware_snapshot (device_id, status) VALUES ($1, 'ok')")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO hardware_components (device_id, entity_index, kind, name, status, value) VALUES ($1, 1, 'temperature', 'sensor', 'ok', 25.5)")
        .bind(device).execute(&pool).await.unwrap();
    let mut legacy = rows[0].clone();
    legacy.ts = now - Duration::seconds(30);
    legacy.mapping_verified = false;
    netwatch_db::writer::copy_samples(&pool, &[legacy])
        .await
        .unwrap();
    let app = crate::router(
        crate::AppState::new(pool.clone(), SecretKey::from_bytes([9; 32]), vec![])
            .await
            .unwrap(),
        PathBuf::from("nonexistent-test-web"),
    );
    assert_eq!(
        call(&app, "GET", "/api/devices", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, login) = call(
        &app,
        "POST",
        "/api/login",
        None,
        Some(json!({"username":"monitor-viewer", "password":"test password for monitoring"})),
    )
    .await;
    let token = login["token"].as_str().unwrap();
    let (_, list) = call(&app, "GET", "/api/devices", Some(token), None).await;
    assert_eq!(list[0]["status"], "partial");
    let (_, detail) = call(
        &app,
        "GET",
        &format!("/api/devices/{device}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(detail["interfaces"][0]["id"], interface);
    assert!(detail["interfaces"][0]["first_seen"].is_string());
    assert_eq!(detail["interfaces"][1]["id"], removed_interface);
    assert!(detail["interfaces"][1]["first_seen"].is_string());
    assert!(detail["interfaces"][1]["removed_at"].is_string());
    assert_eq!(detail["probes"][0]["latency_ms"], 0.5);
    assert_eq!(detail["components"][0]["value"], 25.5);
    let fmt = |t: chrono::DateTime<Utc>| {
        t.to_rfc3339_opts(SecondsFormat::Secs, true)
            .replace(':', "%3A")
    };
    let range = format!(
        "from={}&to={}",
        fmt(now - Duration::hours(1)),
        fmt(now + Duration::seconds(1))
    );
    let (status, traffic) = call(
        &app,
        "GET",
        &format!("/api/metrics/traffic?device={device}&interface={interface}&{range}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{traffic}");
    assert_eq!(traffic["source"], "raw");
    assert_eq!(traffic["points"].as_array().unwrap().len(), 1);
    assert_eq!(traffic["points"][0]["value"], 800.0);
    let (status, health) = call(
        &app,
        "GET",
        &format!("/api/metrics/interface-health?device={device}&interface={interface}&{range}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{health}");
    let points = health["points"].as_array().unwrap();
    for (name, value) in [
        ("if_in_errors", 5.0),
        ("if_oper_status", 2.0),
        ("if_in_util_pct", 12.5),
    ] {
        assert_eq!(
            points.iter().find(|p| p["metric"] == name).unwrap()["value"],
            value
        );
    }
    sqlx::query(
        "INSERT INTO rollup_1m
         (device_id, metric_id, interface_id, bucket, min, max, avg, last, count, sum_value, sum_delta, sum_seconds, reset_count, mapping_verified)
         VALUES ($1, $2, $3, date_trunc('minute', now() - interval '1 hour'), 1, 2, 1.5, 2, 2, 3, 200, 2, 0, true)",
    ).bind(device).bind(ids["if_hc_in_octets"]).bind(interface).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO rollup_1m
         (device_id, metric_id, interface_id, bucket, min, max, avg, last, count, sum_value, mapping_verified)
         VALUES ($1, $2, $3, date_trunc('minute', now() - interval '1 hour'), 1, 2, 1.5, 2, 2, 3, true)",
    ).bind(device).bind(ids["if_oper_status"]).bind(interface).execute(&pool).await.unwrap();
    sqlx::query("UPDATE rollup_state SET last_rolled_up_at = date_trunc('second', $1::timestamptz) WHERE granularity = '1m'")
        .bind(now).execute(&pool).await.unwrap();
    let longer = format!(
        "from={}&to={}",
        fmt(now - Duration::hours(24)),
        fmt(now + Duration::seconds(1))
    );
    let (status, rolled) = call(
        &app,
        "GET",
        &format!("/api/metrics/traffic?device={device}&interface={interface}&{longer}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rolled}");
    assert_eq!(rolled["source"], "1m");
    assert_eq!(
        rolled["processed_through"],
        now.to_rfc3339_opts(SecondsFormat::Secs, true)
    );
    assert_eq!(rolled["points"][0]["value"], 800.0);
    let (_, rolled_health) = call(
        &app,
        "GET",
        &format!("/api/metrics/interface-health?device={device}&interface={interface}&{longer}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(rolled_health["points"][0]["value"], 2.0);
    let (_, resources) = call(
        &app,
        "GET",
        &format!("/api/metrics/resources?device={device}&{range}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(resources["points"][0]["value"], 42.0);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/metrics/traffic?device={device}&interface=999999&{range}"),
            Some(token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE poll_status SET ts = now() - interval '1 hour' WHERE device_id = $1")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    let (_, list) = call(&app, "GET", "/api/devices", Some(token), None).await;
    assert_eq!(list[0]["status"], "stale");
    sqlx::query("DELETE FROM raw_samples WHERE device_id = $1")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM rollup_1m WHERE device_id = $1")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM poll_status WHERE device_id = $1")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM devices WHERE id = $1")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM local_users WHERE username = 'monitor-viewer'")
        .execute(&pool)
        .await
        .unwrap();
}

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("Content-Type", "application/json");
    }
    let mut request = builder
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
    ));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let data = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if data.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&data).unwrap()
        },
    )
}

#[tokio::test]
#[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn authenticated_scan_lifecycle() {
    let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
    let password = tokio::task::spawn_blocking(|| {
        netwatch_crypto::password::hash(b"correct password for testing")
    })
    .await
    .unwrap()
    .unwrap();
    for (name, role) in [("api-admin", "admin"), ("api-viewer", "viewer")] {
        sqlx::query("INSERT INTO local_users (username, password_hash, role) VALUES ($1, $2, $3)")
            .bind(name)
            .bind(&password)
            .bind(role)
            .execute(&pool)
            .await
            .unwrap();
    }
    let key = SecretKey::from_bytes([9; 32]);
    let app = crate::router(
        crate::AppState::new(
            pool.clone(),
            key.clone(),
            parse_allowlist("192.0.2.0/24").unwrap(),
        )
        .await
        .unwrap(),
        PathBuf::from("nonexistent-test-web"),
    );
    assert_eq!(
        call(&app, "GET", "/api/scans", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/login",
            None,
            Some(json!({"username":"api-admin", "password":"wrong"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, admin_login) = call(
        &app,
        "POST",
        "/api/login",
        None,
        Some(json!({"username":"api-admin", "password":"correct password for testing"})),
    )
    .await;
    let admin = admin_login["token"].as_str().unwrap();
    let (_, viewer_login) = call(
        &app,
        "POST",
        "/api/login",
        None,
        Some(json!({"username":"api-viewer", "password":"correct password for testing"})),
    )
    .await;
    let viewer = viewer_login["token"].as_str().unwrap();
    let profile = json!({"name":"api-profile", "credentials":{"version":"v2c", "community":"example-secret"}});
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/credential-profiles",
            Some(viewer),
            Some(profile.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, profile) = call(
        &app,
        "POST",
        "/api/credential-profiles",
        Some(admin),
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let profile_id = profile["id"].as_i64().unwrap();
    let (_, listed) = call(&app, "GET", "/api/credential-profiles", Some(viewer), None).await;
    assert!(!listed.to_string().contains("example-secret"));
    let v3 = json!({
        "name":"api-v3",
        "credentials":{"version":"v3", "username":"test-user", "auth_protocol":"sha256",
        "auth_secret":"auth-passphrase", "priv_protocol":"aes128", "priv_secret":"priv-passphrase"}
    });
    let (status, v3_profile) = call(
        &app,
        "POST",
        "/api/credential-profiles",
        Some(admin),
        Some(v3),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let v3_id = v3_profile["id"].as_i64().unwrap();
    let credentials = netwatch_db::scans::load_profile_credentials(&pool, &key, v3_id)
        .await
        .unwrap();
    match credentials {
        netwatch_core::model::SnmpCredentials::V3 {
            username,
            auth_secret,
            priv_secret,
            ..
        } => {
            assert_eq!(username, "test-user");
            assert_eq!(auth_secret.as_slice(), b"auth-passphrase");
            assert_eq!(priv_secret.as_slice(), b"priv-passphrase");
        }
        _ => panic!("wrong credential type"),
    }
    let invalid = json!({"cidr":"198.51.100.0/24", "credential_profile_id":profile_id});
    assert_eq!(
        call(&app, "POST", "/api/scans", Some(admin), Some(invalid))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let body = json!({"cidr":"192.0.2.0/30", "credential_profile_id":profile_id});
    assert_eq!(
        call(&app, "POST", "/api/scans", Some(viewer), Some(body.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, job) = call(&app, "POST", "/api/scans", Some(admin), Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let job_id = job["id"].as_i64().unwrap();
    assert_eq!(job["addresses_total"], 2);
    assert_eq!(job["requested_by"], "api-admin");
    let result_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_results (scan_job_id, ip, sys_name, vendor, profile) VALUES ($1, '192.0.2.1', 'api-router', 'cisco', 'cisco_ios') RETURNING id"
    ).bind(job_id).fetch_one(&pool).await.unwrap();
    let approve = format!("/api/scan-results/{result_id}/approve");
    assert_eq!(
        call(&app, "POST", &approve, Some(viewer), None).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, result) = call(&app, "POST", &approve, Some(admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let device_id = result["device_id"].as_i64().unwrap();
    let credentials = netwatch_db::credentials::load_decrypted(&pool, device_id, &key)
        .await
        .unwrap();
    match credentials {
        netwatch_core::model::SnmpCredentials::V2c { community } => {
            assert_eq!(&*community, "example-secret")
        }
        _ => panic!("wrong credential type"),
    }
    assert_eq!(
        call(&app, "POST", &approve, Some(admin), None).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/scan-results/999999999/reject",
            Some(admin),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", "/api/logout", Some(admin), None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", "/api/scans", Some(admin), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE user_sessions SET expires_at = now() WHERE user_id = (SELECT id FROM local_users WHERE username = 'api-viewer')")
        .execute(&pool).await.unwrap();
    assert_eq!(
        call(&app, "GET", "/api/scans", Some(viewer), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("DELETE FROM devices WHERE id = $1")
        .bind(device_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM scan_jobs WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM credential_profiles WHERE id = $1 OR id = $2")
        .bind(profile_id)
        .bind(v3_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM local_users WHERE username IN ('api-admin', 'api-viewer')")
        .execute(&pool)
        .await
        .unwrap();
}
