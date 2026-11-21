//! Credential encryption at rest.
//!
//! Format: `v1:` || base64( nonce[12] || ciphertext+tag )
//!
//! The `v1:` prefix exists so we can rotate the algorithm or key without a
//! flag day. Every new format gets a new prefix; the opener dispatches on it.
//!
//! Associated data (AAD) binds a ciphertext to its purpose. A v2c community
//! ciphertext sealed with AAD `credential:v2c` will not open under
//! `credential:v3:auth` even with the right key. This is what stops
//! ciphertext-swapping attacks between credential fields.

pub mod password;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
const VERSION: &str = "v1";

/// A 256-bit key that zeroizes on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretKey([u8; KEY_LEN]);

impl SecretKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Parse a 64-char hex string (what the file loader writes).
    pub fn from_hex(s: &str) -> Result<Self, KeyError> {
        let s = s.trim();
        if s.len() != KEY_LEN * 2 {
            return Err(KeyError::BadLength);
        }
        let mut out = [0u8; KEY_LEN];
        hex::decode_to_slice(s, &mut out).map_err(|_| KeyError::BadHex)?;
        Ok(Self(out))
    }

    /// Load from a file. On Unix, refuses to load if permissions allow
    /// group/other access — the runbook says the key must be 0600.
    pub fn from_file(path: &std::path::Path) -> Result<Self, KeyError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path).map_err(KeyError::Io)?;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(KeyError::InsecurePermissions(mode));
            }
        }
        let raw = std::fs::read_to_string(path).map_err(KeyError::Io)?;
        Self::from_hex(&raw)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(***)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("key must be {KEY_LEN} bytes ({} hex chars)", KEY_LEN * 2)]
    BadLength,
    #[error("key is not valid hex")]
    BadHex,
    #[error("key file has insecure permissions: {0:o} (expected 0600)")]
    InsecurePermissions(u32),
    #[error("io error reading key: {0}")]
    Io(#[from] std::io::Error),
}

/// What a ciphertext is allowed to be used for. Encoded into the AAD.
#[derive(Debug, Clone, Copy)]
pub enum Purpose {
    SnmpV2cCommunity,
    SnmpV3AuthSecret,
    SnmpV3PrivSecret,
}

impl Purpose {
    fn aad(self) -> &'static [u8] {
        match self {
            Purpose::SnmpV2cCommunity => b"credential:v2c",
            Purpose::SnmpV3AuthSecret => b"credential:v3:auth",
            Purpose::SnmpV3PrivSecret => b"credential:v3:priv",
        }
    }
}

/// Seal plaintext into a `v1:<base64>` string bound to `purpose`.
pub fn seal(key: &SecretKey, purpose: Purpose, plaintext: &[u8]) -> String {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // 96-bit, fresh per seal
    let payload = Payload { msg: plaintext, aad: purpose.aad() };
    let ct = cipher
        .encrypt(&nonce, payload)
        .expect("AES-GCM encryption cannot fail with a 96-bit nonce");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ct);
    format!("{VERSION}:{}", B64.encode(out))
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("ciphertext has no version prefix")]
    NoVersion,
    #[error("unsupported ciphertext version: {0}")]
    UnknownVersion(String),
    #[error("ciphertext is not valid base64")]
    BadBase64,
    #[error("ciphertext is too short")]
    TooShort,
    #[error("authentication failed (wrong key, wrong purpose, or tampered)")]
    AuthFailed,
}

/// Open a `v1:<base64>` ciphertext bound to `purpose`.
pub fn open(key: &SecretKey, purpose: Purpose, sealed: &str) -> Result<Vec<u8>, OpenError> {
    let (ver, b64) = sealed.split_once(':').ok_or(OpenError::NoVersion)?;
    if ver != VERSION {
        return Err(OpenError::UnknownVersion(ver.to_string()));
    }
    let raw = B64.decode(b64).map_err(|_| OpenError::BadBase64)?;
    if raw.len() < NONCE_LEN + 16 {
        return Err(OpenError::TooShort);
    }
    let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
    let nonce = Nonce::from_slice(nonce_bytes);
    let payload = Payload { msg: ct, aad: purpose.aad() };
    cipher.decrypt(nonce, payload).map_err(|_| OpenError::AuthFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SecretKey {
        SecretKey::from_bytes([7u8; KEY_LEN])
    }

    #[test]
    fn roundtrip() {
        let sealed = seal(&key(), Purpose::SnmpV2cCommunity, b"public");
        let opened = open(&key(), Purpose::SnmpV2cCommunity, &sealed).unwrap();
        assert_eq!(opened, b"public");
    }

    #[test]
    fn nonce_is_fresh_per_seal() {
        let k = key();
        let a = seal(&k, Purpose::SnmpV2cCommunity, b"same");
        let b = seal(&k, Purpose::SnmpV2cCommunity, b"same");
        assert_ne!(a, b, "nonce reuse would be catastrophic for GCM");
    }

    #[test]
    fn wrong_key_fails() {
        let sealed = seal(&key(), Purpose::SnmpV2cCommunity, b"public");
        let other = SecretKey::from_bytes([8u8; KEY_LEN]);
        assert!(matches!(
            open(&other, Purpose::SnmpV2cCommunity, &sealed),
            Err(OpenError::AuthFailed)
        ));
    }

    #[test]
    fn wrong_purpose_fails() {
        // Same key, same ciphertext, different AAD → must fail.
        let sealed = seal(&key(), Purpose::SnmpV2cCommunity, b"secret");
        assert!(matches!(
            open(&key(), Purpose::SnmpV3AuthSecret, &sealed),
            Err(OpenError::AuthFailed)
        ));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut sealed = seal(&key(), Purpose::SnmpV2cCommunity, b"public");
        // Flip the last base64 character — changes the tag.
        let last = sealed.pop().unwrap();
        let swapped = if last == 'A' { 'B' } else { 'A' };
        sealed.push(swapped);
        assert!(matches!(
            open(&key(), Purpose::SnmpV2cCommunity, &sealed),
            Err(OpenError::AuthFailed) | Err(OpenError::BadBase64)
        ));
    }

    #[test]
    fn bad_version_fails() {
        let sealed = seal(&key(), Purpose::SnmpV2cCommunity, b"public");
        let tampered = sealed.replacen("v1:", "v9:", 1);
        assert!(matches!(
            open(&key(), Purpose::SnmpV2cCommunity, &tampered),
            Err(OpenError::UnknownVersion(_))
        ));
    }

    #[test]
    fn hex_roundtrip() {
        let k = key();
        let parsed = SecretKey::from_hex(&k.to_hex()).unwrap();
        let sealed = seal(&parsed, Purpose::SnmpV2cCommunity, b"x");
        assert_eq!(open(&k, Purpose::SnmpV2cCommunity, &sealed).unwrap(), b"x");
    }

    #[test]
    fn bad_hex_length() {
        assert!(matches!(SecretKey::from_hex("abcd"), Err(KeyError::BadLength)));
    }
}