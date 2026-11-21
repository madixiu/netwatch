use std::{net::IpAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use netwatch_core::model::Device;
use sqlx::PgPool;
use tokio::{
    process::Command,
    sync::{watch, Semaphore},
};
use tracing::warn;

struct ProbeResult {
    status: &'static str,
    latency_ms: Option<f64>,
    detail: Option<String>,
}

fn parse_latency(output: &str) -> Option<f64> {
    let value = output.split("time=").nth(1)?.split_whitespace().next()?;
    let value: f64 = value.parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

async fn probe(address: IpAddr) -> ProbeResult {
    let mut command = Command::new("ping");
    command
        .args(["-n", "-c", "1", "-W", "2", "--", &address.to_string()])
        .env("LC_ALL", "C")
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(4), command.output()).await {
        Ok(Ok(output)) if output.status.success() => ProbeResult {
            status: "ok",
            latency_ms: parse_latency(&String::from_utf8_lossy(&output.stdout)),
            detail: None,
        },
        Ok(Ok(output)) if output.status.code() == Some(1) => ProbeResult {
            status: "failed",
            latency_ms: None,
            detail: Some("No ICMP reply; traffic may be filtered".into()),
        },
        Ok(Ok(output)) => ProbeResult {
            status: "error",
            latency_ms: None,
            detail: Some(format!(
                "ping failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        },
        Ok(Err(error)) => ProbeResult {
            status: "error",
            latency_ms: None,
            detail: Some(format!("cannot run ping: {error}")),
        },
        Err(_) => ProbeResult {
            status: "error",
            latency_ms: None,
            detail: Some("ping process timed out".into()),
        },
    }
}

pub async fn run(
    pool: PgPool,
    device: Device,
    limit: Arc<Semaphore>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let address: IpAddr = device
        .mgmt_ip
        .parse()
        .context("invalid ICMP target address")?;
    loop {
        let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_optional(&pool)
            .await?;
        if enabled != Some(true) {
            return Ok(());
        }
        let permit = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            permit = limit.acquire() => permit.context("ICMP limiter closed")?,
        };
        let result = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            result = probe(address) => result,
        };
        drop(permit);
        if result.status == "error" {
            warn!(device = %device.name, detail = ?result.detail, "ICMP probe failed to run");
        }
        netwatch_db::probes::record(
            &pool,
            device.id,
            "icmp",
            result.status,
            result.latency_ms,
            result.detail.as_deref(),
        )
        .await?;
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tokio::time::sleep(Duration::from_secs(device.poll_interval_secs.max(1))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_latency_without_misrepresenting_upper_bounds() {
        assert_eq!(parse_latency("64 bytes: time=0.034 ms"), Some(0.034));
        assert_eq!(parse_latency("64 bytes: time<1 ms"), None);
        assert_eq!(parse_latency("time=NaN ms"), None);
    }

    #[tokio::test]
    #[ignore = "requires system ping and permission to send ICMP on loopback"]
    async fn pings_loopback_without_external_network() {
        let result = probe("127.0.0.1".parse().unwrap()).await;
        assert_eq!(result.status, "ok", "{:?}", result.detail);
        assert!(result.latency_ms.is_some());
    }

    #[tokio::test]
    #[ignore = "requires disposable DATABASE_URL and system ping with ICMP loopback permission"]
    async fn records_loopback_probe_and_stops_when_disabled() {
        let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version, poll_interval_secs) VALUES ('icmp-test', '127.0.0.1', 'test', 'cisco_ios', 2, 1) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let device = Device {
            id,
            name: "icmp-test".into(),
            mgmt_ip: "127.0.0.1".into(),
            profile: "cisco_ios".into(),
            snmp_version: 2,
            enabled: true,
            poll_interval_secs: 1,
        };
        let (_tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(pool.clone(), device, Arc::new(Semaphore::new(1)), rx));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM device_probe_results WHERE device_id = $1 AND protocol = 'icmp' AND status = 'ok' AND latency_ms IS NOT NULL")
                    .bind(id).fetch_one(&pool).await.unwrap();
                if count > 0 { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        sqlx::query("UPDATE devices SET enabled = false WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
