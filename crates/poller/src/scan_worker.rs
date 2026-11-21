//! Scan worker: claims queued scan jobs and probes each address for SNMP.
//! Rules are in SCANNING.md sections 6 and 8.

use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Result;
use netwatch_core::{
    model::SnmpCredentials,
    oid::Oid,
    scan::{plan_scan, Cidr4},
    snmp_value::SnmpValue,
};
use netwatch_crypto::SecretKey;
use netwatch_db::scans::{self, FoundDevice, ScanJob};
use netwatch_profiles::DeviceContext;
use netwatch_snmp::{AsyncSnmpSession, LimitedSnmpClient, SnmpClient, SnmpError};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::{
    sync::{watch, Semaphore},
    task::JoinSet,
};
use tracing::{error, info, warn};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Addresses probed between progress writes and cancel checks.
const BATCH: usize = 25;
/// Concurrent probes per scan (SCANNING.md section 6).
const CONCURRENCY: usize = 32;
/// Hard limit per address, covering the SNMP timeout and retries.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn run(
    pool: PgPool,
    db_url: &str,
    key: SecretKey,
    allow: Vec<Cidr4>,
    limit: Arc<Semaphore>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    // A session lock ensures that a second poller cannot interrupt or run
    // another poller's scan. The dedicated connection releases it on exit.
    let mut lock = PgConnection::connect(db_url).await?;
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    loop {
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(724382901)")
            .fetch_one(&mut lock)
            .await?;
        if acquired {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => {}
        }
    }
    let interrupted = scans::mark_running_interrupted(&pool).await?;
    if interrupted > 0 {
        warn!(
            jobs = interrupted,
            "marked scans left running by a previous poller as interrupted"
        );
    }
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => {}
        }
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let job = match scans::claim_next_job(&pool).await {
                Ok(Some(job)) => job,
                Ok(None) => break,
                Err(e) => {
                    error!(error = %e, "claiming scan job failed");
                    break;
                }
            };
            info!(job = job.id, cidr = %job.cidr, profile = job.credential_profile_id, requester = %job.requested_by, "scan started");
            let outcome = run_job(&pool, &key, &allow, &limit, &job, &shutdown).await;
            let (status, error) = match &outcome {
                Ok(JobEnd::Done { auth_failures }) => (
                    "done",
                    (*auth_failures > 0).then(|| format!(
                        "SNMP authentication failed for {auth_failures} address(es). Check the credentials and protocols; client compatibility may also be a factor."
                    )),
                ),
                Ok(JobEnd::Cancelled) => ("cancelled", None),
                Ok(JobEnd::Interrupted) => {
                    ("interrupted", Some("poller shutting down".to_string()))
                }
                Err(e) => ("failed", Some(e.clone())),
            };
            scans::finish_job(&pool, job.id, status, error.as_deref()).await?;
            info!(job = job.id, status, "scan finished");
        }
    }
}

enum JobEnd {
    Done { auth_failures: usize },
    Cancelled,
    Interrupted,
}

async fn run_job(
    pool: &PgPool,
    key: &SecretKey,
    allow: &[Cidr4],
    limit: &Arc<Semaphore>,
    job: &ScanJob,
    shutdown: &watch::Receiver<bool>,
) -> Result<JobEnd, String> {
    let creds = scans::load_profile_credentials(pool, key, job.credential_profile_id)
        .await
        .map_err(|e| e.to_string())?;
    // Checked again here: the API validates too, but the allowlist can change between request and run.
    let (_, hosts) = plan_scan(&job.cidr, allow).map_err(|e| e.to_string())?;
    let monitored = scans::monitored_ips_in(pool, &job.cidr)
        .await
        .map_err(|e| e.to_string())?;

    let mut shutdown_rx = shutdown.clone();
    let mut auth_failures = 0;
    for chunk in hosts.chunks(BATCH) {
        if *shutdown.borrow() {
            return Ok(JobEnd::Interrupted);
        }
        let (found, batch_auth_failures) = tokio::select! {
            _ = shutdown_rx.changed() => return Ok(JobEnd::Interrupted),
            result = probe_batch(chunk, &creds, &monitored, limit) => {
                result.map_err(|e| e.to_string())?
            }
        };
        auth_failures += batch_auth_failures;
        let cancel = scans::record_progress(pool, job.id, chunk.len() as i32, &found)
            .await
            .map_err(|e| e.to_string())?;
        if cancel {
            return Ok(JobEnd::Cancelled);
        }
        if *shutdown.borrow() {
            return Ok(JobEnd::Interrupted);
        }
    }
    Ok(JobEnd::Done { auth_failures })
}

