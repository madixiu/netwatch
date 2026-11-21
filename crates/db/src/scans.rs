//! Network scan persistence: credential profiles, scan jobs, results and approval.
//!
//! Secrets are stored sealed (see netwatch_crypto). Approval copies the sealed
//! columns from the profile into a new `credentials` row, so no re-encryption
//! or plaintext handling happens in the database layer.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found")]
    NotFound,
    #[error("credentials: {0}")]
    Credentials(String),
    #[error("{0}")]
    Conflict(String),
}

/// Sealed credential profile as written. Never returned by the API.
#[derive(Debug, Clone)]
pub struct NewCredentialProfile {
    pub name: String,
    pub snmp_version: i16,
    pub v2c_community_enc: Option<String>,
    pub v3_username: Option<String>,
    pub v3_auth_proto: Option<String>,
    pub v3_auth_secret_enc: Option<String>,
    pub v3_priv_proto: Option<String>,
    pub v3_priv_secret_enc: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct CredentialProfileSummary {
    pub id: i64,
    pub name: String,
    pub snmp_version: i16,
    pub v3_username: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn create_credential_profile(
    pool: &PgPool,
    p: &NewCredentialProfile,
) -> Result<i64, ScanError> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO credential_profiles
           (name, snmp_version, snmp_v2c_community_enc, snmp_v3_username, snmp_v3_auth_proto,
            snmp_v3_auth_secret_enc, snmp_v3_priv_proto, snmp_v3_priv_secret_enc)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&p.name)
    .bind(p.snmp_version)
    .bind(&p.v2c_community_enc)
    .bind(&p.v3_username)
    .bind(&p.v3_auth_proto)
    .bind(&p.v3_auth_secret_enc)
    .bind(&p.v3_priv_proto)
    .bind(&p.v3_priv_secret_enc)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn list_credential_profiles(
    pool: &PgPool,
) -> Result<Vec<CredentialProfileSummary>, ScanError> {
    Ok(sqlx::query_as::<_, CredentialProfileSummary>(
        "SELECT id, name, snmp_version, snmp_v3_username AS v3_username, created_at
         FROM credential_profiles ORDER BY id",
    )
    .fetch_all(pool)
    .await?)
}

/// A scan job as claimed by the poller.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ScanJob {
    pub id: i64,
    pub cidr: String,
    pub credential_profile_id: i64,
    pub status: String,
    pub requested_by: String,
    pub addresses_total: i32,
    pub addresses_done: i32,
    pub found_count: i32,
    pub cancel_requested: bool,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

const JOB_COLUMNS: &str = "id, host(cidr) || '/' || masklen(cidr) AS cidr, credential_profile_id,
    status::text AS status, requested_by, addresses_total, addresses_done, found_count,
    cancel_requested, error, created_at, started_at, finished_at";

pub async fn create_job(
    pool: &PgPool,
    cidr: &str,
    credential_profile_id: i64,
    requested_by: &str,
    addresses_total: i32,
) -> Result<ScanJob, ScanError> {
    let sql = format!(
        "INSERT INTO scan_jobs (cidr, credential_profile_id, requested_by, addresses_total)
         VALUES ($1::cidr, $2, $3, $4) RETURNING {JOB_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, ScanJob>(&sql)
        .bind(cidr)
        .bind(credential_profile_id)
        .bind(requested_by)
        .bind(addresses_total)
        .fetch_one(pool)
        .await?)
}

pub async fn get_job(pool: &PgPool, id: i64) -> Result<ScanJob, ScanError> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM scan_jobs WHERE id = $1");
    sqlx::query_as::<_, ScanJob>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ScanError::NotFound)
}

