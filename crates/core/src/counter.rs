//! Counter math: 32/64-bit wrap and reset detection, rate computation.
//!
//! Rules encoded here:
//! - A counter going up is a normal delta.
//! - A 64-bit counter going down is treated as a reset.
//! - A 32-bit counter going down is treated as a possible wrap if its previous
//!   value was in the top half of its range, and as a reset otherwise.
//!   Resets produce no rate.
//! - Rates are computed from *actual* elapsed time, never the nominal poll
//!   interval. A poll that took 65s gets divided by 65, not 60.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterWidth {
    Bits32,
    Bits64,
}

impl CounterWidth {
    pub fn max(self) -> u64 {
        match self {
            CounterWidth::Bits32 => u32::MAX as u64,
            CounterWidth::Bits64 => u64::MAX,
        }
    }
}

/// Outcome of comparing two consecutive counter samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta {
    /// No previous sample — cannot compute a rate yet.
    First,
    /// Normal increase.
    Normal(u64),
    /// Counter rolled over from max back to 0 and advanced by the wrapped amount.
    Wrapped(u64),
    /// Counter went backwards and is treated as a reset, with no rate.
    Reset,
}

/// Compute the delta between a previous and current sample.
///
/// `prev` may be `None` on the first sample after a restart; that returns
/// `Delta::First`.
pub fn delta(prev: Option<u64>, curr: u64, width: CounterWidth) -> Delta {
    let prev = match prev {
        Some(p) => p,
        None => return Delta::First,
    };
    if curr >= prev {
        return Delta::Normal(curr - prev);
    }
    if width == CounterWidth::Bits64 {
        return Delta::Reset;
    }
    let max = width.max();
    let half = max / 2;
    if prev > half {
        // Wrapped: (max - prev) steps to reach max, +1 to roll to 0, +curr.
        let wrapped = (max - prev).saturating_add(1).saturating_add(curr);
        Delta::Wrapped(wrapped)
    } else {
        Delta::Reset
    }
}

/// Convert a delta into a per-second rate. Returns `None` for `First` and
/// `Reset` (no meaningful rate exists), and for non-positive elapsed time.
pub fn rate(d: Delta, elapsed_secs: f64) -> Option<f64> {
    if elapsed_secs <= 0.0 {
        return None;
    }
    match d {
        Delta::Normal(v) | Delta::Wrapped(v) => Some(v as f64 / elapsed_secs),
        Delta::First | Delta::Reset => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_delta() {
        assert_eq!(
            delta(Some(100), 150, CounterWidth::Bits64),
            Delta::Normal(50)
        );
    }

    #[test]
    fn first_sample() {
        assert_eq!(delta(None, 42, CounterWidth::Bits64), Delta::First);
    }

    #[test]
    fn reset_small_to_small() {
        // Went from 100 to 50 — clearly a reset, not a wrap.
        assert_eq!(delta(Some(100), 50, CounterWidth::Bits32), Delta::Reset);
    }

    #[test]
    fn wrap_32bit() {
        // u32::MAX = 4294967295. Wrapped from near max to small value.
        let prev = u32::MAX as u64 - 10;
        let curr = 5u64;
        // 10 steps to max, +1 to roll to 0, +5 = 16.
        assert_eq!(
            delta(Some(prev), curr, CounterWidth::Bits32),
            Delta::Wrapped(16)
        );
    }

    #[test]
    fn decrease_64bit_is_reset_even_near_maximum() {
        assert_eq!(
            delta(Some(u64::MAX - 7), 3, CounterWidth::Bits64),
            Delta::Reset
        );
        assert_eq!(delta(Some(100), 50, CounterWidth::Bits64), Delta::Reset);
    }

    #[test]
    fn rate_uses_actual_elapsed_time() {
        let d = Delta::Normal(130);
        // A 65s poll, not the nominal 60s.
        assert!((rate(d, 65.0).unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn rate_none_on_reset_and_first() {
        assert!(rate(Delta::Reset, 60.0).is_none());
        assert!(rate(Delta::First, 60.0).is_none());
    }

    #[test]
    fn rate_none_on_zero_elapsed() {
        assert!(rate(Delta::Normal(10), 0.0).is_none());
        assert!(rate(Delta::Normal(10), -1.0).is_none());
    }

    #[test]
    fn wrap_rate_is_correct() {
        // 32-bit wrapped, 16 units over 1 second → 16/s.
        let d = delta(Some(u32::MAX as u64 - 10), 5, CounterWidth::Bits32);
        assert!((rate(d, 1.0).unwrap() - 16.0).abs() < 1e-9);
    }
}