enum ProbeOutcome {
    Found(FoundDevice),
    Silent,
    AuthenticationFailure,
}

async fn probe_batch(
    chunk: &[std::net::Ipv4Addr],
    creds: &SnmpCredentials,
    monitored: &HashSet<String>,
    limit: &Arc<Semaphore>,
) -> Result<(Vec<FoundDevice>, usize)> {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set = JoinSet::new();
    for &ip in chunk {
        let sem = sem.clone();
        let creds = creds.clone();
        let limit = limit.clone();
        let already = monitored.contains(&ip.to_string());
        set.spawn(async move {
            let _permit = sem.acquire_owned().await?;
            let mut outcome = probe(ip, &creds, limit).await?;
            if let ProbeOutcome::Found(ref mut found) = outcome {
                found.already_monitored = already;
            }
            Ok::<_, anyhow::Error>(outcome)
        });
    }
    let mut out = Vec::new();
    let mut auth_failures = 0;
    while let Some(joined) = set.join_next().await {
        match joined?? {
            ProbeOutcome::Found(found) => out.push(found),
            ProbeOutcome::Silent => {}
            ProbeOutcome::AuthenticationFailure => auth_failures += 1,
        }
    }
    Ok((out, auth_failures))
}

/// Returns `Some` only when the address answers sysObjectID. Silent addresses are not recorded.
async fn probe(
    ip: std::net::Ipv4Addr,
    creds: &SnmpCredentials,
    limit: Arc<Semaphore>,
) -> Result<ProbeOutcome> {
    let permit = limit.acquire().await?;
    let session = tokio::time::timeout(
        PROBE_TIMEOUT,
        AsyncSnmpSession::open_for_scan(&format!("{ip}:161"), creds),
    )
    .await;
    let session = match session {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            warn!(%ip, error = %error, "could not initialize scan session");
            return Ok(ProbeOutcome::Silent);
        }
        Err(_) => return Ok(ProbeOutcome::Silent),
    };
    drop(permit);
    let session = LimitedSnmpClient::new(Arc::new(session), limit);
    Ok(probe_session(ip, &session).await)
}

