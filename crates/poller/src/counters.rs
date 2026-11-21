use std::collections::HashMap;

use chrono::{DateTime, Utc};
use netwatch_core::counter::{self, CounterWidth, Delta};
use tracing::warn;

#[derive(Clone, Copy)]
struct Baseline {
    value: u64,
    width: CounterWidth,
    ts: DateTime<Utc>,
    uptime: u32,
    discontinuity: u32,
}

#[derive(Default)]
pub struct CounterTracker {
    previous: HashMap<(i32, i64), Baseline>,
}

#[derive(Debug, Default, PartialEq)]
pub struct CounterResult {
    pub delta: Option<f64>,
    pub dt_seconds: Option<f64>,
    pub reset: bool,
}

impl CounterTracker {
    pub fn retain_interfaces(&mut self, ids: &std::collections::HashSet<i64>) {
        self.previous
            .retain(|&(_, interface_id), _| interface_id == 0 || ids.contains(&interface_id));
    }

    pub fn observe(
        &mut self,
        key: (i32, i64),
        value: u64,
        width: CounterWidth,
        ts: DateTime<Utc>,
        continuity: Option<(u32, u32)>,
        speed_bps: Option<u64>,
    ) -> CounterResult {
        let Some((uptime, discontinuity)) = continuity else {
            self.previous.remove(&key);
            return CounterResult::default();
        };
        let next = Baseline {
            value,
            width,
            ts,
            uptime,
            discontinuity,
        };
        let Some(prev) = self.previous.insert(key, next) else {
            return CounterResult::default();
        };
        let Some(micros) = ts.signed_duration_since(prev.ts).num_microseconds() else {
            warn!(
                metric_id = key.0,
                interface_id = key.1,
                "counter time difference overflowed"
            );
            return CounterResult::default();
        };
        if micros <= 0 {
            warn!(
                metric_id = key.0,
                interface_id = key.1,
                "counter timestamps did not advance"
            );
            self.previous.remove(&key);
            return CounterResult::default();
        }
        let elapsed = micros as f64 / 1_000_000.0;
        if discontinuity != prev.discontinuity {
            return CounterResult {
                reset: true,
                ..Default::default()
            };
        }
        if prev.width != width {
            warn!(
                metric_id = key.0,
                interface_id = key.1,
                "counter width changed; rate unavailable"
            );
            return CounterResult::default();
        }
        let delta = counter::delta(Some(prev.value), value, width);
        if width == CounterWidth::Bits64 && delta == Delta::Reset {
            return CounterResult {
                reset: true,
                ..Default::default()
            };
        }
        if uptime < prev.uptime {
            warn!(
                metric_id = key.0,
                interface_id = key.1,
                "device uptime moved backwards; rate unavailable"
            );
            return CounterResult::default();
        }
        // Check the device's uptime against real elapsed time to reject a reboot
        // even when a device reports the same discontinuity time after restarting.
        let uptime_elapsed = (uptime - prev.uptime) as f64 / 100.0;
        if (uptime_elapsed - elapsed).abs() > (elapsed * 0.05).max(5.0) {
            warn!(
                metric_id = key.0,
                interface_id = key.1,
                "device uptime does not match elapsed time; rate unavailable"
            );
            return CounterResult::default();
        }
        match delta {
            Delta::Normal(delta) => CounterResult {
                delta: Some(delta as f64),
                dt_seconds: Some(elapsed),
                reset: false,
            },
            Delta::Wrapped(delta)
                if speed_bps.is_some_and(|speed| {
                    delta as f64 <= speed as f64 / 8.0 * (elapsed + 5.0) * 1.05
                }) =>
            {
                CounterResult {
                    delta: Some(delta as f64),
                    dt_seconds: Some(elapsed),
                    reset: false,
                }
            }
            Delta::Reset => CounterResult {
                reset: true,
                ..Default::default()
            },
            Delta::Wrapped(_) => {
                warn!(
                    metric_id = key.0,
                    interface_id = key.1,
                    "ambiguous counter decrease; rate unavailable"
                );
                CounterResult::default()
            }
            Delta::First => CounterResult::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
    }

    #[test]
    fn exact_delta_above_f64_precision_uses_actual_time() {
        let mut tracker = CounterTracker::default();
        let value = (1_u64 << 60) + 1;
        assert_eq!(
            tracker.observe(
                (1, 2),
                value,
                CounterWidth::Bits64,
                at(0),
                Some((10_000, 0)),
                None
            ),
            CounterResult::default()
        );
        assert_eq!(
            tracker.observe(
                (1, 2),
                value + 7,
                CounterWidth::Bits64,
                at(65),
                Some((16_500, 0)),
                None
            ),
            CounterResult {
                delta: Some(7.0),
                dt_seconds: Some(65.0),
                reset: false
            }
        );
    }

    #[test]
    fn missing_metadata_and_reboot_do_not_emit_rates() {
        let mut tracker = CounterTracker::default();
        let key = (1, 2);
        tracker.observe(
            key,
            100,
            CounterWidth::Bits64,
            at(0),
            Some((10_000, 0)),
            None,
        );
        assert_eq!(
            tracker.observe(key, 120, CounterWidth::Bits64, at(60), None, None),
            CounterResult::default()
        );
        assert_eq!(
            tracker.observe(
                key,
                150,
                CounterWidth::Bits64,
                at(120),
                Some((22_000, 0)),
                None
            ),
            CounterResult::default()
        );
        assert_eq!(
            tracker.observe(key, 10, CounterWidth::Bits64, at(180), Some((100, 0)), None),
            CounterResult {
                reset: true,
                ..Default::default()
            }
        );
        assert!(
            tracker
                .observe(
                    key,
                    20,
                    CounterWidth::Bits64,
                    at(240),
                    Some((6_100, 100)),
                    None
                )
                .reset
        );
    }

    #[test]
    fn wrap_requires_plausible_link_speed() {
        let key = (1, 2);
        let mut tracker = CounterTracker::default();
        tracker.observe(
            key,
            u32::MAX as u64 - 10,
            CounterWidth::Bits32,
            at(0),
            Some((100, 0)),
            Some(1_000),
        );
        let wrapped = tracker.observe(
            key,
            5,
            CounterWidth::Bits32,
            at(1),
            Some((200, 0)),
            Some(1_000),
        );
        assert_eq!(wrapped.delta, Some(16.0));
        tracker.observe(
            key,
            u32::MAX as u64 - 10,
            CounterWidth::Bits32,
            at(2),
            Some((300, 0)),
            None,
        );
        assert_eq!(
            tracker.observe(key, 5, CounterWidth::Bits32, at(3), Some((400, 0)), None),
            CounterResult::default()
        );
    }

    #[test]
    fn counter64_decrease_is_reset_and_next_sample_uses_new_baseline() {
        let key = (1, 2);
        let mut tracker = CounterTracker::default();
        tracker.observe(
            key,
            u64::MAX - 7,
            CounterWidth::Bits64,
            at(0),
            Some((100, 0)),
            Some(1_000),
        );
        assert_eq!(
            tracker.observe(
                key,
                3,
                CounterWidth::Bits64,
                at(1),
                Some((200, 0)),
                Some(1_000),
            ),
            CounterResult {
                reset: true,
                ..Default::default()
            }
        );
        assert_eq!(
            tracker.observe(
                key,
                10,
                CounterWidth::Bits64,
                at(2),
                Some((300, 0)),
                Some(1_000),
            ),
            CounterResult {
                delta: Some(7.0),
                dt_seconds: Some(1.0),
                reset: false,
            }
        );
    }

    #[test]
    fn counter32_decrease_below_half_is_recorded_as_reset() {
        let key = (1, 2);
        let mut tracker = CounterTracker::default();
        tracker.observe(key, 100, CounterWidth::Bits32, at(0), Some((100, 0)), None);
        assert_eq!(
            tracker.observe(key, 50, CounterWidth::Bits32, at(1), Some((200, 0)), None),
            CounterResult {
                reset: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn detects_unreliable_time_and_unchanged_uptime_after_reboot() {
        let mut tracker = CounterTracker::default();
        let key = (1, 2);
        tracker.observe(
            key,
            100,
            CounterWidth::Bits64,
            at(0),
            Some((10_000, 0)),
            None,
        );
        assert_eq!(
            tracker.observe(
                key,
                200,
                CounterWidth::Bits64,
                at(60),
                Some((11_000, 0)),
                None
            ),
            CounterResult::default()
        );
        assert_eq!(
            tracker.observe(
                key,
                220,
                CounterWidth::Bits64,
                at(60),
                Some((11_000, 0)),
                None
            ),
            CounterResult::default()
        );
        assert_eq!(
            tracker.observe(
                key,
                230,
                CounterWidth::Bits64,
                at(120),
                Some((17_000, 0)),
                None
            ),
            CounterResult::default()
        );
    }
}
