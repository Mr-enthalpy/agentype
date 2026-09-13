//! Shared start-observation and collected-outcome classification (M5.4-C).
//!
//! Dispatch and restart reconciliation MUST interpret the same adapter
//! evidence the same way. These functions are pure: they do not touch
//! Kernel state and they never ACK/NACK. Callers persist and apply
//! authority consequences through the existing fenced primitives.

use agentype_adapter_api::{
    AdapterError, PhysicalExecutionOutcome, PhysicalState, StartObservation,
};
use agentype_core::{ExecutionState, FailureClass};

/// Mechanical normalization of adapter invocation errors into the existing
/// `FailureClass` vocabulary (M5.2 task §14 / M5.4 plan §14). Vendor-specific
/// classification belongs inside adapter implementations; no provider strings
/// are parsed at the runtime or core layer.
pub fn adapter_invocation_failure_class(err: &AdapterError) -> FailureClass {
    match err.kind() {
        agentype_adapter_api::AdapterErrorKind::Unavailable => FailureClass::ResourceUnavailable,
        agentype_adapter_api::AdapterErrorKind::DeadlineExceeded => FailureClass::Timeout,
        agentype_adapter_api::AdapterErrorKind::Protocol => FailureClass::AdapterProtocolFailure,
        agentype_adapter_api::AdapterErrorKind::Other => FailureClass::Unknown,
    }
}

/// What a `StartObservation` means. `reconcile_start` returns the same
/// type as `start_execution`; both MUST go through this classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartObservationKind {
    /// Protocol-consistent exact RUNNING. The caller MAY attempt the fenced
    /// RUNNING-confirmation-and-renewal transaction. This is the only
    /// start-observation kind that may produce a `RunningAuthorityGrant`.
    ExactRunning,
    /// Observation claims terminality. The caller MUST `collect_outcome`
    /// before any ACK/NACK; `reconcile_start` / `start_execution` itself
    /// never authorizes a Result.
    TerminalCandidate,
    /// Ambiguous, STARTING, UNKNOWN, protocol-invalid, or any other
    /// unresolved shape. Persist physical history and apply the mechanical
    /// nonterminal NACK when authority is current.
    Unresolved { failure_class: FailureClass },
}

/// What a collected physical outcome means. Collect is not Task Result
/// authority; this classifier does not mutate and never ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectedOutcomeKind {
    /// Process/environment ended without Task Result authority.
    PhysicalEnded,
    /// Contradictory or nonterminal collection. Zero inherited
    /// terminal/quiescence proof.
    Unresolved { failure_class: FailureClass },
}

/// Classify a start / reconcile observation. Order matches the M5.2
/// dispatcher (contradictory RUNNING, then exact RUNNING, then
/// ambiguous/unresolved, then terminal-looking, then catch-all).
pub fn normalize_start_observation(observation: &StartObservation) -> StartObservationKind {
    // An ACTIVE state carrying end-of-execution claims is internally
    // contradictory; fail closed as unresolved so no grant can be minted.
    if observation.state == ExecutionState::Running
        && !observation.ambiguous
        && (observation.terminal_confirmed || observation.quiescent_confirmed)
    {
        return StartObservationKind::Unresolved {
            failure_class: FailureClass::AdapterProtocolFailure,
        };
    }
    if observation.state == ExecutionState::Running && !observation.ambiguous {
        return StartObservationKind::ExactRunning;
    }
    // Identified process ended: collect. Death is not success or quiescence,
    // but it is a terminal candidate. Unknown/ambiguous stay EXECUTION_LOST.
    if observation.state == ExecutionState::Terminated && !observation.ambiguous {
        return StartObservationKind::TerminalCandidate;
    }
    if observation.ambiguous
        || matches!(
            observation.state,
            ExecutionState::Unknown | ExecutionState::Starting
        )
    {
        return StartObservationKind::Unresolved {
            failure_class: FailureClass::ExecutionLost,
        };
    }
    if observation.terminal_confirmed {
        return StartObservationKind::TerminalCandidate;
    }
    StartObservationKind::Unresolved {
        failure_class: FailureClass::ExecutionLost,
    }
}

