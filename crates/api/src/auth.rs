use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};
use zeroize::Zeroizing;

use crate::{ApiError, AppState};

#[derive(Clone, Debug, sqlx::FromRow, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: String,
}

impl User {
    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.role == "admin" {
            Ok(())
        } else {
            Err(ApiError::new(StatusCode::FORBIDDEN, "admin role required"))
        }
    }
}

pub struct LoginLimiter {
    entries: Mutex<HashMap<String, (Instant, u32)>>,
    hashing: Arc<Semaphore>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            hashing: Arc::new(Semaphore::new(4)),
        }
    }

    async fn allow(&self, ip: IpAddr, username: &str) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        entries.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        if entries.len() >= 8192 {
            return false;
        }
        for key in [format!("ip:{ip}"), format!("user:{username}")] {
            let entry = entries.entry(key).or_insert((now, 0));
            if entry.1 >= 10 {
                return false;
            }
            entry.1 += 1;
        }
        true
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    token: String,
    user: User,
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    if request.username.is_empty() || request.username.len() > 128 || request.password.len() > 1024
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid credentials",
        ));
    }
    if !state
        .login_limiter
        .allow(peer.ip(), &request.username)
        .await
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many login attempts",
        ));
    }
    let user: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, username, role, password_hash FROM local_users WHERE username = $1 AND enabled",
    )
    .bind(&request.username)
    .fetch_optional(&state.pool)
    .await?;
    let password = Zeroizing::new(request.password.into_bytes());
    let stored_hash = user
        .as_ref()
        .map(|u| u.3.clone())
        .unwrap_or_else(|| state.dummy_hash.clone());
    let permit = state
        .login_limiter
        .hashing
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "login temporarily busy"))?;
    let verified = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        netwatch_crypto::password::verify(&password, &stored_hash)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    let Some((id, username, role, _)) = user.filter(|_| verified) else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid credentials",
        ));
    };
    let mut token = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(&mut token[..])
        .map_err(|_| ApiError::internal("random generator failed"))?;
    let hash = digest::digest(&digest::SHA256, &token[..]);
    sqlx::query("DELETE FROM user_sessions WHERE expires_at <= now()")
        .execute(&state.pool)
        .await?;
    sqlx::query(
        "INSERT INTO user_sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + interval '8 hours')",
    ).bind(hash.as_ref()).bind(id).execute(&state.pool).await?;
    Ok(Json(LoginResponse {
        token: hex::encode(&token[..]),
        user: User { id, username, role },
    }))
}

fn token_hash(headers: &axum::http::HeaderMap) -> Option<digest::Digest> {
    let token = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let bytes = hex::decode(token).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    Some(digest::digest(&digest::SHA256, &bytes))
}

pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(hash) = token_hash(request.headers()) else {
        return ApiError::new(StatusCode::UNAUTHORIZED, "authentication required").into_response();
    };
    let user = sqlx::query_as::<_, User>(
        "SELECT u.id, u.username, u.role FROM user_sessions s JOIN local_users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_at > now() AND u.enabled",
    ).bind(hash.as_ref()).fetch_optional(&state.pool).await;
    match user {
        Ok(Some(user)) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        Ok(None) => {
            ApiError::new(StatusCode::UNAUTHORIZED, "authentication required").into_response()
        }
        Err(e) => ApiError::internal(e).into_response(),
    }
}

pub async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, ApiError> {
    let hash = token_hash(&headers)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "authentication required"))?;
    sqlx::query("DELETE FROM user_sessions WHERE token_hash = $1")
        .bind(hash.as_ref())
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limits_login_attempts_by_address_and_username() {
        let limiter = LoginLimiter::new();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        for _ in 0..10 {
            assert!(limiter.allow(ip, "admin").await);
        }
        assert!(!limiter.allow(ip, "different-user").await);
        assert!(!limiter.allow("192.0.2.2".parse().unwrap(), "admin").await);
        assert!(
            limiter
                .allow("192.0.2.3".parse().unwrap(), "another-user")
                .await
        );
    }
}
