//! Injectable clock. Times are UTC epoch seconds (fractional).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub type UnixTime = f64;

pub trait Clock: Send + Sync {
    fn now(&self) -> UnixTime;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixTime {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}

/// Deterministic clock for tests. Milliseconds of precision.
#[derive(Debug, Clone)]
pub struct ManualClock {
    millis: Arc<AtomicU64>,
}

impl ManualClock {
    pub fn new(start: UnixTime) -> Self {
        Self {
            millis: Arc::new(AtomicU64::new((start * 1000.0).round() as u64)),
        }
    }

    pub fn set(&self, t: UnixTime) {
        self.millis
            .store((t * 1000.0).round() as u64, Ordering::SeqCst);
    }

    pub fn advance(&self, dt: UnixTime) {
        let add = (dt * 1000.0).round() as u64;
        self.millis.fetch_add(add, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> UnixTime {
        self.millis.load(Ordering::SeqCst) as f64 / 1000.0
    }
}
