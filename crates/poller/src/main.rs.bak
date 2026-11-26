mod buffer;
mod config;
mod counters;
mod devices_task;
mod fake;
mod hardware;
mod icmp;
mod interfaces;
mod macs;
mod jobs;
mod scan_worker;
mod scheduler;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::watch;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::PollerConfig;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    // Fails fast if DATABASE_URL or a valid key is missing.
    let cfg = PollerConfig::from_env()?;
    info!("configuration loaded");

    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&cfg.db_url)
        .await?;

    // Migrations: in production this is a separate `netwatch-migrate` binary.
    // Here, one binary → one migration run → no race with the API.
    sqlx::migrate!("../db/migrations").run(&pool).await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let snmp_limit = Arc::new(tokio::sync::Semaphore::new(64));
    info!("netwatch-poller starting");

    let sched_pool = pool.clone();
    let sched_rx = shutdown_rx.clone();
    let sched_key = cfg.secret_key.clone();
    let sched_limit = snmp_limit.clone();
    let mut sched =
        tokio::spawn(
            async move { scheduler::run(sched_pool, sched_key, sched_limit, sched_rx).await },
        );

    let mut jobs = tokio::task::JoinSet::new();
    {
        let p = pool.clone();
        let r = shutdown_rx.clone();
        let key = cfg.secret_key.clone();
        let allow = cfg.scan_allow.clone();
        let db_url = cfg.db_url.clone();
        jobs.spawn(async move { scan_worker::run(p, &db_url, key, allow, snmp_limit, r).await });
    }
    {
        let p = pool.clone();
        let r = shutdown_rx.clone();
        jobs.spawn(async move { jobs::partitions::run(p, r).await });
    }
    {
        let p = pool.clone();
        let r = shutdown_rx.clone();
        jobs.spawn(async move { jobs::rollup::run(p, r).await });
    }
    {
        let p = pool.clone();
        let r = shutdown_rx.clone();
        jobs.spawn(async move { jobs::retention::run(p, r).await });
    }
    {
        let p = pool.clone();
        let r = shutdown_rx.clone();
        jobs.spawn(async move { jobs::heartbeat::run(p, env!("CARGO_PKG_VERSION"), r).await });
    }

    let failure = tokio::select! {
        _ = wait_for_shutdown() => None,
        result = &mut sched => Some(anyhow::anyhow!("scheduler stopped unexpectedly: {result:?}")),
        result = jobs.join_next() => Some(anyhow::anyhow!("background job stopped unexpectedly: {result:?}")),
    };
    info!("shutdown signal received");
    let _ = shutdown_tx.send(true);

    let _ = tokio::time::timeout(Duration::from_secs(15), async {
        if !sched.is_finished() {
            if let Err(e) = sched.await {
                warn!(error = %e, "scheduler join failed during shutdown");
            }
        }
        while let Some(r) = jobs.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "job failed"),
                Err(e) => warn!(error = %e, "job join failed during shutdown"),
            }
        }
    })
    .await;

    if let Some(error) = failure {
        return Err(error);
    }
    info!("netwatch-poller stopped");
    Ok(())
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
