//! Agentype domain types, closed state machines, and authority predicates.
//!
//! This crate MUST NOT depend on SQLite, Tokio, vendors, CLI, or env vars.

mod authority;
mod clock;
mod errors;
mod ids;
mod records;
mod states;

pub use authority::{
    completed_task_must_not_reopen, physical_transition_allowed, require_physical_transition,
    tags_match, task_create_establishes_authority, unavailable_configuration_failure,
    validate_authority, writer_is_safe_to_replace, AuthoritySnapshot,
};
pub use clock::{Clock, ManualClock, SystemClock, UnixTime};
pub use errors::Error;
pub use ids::*;
pub use records::*;
pub use states::*;

pub type CoreResult<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_may_become_running() {
        assert!(physical_transition_allowed(
            ExecutionState::Unknown,
            ExecutionState::Running
        ));
        assert!(physical_transition_allowed(
            ExecutionState::Lost,
            ExecutionState::Terminated
        ));
        assert!(!physical_transition_allowed(
            ExecutionState::Terminated,
            ExecutionState::Running
        ));
    }

    #[test]
    fn expired_lease_is_stale_before_sweeper() {
        let attempt = AttemptId::new();
        let snap = AuthoritySnapshot {
            attempt_id: attempt.clone(),
            attempt_state: AttemptState::Active,
            lease_state: LeaseState::Active,
            lease_epoch: LeaseEpoch(1),
            lease_expires_at: 10.0,
            task_current_attempt_id: Some(attempt),
            task_fencing_epoch: LeaseEpoch(1),
        };
        assert!(validate_authority(&snap, LeaseEpoch(1), 10.0).is_err());
        assert!(validate_authority(&snap, LeaseEpoch(1), 9.9).is_ok());
    }

    #[test]
    fn writer_safety_ignores_omitted_execution_when_row_exists() {
        assert!(!writer_is_safe_to_replace(true, true, false, false));
        assert!(writer_is_safe_to_replace(true, false, false, false));
        assert!(writer_is_safe_to_replace(true, true, false, true));
        assert!(writer_is_safe_to_replace(false, true, false, false));
    }

    #[test]
    fn retry_backoff_is_bounded() {
        let p = RetryPolicy {
            max_attempts: 8,
            retry_classes: vec![FailureClass::Timeout],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        };
        assert_eq!(p.delay_for_attempt(1), 1.0);
        assert_eq!(p.delay_for_attempt(2), 2.0);
        assert_eq!(p.delay_for_attempt(4), 8.0);
        assert_eq!(p.delay_for_attempt(10), 8.0);
        assert!(!p.allows(FailureClass::Unknown, 1));
        assert!(p.allows(FailureClass::Timeout, 1));
        assert!(!p.allows(FailureClass::Timeout, 8));
    }
}
