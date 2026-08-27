//! Agentype domain types, closed state machines, and authority predicates.
//!
//! This crate MUST NOT depend on SQLite, Tokio, vendors, CLI, or env vars.

mod authority;
mod clock;
mod decisions;
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
pub use decisions::{
    batch_next_state, claim_selection_rank, claim_tiebreak, durable_quiescence,
    excess_disposition, incarnation_presence, retry_allowed, retry_backoff_seconds,
    suspension_failure_class, ExcessDisposition, PresenceAction,
};
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

    #[test]
    fn claim_rank_prefers_workstream_continuity() {
        use decisions::*;
        assert_eq!(
            claim_selection_rank(ContinuityPreference::Required, true),
            0
        );
        assert_eq!(
            claim_selection_rank(ContinuityPreference::Preferred, false),
            1
        );
        // Plain "none" continuity is generic placement even on same workstream.
        assert_eq!(claim_selection_rank(ContinuityPreference::None, true), 1);
    }

    #[test]
    fn claim_tiebreak_is_oldest_availability_then_lowest_id() {
        use decisions::*;
        let older = claim_tiebreak(Some(900.0), 1000.0, "zzz");
        let newer = claim_tiebreak(Some(1100.0), 900.0, "aaa");
        assert!(
            older < newer,
            "availability must dominate the id tiebreak"
        );
        let tied_low = claim_tiebreak(Some(5.0), 5.0, "a");
        let tied_high = claim_tiebreak(Some(5.0), 5.0, "b");
        assert!(tied_low < tied_high);
        // NULL availability falls back to created_at.
        assert_eq!(claim_tiebreak(None, 42.0, "x").0, 42.0);
    }

    #[test]
    fn durable_quiescence_requires_terminality() {
        use decisions::*;
        assert!(!durable_quiescence(false, true));
        assert!(!durable_quiescence(true, false));
        assert!(durable_quiescence(true, true));
    }

    #[test]
    fn suspension_class_escapes_to_writer_safety() {
        use decisions::*;
        assert_eq!(
            suspension_failure_class(true, FailureClass::ExecutionLost),
            FailureClass::ExecutionLost
        );
        assert_eq!(
            suspension_failure_class(false, FailureClass::ResourceUnavailable),
            FailureClass::WriterQuiescenceUnknown
        );
    }

    #[test]
    fn batch_aggregate_states() {
        use decisions::*;
        assert_eq!(batch_next_state(true, false, false), BatchState::Suspended);
        assert_eq!(batch_next_state(false, true, false), BatchState::Suspended);
        assert_eq!(batch_next_state(false, false, false), BatchState::Active);
        assert_eq!(batch_next_state(false, false, true), BatchState::Completed);
    }

    #[test]
    fn excess_disposition_retires_only_idle_unassigned() {
        use decisions::*;
        for state in [
            LogicalAgentState::Ready,
            LogicalAgentState::Initializing,
            LogicalAgentState::Reviving,
        ] {
            assert_eq!(excess_disposition(state, false), ExcessDisposition::RetireDirectly);
            assert_eq!(
                excess_disposition(state, true),
                ExcessDisposition::DrainForRetirement
            );
        }
        assert_eq!(
            excess_disposition(LogicalAgentState::Assigned, true),
            ExcessDisposition::DrainForRetirement
        );
        assert_eq!(
            excess_disposition(LogicalAgentState::Suspended, false),
            ExcessDisposition::DrainForRetirement
        );
    }

    #[test]
    fn incarnation_presence_actions_match_oracle_branches() {
        use decisions::*;
        // Proof of life promotes warmth.
        assert_eq!(
            incarnation_presence(ExecutionState::Running, false, false, false),
            PresenceAction::PromoteWarm
        );
        // Non-terminal observations never touch a fenced presence.
        for s in [ExecutionState::Starting, ExecutionState::Unknown] {
            assert_eq!(
                incarnation_presence(s, false, false, false),
                PresenceAction::Ignore
            );
        }
        // Declared-reusable quiet end keeps the presence warm.
        assert_eq!(
            incarnation_presence(ExecutionState::Succeeded, true, true, true),
            PresenceAction::PromoteWarm
        );
        // Confirmed quiet end (not from LOST) terminates.
        assert_eq!(
            incarnation_presence(ExecutionState::Succeeded, true, true, false),
            PresenceAction::FenceTerminated
        );
        // LOST can never be a confirmed end; unproven ends stay LOST.
        assert_eq!(
            incarnation_presence(ExecutionState::Lost, true, true, false),
            PresenceAction::FenceLost
        );
        assert_eq!(
            incarnation_presence(ExecutionState::Failed, true, false, false),
            PresenceAction::FenceLost
        );
        assert_eq!(
            incarnation_presence(ExecutionState::Terminated, true, true, false),
            PresenceAction::FenceTerminated
        );
    }
}
