//! Load and decrypt SNMP credentials from the `credentials` table.
//!
//! Decryption lives here because this module owns the column semantics —
//! `snmp_v2c_community_enc` was sealed with `Purpose::SnmpV2cCommunity`,
//! `snmp_v3_auth_secret_enc` with `Purpose::SnmpV3AuthSecret`, and so on.
//! `netwatch-crypto` stays SNMP-agnostic.

use netwatch_core::model::{AuthProtocol, PrivProtocol, SnmpCredentials};
use netwatch_crypto::{open, OpenError, Purpose, SecretKey};
use sqlx::PgPool;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("no credentials row for device {0}")]
    NotFound(i64),
    #[error("missing credential field: {0}")]
    MissingField(&'static str),
    #[error("decryption failed for {field}: {source}")]
    Open {
        field: &'static str,
        #[source]
        source: OpenError,
    },
    #[error("decrypted {field} is not valid UTF-8")]
    NotUtf8 { field: &'static str },
    #[error("unknown {field}: {value:?}")]
    UnknownProto { field: &'static str, value: String },
}

/// Raw row. Public so `decrypt_row` can be unit-tested without a database.
#[derive(Debug, Clone)]
pub struct CredRow {
    pub snmp_v2c_community_enc: Option<String>,
    pub snmp_v3_username: Option<String>,
    pub snmp_v3_auth_proto: Option<String>,
    pub snmp_v3_auth_secret_enc: Option<String>,
    pub snmp_v3_priv_proto: Option<String>,
    pub snmp_v3_priv_secret_enc: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CredRowSql {
    snmp_v2c_community_enc: Option<String>,
    snmp_v3_username: Option<String>,
    snmp_v3_auth_proto: Option<String>,
    snmp_v3_auth_secret_enc: Option<String>,
    snmp_v3_priv_proto: Option<String>,
    snmp_v3_priv_secret_enc: Option<String>,
}

impl From<CredRowSql> for CredRow {
    fn from(r: CredRowSql) -> Self {
        Self {
            snmp_v2c_community_enc: r.snmp_v2c_community_enc,
            snmp_v3_username: r.snmp_v3_username,
            snmp_v3_auth_proto: r.snmp_v3_auth_proto,
            snmp_v3_auth_secret_enc: r.snmp_v3_auth_secret_enc,
            snmp_v3_priv_proto: r.snmp_v3_priv_proto,
            snmp_v3_priv_secret_enc: r.snmp_v3_priv_secret_enc,
        }
    }
}

/// Pure decryption. Testable without Postgres.
pub fn decrypt_row(row: CredRow, key: &SecretKey) -> Result<SnmpCredentials, CredentialError> {
    // v2c: present wins. Only one set of fields is populated by design,
    // but if both were somehow set, v2c takes precedence (safer default:
    // if an admin left a v3 remnant, we'd still authenticate).
    if let Some(enc) = &row.snmp_v2c_community_enc {
        let bytes = open(key, Purpose::SnmpV2cCommunity, enc).map_err(|source| {
            CredentialError::Open { field: "snmp_v2c_community_enc", source }
        })?;
        let community = String::from_utf8(bytes).map_err(|_| CredentialError::NotUtf8 {
            field: "snmp_v2c_community_enc",
        })?;
        return Ok(SnmpCredentials::V2c {
            community: Zeroizing::new(community),
        });
    }

    // v3: all six fields required.
    let username = row
        .snmp_v3_username
        .ok_or(CredentialError::MissingField("snmp_v3_username"))?;
    let auth_proto_s = row
        .snmp_v3_auth_proto
        .ok_or(CredentialError::MissingField("snmp_v3_auth_proto"))?;
    let auth_protocol =
        AuthProtocol::parse(&auth_proto_s).ok_or_else(|| CredentialError::UnknownProto {
            field: "snmp_v3_auth_proto",
            value: auth_proto_s.clone(),
        })?;
    let auth_enc = row
        .snmp_v3_auth_secret_enc
        .ok_or(CredentialError::MissingField("snmp_v3_auth_secret_enc"))?;
    let auth_secret = open(key, Purpose::SnmpV3AuthSecret, &auth_enc).map_err(|source| {
        CredentialError::Open { field: "snmp_v3_auth_secret_enc", source }
    })?;

    let priv_proto_s = row
        .snmp_v3_priv_proto
        .ok_or(CredentialError::MissingField("snmp_v3_priv_proto"))?;
    let priv_protocol =
        PrivProtocol::parse(&priv_proto_s).ok_or_else(|| CredentialError::UnknownProto {
            field: "snmp_v3_priv_proto",
            value: priv_proto_s.clone(),
        })?;
    let priv_enc = row
        .snmp_v3_priv_secret_enc
        .ok_or(CredentialError::MissingField("snmp_v3_priv_secret_enc"))?;
    let priv_secret = open(key, Purpose::SnmpV3PrivSecret, &priv_enc).map_err(|source| {
        CredentialError::Open { field: "snmp_v3_priv_secret_enc", source }
    })?;

    Ok(SnmpCredentials::V3 {
        username,
        auth_protocol,
        auth_secret: Zeroizing::new(auth_secret),
        priv_protocol,
        priv_secret: Zeroizing::new(priv_secret),
    })
}

pub async fn load_decrypted(
    pool: &PgPool,
    device_id: i64,
    key: &SecretKey,
) -> Result<SnmpCredentials, CredentialError> {
    let row: Option<CredRowSql> = sqlx::query_as(
        "SELECT snmp_v2c_community_enc,
                snmp_v3_username, snmp_v3_auth_proto, snmp_v3_auth_secret_enc,
                snmp_v3_priv_proto, snmp_v3_priv_secret_enc
         FROM credentials WHERE device_id = $1",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    let row = row.ok_or(CredentialError::NotFound(device_id))?.into();
    decrypt_row(row, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_crypto::seal;

    fn key() -> SecretKey {
        SecretKey::from_bytes([42u8; 32])
    }

    fn v2c_row(community_enc: String) -> CredRow {
        CredRow {
            snmp_v2c_community_enc: Some(community_enc),
            snmp_v3_username: None,
            snmp_v3_auth_proto: None,
            snmp_v3_auth_secret_enc: None,
            snmp_v3_priv_proto: None,
            snmp_v3_priv_secret_enc: None,
        }
    }

    #[test]
    fn v2c_roundtrip() {
        let k = key();
        let row = v2c_row(seal(&k, Purpose::SnmpV2cCommunity, b"secret"));
        match decrypt_row(row, &k).unwrap() {
            SnmpCredentials::V2c { community } => assert_eq!(&**community, "secret"),
            _ => panic!("expected V2c"),
        }
    }

    #[test]
    fn v3_roundtrip() {
        let k = key();
        let row = CredRow {
            snmp_v2c_community_enc: None,
            snmp_v3_username: Some("monitor".into()),
            snmp_v3_auth_proto: Some("sha256".into()),
            snmp_v3_auth_secret_enc: Some(seal(&k, Purpose::SnmpV3AuthSecret, b"authpass")),
            snmp_v3_priv_proto: Some("aes256".into()),
            snmp_v3_priv_secret_enc: Some(seal(&k, Purpose::SnmpV3PrivSecret, b"privpass")),
        };
        match decrypt_row(row, &k).unwrap() {
            SnmpCredentials::V3 {
                username,
                auth_protocol,
                auth_secret,
                priv_protocol,
                priv_secret,
            } => {
                assert_eq!(username, "monitor");
                assert_eq!(auth_protocol, AuthProtocol::Sha256);
                assert_eq!(&**auth_secret, b"authpass");
                assert_eq!(priv_protocol, PrivProtocol::Aes256);
                assert_eq!(&**priv_secret, b"privpass");
            }
            _ => panic!("expected V3"),
        }
    }

    #[test]
    fn wrong_purpose_rejected() {
        let k = key();
        // Seal the auth secret with the *priv* purpose — open() must reject.
        let enc = seal(&k, Purpose::SnmpV3PrivSecret, b"authpass");
        let row = CredRow {
            snmp_v2c_community_enc: None,
            snmp_v3_username: Some("monitor".into()),
            snmp_v3_auth_proto: Some("sha1".into()),
            snmp_v3_auth_secret_enc: Some(enc),
            snmp_v3_priv_proto: Some("des".into()),
            snmp_v3_priv_secret_enc: Some(seal(&k, Purpose::SnmpV3PrivSecret, b"privpass")),
        };
        assert!(matches!(
            decrypt_row(row, &k),
            Err(CredentialError::Open { field: "snmp_v3_auth_secret_enc", .. })
        ));
    }

    #[test]
    fn unknown_proto_rejected() {
        let k = key();
        let row = CredRow {
            snmp_v2c_community_enc: None,
            snmp_v3_username: Some("u".into()),
            snmp_v3_auth_proto: Some("rot13".into()),
            snmp_v3_auth_secret_enc: Some(seal(&k, Purpose::SnmpV3AuthSecret, b"x")),
            snmp_v3_priv_proto: Some("des".into()),
            snmp_v3_priv_secret_enc: Some(seal(&k, Purpose::SnmpV3PrivSecret, b"y")),
        };
        assert!(matches!(
            decrypt_row(row, &k),
            Err(CredentialError::UnknownProto { field: "snmp_v3_auth_proto", .. })
        ));
    }

    #[test]
    fn missing_field_rejected() {
        let k = key();
        let row = CredRow {
            snmp_v2c_community_enc: None,
            snmp_v3_username: Some("u".into()),
            snmp_v3_auth_proto: Some("sha1".into()),
            snmp_v3_auth_secret_enc: None, // missing
            snmp_v3_priv_proto: Some("des".into()),
            snmp_v3_priv_secret_enc: Some(seal(&k, Purpose::SnmpV3PrivSecret, b"y")),
        };
        assert!(matches!(
            decrypt_row(row, &k),
            Err(CredentialError::MissingField("snmp_v3_auth_secret_enc"))
        ));
    }
}