async fn probe_session(ip: std::net::Ipv4Addr, session: &dyn SnmpClient) -> ProbeOutcome {
    let object_oid = Oid::parse("1.3.6.1.2.1.1.2.0").expect("valid OID");
    let (_, object_id) = match session.get(&object_oid).await {
        Ok(value) => value,
        Err(SnmpError::Authentication) => return ProbeOutcome::AuthenticationFailure,
        Err(_) => return ProbeOutcome::Silent,
    };
    let SnmpValue::ObjectIdentifier(object_id) = object_id else {
        warn!(%ip, "scan response has an invalid sysObjectID type");
        return ProbeOutcome::Silent;
    };
    let sys_object_id = object_id.to_string();
    let read_text = |oid: &'static str| async move {
        let oid = Oid::parse(oid).expect("valid OID");
        session
            .get(&oid)
            .await
            .ok()?
            .1
            .as_str_lossy()
            .map(|s| s.into_owned())
    };
    let (sys_descr, sys_name) = tokio::join!(
        read_text("1.3.6.1.2.1.1.1.0"),
        read_text("1.3.6.1.2.1.1.5.0")
    );

    let profile = netwatch_profiles::detect(&DeviceContext {
        sys_object_id: &sys_object_id,
        sys_descr: sys_descr.as_deref().unwrap_or(""),
    });
    let (profile_name, vendor) = match &profile {
        Some(p) => {
            let name = p.name().to_string();
            let vendor = name.split('_').next().unwrap_or("unknown").to_string();
            (Some(name), Some(vendor))
        }

        None => (None, None),
    };

    ProbeOutcome::Found(FoundDevice {
        ip: ip.to_string(),
        sys_object_id: Some(sys_object_id),
        sys_descr,
        sys_name,
        vendor,
        profile: profile_name,
        already_monitored: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_core::oid::Oid;
    use netwatch_snmp::MockSnmpClient;
    use std::collections::HashMap;

    #[tokio::test]
    async fn identifies_responder_even_when_optional_fields_are_missing() {
        let ip = "192.0.2.1".parse().unwrap();
        let object_oid = Oid::parse("1.3.6.1.2.1.1.2.0").unwrap();
        let mut scalars = HashMap::new();
        scalars.insert(
            object_oid,
            SnmpValue::ObjectIdentifier(Oid::parse("1.3.6.1.4.1.14988.1").unwrap()),
        );
        let mock = MockSnmpClient {
            scalars,
            tables: vec![],
        };
        let ProbeOutcome::Found(result) = probe_session(ip, &mock).await else {
            panic!("expected responder");
        };
        assert_eq!(result.profile.as_deref(), Some("mikrotik"));
        assert_eq!(result.ip, "192.0.2.1");
        assert!(result.sys_name.is_none());
    }

    #[tokio::test]
    async fn does_not_record_missing_object_id() {
        let mock = MockSnmpClient {
            scalars: HashMap::new(),
            tables: vec![],
        };
        assert!(matches!(
            probe_session("192.0.2.2".parse().unwrap(), &mock).await,
            ProbeOutcome::Silent
        ));
    }

    #[tokio::test]
    async fn reports_authentication_failure_separately_from_silent_hosts() {
        struct AuthFailure;
        #[async_trait::async_trait]
        impl SnmpClient for AuthFailure {
            async fn get(&self, _: &Oid) -> Result<netwatch_snmp::VarBind, SnmpError> {
                Err(SnmpError::Authentication)
            }
            async fn walk(&self, _: &Oid) -> Result<Vec<netwatch_snmp::VarBind>, SnmpError> {
                Err(SnmpError::Authentication)
            }
        }
        assert!(matches!(
            probe_session("192.0.2.2".parse().unwrap(), &AuthFailure).await,
            ProbeOutcome::AuthenticationFailure
        ));
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
    async fn worker_refuses_ranges_outside_allowlist() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let key = SecretKey::from_bytes([7; 32]);
        let profile = scans::NewCredentialProfile {
            name: "scan-worker-test".into(),
            snmp_version: 2,
            v2c_community_enc: Some(netwatch_crypto::seal(
                &key,
                netwatch_crypto::Purpose::SnmpV2cCommunity,
                b"test-community",
            )),
            v3_username: None,
            v3_auth_proto: None,
            v3_auth_secret_enc: None,
            v3_priv_proto: None,
            v3_priv_secret_enc: None,
        };
        let profile_id = scans::create_credential_profile(&pool, &profile)
            .await
            .unwrap();
        let job = scans::create_job(&pool, "192.0.2.1/32", profile_id, "test", 1)
            .await
            .unwrap();
        let (tx, rx) = watch::channel(false);
        let worker_pool = pool.clone();
        let worker = tokio::spawn(async move {
            run(
                worker_pool,
                &url,
                key,
                vec![],
                Arc::new(Semaphore::new(64)),
                rx,
            )
            .await
        });
        let completed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let job = scans::get_job(&pool, job.id).await.unwrap();
                if job.status == "failed" {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        tx.send(true).unwrap();
        worker.await.unwrap().unwrap();
        sqlx::query("DELETE FROM scan_jobs WHERE id = $1")
            .bind(job.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM credential_profiles WHERE id = $1")
            .bind(profile_id)
            .execute(&pool)
            .await
            .unwrap();
        let completed = completed.expect("worker did not finish job");
        assert!(completed.error.unwrap().contains("allowlist"));
        assert_eq!(completed.addresses_done, 0);
    }
}
