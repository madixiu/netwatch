//! Discover enabled devices and supervise their polling tasks.

use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Result;
use netwatch_core::model::Device;
use netwatch_core::{oid::Oid, snmp_value::SnmpValue};
use netwatch_crypto::SecretKey;
use netwatch_db::{credentials, devices, writer};
use netwatch_profiles::{registry, Profile};
use netwatch_snmp::{AsyncSnmpSession, LimitedSnmpClient, MockSnmpClient, SnmpClient};
use sqlx::PgPool;
use tokio::{
    sync::{watch, Semaphore},
    task::JoinSet,
};
use tracing::{error, info};

use crate::fake::FakeProfile;

pub async fn run(
    pool: PgPool,
    key: SecretKey,
    limit: Arc<Semaphore>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let fake_mode = std::env::var("NETWATCH_FAKE_SNMP").ok().as_deref() == Some("1");
    let profiles = registry();
    if fake_mode {
        writer::upsert_metric_defs(&pool, FakeProfile.metrics()).await?;
    } else {
        for profile in &profiles {
            writer::upsert_metric_defs(&pool, profile.metrics()).await?;
        }
    }
    let metric_ids = Arc::new(writer::load_metric_ids(&pool).await?);
    let mut tasks: JoinSet<(i64, Result<()>)> = JoinSet::new();
    let mut active = HashSet::new();
    let mut icmp_tasks: JoinSet<(i64, Result<()>)> = JoinSet::new();
    let mut icmp_active = HashSet::new();
    let icmp_limit = Arc::new(Semaphore::new(4));
    let mut ticker = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Ok((id, result)) => {
                        active.remove(&id);
                        if let Err(e) = result {
                            error!(device_id = id, error = %e, "device task failed");
                        }
                    }
                    Err(e) => {
                        // A panicked task cannot be mapped back to its device ID.
                        // Restarting the scheduler avoids silently losing that device.
                        return Err(e.into());
                    }
                }
            }
            Some(result) = icmp_tasks.join_next(), if !icmp_tasks.is_empty() => {
                match result {
                    Ok((id, result)) => {
                        icmp_active.remove(&id);
                        if let Err(e) = result { error!(device_id = id, error = %e, "ICMP task failed"); }
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            _ = ticker.tick() => {
                match devices::list_enabled(&pool).await {
                    Ok(list) => {
                        for device in list {
                            if !fake_mode && icmp_active.insert(device.id) {
                                let id = device.id;
                                let probe_device = device.clone();
                                let p = pool.clone();
                                let rx = shutdown.clone();
                                let probe_limit = icmp_limit.clone();
                                icmp_tasks.spawn(async move {
                                    (id, crate::icmp::run(p, probe_device, probe_limit, rx).await)
                                });
                            }
                            if active.contains(&device.id) {
                                continue;
                            }
                            let permit = limit.acquire().await?;
                            let result = make_client(&pool, &key, &profiles, &device, fake_mode).await;
                            drop(permit);
                            match result {
                                Ok((profile, client)) => {
                                    let client = Arc::new(LimitedSnmpClient::new(client, limit.clone()));
                                    let id = device.id;
                                    let p = pool.clone();
                                    let ids = metric_ids.clone();
                                    let rx = shutdown.clone();
                                    active.insert(id);
                                    let mac_rx = shutdown.clone();
                                    tokio::spawn(crate::macs::run(
                                        pool.clone(),
                                        device.id,
                                        device.mgmt_ip.clone(),
                                        key.clone(),
                                        mac_rx,
                                    ));
                                    tasks.spawn(async move {
                                        (id, crate::devices_task::run(p, device, profile, client, ids, rx).await)
                                    });
                                }
                                Err(e) => {
                                    error!(device = %device.name, error = %e, "cannot start device task");
                                    if let Err(probe_error) = netwatch_db::probes::record(&pool, device.id, "snmp", "error", None, Some("Cannot start SNMP session; see poller logs")).await {
                                        error!(device = %device.name, error = %probe_error, "cannot record SNMP probe");
                                    }
                                    if let Err(status_error) = devices::record_poll_status(&pool, device.id, "error", None, Some("Cannot start device polling; see poller logs")).await {
                                        error!(device = %device.name, error = %status_error, "cannot record poll status");
                                    }
                                },
                            }
                        }
                    }
                    Err(e) => error!(error = %e, "cannot reload devices"),
                }
            }
        }
    }

    info!("scheduler shutting down; waiting for tasks to flush");
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((_, Ok(()))) => {}
            Ok((id, Err(e))) => {
                error!(device_id = id, error = %e, "device task failed during shutdown")
            }
            Err(e) => error!(error = %e, "device task panicked during shutdown"),
        }
    }
    while let Some(result) = icmp_tasks.join_next().await {
        match result {
            Ok((_, Ok(()))) => {}
            Ok((id, Err(e))) => {
                error!(device_id = id, error = %e, "ICMP task failed during shutdown")
            }
            Err(e) => error!(error = %e, "ICMP task panicked during shutdown"),
        }
    }
    Ok(())
}

