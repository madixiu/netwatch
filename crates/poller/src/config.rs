//! Poller configuration, read from environment.
//!
//! Required:
//!   DATABASE_URL       — Postgres connection string
//!   NETWATCH_KEY_FILE  — path to the 0600 key file (hex), OR
//!   NETWATCH_KEY_HEX   — 64-char hex string (dev convenience; visible in
//!                        /proc/<pid>/environ, so not for prod)
//!
//! Fails fast on invalid key or missing env, before any DB work.

use std::path::PathBuf;

use netwatch_crypto::{KeyError, SecretKey};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing env var: {0}")]
    MissingEnv(&'static str),
    #[error("invalid key: {0}")]
    Key(#[from] KeyError),
    #[error("invalid NETWATCH_SCAN_ALLOW: {0}")]
    BadAllowlist(String),
}

pub struct PollerConfig {
    pub db_url: String,
    pub secret_key: SecretKey,
    /// Ranges a scan may target (NETWATCH_SCAN_ALLOW). Empty means scanning is disabled.
    pub scan_allow: Vec<netwatch_core::scan::Cidr4>,
}

impl PollerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let db_url =
            std::env::var("DATABASE_URL").map_err(|_| ConfigError::MissingEnv("DATABASE_URL"))?;
        let secret_key = load_secret_key()?;
        let scan_allow = match std::env::var("NETWATCH_SCAN_ALLOW") {
            Ok(s) => netwatch_core::scan::parse_allowlist(&s)
                .map_err(|e| ConfigError::BadAllowlist(e.to_string()))?,
            Err(std::env::VarError::NotPresent) => Vec::new(),
            Err(error) => return Err(ConfigError::BadAllowlist(error.to_string())),
        };
        Ok(Self {
            db_url,
            secret_key,
            scan_allow,
        })
    }
}

fn load_secret_key() -> Result<SecretKey, ConfigError> {
    if let Ok(path) = std::env::var("NETWATCH_KEY_FILE") {
        return Ok(SecretKey::from_file(&PathBuf::from(path))?);
    }
    if let Ok(hex) = std::env::var("NETWATCH_KEY_HEX") {
        return Ok(SecretKey::from_hex(&hex)?);
    }
    Err(ConfigError::MissingEnv(
        "NETWATCH_KEY_FILE or NETWATCH_KEY_HEX",
    ))
}