/// Classify a collected outcome. Order matches the M5.2 dispatcher
/// (active+terminal, LOST+proof, success-without-terminal,
/// quiescence-without-terminality, then terminal success/failure, then
/// nonterminal catch-all).
pub fn normalize_collected_outcome(outcome: &PhysicalExecutionOutcome) -> CollectedOutcomeKind {
    match outcome.physical_state {
        PhysicalState::Exited | PhysicalState::Terminated => CollectedOutcomeKind::PhysicalEnded,
        PhysicalState::Unknown => CollectedOutcomeKind::Unresolved {
            failure_class: FailureClass::ExecutionLost,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_adapter_api::RuntimeHandle;
    use serde_json::json;

    fn start(
        state: ExecutionState,
        ambiguous: bool,
        terminal: bool,
        quiescent: bool,
    ) -> StartObservation {
        StartObservation {
            state,
            runtime_handle: RuntimeHandle(json!({"h": 1})),
            ambiguous,
            detail: None,
            terminal_confirmed: terminal,
            quiescent_confirmed: quiescent,
        }
    }

    fn physical(state: PhysicalState) -> PhysicalExecutionOutcome {
        PhysicalExecutionOutcome {
            physical_state: state,
            exit_status: None,
            artifact_refs: None,
            diagnostic: None,
        }
    }

    #[test]
    fn exact_running_is_the_only_start_kind_that_may_grant() {
        assert_eq!(
            normalize_start_observation(&start(ExecutionState::Running, false, false, false)),
            StartObservationKind::ExactRunning
        );
    }

    #[test]
    fn contradictory_running_is_unresolved_protocol_failure() {
        let mut obs = start(ExecutionState::Running, false, true, false);
        assert_eq!(
            normalize_start_observation(&obs),
            StartObservationKind::Unresolved {
                failure_class: FailureClass::AdapterProtocolFailure
            }
        );
        obs.terminal_confirmed = false;
        obs.quiescent_confirmed = true;
        assert_eq!(
            normalize_start_observation(&obs),
            StartObservationKind::Unresolved {
                failure_class: FailureClass::AdapterProtocolFailure
            }
        );
        obs.quiescent_confirmed = false;
    }

    #[test]
    fn ambiguous_and_unresolved_states_never_grant() {
        for state in [
            ExecutionState::Starting,
            ExecutionState::Unknown,
            ExecutionState::Lost,
        ] {
            assert_eq!(
                normalize_start_observation(&start(state, false, false, false)),
                StartObservationKind::Unresolved {
                    failure_class: FailureClass::ExecutionLost
                }
            );
        }
        assert_eq!(
            normalize_start_observation(&start(ExecutionState::Running, true, false, false)),
            StartObservationKind::Unresolved {
                failure_class: FailureClass::ExecutionLost
            }
        );
    }

    #[test]
    fn unknown_to_running_is_exact_running_on_the_observation() {
        // Physical-history UNKNOWN → RUNNING is legal; the classifier only
        // sees the observation, not the persisted row.
        assert_eq!(
            normalize_start_observation(&start(ExecutionState::Running, false, false, false)),
            StartObservationKind::ExactRunning
        );
    }

    #[test]
    fn terminal_looking_start_requires_collect() {
        assert_eq!(
            normalize_start_observation(&start(ExecutionState::Succeeded, false, true, true)),
            StartObservationKind::TerminalCandidate
        );
        assert_eq!(
            normalize_start_observation(&start(ExecutionState::Failed, false, true, false)),
            StartObservationKind::TerminalCandidate
        );
        assert_eq!(
            normalize_start_observation(&start(ExecutionState::Terminated, false, false, false)),
            StartObservationKind::TerminalCandidate
        );
    }

    #[test]
    fn collected_physical_exit_is_not_task_result() {
        assert_eq!(
            normalize_collected_outcome(&physical(PhysicalState::Exited)),
            CollectedOutcomeKind::PhysicalEnded
        );
        assert_eq!(
            normalize_collected_outcome(&physical(PhysicalState::Terminated)),
            CollectedOutcomeKind::PhysicalEnded
        );
        assert_eq!(
            normalize_collected_outcome(&physical(PhysicalState::Unknown)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::ExecutionLost
            }
        );
    }

    #[test]
    fn nonterminal_collect_cannot_inherit_reconcile_proof() {
        assert_eq!(
            normalize_collected_outcome(&physical(PhysicalState::Unknown)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::ExecutionLost
            }
        );
    }

    #[test]
    fn adapter_error_taxonomy() {
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::unavailable("x")),
            FailureClass::ResourceUnavailable
        );
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::deadline_exceeded("x")),
            FailureClass::Timeout
        );
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::protocol("x")),
            FailureClass::AdapterProtocolFailure
        );
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::other("x")),
            FailureClass::Unknown
        );
    }

    /// M5.6 §27/#26-27: a generic invocation error is UNKNOWN, never
    /// START_FAILURE; START_FAILURE remains reserved for positively
    /// collected terminal failure without an explicit class.
    #[test]
    fn invocation_other_is_unknown_and_start_failure_needs_positive_proof() {
        let class = adapter_invocation_failure_class(&AdapterError::other("opaque failure"));
        assert_eq!(class, FailureClass::Unknown);
        assert_ne!(class, FailureClass::StartFailure);
        assert_eq!(
            normalize_collected_outcome(&physical(PhysicalState::Unknown)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::ExecutionLost
            }
        );
    }

    /// M5.6 §25/#29: `WRITER_QUIESCENCE_UNKNOWN` is Scheduler-derived. An
    /// adapter supplying it in a start observation or collected outcome is
    /// rejected as an adapter protocol failure — writer-safety escalation
    /// stays exclusively Scheduler policy.
    #[test]
    fn adapter_cannot_author_scheduler_failure_class() {
        // ExecutionOutcome no longer carries FailureClass. Terminal Failed
        // is classified by Runtime as StartFailure; writer-safety remains
        // Scheduler-only (invocation Protocol still maps via AdapterError).
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::protocol(
                "scheduler-derived failure_class is not adapter-authored"
            )),
            FailureClass::AdapterProtocolFailure
        );
        assert_eq!(
            normalize_collected_outcome(&physical(PhysicalState::Exited)),
            CollectedOutcomeKind::PhysicalEnded
        );
    }
}