async fn make_client(
    pool: &PgPool,
    key: &SecretKey,
    profiles: &[Arc<dyn Profile>],
    device: &Device,
    fake_mode: bool,
) -> Result<(Arc<dyn Profile>, Arc<dyn SnmpClient>)> {
    if fake_mode {
        return Ok((
            Arc::new(FakeProfile),
            Arc::new(MockSnmpClient {
                scalars: Default::default(),
                tables: vec![
                    (
                        Oid::parse("1.3.6.1.2.1.2.2.1.2")?,
                        vec![(
                            Oid::parse("1.3.6.1.2.1.2.2.1.2.1")?,
                            SnmpValue::OctetString(b"fake-port-1".to_vec()),
                        )],
                    ),
                    (Oid::parse("1.3.6.1.2.1.2.2.1.5")?, vec![]),
                    (Oid::parse("1.3.6.1.2.1.31.1.1.1.15")?, vec![]),
                ],
            }),
        ));
    }
    let profile = profiles
        .iter()
        .find(|p| p.name() == device.profile)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("unknown profile {}", device.profile))?;
    let creds = credentials::load_decrypted(pool, device.id, key).await?;
    let session = AsyncSnmpSession::open(&format!("{}:161", device.mgmt_ip), &creds).await?;
    Ok((profile, Arc::new(session)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
    async fn starts_polling_devices_added_after_startup() {
        let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        sqlx::query("SELECT ensure_raw_partitions(7)")
            .execute(&pool)
            .await
            .unwrap();
        let seed: i64 = sqlx::query_scalar(
            "INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version, enabled)
             VALUES ('scheduler-seed', '192.0.2.112', 'test', 'fake', 2, false) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO interfaces (device_id, if_index, name) VALUES ($1, 1, 'seed')")
            .bind(seed)
            .execute(&pool)
            .await
            .unwrap();
        std::env::set_var("NETWATCH_FAKE_SNMP", "1");
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(
            pool.clone(),
            SecretKey::from_bytes([0; 32]),
            Arc::new(Semaphore::new(64)),
            rx,
        ));
        tokio::time::sleep(Duration::from_secs(1)).await;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version, poll_interval_secs)
             VALUES ('scheduler-test', '192.0.2.111', 'test', 'fake', 2, 1) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM raw_samples WHERE device_id = $1")
                        .bind(id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                if count > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await;
        sqlx::query("UPDATE devices SET enabled = false WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let stopped_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM poll_status WHERE device_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let later_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM poll_status WHERE device_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        tx.send(true).unwrap();
        task.await.unwrap().unwrap();
        let mapped: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM raw_samples s
             JOIN interfaces i ON i.id = s.interface_id AND i.device_id = s.device_id
             JOIN metrics_catalog m ON m.id = s.metric_id
             WHERE s.device_id = $1 AND i.if_index = 1 AND i.id <> 1 AND m.name = 'fake_in_octets'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM poll_status WHERE device_id = $1 ORDER BY ts DESC LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM raw_samples WHERE device_id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM poll_status WHERE device_id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(seed)
            .execute(&pool)
            .await
            .unwrap();
        assert!(result.is_ok(), "new device was not polled");
        assert_eq!(stopped_count, later_count, "disabled device kept polling");
        assert!(
            mapped > 0,
            "interface samples must reference the interface row"
        );
        assert_eq!(status.as_deref(), Some("partial"));
    }
}
