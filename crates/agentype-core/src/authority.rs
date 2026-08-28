//! Pure authority predicates. Storage applies them inside transactions.

use crate::ids::{AttemptId, LeaseEpoch};
use crate::states::{AttemptState, ExecutionState, LeaseState, TaskState};
use crate::UnixTime;
use crate::{CoreResult, Error};

/// Task create does not grant execution authority. Claim does.
pub fn task_create_establishes_authority() -> bool {
    false
}

#[derive(Clone, Debug)]
pub struct AuthoritySnapshot {
    pub attempt_id: AttemptId,
    pub attempt_state: AttemptState,
    pub lease_state: LeaseState,
    pub lease_epoch: LeaseEpoch,
    pub lease_expires_at: UnixTime,
    pub task_current_attempt_id: Option<AttemptId>,
    pub task_fencing_epoch: LeaseEpoch,
}

/// Authority is current Attempt + ACTIVE unexpired Lease + matching epoch
/// + task.current_attempt_id. Expiry is evaluated against `now`, not a sweeper.
pub fn validate_authority(
    snap: &AuthoritySnapshot,
    expected_epoch: LeaseEpoch,
    now: UnixTime,
) -> CoreResult<()> {
    if snap.attempt_state != AttemptState::Active
        || snap.lease_state != LeaseState::Active
        || snap.lease_epoch != expected_epoch
        || snap.task_current_attempt_id.as_ref() != Some(&snap.attempt_id)
        || snap.task_fencing_epoch != expected_epoch
        || snap.lease_expires_at <= now
    {
        return Err(Error::stale(
            "attempt no longer owns authoritative task state",
        ));
    }
    Ok(())
}

/// Physical Execution graph. UNKNOWN → RUNNING is required.
pub fn physical_transition_allowed(from: ExecutionState, to: ExecutionState) -> bool {
    use ExecutionState::*;
    match from {
        Starting => matches!(
            to,
            Starting | Running | Succeeded | Failed | Lost | Unknown | Terminated
        ),
        Unknown => matches!(
            to,
            Unknown | Running | Succeeded | Failed | Lost | Terminated
        ),
        Running => matches!(to, Running | Succeeded | Failed | Lost | Terminated),
        Lost => matches!(to, Lost | Succeeded | Failed | Terminated),
        Succeeded => matches!(to, Succeeded | Terminated),
        Failed => matches!(to, Failed | Terminated),
        Terminated => to == Terminated,
    }
}

pub fn require_physical_transition(from: ExecutionState, to: ExecutionState) -> CoreResult<()> {
    if physical_transition_allowed(from, to) {
        Ok(())
    } else {
        Err(Error::invalid_transition(format!(
            "execution cannot transition from {} to {}",
            from.as_sql(),
            to.as_sql()
        )))
    }
}

/// Writer replacement is allowed only with terminal/quiescent confirmation
/// or frozen attempt_isolation. Lease expiry is not quiescence.
pub fn writer_is_safe_to_replace(
    workspace_write: bool,
    execution_exists: bool,
    quiescent_confirmed: bool,
    attempt_isolation: bool,
) -> bool {
    !workspace_write || !execution_exists || quiescent_confirmed || attempt_isolation
}

pub fn completed_task_must_not_reopen(state: TaskState) -> bool {
    state.is_terminal()
}

/// Task tags are a subset of agent tags.
pub fn tags_match(required: &[String], actual: &[String]) -> bool {
    required.iter().all(|t| actual.iter().any(|a| a == t))
}

/// Missing runtime/target configuration is a mechanical failure, not a panic.
pub fn unavailable_configuration_failure() -> crate::FailureClass {
    crate::FailureClass::ResourceUnavailable
}
