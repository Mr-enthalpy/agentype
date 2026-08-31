//! Shared start-observation and collected-outcome classification (M5.4-C).
//!
//! Dispatch and restart reconciliation MUST interpret the same adapter
//! evidence the same way. These functions are pure: they do not touch
//! Kernel state and they never ACK/NACK. Callers persist and apply
//! authority consequences through the existing fenced primitives.

use agentype_adapter_api::{AdapterError, ExecutionOutcome, StartObservation};
use agentype_core::{ExecutionState, FailureClass};

/// Mechanical normalization of adapter invocation errors into the existing
/// `FailureClass` vocabulary (M5.2 task §14 / M5.4 plan §14). Vendor-specific
/// classification belongs inside adapter implementations; no provider strings
/// are parsed at the runtime or core layer.
pub fn adapter_invocation_failure_class(err: &AdapterError) -> FailureClass {
    match err {
        AdapterError::Unavailable(_) => FailureClass::ResourceUnavailable,
        AdapterError::DeadlineExceeded(_) => FailureClass::Timeout,
        AdapterError::Protocol(_) => FailureClass::AdapterProtocolFailure,
        AdapterError::Other(_) => FailureClass::StartFailure,
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

/// What a collected `ExecutionOutcome` means. `collect_outcome` is the
/// ACK/NACK proof authority (spec 07); this classifier does not mutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectedOutcomeKind {
    /// `SUCCEEDED` with terminal proof. The caller MAY ACK; writer safety
    /// still decides whether a Result is created.
    TerminalSuccess,
    /// Terminal non-success. The caller MAY NACK with terminal proof bits.
    TerminalFailure { failure_class: FailureClass },
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
        && (observation.terminal_confirmed
            || observation.quiescent_confirmed
            || observation.failure_class.is_some())
    {
        return StartObservationKind::Unresolved {
            failure_class: FailureClass::AdapterProtocolFailure,
        };
    }
    if observation.state == ExecutionState::Running && !observation.ambiguous {
        return StartObservationKind::ExactRunning;
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
pub fn normalize_collected_outcome(outcome: &ExecutionOutcome) -> CollectedOutcomeKind {
    if outcome.terminal_confirmed && outcome.state.is_active_physical() {
        return CollectedOutcomeKind::Unresolved {
            failure_class: FailureClass::AdapterProtocolFailure,
        };
    }
    if outcome.state == ExecutionState::Lost
        && (outcome.terminal_confirmed || outcome.quiescent_confirmed)
    {
        return CollectedOutcomeKind::Unresolved {
            failure_class: FailureClass::AdapterProtocolFailure,
        };
    }
    if outcome.state == ExecutionState::Succeeded && !outcome.terminal_confirmed {
        return CollectedOutcomeKind::Unresolved {
            failure_class: FailureClass::InvalidResult,
        };
    }
    if outcome.quiescent_confirmed && !outcome.terminal_confirmed {
        return CollectedOutcomeKind::Unresolved {
            failure_class: FailureClass::AdapterProtocolFailure,
        };
    }
    if outcome.terminal_confirmed {
        if outcome.state == ExecutionState::Succeeded {
            return CollectedOutcomeKind::TerminalSuccess;
        }
        return CollectedOutcomeKind::TerminalFailure {
            failure_class: outcome.failure_class.unwrap_or(FailureClass::StartFailure),
        };
    }
    CollectedOutcomeKind::Unresolved {
        failure_class: outcome.failure_class.unwrap_or(FailureClass::ExecutionLost),
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
            failure_class: None,
            detail: None,
            terminal_confirmed: terminal,
            quiescent_confirmed: quiescent,
        }
    }

    fn outcome(
        state: ExecutionState,
        terminal: bool,
        quiescent: bool,
        failure_class: Option<FailureClass>,
    ) -> ExecutionOutcome {
        ExecutionOutcome {
            state,
            payload: None,
            summary: None,
            failure_class,
            terminal_confirmed: terminal,
            quiescent_confirmed: quiescent,
            incarnation_reusable: false,
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
        obs.failure_class = Some(FailureClass::Timeout);
        assert_eq!(
            normalize_start_observation(&obs),
            StartObservationKind::Unresolved {
                failure_class: FailureClass::AdapterProtocolFailure
            }
        );
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
    }

    #[test]
    fn collected_success_and_failure() {
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Succeeded, true, true, None)),
            CollectedOutcomeKind::TerminalSuccess
        );
        assert_eq!(
            normalize_collected_outcome(&outcome(
                ExecutionState::Failed,
                true,
                true,
                Some(FailureClass::Timeout)
            )),
            CollectedOutcomeKind::TerminalFailure {
                failure_class: FailureClass::Timeout
            }
        );
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Terminated, true, true, None)),
            CollectedOutcomeKind::TerminalFailure {
                failure_class: FailureClass::StartFailure
            }
        );
    }

    #[test]
    fn collected_contradictions_are_unresolved() {
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Running, true, true, None)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::AdapterProtocolFailure
            }
        );
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Lost, true, true, None)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::AdapterProtocolFailure
            }
        );
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Succeeded, false, false, None)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::InvalidResult
            }
        );
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Unknown, false, true, None)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::AdapterProtocolFailure
            }
        );
    }

    #[test]
    fn nonterminal_collect_cannot_inherit_reconcile_proof() {
        assert_eq!(
            normalize_collected_outcome(&outcome(ExecutionState::Unknown, false, false, None)),
            CollectedOutcomeKind::Unresolved {
                failure_class: FailureClass::ExecutionLost
            }
        );
    }

    #[test]
    fn adapter_error_taxonomy() {
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::Unavailable("x".into())),
            FailureClass::ResourceUnavailable
        );
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::DeadlineExceeded("x".into())),
            FailureClass::Timeout
        );
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::Protocol("x".into())),
            FailureClass::AdapterProtocolFailure
        );
        assert_eq!(
            adapter_invocation_failure_class(&AdapterError::Other("x".into())),
            FailureClass::StartFailure
        );
    }
}
