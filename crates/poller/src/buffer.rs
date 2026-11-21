//! Per-device sample buffer.
//!
//! The buffer absorbs bursts from a single poll and flushes in batches via
//! `COPY`. Backpressure rules (documented in `docs/data-lifecycle.md`):
//!
//! - Capacity is bounded. When full, the **oldest** row is dropped, because
//!   in monitoring a slightly stale sample is less harmful than an OOM.
//! - Flush is triggered by size (>= `flush_rows`) or time (>= `flush_interval`).
//! - `flush_on_shutdown` must be awaited before process exit — systemd sends
//!   SIGTERM and will `SIGKILL` after `TimeoutStopSec`. The poller's SIGTERM
//!   handler awaits this.
//!
//! The buffer does **not** own a pool. It takes a `&mut PgConnection` on
//! flush so the caller (the scheduler) can acquire once per flush burst and
//! reuse across devices, or hold one per device task — either works.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use sqlx::PgConnection;

use netwatch_core::model::SampleRow;
use netwatch_db::writer::{copy_samples_on, WriterError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferResult {
    Ok,
    DroppedOldest,
}

pub struct Buffer {
    rows: VecDeque<SampleRow>,
    max_rows: usize,
    flush_rows: usize,
    flush_interval: Duration,
    last_flush: Instant,
    dropped_total: u64,
}

impl Buffer {
    pub fn new(max_rows: usize, flush_rows: usize, flush_interval: Duration) -> Self {
        assert!(flush_rows > 0, "flush_rows must be > 0");
        assert!(max_rows >= flush_rows, "max_rows must be >= flush_rows");
        Self {
            rows: VecDeque::with_capacity(flush_rows),
            max_rows,
            flush_rows,
            flush_interval,
            last_flush: Instant::now(),
            dropped_total: 0,
        }
    }

    pub fn push(&mut self, row: SampleRow) -> BufferResult {
        if self.rows.len() >= self.max_rows {
            self.rows.pop_front();
            self.rows.push_back(row);
            self.dropped_total += 1;
            BufferResult::DroppedOldest
        } else {
            self.rows.push_back(row);
            BufferResult::Ok
        }
    }

    /// True when there is something to flush AND it is due. The `is_empty`
    /// guard matters: without it, `should_flush` returns true forever once
    /// `flush_interval` passes on an idle device.
    pub fn should_flush(&self) -> bool {
        !self.rows.is_empty()
            && (self.rows.len() >= self.flush_rows
                || self.last_flush.elapsed() >= self.flush_interval)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    /// Flush all buffered rows via COPY. On DB error, the rows are returned
    /// to the buffer intact so the caller can retry — no silent data loss.
    pub async fn flush(&mut self, conn: &mut PgConnection) -> Result<usize, BufferFlushError> {
        if self.rows.is_empty() {
            return Ok(0);
        }
        let drain: Vec<SampleRow> = self.rows.drain(..).collect();
        let n = drain.len();
        if let Some(oldest) = drain.iter().map(|row| row.ts).min() {
            if oldest < chrono::Utc::now() - chrono::Duration::minutes(2) {
                tracing::warn!(%oldest, count = n, "delayed sample batch may fall outside the rollup lookback; historical rollups are not automatically backfilled");
            }
        }
        if let Err(e) = copy_samples_on(conn, &drain).await {
            // Restore in original order so the next attempt is identical.
            for row in drain.into_iter().rev() {
                self.rows.push_front(row);
            }
            return Err(BufferFlushError::Writer(e));
        }
        self.last_flush = Instant::now();
        Ok(n)
    }

    /// Final flush before process exit.
    pub async fn flush_on_shutdown(
        mut self,
        conn: &mut PgConnection,
    ) -> Result<usize, BufferFlushError> {
        self.flush(conn).await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BufferFlushError {
    #[error("writer error: {0}")]
    Writer(#[from] WriterError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use netwatch_core::model::SampleRow;

    fn row(i: i64) -> SampleRow {
        SampleRow {
            device_id: 1,
            metric_id: 1,
            interface_id: 0,
            ts: Utc::now() + chrono::Duration::seconds(i),
            value: i as f64,
            delta: None,
            dt_seconds: None,
            counter_reset: false,
            mapping_verified: true,
        }
    }

    #[test]
    fn push_returns_ok_until_capacity_then_drops_oldest() {
        let mut b = Buffer::new(3, 2, Duration::from_secs(60));
        assert_eq!(b.push(row(0)), BufferResult::Ok);
        assert_eq!(b.push(row(1)), BufferResult::Ok);
        assert_eq!(b.push(row(2)), BufferResult::Ok);
        // Full now: next push drops row(0).
        assert_eq!(b.push(row(3)), BufferResult::DroppedOldest);
        assert_eq!(b.len(), 3);
        assert_eq!(b.dropped_total(), 1);
    }

    #[test]
    fn should_flush_on_size() {
        let mut b = Buffer::new(100, 2, Duration::from_secs(3600));
        assert!(!b.should_flush());
        b.push(row(0));
        assert!(!b.should_flush());
        b.push(row(1));
        assert!(b.should_flush(), "should flush once size threshold hit");
    }

    #[test]
    fn should_flush_false_when_empty_even_if_interval_passed() {
        let b = Buffer::new(100, 10, Duration::from_millis(0));
        assert!(!b.should_flush(), "empty buffer must not trigger a flush");
    }

    #[test]
    fn should_flush_on_time() {
        let mut b = Buffer::new(100, 10, Duration::from_millis(50));
        b.push(row(0));
        std::thread::sleep(Duration::from_millis(60));
        assert!(b.should_flush());
    }
}
