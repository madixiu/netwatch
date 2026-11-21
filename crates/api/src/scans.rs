use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use netwatch_core::{
    model::{AuthProtocol, PrivProtocol},
    scan::plan_scan,
};
use netwatch_crypto::{seal, Purpose};
use netwatch_db::scans::{self, NewCredentialProfile, ScanError};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::{auth::User, ApiError, AppState};

#[derive(Deserialize)]
pub struct CreateProfile {
    name: String,
    credentials: Credentials,
}

#[derive(Deserialize)]
#[serde(tag = "version", rename_all = "lowercase")]
enum Credentials {
    V2c {
        community: String,
    },
    V3 {
        username: String,
        auth_protocol: String,
        auth_secret: String,
        priv_protocol: String,
        priv_secret: String,
    },
}

fn invalid(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
}

pub async fn create_profile(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(req): Json<CreateProfile>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    user.require_admin()?;
    let name = req.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(invalid("profile name must contain 1 to 128 bytes"));
    }
    let mut profile = NewCredentialProfile {
        name: name.to_owned(),
        snmp_version: 2,
        v2c_community_enc: None,
        v3_username: None,
        v3_auth_proto: None,
        v3_auth_secret_enc: None,
        v3_priv_proto: None,
        v3_priv_secret_enc: None,
    };
    match req.credentials {
        Credentials::V2c { community } => {
            let community = Zeroizing::new(community);
            if community.is_empty() || community.len() > 255 {
                return Err(invalid("community must contain 1 to 255 bytes"));
            }
            profile.v2c_community_enc = Some(seal(
                &state.key,
                Purpose::SnmpV2cCommunity,
                community.as_bytes(),
            ));
        }
        Credentials::V3 {
            username,
            auth_protocol,
            auth_secret,
            priv_protocol,
            priv_secret,
        } => {
            let auth_secret = Zeroizing::new(auth_secret);
            let priv_secret = Zeroizing::new(priv_secret);
            if username.trim().is_empty()
                || username.len() > 128
                || !(8..=255).contains(&auth_secret.len())
                || !(8..=255).contains(&priv_secret.len())
                || AuthProtocol::parse(&auth_protocol).is_none()
                || PrivProtocol::parse(&priv_protocol).is_none()
            {
                return Err(invalid(
                    "invalid SNMPv3 username, protocol, or secret (secrets require 8 to 255 bytes)",
                ));
            }
            profile.snmp_version = 3;
            profile.v3_username = Some(username);
            profile.v3_auth_proto = Some(auth_protocol);
            profile.v3_priv_proto = Some(priv_protocol);
            profile.v3_auth_secret_enc = Some(seal(
                &state.key,
                Purpose::SnmpV3AuthSecret,
                auth_secret.as_bytes(),
            ));
            profile.v3_priv_secret_enc = Some(seal(
                &state.key,
                Purpose::SnmpV3PrivSecret,
                priv_secret.as_bytes(),
            ));
        }
    }
    let id = scans::create_credential_profile(&state.pool, &profile)
        .await
        .map_err(scan_error)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<scans::CredentialProfileSummary>>, ApiError> {
    Ok(Json(
        scans::list_credential_profiles(&state.pool)
            .await
            .map_err(scan_error)?,
    ))
}

#[derive(Deserialize)]
pub struct CreateScan {
    cidr: String,
    credential_profile_id: i64,
}

pub async fn create_scan(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(req): Json<CreateScan>,
) -> Result<(StatusCode, Json<scans::ScanJob>), ApiError> {
    user.require_admin()?;
    let (_, hosts) = plan_scan(&req.cidr, &state.scan_allow)
        .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM credential_profiles WHERE id = $1)")
            .bind(req.credential_profile_id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(invalid("credential profile not found"));
    }
    let job = scans::create_job(
        &state.pool,
        &req.cidr,
        req.credential_profile_id,
        &user.username,
        hosts.len() as i32,
    )
    .await
    .map_err(scan_error)?;
    tracing::info!(user = %user.username, cidr = %req.cidr, credential_profile_id = req.credential_profile_id, job = job.id, "scan queued");
    Ok((StatusCode::ACCEPTED, Json(job)))
}

pub async fn list_scans(
    State(state): State<AppState>,
) -> Result<Json<Vec<scans::ScanJob>>, ApiError> {
    Ok(Json(
        scans::list_jobs(&state.pool, 100)
            .await
            .map_err(scan_error)?,
    ))
}

pub async fn get_scan(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<scans::ScanJob>, ApiError> {
    Ok(Json(
        scans::get_job(&state.pool, id).await.map_err(scan_error)?,
    ))
}

pub async fn list_results(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<scans::ScanResult>>, ApiError> {
    scans::get_job(&state.pool, id).await.map_err(scan_error)?;
    Ok(Json(
        scans::list_results(&state.pool, id)
            .await
            .map_err(scan_error)?,
    ))
}

pub async fn cancel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<Json<scans::ScanJob>, ApiError> {
    user.require_admin()?;
    Ok(Json(
        scans::request_cancel(&state.pool, id)
            .await
            .map_err(scan_error)?,
    ))
}

pub async fn approve(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    user.require_admin()?;
    let device_id = scans::approve_result(&state.pool, id)
        .await
        .map_err(scan_error)?;
    tracing::info!(user = %user.username, result_id = id, device_id, "scan result approved");
    Ok(Json(json!({ "device_id": device_id })))
}

pub async fn reject(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin()?;
    scans::reject_result(&state.pool, id)
        .await
        .map_err(scan_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn scan_error(error: ScanError) -> ApiError {
    match error {
        ScanError::NotFound => ApiError::new(StatusCode::NOT_FOUND, "not found"),
        ScanError::Conflict(message) => ApiError::new(StatusCode::CONFLICT, message),
        ScanError::Db(sqlx::Error::Database(ref db)) if db.is_unique_violation() => ApiError::new(
            StatusCode::CONFLICT,
            "a record with this value already exists",
        ),
        other => ApiError::internal(other),
    }
}
