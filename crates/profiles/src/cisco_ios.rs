//! Cisco IOS profile.
//!
//! OIDs:
//!   sysDescr.0             1.3.6.1.2.1.1.1.0
//!   sysObjectID.0          1.3.6.1.2.1.1.2.0
//!   ifName                 1.3.6.1.2.1.31.1.1.1.1        (walk; string)
//!   ifHCInOctets           1.3.6.1.2.1.31.1.1.1.6        (walk; Counter64)
//!   ifHCOutOctets          1.3.6.1.2.1.31.1.1.1.10       (walk; Counter64)
//!   cpmCPUTotal5minRev     1.3.6.1.4.1.9.9.109.1.1.1.1.8 (walk; Gauge32)
//!
//! Memory uses a single HOST-RESOURCES-MIB RAM storage row when available.

use netwatch_core::model::{InterfaceRef, MetricValue};

use crate::{
    first_row_gauge, memory_used_pct, traffic_metrics, traffic_request, DeviceContext, MetricDef,
    PollRequest, PollResponse, Profile, COMMON_METRICS,
};

const OID_CPU_5MIN: &str = "1.3.6.1.4.1.9.9.109.1.1.1.1.8";

pub struct CiscoIos;

impl Profile for CiscoIos {
    fn name(&self) -> &'static str {
        "cisco_ios"
    }

    fn detect(&self, ctx: &DeviceContext<'_>) -> bool {
        // sysObjectID under 1.3.6.1.4.1.9.1.* (Cisco IOS). NX-OS uses
        // 1.3.6.1.4.1.9.12.* and has its own profile. Prefer sysObjectID
        // over sysDescr — descriptions get customized by operators.
        ctx.sys_object_id.starts_with("1.3.6.1.4.1.9.1.")
            || ctx.sys_descr.starts_with("Cisco IOS Software")
    }

    fn metrics(&self) -> &'static [MetricDef] {
        COMMON_METRICS
    }

    fn request(&self, _interfaces: &[InterfaceRef]) -> PollRequest {
        // Walk whole columns; the device filters by interface via ifIndex,
        // not by us sending per-ifIndex GETs. One PDU round trip per column.
        traffic_request(&[OID_CPU_5MIN])
    }

    fn parse(&self, resp: &PollResponse) -> Vec<MetricValue> {
        let mut out = traffic_metrics(resp);
        // CPU table is keyed by cpmCPUTotalIndex. For phase 1 we take the
        // first row, which on most IOS devices is the "overall" CPU. If
        // this proves wrong on a device, refine the profile.
        out.extend(first_row_gauge(resp, OID_CPU_5MIN, "cpu_5min_pct"));
        out.extend(memory_used_pct(resp));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_core::{oid::Oid, snmp_value::SnmpValue};

    fn resp(pairs: Vec<(&str, SnmpValue)>) -> PollResponse {
        PollResponse {
            values: pairs
                .into_iter()
                .map(|(o, v)| (Oid::parse(o).unwrap(), v))
                .collect(),
        }
    }

    #[test]
    fn detect_by_sys_object_id() {
        let p = CiscoIos;
        assert!(p.detect(&DeviceContext {
            sys_object_id: "1.3.6.1.4.1.9.1.1208",
            sys_descr: "",
        }));
        assert!(!p.detect(&DeviceContext {
            sys_object_id: "1.3.6.1.4.1.9.12.3.1.3.1",
            sys_descr: "",
        }));
    }

    #[test]
    fn parse_traffic_by_ifindex() {
        let r = resp(vec![
            ("1.3.6.1.2.1.31.1.1.1.6.1", SnmpValue::Counter64(1_000_000)),
            ("1.3.6.1.2.1.31.1.1.1.6.2", SnmpValue::Counter64(2_000_000)),
            ("1.3.6.1.2.1.31.1.1.1.10.1", SnmpValue::Counter64(500_000)),
            ("1.3.6.1.2.1.31.1.1.1.10.2", SnmpValue::Counter64(700_000)),
        ]);
        let mut vals = CiscoIos.parse(&r);
        vals.sort_by_key(|v| (v.name, v.interface_index));
        assert_eq!(vals.len(), 4);
        assert_eq!(vals[0].name, "if_hc_in_octets");
        assert_eq!(vals[0].interface_index, Some(1));
        assert_eq!(
            vals[0].value,
            crate::MetricReading::Counter {
                value: 1_000_000,
                width: netwatch_core::counter::CounterWidth::Bits64
            }
        );
        assert_eq!(vals[2].name, "if_hc_out_octets");
        assert_eq!(vals[2].interface_index, Some(1));
    }

    #[test]
    fn parse_cpu_takes_first_row() {
        let r = resp(vec![
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.8.1", SnmpValue::Gauge32(42)),
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.8.2", SnmpValue::Gauge32(11)),
        ]);
        let vals = CiscoIos.parse(&r);
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0].name, "cpu_5min_pct");
        assert_eq!(vals[0].value, crate::MetricReading::Gauge(42.0));
    }

    #[test]
    fn memory_requires_one_valid_ram_row() {
        let r = resp(vec![
            (
                "1.3.6.1.2.1.25.2.3.1.2.7",
                SnmpValue::ObjectIdentifier(Oid::parse("1.3.6.1.2.1.25.2.1.2").unwrap()),
            ),
            ("1.3.6.1.2.1.25.2.3.1.5.7", SnmpValue::Integer(1000)),
            ("1.3.6.1.2.1.25.2.3.1.6.7", SnmpValue::Integer(250)),
        ]);
        assert_eq!(
            CiscoIos.parse(&r)[0].value,
            crate::MetricReading::Gauge(25.0)
        );
        let invalid = resp(vec![
            (
                "1.3.6.1.2.1.25.2.3.1.2.7",
                SnmpValue::ObjectIdentifier(Oid::parse("1.3.6.1.2.1.25.2.1.2").unwrap()),
            ),
            ("1.3.6.1.2.1.25.2.3.1.5.7", SnmpValue::Integer(100)),
            ("1.3.6.1.2.1.25.2.3.1.6.7", SnmpValue::Integer(101)),
        ]);
        assert!(CiscoIos.parse(&invalid).is_empty());
        let multiple = resp(vec![
            (
                "1.3.6.1.2.1.25.2.3.1.2.1",
                SnmpValue::ObjectIdentifier(Oid::parse("1.3.6.1.2.1.25.2.1.2").unwrap()),
            ),
            (
                "1.3.6.1.2.1.25.2.3.1.2.2",
                SnmpValue::ObjectIdentifier(Oid::parse("1.3.6.1.2.1.25.2.1.2").unwrap()),
            ),
        ]);
        assert!(CiscoIos.parse(&multiple).is_empty());
    }

    #[test]
    fn ignores_wrong_types() {
        let r = resp(vec![("1.3.6.1.2.1.31.1.1.1.6.1", SnmpValue::Integer(5))]);
        let vals = CiscoIos.parse(&r);
        assert!(vals.is_empty());
    }

    #[test]
    fn reads_interface_status_errors_and_packet_counters() {
        let r = resp(vec![
            ("1.3.6.1.2.1.2.2.1.8.3", SnmpValue::Integer(2)),
            ("1.3.6.1.2.1.2.2.1.14.3", SnmpValue::Counter32(7)),
            ("1.3.6.1.2.1.31.1.1.1.7.3", SnmpValue::Counter64(42)),
            ("1.3.6.1.2.1.2.2.1.19.3", SnmpValue::Integer(10)),
            ("1.3.6.1.2.1.2.2.1.8.4", SnmpValue::Integer(9)),
        ]);
        let vals = CiscoIos.parse(&r);
        assert_eq!(vals.len(), 3);
        assert!(vals.iter().any(|v| v.name == "if_oper_status"
            && v.interface_index == Some(3)
            && v.value == crate::MetricReading::Gauge(2.0)));
        assert!(vals.iter().any(|v| v.name == "if_in_errors"
            && matches!(
                v.value,
                crate::MetricReading::Counter {
                    value: 7,
                    width: netwatch_core::counter::CounterWidth::Bits32
                }
            )));
        assert!(vals.iter().any(|v| v.name == "if_hc_in_ucast_pkts"
            && matches!(
                v.value,
                crate::MetricReading::Counter {
                    value: 42,
                    width: netwatch_core::counter::CounterWidth::Bits64
                }
            )));
    }
}
