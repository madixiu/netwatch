//! Conversion between our types and `async-snmp`'s. Everything crate-specific
//! lives here, so a future `net-snmp` shell-out only touches this file plus
//! `session.rs`.

use async_snmp::oid::Oid as AsnOid;
use async_snmp::Value as AsnValue;
use netwatch_core::{oid::Oid, snmp_value::SnmpValue};

use crate::SnmpError;

pub fn to_async_snmp_oid(o: &Oid) -> AsnOid {
    AsnOid::from(o.0.as_slice())
}

pub fn from_async_snmp_oid(o: &AsnOid) -> Oid {
    Oid::new(o.arcs().to_vec())
}

pub fn from_async_snmp_value(v: AsnValue) -> Result<SnmpValue, SnmpError> {
    Ok(match v {
        AsnValue::Integer(i) => SnmpValue::Integer(i64::from(i)),
        AsnValue::OctetString(b) => SnmpValue::OctetString(b.to_vec()),
        AsnValue::ObjectIdentifier(o) => SnmpValue::ObjectIdentifier(from_async_snmp_oid(&o)),
        AsnValue::IpAddress(ip) => SnmpValue::IpAddress(ip),
        AsnValue::Counter32(v) => SnmpValue::Counter32(v),
        AsnValue::Gauge32(v) => SnmpValue::Gauge32(v),
        AsnValue::TimeTicks(v) => SnmpValue::TimeTicks(v),
        AsnValue::Counter64(v) => SnmpValue::Counter64(v),
        AsnValue::Opaque(b) => SnmpValue::Opaque(b.to_vec()),
        _ => {
            return Err(SnmpError::UnexpectedType(format!(
                "unsupported value: {v:?}"
            )))
        }
    })
}
