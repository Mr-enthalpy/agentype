//! M5.3 timing configuration gate (spec 16 §A2).
//!
//! The runtime loops (dispatcher poll, heartbeat supervision, lease expiry)
//! are mechanics, not Core semantics, but their timing relationship is
//! normative: `dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`.
//! A configuration that violates the chain is rejected at construction —
//! never at first use, never silently.
//!
//! The lease duration itself remains Kernel-owned authority: this config
//! carries it only so the gate can be validated against the composed
//! Kernel's actual `lease_seconds` (see `SupervisionService::new`), never as
//! an independent renewal decision.

use agentype_core::Error;
use std::fmt;
use std::time::Duration;

/// Fail-closed runtime timing configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeTimingConfig {
    dispatcher_poll_seconds: f64,
    heartbeat_seconds: f64,
    lease_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimingConfigError {
    NonPositiveDuration { field: &'static str, value: f64 },
    PollExceedsHeartbeat { poll: f64, heartbeat: f64 },
    HeartbeatNotBelowLease { heartbeat: f64, lease: f64 },
}

impl fmt::Display for TimingConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveDuration { field, value } => {
                write!(f, "timing {field} must be positive, got {value}")
            }
            Self::PollExceedsHeartbeat { poll, heartbeat } => write!(
                f,
                "dispatcher_poll_seconds ({poll}) must be <= heartbeat_seconds ({heartbeat})"
            ),
            Self::HeartbeatNotBelowLease { heartbeat, lease } => write!(
                f,
                "heartbeat_seconds ({heartbeat}) must be < lease_seconds ({lease})"
            ),
        }
    }
}

impl std::error::Error for TimingConfigError {}

impl RuntimeTimingConfig {
    /// Validate the normative timing chain (spec 16 §A2):
    /// `0 < dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`.
    ///
    /// The M5.3 plan's stronger headroom guidance
    /// (`heartbeat_seconds <= lease_seconds / 2`) is deliberately not
    /// enforced: the normative gate is the chain above, and operators may
    /// tighten headroom per deployment.
    pub fn new(
        dispatcher_poll_seconds: f64,
        heartbeat_seconds: f64,
        lease_seconds: f64,
    ) -> Result<Self, TimingConfigError> {
        for (field, value) in [
            ("dispatcher_poll_seconds", dispatcher_poll_seconds),
            ("heartbeat_seconds", heartbeat_seconds),
            ("lease_seconds", lease_seconds),
        ] {
            // `value > 0.0` is false for zero, negatives, and NaN.
            if value > 0.0 {
                continue;
            }
            return Err(TimingConfigError::NonPositiveDuration { field, value });
        }
        if dispatcher_poll_seconds > heartbeat_seconds {
            return Err(TimingConfigError::PollExceedsHeartbeat {
                poll: dispatcher_poll_seconds,
                heartbeat: heartbeat_seconds,
            });
        }
        if heartbeat_seconds >= lease_seconds {
            return Err(TimingConfigError::HeartbeatNotBelowLease {
                heartbeat: heartbeat_seconds,
                lease: lease_seconds,
            });
        }
        Ok(Self {
            dispatcher_poll_seconds,
            heartbeat_seconds,
            lease_seconds,
        })
    }

    pub fn dispatcher_poll_seconds(&self) -> f64 {
        self.dispatcher_poll_seconds
    }

    pub fn heartbeat_seconds(&self) -> f64 {
        self.heartbeat_seconds
    }

    pub fn lease_seconds(&self) -> f64 {
        self.lease_seconds
    }

    /// The heartbeat interval as a duration for scheduling mechanics.
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs_f64(self.heartbeat_seconds)
    }

    /// The dispatcher poll interval as a duration for scheduling mechanics.
    pub fn dispatcher_poll_interval(&self) -> Duration {
        Duration::from_secs_f64(self.dispatcher_poll_seconds)
    }
}

/// Composition helper for the supervision layer: the configured lease
/// duration must match the Kernel's actual lease authority exactly, so the
/// supervisor never computes renewal durations independently from the
/// Kernel (M5.3 §30).
pub(crate) fn validate_lease_authority_match(
    timing: &RuntimeTimingConfig,
    kernel_lease_seconds: f64,
) -> Result<(), Error> {
    if (timing.lease_seconds() - kernel_lease_seconds).abs() > f64::EPSILON {
        return Err(Error::invariant(format!(
            "timing configuration lease_seconds ({}) does not match the Kernel lease authority ({})",
            timing.lease_seconds(),
            kernel_lease_seconds
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 16 §A2: a valid chain is accepted and readable.
    #[test]
    fn valid_timing_chain_is_accepted() {
        let t = RuntimeTimingConfig::new(1.0, 2.0, 10.0).unwrap();
        assert_eq!(t.dispatcher_poll_seconds(), 1.0);
        assert_eq!(t.heartbeat_seconds(), 2.0);
        assert_eq!(t.lease_seconds(), 10.0);
        assert_eq!(t.heartbeat_interval(), Duration::from_secs_f64(2.0));
    }

    /// poll > heartbeat is rejected.
    #[test]
    fn poll_exceeding_heartbeat_is_rejected() {
        let err = RuntimeTimingConfig::new(3.0, 2.0, 10.0).unwrap_err();
        assert!(matches!(
            err,
            TimingConfigError::PollExceedsHeartbeat { .. }
        ));
    }

    /// heartbeat >= lease is rejected (the normative gate is strict `<`).
    #[test]
    fn heartbeat_at_or_above_lease_is_rejected() {
        let err = RuntimeTimingConfig::new(1.0, 10.0, 10.0).unwrap_err();
        assert!(matches!(
            err,
            TimingConfigError::HeartbeatNotBelowLease { .. }
        ));
        let err = RuntimeTimingConfig::new(1.0, 11.0, 10.0).unwrap_err();
        assert!(matches!(
            err,
            TimingConfigError::HeartbeatNotBelowLease { .. }
        ));
    }

    /// Non-positive (or NaN) durations are rejected for every field.
    #[test]
    fn non_positive_durations_are_rejected() {
        for (poll, heartbeat, lease) in [
            (0.0, 2.0, 10.0),
            (-1.0, 2.0, 10.0),
            (1.0, 0.0, 10.0),
            (1.0, f64::NAN, 10.0),
            (1.0, 2.0, 0.0),
            (1.0, 2.0, -3.0),
        ] {
            let err = RuntimeTimingConfig::new(poll, heartbeat, lease).unwrap_err();
            assert!(
                matches!(err, TimingConfigError::NonPositiveDuration { .. }),
                "expected NonPositiveDuration for ({poll}, {heartbeat}, {lease}), got {err:?}"
            );
        }
    }

    /// Lease-authority match: a drift between the configured lease duration
    /// and the Kernel's actual lease authority is a composition failure.
    #[test]
    fn lease_authority_drift_is_rejected() {
        let t = RuntimeTimingConfig::new(1.0, 2.0, 10.0).unwrap();
        assert!(validate_lease_authority_match(&t, 10.0).is_ok());
        let err = validate_lease_authority_match(&t, 12.0).unwrap_err();
        assert!(matches!(err, Error::InvariantViolation(_)));
    }
}
