//! Password hashing for local accounts.

use std::num::NonZeroU32;

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};

const ITERATIONS: u32 = 600_000;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("could not generate a password salt")]
    Random,
    #[error("invalid stored password hash")]
    InvalidHash,
}

pub fn hash(password: &[u8]) -> Result<String, PasswordError> {
    let mut salt = [0; SALT_LEN];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| PasswordError::Random)?;
    let mut output = [0; HASH_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).expect("nonzero iterations"),
        &salt,
        password,
        &mut output,
    );
    Ok(format!(
        "pbkdf2-sha256${ITERATIONS}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(output)
    ))
}

pub fn verify(password: &[u8], encoded: &str) -> Result<bool, PasswordError> {
    let mut parts = encoded.split('$');
    let algorithm = parts.next().ok_or(PasswordError::InvalidHash)?;
    let iterations = parts.next().ok_or(PasswordError::InvalidHash)?;
    let salt = STANDARD_NO_PAD
        .decode(parts.next().ok_or(PasswordError::InvalidHash)?)
        .map_err(|_| PasswordError::InvalidHash)?;
    let expected = STANDARD_NO_PAD
        .decode(parts.next().ok_or(PasswordError::InvalidHash)?)
        .map_err(|_| PasswordError::InvalidHash)?;
    if algorithm != "pbkdf2-sha256"
        || iterations != ITERATIONS.to_string()
        || salt.len() != SALT_LEN
        || expected.len() != HASH_LEN
        || parts.next().is_some()
    {
        return Err(PasswordError::InvalidHash);
    }
    Ok(pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).expect("nonzero iterations"),
        &salt,
        password,
        &expected,
    )
    .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salted_hashes_verify_only_the_correct_password() {
        let first = hash(b"correct horse battery staple").unwrap();
        let second = hash(b"correct horse battery staple").unwrap();
        assert_ne!(first, second);
        assert!(verify(b"correct horse battery staple", &first).unwrap());
        assert!(!verify(b"wrong password", &first).unwrap());
    }

    #[test]
    fn malformed_hash_is_rejected() {
        assert!(matches!(
            verify(b"password", "invalid"),
            Err(PasswordError::InvalidHash)
        ));
    }
}
