//! Scan persistence against a real Postgres. Run with:
//!   DATABASE_URL=postgres://... cargo test -p netwatch-db --test scans -- --ignored
use netwatch_db::scans::*;
use sqlx::PgPool;

#[tokio::test]
#[ignore]
async fn scan_lifecycle_and_approval() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let tag = format!("t{}", std::process::id());

    let profile = create_credential_profile(&pool, &NewCredentialProfile {
        name: format!("{tag}-prof"),
        snmp_version: 2,
        v2c_community_enc: Some("sealed-community".into()),
        v3_username: None,
        v3_auth_proto: None,
        v3_auth_secret_enc: None,
        v3_priv_proto: None,
        v3_priv_secret_enc: None,
    }).await.unwrap();

    let job = create_job(&pool, "10.251.0.0/30", profile, "tester", 2).await.unwrap();
    assert_eq!(job.status, "queued");
    assert_eq!(job.cidr, "10.251.0.0/30");

    let claimed = claim_next_job(&pool).await.unwrap().expect("queued job");
    assert_eq!(claimed.id, job.id);
    assert_eq!(claimed.status, "running");

    let found = vec![FoundDevice {
        ip: "10.251.0.1".into(),
        sys_object_id: Some("1.3.6.1.4.1.9.1.1".into()),
        sys_descr: Some("Cisco IOS".into()),
        sys_name: Some(format!("{tag}-core")),
        vendor: Some("cisco".into()),
        profile: Some("cisco_ios".into()),
        already_monitored: false,
    }];
    let cancel = record_progress(&pool, job.id, 2, &found).await.unwrap();
    assert!(!cancel);
    let results = list_results(&pool, job.id).await.unwrap();
    assert_eq!(results.len(), 1);
    let rid = results[0].id;

    // Approval copies the sealed secret and suffixes a colliding name.
    let dev1 = approve_result(&pool, rid).await.unwrap();
    let again = approve_result(&pool, rid).await;
    assert!(matches!(again, Err(ScanError::Conflict(_))));

    let secret: Option<String> = sqlx::query_scalar(
        "SELECT snmp_v2c_community_enc FROM credentials WHERE device_id = $1")
        .bind(dev1).fetch_one(&pool).await.unwrap();
    assert_eq!(secret.as_deref(), Some("sealed-community"));

    // Same IP from another scan: already the device's mgmt_ip, so the unique index blocks a duplicate.
    let job2 = create_job(&pool, "10.251.0.0/30", profile, "tester", 2).await.unwrap();
    claim_next_job(&pool).await.unwrap();
    record_progress(&pool, job2.id, 2, &[FoundDevice {
        ip: "10.251.0.1".into(), sys_object_id: None, sys_descr: None,
        sys_name: Some(format!("{tag}-core")), vendor: None,
        profile: Some("cisco_ios".into()), already_monitored: false,
    }]).await.unwrap();
    let r2 = list_results(&pool, job2.id).await.unwrap()[0].id;
    assert!(matches!(approve_result(&pool, r2).await, Err(ScanError::Conflict(_))));
    reject_result(&pool, r2).await.unwrap();

    // Cancel a queued job before it is claimed.
    let job3 = create_job(&pool, "10.252.0.0/30", profile, "tester", 2).await.unwrap();
    let cancelled = request_cancel(&pool, job3.id).await.unwrap();
    assert_eq!(cancelled.status, "cancelled");

    // Cleanup.
    sqlx::query("DELETE FROM devices WHERE id = $1").bind(dev1).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM scan_jobs WHERE credential_profile_id = $1").bind(profile).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM credential_profiles WHERE id = $1").bind(profile).execute(&pool).await.unwrap();
}
