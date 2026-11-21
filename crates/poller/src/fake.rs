//! Fake profile used when `NETWATCH_FAKE_SNMP=1`. Produces deterministic
//! values so the write path can be exercised without any SNMP.
//!
//! In phase 1 this is how you "run the poller" against a real Postgres and
//! watch rows land, before you've decided on an SNMP crate.

use netwatch_core::{
    counter::CounterWidth,
    model::{MetricReading, MetricValue},
};
use netwatch_profiles::{DeviceContext, MetricDef, MetricKind, PollRequest, PollResponse, Profile};

pub struct FakeProfile;

impl Profile for FakeProfile {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn detect(&self, _: &DeviceContext<'_>) -> bool {
        true
    }
    fn metrics(&self) -> &'static [MetricDef] {
        &[
            MetricDef {
                name: "fake_cpu",
                unit: "%",
                kind: MetricKind::Gauge,
            },
            MetricDef {
                name: "fake_mem",
                unit: "%",
                kind: MetricKind::Gauge,
            },
            MetricDef {
                name: "fake_in_octets",
                unit: "bytes",
                kind: MetricKind::Counter,
            },
        ]
    }
    fn request(&self, _: &[netwatch_core::model::InterfaceRef]) -> PollRequest {
        PollRequest::default()
    }
    fn parse(&self, _: &PollResponse) -> Vec<MetricValue> {
        // Slight variation so graphs aren't flat lines.
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let wobble = (n % 100) as f64;
        vec![
            MetricValue {
                name: "fake_cpu",
                interface_index: None,
                value: MetricReading::Gauge(40.0 + wobble),
            },
            MetricValue {
                name: "fake_mem",
                interface_index: None,
                value: MetricReading::Gauge(55.0 + wobble / 2.0),
            },
            MetricValue {
                name: "fake_in_octets",
                interface_index: Some(1),
                value: MetricReading::Counter {
                    value: n * 1000,
                    width: CounterWidth::Bits64,
                },
            },
        ]
    }
}
