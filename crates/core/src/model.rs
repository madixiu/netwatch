//! Shared model types used by db, snmp, poller, and profiles.

use chrono::{DateTime, Utc};
use zeroize::Zeroizing;

use crate::counter::CounterWidth;

pub type DeviceId = i64;
pub type MetricId = i32;
pub type InterfaceId = i64;

// --- Sample / metric rows ---

#[derive(Debug, Clone, PartialEq)]
pub struct SampleRow {
    pub device_id: DeviceId,
    pub metric_id: MetricId,
    pub interface_id: InterfaceId, // 0 = no interface
    pub ts: DateTime<Utc>,
    pub value: f64,
    pub delta: Option<f64>,
    pub dt_seconds: Option<f64>,
    pub counter_reset: bool,
    pub mapping_verified: bool,
}

impl SampleRow {
    /// Tab-separated line for `COPY ... FROM STDIN` (text format).
    /// Timestamps use RFC 3339 with `+00:00` so Postgres parses them as
    /// `timestamptz` unambiguously regardless of the server's locale.
    pub fn to_copy_line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.device_id,
            self.metric_id,
            self.interface_id,
            self.ts.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            self.value,
            self.delta.map_or_else(|| "\\N".into(), |v| v.to_string()),
            self.dt_seconds
                .map_or_else(|| "\\N".into(), |v| v.to_string()),
            self.counter_reset,
            self.mapping_verified,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricDef {
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: MetricKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
}

// --- Devices / interfaces ---

#[derive(Debug, Clone)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub mgmt_ip: String,
    pub poll_interval_secs: u64,
    pub profile: String,
    pub snmp_version: i16, // 2 or 3
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct InterfaceRef {
    pub if_index: u32,
    pub name: String,
    pub speed_bps: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MetricValue {
    pub name: &'static str,
    pub interface_index: Option<u32>,
    pub value: MetricReading,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricReading {
    Gauge(f64),
    Counter { value: u64, width: CounterWidth },
}

// --- SNMP credentials ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProtocol {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl AuthProtocol {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "md5" => Some(Self::Md5),
            "sha" | "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivProtocol {
    Des,
    Aes128,
    Aes192,
    Aes256,
}

impl PrivProtocol {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "des" => Some(Self::Des),
            "aes" | "aes128" => Some(Self::Aes128),
            "aes192" => Some(Self::Aes192),
            "aes256" => Some(Self::Aes256),
            _ => None,
        }
    }
}

/// Decrypted SNMP credentials. Secrets are `Zeroizing` so they are scrubbed
/// on drop. `Debug` is hand-rolled to redact secrets — never log the raw
/// values via `{:?}` on the constituent fields.
#[derive(Clone)]
pub enum SnmpCredentials {
    V2c {
        community: Zeroizing<String>,
    },
    V3 {
        username: String,
        auth_protocol: AuthProtocol,
        auth_secret: Zeroizing<Vec<u8>>,
        priv_protocol: PrivProtocol,
        priv_secret: Zeroizing<Vec<u8>>,
    },
}

impl std::fmt::Debug for SnmpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnmpCredentials::V2c { .. } => f.write_str("SnmpCredentials::V2c(***)"),
            SnmpCredentials::V3 {
                username,
                auth_protocol,
                priv_protocol,
                ..
            } => f
                .debug_struct("SnmpCredentials::V3")
                .field("username", username)
                .field("auth_protocol", auth_protocol)
                .field("priv_protocol", priv_protocol)
                .field("auth_secret", &"***")
                .field("priv_secret", &"***")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn copy_line_is_tab_separated_and_newline_terminated() {
        let row = SampleRow {
            device_id: 1,
            metric_id: 2,
            interface_id: 3,
            ts: Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap(),
            value: 1.5,
            delta: None,
            dt_seconds: None,
            counter_reset: false,
            mapping_verified: true,
        };
        let line = row.to_copy_line();
        assert!(line.ends_with('\n'));
        let fields: Vec<&str> = line.trim_end_matches('\n').split('\t').collect();
        assert_eq!(fields.len(), 9);
        assert_eq!(fields[4], "1.5");
        assert_eq!(&fields[5..], ["\\N", "\\N", "false", "true"]);
    }

    #[test]
    fn auth_protocol_parse() {
        assert_eq!(AuthProtocol::parse("sha1"), Some(AuthProtocol::Sha1));
        assert_eq!(AuthProtocol::parse("SHA"), Some(AuthProtocol::Sha1));
        assert_eq!(AuthProtocol::parse("sha256"), Some(AuthProtocol::Sha256));
        assert_eq!(AuthProtocol::parse("sha-256"), None);
    }

    #[test]
    fn priv_protocol_parse() {
        assert_eq!(PrivProtocol::parse("AES"), Some(PrivProtocol::Aes128));
        assert_eq!(PrivProtocol::parse("aes256"), Some(PrivProtocol::Aes256));
        assert_eq!(PrivProtocol::parse("3des"), None);
    }

    #[test]
    fn snmp_credentials_debug_redacts_secrets() {
        let c = SnmpCredentials::V2c {
            community: Zeroizing::new("public".into()),
        };
        let s = format!("{c:?}");
        assert!(!s.contains("public"), "Debug must not leak community: {s}");
    }
}
