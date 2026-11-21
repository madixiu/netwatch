//! Client construction: v2c and v3.
//!
//! Protocol enums live in `netwatch-core` so `db::credentials` and `snmp`
//! share one type. This file is the only place that knows about
//! `async_snmp::auth::AuthProtocol` / `PrivProtocol`.

use std::time::Duration;

use async_snmp::v3::{AuthProtocol, PrivProtocol};
use async_snmp::{Auth, Client, Retry};
use netwatch_core::model::{AuthProtocol as CoreAuth, PrivProtocol as CorePriv};

use crate::SnmpError;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn build_v2c(
    addr: &str,
    community: &str,
    timeout: Duration,
    single_attempt: bool,
) -> Result<Client, SnmpError> {
    let builder = Client::builder(addr, Auth::v2c(community)).timeout(timeout);
    let builder = if single_attempt {
        builder.retry(Retry::none())
    } else {
        builder
    };
    builder
        .connect()
        .await
        .map_err(|e| SnmpError::Build(e.to_string()))
}

pub struct V3Config<'a> {
    pub username: &'a str,
    pub auth_protocol: CoreAuth,
    pub auth_secret: &'a [u8],
    pub priv_protocol: CorePriv,
    pub priv_secret: &'a [u8],
    pub context_name: Option<&'a str>,
}

pub async fn build_v3(
    addr: &str,
    cfg: V3Config<'_>,
    timeout: Duration,
    single_attempt: bool,
) -> Result<Client, SnmpError> {
    let mut usm = Auth::usm(cfg.username)
        .auth(to_asn_auth(cfg.auth_protocol), secret_str(cfg.auth_secret)?)
        .privacy(to_asn_priv(cfg.priv_protocol), secret_str(cfg.priv_secret)?);
    if let Some(ctx) = cfg.context_name {
        usm = usm.context_name(ctx);
    }

    let builder = Client::builder(addr, usm).timeout(timeout);
    let builder = if single_attempt {
        builder.retry(Retry::none())
    } else {
        builder
    };
    builder
        .connect()
        .await
        .map_err(|e| SnmpError::Build(e.to_string()))
}

fn secret_str(bytes: &[u8]) -> Result<String, SnmpError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| SnmpError::Build("SNMPv3 secrets must be valid UTF-8".into()))
}

fn to_asn_auth(p: CoreAuth) -> AuthProtocol {
    match p {
        CoreAuth::Md5 => AuthProtocol::Md5,
        CoreAuth::Sha1 => AuthProtocol::Sha1,
        CoreAuth::Sha256 => AuthProtocol::Sha256,
        CoreAuth::Sha512 => AuthProtocol::Sha512,
    }
}

fn to_asn_priv(p: CorePriv) -> PrivProtocol {
    match p {
        CorePriv::Des => PrivProtocol::Des,
        CorePriv::Aes128 => PrivProtocol::Aes128,
        CorePriv::Aes192 => PrivProtocol::Aes192,
        CorePriv::Aes256 => PrivProtocol::Aes256,
    }
}
