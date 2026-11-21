//! Constants and the `SnmpError` enum. Kept small; behavior lives in
//! `session.rs` and `client.rs`.

/// Cap on walk length. A device that returns 100k entries in a walk is
/// misbehaving (usually a vendor returning the whole MIB); failing loudly
/// is better than silently truncating.
pub const MAX_WALK_ENTRIES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum SnmpError {
    #[error("client build failed: {0}")]
    Build(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("SNMP authentication failed")]
    Authentication,
    #[error("device returned an empty response")]
    EmptyResponse,
    #[error("OID not found: {0}")]
    NotFound(String),
    #[error("unexpected value type: {0}")]
    UnexpectedType(String),
    #[error("walk exceeded {0} entries")]
    WalkTooLong(usize),
}