pub async fn list_jobs(pool: &PgPool, limit: i64) -> Result<Vec<ScanJob>, ScanError> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM scan_jobs ORDER BY id DESC LIMIT $1");
    Ok(sqlx::query_as::<_, ScanJob>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Claims the oldest queued job. `FOR UPDATE SKIP LOCKED` keeps two poller instances
/// from running the same job.
pub async fn claim_next_job(pool: &PgPool) -> Result<Option<ScanJob>, ScanError> {
    let sql = format!(
        "UPDATE scan_jobs SET status = 'running', started_at = now()
         WHERE id = (
            SELECT id FROM scan_jobs WHERE status = 'queued'
            ORDER BY created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED)
         RETURNING {JOB_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, ScanJob>(&sql)
        .fetch_optional(pool)
        .await?)
}

/// Marks jobs left `running` by a crashed poller. They cannot be resumed safely
/// because their probes were not recorded, so the user should start a new scan.
pub async fn mark_running_interrupted(pool: &PgPool) -> Result<u64, ScanError> {
    let r = sqlx::query(
        "UPDATE scan_jobs SET status = 'interrupted', finished_at = now(),
            error = 'poller restarted during scan'
         WHERE status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// One probed responder. Non-responders are not stored (see SCANNING.md section 8).
#[derive(Debug, Clone)]
pub struct FoundDevice {
    pub ip: String,
    pub sys_object_id: Option<String>,
    pub sys_descr: Option<String>,
    pub sys_name: Option<String>,
    pub vendor: Option<String>,
    pub profile: Option<String>,
    pub already_monitored: bool,
}

/// Saves a batch of results and advances progress in one transaction. Returns whether
/// the user has asked to cancel, so the poller can stop between batches.
pub async fn record_progress(
    pool: &PgPool,
    job_id: i64,
    addresses_done_delta: i32,
    found: &[FoundDevice],
) -> Result<bool, ScanError> {
    let mut tx = pool.begin().await?;
    for f in found {
        sqlx::query(
            "INSERT INTO scan_results
               (scan_job_id, ip, sys_object_id, sys_descr, sys_name, vendor, profile, already_monitored)
             VALUES ($1, $2::inet, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (scan_job_id, ip) DO NOTHING",
        )
        .bind(job_id)
        .bind(&f.ip)
        .bind(&f.sys_object_id)
        .bind(&f.sys_descr)
        .bind(&f.sys_name)
        .bind(&f.vendor)
        .bind(&f.profile)
        .bind(f.already_monitored)
        .execute(&mut *tx)
        .await?;
    }
    let cancel: bool = sqlx::query_scalar(
        "UPDATE scan_jobs SET addresses_done = addresses_done + $2,
            found_count = found_count + $3
         WHERE id = $1 RETURNING cancel_requested",
    )
    .bind(job_id)
    .bind(addresses_done_delta)
    .bind(found.len() as i32)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(cancel)
}

pub async fn finish_job(
    pool: &PgPool,
    job_id: i64,
    status: &str,
    error: Option<&str>,
) -> Result<(), ScanError> {
    sqlx::query(
        "UPDATE scan_jobs SET status = CASE WHEN $2 = 'done' AND cancel_requested
               THEN 'cancelled'::scan_status ELSE $2::scan_status END,
            error = $3, finished_at = now()
         WHERE id = $1",
    )
    .bind(job_id)
    .bind(status)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Sets the cancel flag. A queued job is cancelled immediately; a running job stops at the next batch.
pub async fn request_cancel(pool: &PgPool, job_id: i64) -> Result<ScanJob, ScanError> {
    let mut tx = pool.begin().await?;
    let sql = format!("SELECT {JOB_COLUMNS} FROM scan_jobs WHERE id = $1 FOR UPDATE");
    let job = sqlx::query_as::<_, ScanJob>(&sql)
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ScanError::NotFound)?;
    match job.status.as_str() {
        "queued" => {
            sqlx::query("UPDATE scan_jobs SET status = 'cancelled', finished_at = now(), cancel_requested = TRUE WHERE id = $1")
                .bind(job_id)
                .execute(&mut *tx)
                .await?;
        }
        "running" => {
            sqlx::query("UPDATE scan_jobs SET cancel_requested = TRUE WHERE id = $1")
                .bind(job_id)
                .execute(&mut *tx)
                .await?;
        }
        _ => {
            return Err(ScanError::Conflict(format!(
                "scan is already {}",
                job.status
            )))
        }
    }
    tx.commit().await?;
    get_job(pool, job_id).await
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ScanResult {
    pub id: i64,
    pub ip: String,
    pub sys_object_id: Option<String>,
    pub sys_descr: Option<String>,
    pub sys_name: Option<String>,
    pub vendor: Option<String>,
    pub profile: Option<String>,
    pub already_monitored: bool,
    pub decision: String,
    pub device_id: Option<i64>,
}

pub async fn list_results(pool: &PgPool, job_id: i64) -> Result<Vec<ScanResult>, ScanError> {
    Ok(sqlx::query_as::<_, ScanResult>(
        "SELECT r.id, host(r.ip) AS ip, r.sys_object_id, r.sys_descr, r.sys_name, r.vendor, r.profile,
                (r.already_monitored OR EXISTS (SELECT 1 FROM devices d WHERE d.mgmt_ip = r.ip)) AS already_monitored,
                r.decision::text AS decision, r.device_id
         FROM scan_results r WHERE r.scan_job_id = $1 ORDER BY r.ip",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?)
}

/// Creates a device from a scan result and copies the credential profile's sealed secrets.
/// Runs in one transaction, so a device never exists without credentials or vice versa.
/// Returns the new device id. Name collisions get a numeric suffix.
pub async fn approve_result(pool: &PgPool, result_id: i64) -> Result<i64, ScanError> {
    let mut tx = pool.begin().await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        ip: String,
        sys_name: Option<String>,
        vendor: Option<String>,
        profile: Option<String>,
        decision: String,
        already_monitored: bool,
        credential_profile_id: i64,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT host(r.ip) AS ip, r.sys_name, r.vendor, r.profile, r.decision::text AS decision,
                r.already_monitored, j.credential_profile_id
         FROM scan_results r JOIN scan_jobs j ON j.id = r.scan_job_id
         WHERE r.id = $1 FOR UPDATE OF r",
    )
    .bind(result_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ScanError::NotFound)?;

    if row.decision != "pending" {
        return Err(ScanError::Conflict(format!(
            "result already {}",
            row.decision
        )));
    }
    if row.already_monitored {
        return Err(ScanError::Conflict("device is already monitored".into()));
    }
    let profile = row
        .profile
        .clone()
        .ok_or_else(|| ScanError::Conflict("device type not recognised (no_profile)".into()))?;

    let base_name = row
        .sys_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| row.ip.clone());
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&base_name)
        .execute(&mut *tx)
        .await?;
    let name = unique_device_name(&mut tx, &base_name).await?;
    let vendor = row.vendor.clone().unwrap_or_else(|| "unknown".into());

    let device_id: i64 = sqlx::query_scalar(
        "INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version)
         SELECT $1, $2::inet, $3, $4, p.snmp_version FROM credential_profiles p WHERE p.id = $5
         RETURNING id",
    )
    .bind(&name)
    .bind(&row.ip)
    .bind(&vendor)
    .bind(&profile)
    .bind(row.credential_profile_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ScanError::Conflict("device name or management IP already exists".into())
        }
        _ => ScanError::Db(e),
    })?;

    sqlx::query(
        "INSERT INTO credentials
           (device_id, snmp_v2c_community_enc, snmp_v3_username, snmp_v3_auth_proto,
            snmp_v3_auth_secret_enc, snmp_v3_priv_proto, snmp_v3_priv_secret_enc)
         SELECT $1, p.snmp_v2c_community_enc, p.snmp_v3_username, p.snmp_v3_auth_proto,
                p.snmp_v3_auth_secret_enc, p.snmp_v3_priv_proto, p.snmp_v3_priv_secret_enc
         FROM credential_profiles p WHERE p.id = $2",
    )
    .bind(device_id)
    .bind(row.credential_profile_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE scan_results SET decision = 'approved', device_id = $2 WHERE id = $1")
        .bind(result_id)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(device_id)
}

/// Returns `base` if unused, otherwise `base-2`, `base-3`, ...
async fn unique_device_name(
    tx: &mut Transaction<'_, Postgres>,
    base: &str,
) -> Result<String, ScanError> {
    let mut candidate = base.to_string();
    let mut n = 2;
    loop {
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devices WHERE name = $1)")
                .bind(&candidate)
                .fetch_one(&mut **tx)
                .await?;
        if !taken {
            return Ok(candidate);
        }
        candidate = format!("{base}-{n}");
        n += 1;
    }
}

/// Loads and decrypts a credential profile for probing. Secrets stay in memory only.
pub async fn load_profile_credentials(
    pool: &PgPool,
    key: &netwatch_crypto::SecretKey,
    profile_id: i64,
) -> Result<netwatch_core::model::SnmpCredentials, ScanError> {
    use crate::credentials::{decrypt_row, CredRow};

    #[derive(sqlx::FromRow)]
    struct Sql {
        snmp_v2c_community_enc: Option<String>,
        snmp_v3_username: Option<String>,
        snmp_v3_auth_proto: Option<String>,
        snmp_v3_auth_secret_enc: Option<String>,
        snmp_v3_priv_proto: Option<String>,
        snmp_v3_priv_secret_enc: Option<String>,
    }
    let r = sqlx::query_as::<_, Sql>(
        "SELECT snmp_v2c_community_enc, snmp_v3_username, snmp_v3_auth_proto,
                snmp_v3_auth_secret_enc, snmp_v3_priv_proto, snmp_v3_priv_secret_enc
         FROM credential_profiles WHERE id = $1",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ScanError::NotFound)?;

    decrypt_row(
        CredRow {
            snmp_v2c_community_enc: r.snmp_v2c_community_enc,
            snmp_v3_username: r.snmp_v3_username,
            snmp_v3_auth_proto: r.snmp_v3_auth_proto,
            snmp_v3_auth_secret_enc: r.snmp_v3_auth_secret_enc,
            snmp_v3_priv_proto: r.snmp_v3_priv_proto,
            snmp_v3_priv_secret_enc: r.snmp_v3_priv_secret_enc,
        },
        key,
    )
    .map_err(|e| ScanError::Credentials(e.to_string()))
}

/// Management IPs already in `devices` that fall inside `cidr`, so results can be flagged.
pub async fn monitored_ips_in(
    pool: &PgPool,
    cidr: &str,
) -> Result<std::collections::HashSet<String>, ScanError> {
    let ips: Vec<String> =
        sqlx::query_scalar("SELECT host(mgmt_ip) FROM devices WHERE mgmt_ip <<= $1::cidr")
            .bind(cidr)
            .fetch_all(pool)
            .await?;
    Ok(ips.into_iter().collect())
}

pub async fn reject_result(pool: &PgPool, result_id: i64) -> Result<(), ScanError> {
    let r = sqlx::query(
        "UPDATE scan_results SET decision = 'rejected'
         WHERE id = $1 AND decision = 'pending'",
    )
    .bind(result_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM scan_results WHERE id = $1)")
                .bind(result_id)
                .fetch_one(pool)
                .await?;
        return Err(if exists {
            ScanError::Conflict("result is not pending".into())
        } else {
            ScanError::NotFound
        });
    }
    Ok(())
}
