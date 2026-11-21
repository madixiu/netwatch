//! Cisco NX-OS profile (Nexus switches).
//!
//! Traffic and CPU use the same standard/CISCO-PROCESS OIDs as IOS. Memory
//! uses a HOST-RESOURCES-MIB RAM row when the device exposes one.

use netwatch_core::model::{InterfaceRef, MetricValue};

use crate::{
    first_row_gauge, memory_used_pct, traffic_metrics, traffic_request, DeviceContext, MetricDef,
    PollRequest, PollResponse, Profile, COMMON_METRICS,
};

const OID_CPU_5MIN: &str = "1.3.6.1.4.1.9.9.109.1.1.1.1.8";

pub struct CiscoNxos;

impl Profile for CiscoNxos {
    fn name(&self) -> &'static str {
        "cisco_nxos"
    }

    fn detect(&self, ctx: &DeviceContext<'_>) -> bool {
        ctx.sys_object_id.starts_with("1.3.6.1.4.1.9.12.")
    }

    fn metrics(&self) -> &'static [MetricDef] {
        COMMON_METRICS
    }

    fn request(&self, _interfaces: &[InterfaceRef]) -> PollRequest {
        traffic_request(&[OID_CPU_5MIN])
    }

    fn parse(&self, resp: &PollResponse) -> Vec<MetricValue> {
        let mut out = traffic_metrics(resp);
        out.extend(first_row_gauge(resp, OID_CPU_5MIN, "cpu_5min_pct"));
        out.extend(memory_used_pct(resp));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_core::oid::Oid;

    #[test]
    fn detect_nxos_by_sys_object_id() {
        let p = CiscoNxos;
        assert!(p.detect(&DeviceContext {
            sys_object_id: "1.3.6.1.4.1.9.12.3.1.3.1",
            sys_descr: ""
        }));
        assert!(!p.detect(&DeviceContext {
            sys_object_id: "1.3.6.1.4.1.9.1.1208",
            sys_descr: ""
        }));
    }

    #[test]
    fn request_includes_traffic_and_cpu_columns() {
        let req = CiscoNxos.request(&[]);
        assert!(req
            .walks
            .contains(&Oid::parse("1.3.6.1.2.1.31.1.1.1.6").unwrap()));
        assert!(req
            .walks
            .contains(&Oid::parse("1.3.6.1.2.1.31.1.1.1.10").unwrap()));
        assert!(req.walks.contains(&Oid::parse(OID_CPU_5MIN).unwrap()));
    }
}
