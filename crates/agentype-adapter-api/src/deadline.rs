//! Process-local absolute monotonic deadline for one Scheduler-facing
//! ExecutionAdapter invocation.
//!
//! Lease and durable state use Scheduler `UnixTime`. Adapter I/O MUST NOT.
//! This type is never serialized, never persisted, and dies with the call.

use std::fmt;
use std::time::{Duration, Instant};

/// One Scheduler-facing adapter operation. Runtime mechanics only: not a
/// persisted Scheduler entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdapterOperation {
    StartExecution,
    ReconcileStart,
    ObserveExecution,
    CollectOutcome,
    InterruptExecution,
    TerminateExecution,
}

/// Fail-closed construction of a finite positive operation budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadlineConfigError {
    NonPositive,
    Overflow,
}

impl fmt::Display for DeadlineConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositive => write!(f, "adapter deadline timeout must be positive"),
            Self::Overflow => write!(f, "adapter deadline timeout overflows Instant"),
        }
    }
}

impl std::error::Error for DeadlineConfigError {}

/// Absolute monotonic endpoint for one adapter invocation, including every
/// internal stage and exception cleanup. There is no extend/reset/refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterDeadline {
    expires_at: Instant,
}

impl AdapterDeadline {
    /// Start a new operation budget from now. `timeout` must be > 0 and
    /// representable as an `Instant` offset.
    pub fn after(timeout: Duration) -> Result<Self, DeadlineConfigError> {
        if timeout.is_zero() {
            return Err(DeadlineConfigError::NonPositive);
        }
        let expires_at = Instant::now()
            .checked_add(timeout)
            .ok_or(DeadlineConfigError::Overflow)?;
        Ok(Self { expires_at })
    }

    /// Deterministic constructor for algebra tests and fakes. Not an
    /// extend/reset of an existing deadline.
    pub fn from_instant(expires_at: Instant) -> Self {
        Self { expires_at }
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    pub fn remaining(&self) -> Duration {
        self.remaining_at(Instant::now())
    }

    /// Saturates at zero. `now == expires_at` is expired (zero remaining).
    pub fn remaining_at(&self, now: Instant) -> Duration {
        self.expires_at.saturating_duration_since(now)
    }

    pub fn is_expired(&self) -> bool {
        self.is_expired_at(Instant::now())
    }

    pub fn is_expired_at(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_deadline_is_accepted() {
        let d = AdapterDeadline::after(Duration::from_millis(50)).unwrap();
        assert!(!d.is_expired());
        assert!(d.remaining() > Duration::ZERO);
    }

    #[test]
    fn zero_timeout_is_rejected() {
        assert_eq!(
            AdapterDeadline::after(Duration::ZERO).unwrap_err(),
            DeadlineConfigError::NonPositive
        );
    }

    #[test]
    fn unrepresentable_timeout_is_rejected() {
        assert_eq!(
            AdapterDeadline::after(Duration::MAX).unwrap_err(),
            DeadlineConfigError::Overflow
        );
    }

    #[test]
    fn remaining_before_expiry_is_positive() {
        let origin = Instant::now();
        let d = AdapterDeadline::from_instant(origin + Duration::from_secs(10));
        assert_eq!(d.remaining_at(origin), Duration::from_secs(10));
        assert!(!d.is_expired_at(origin));
    }

    #[test]
    fn exact_expiry_is_zero() {
        let origin = Instant::now();
        let d = AdapterDeadline::from_instant(origin);
        assert_eq!(d.remaining_at(origin), Duration::ZERO);
        assert!(d.is_expired_at(origin));
    }

    #[test]
    fn after_expiry_is_zero_never_negative() {
        let origin = Instant::now();
        let d = AdapterDeadline::from_instant(origin);
        let later = origin + Duration::from_secs(5);
        assert_eq!(d.remaining_at(later), Duration::ZERO);
        assert!(d.is_expired_at(later));
    }

    #[test]
    fn remaining_reads_do_not_move_the_endpoint() {
        let origin = Instant::now();
        let end = origin + Duration::from_secs(7);
        let d = AdapterDeadline::from_instant(end);
        let _ = d.remaining_at(origin);
        let _ = d.remaining_at(origin + Duration::from_secs(1));
        assert_eq!(d.expires_at(), end);
        assert_eq!(d.remaining_at(origin), Duration::from_secs(7));
    }
}
