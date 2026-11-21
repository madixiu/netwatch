//! SNMP value types, decoupled from any particular SNMP crate.

use crate::oid::Oid;

#[derive(Debug, Clone, PartialEq)]
pub enum SnmpValue {
    Integer(i64),
    OctetString(Vec<u8>),
    ObjectIdentifier(Oid),
    IpAddress([u8; 4]),
    Counter32(u32),
    Gauge32(u32),
    TimeTicks(u32),
    Counter64(u64),
    Opaque(Vec<u8>),
}

impl SnmpValue {
    /// Best-effort numeric extraction. Used by counter math and gauge reads.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            SnmpValue::Counter32(v) | SnmpValue::Gauge32(v) | SnmpValue::TimeTicks(v) => {
                Some(*v as u64)
            }
            SnmpValue::Counter64(v) => Some(*v),
            SnmpValue::Integer(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }

    /// True for the counter kinds (32 and 64), which need wrap handling.
    pub fn is_counter(&self) -> bool {
        matches!(self, SnmpValue::Counter32(_) | SnmpValue::Counter64(_))
    }

    /// Counter width derived from the value variant itself. This is how we
    /// avoid the profile having to declare it in a side table.
    pub fn counter_width(&self) -> Option<crate::counter::CounterWidth> {
        use crate::counter::CounterWidth::*;
        match self {
            SnmpValue::Counter32(_) => Some(Bits32),
            SnmpValue::Counter64(_) => Some(Bits64),
            _ => None,
        }
    }

    /// Lossy string for sysDescr / ifDescr.
    pub fn as_str_lossy(&self) -> Option<std::borrow::Cow<'_, str>> {
        match self {
            SnmpValue::OctetString(b) => Some(String::from_utf8_lossy(b)),
            _ => None,
        }
    }
}