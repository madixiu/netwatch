//! MikroTik RouterOS profile.
//!
//! Traffic uses the standard IF-MIB columns. CPU comes from HOST-RESOURCES-MIB
//! hrProcessorLoad, which has one row per core. We report the first core,
//! matching the IOS profile's approach. Memory uses a HOST-RESOURCES-MIB
//! RAM row when the device exposes one.

use netwatch_core::model::{InterfaceRef, MetricValue};

use crate::{
    first_row_gauge, memory_used_pct, traffic_metrics, traffic_request, DeviceContext, MetricDef,
    PollRequest, PollResponse, Profile, COMMON_METRICS,
};

const OID_HR_PROCESSOR_LOAD: &str = "1.3.6.1.2.1.25.3.3.1.2";

pub struct MikroTik;

impl Profile for MikroTik {
    fn name(&self) -> &'static str {
        "mikrotik"
    }

    fn detect(&self, ctx: &DeviceContext<'_>) -> bool {
        ctx.sys_object_id.starts_with("1.3.6.1.4.1.14988.")
    }

    fn metrics(&self) -> &'static [MetricDef] {
        COMMON_METRICS
    }

    fn request(&self, _interfaces: &[InterfaceRef]) -> PollRequest {
        traffic_request(&[OID_HR_PROCESSOR_LOAD])
    }

    fn parse(&self, resp: &PollResponse) -> Vec<MetricValue> {
        let mut out = traffic_metrics(resp);
        out.extend(first_row_gauge(resp, OID_HR_PROCESSOR_LOAD, "cpu_load_pct"));
        out.extend(memory_used_pct(resp));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_by_enterprise_prefix() {
        let p = MikroTik;
        assert!(p.detect(&DeviceContext {
            sys_object_id: "1.3.6.1.4.1.14988.1",
            sys_descr: ""
        }));
        assert!(!p.detect(&DeviceContext {
            sys_object_id: "1.3.6.1.4.1.9.1.1208",
            sys_descr: ""
        }));
    }
}
