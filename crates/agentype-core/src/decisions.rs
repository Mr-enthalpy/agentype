//! Pure scheduling / transition decisions.
//!
//! Spec 15 (normative): storage persists core state; it MUST NOT define
//! scheduling semantics. Every scheduling decision lives here as a
//! deterministic, SQLite-free function. `agentype-storage-sqlite` loads
//! authoritative rows, invokes these functions inside its transaction, and
//! persists the results atomically.

use crate::records::RetryPolicy;
use crate::states::{
    BatchState, ContinuityPreference, ExecutionState, FailureClass, LogicalAgentState,
};

/// Frozen claim-matching rank (V0.1 parity): workstream-aware placement beats
/// generic readiness. Remaining tiebreaks (oldest availability, then lowest
/// LogicalAgent ID) are applied on top by [`claim_tiebreak`].
pub fn claim_selection_rank(preference: ContinuityPreference, same_workstream: bool) -> i32 {
    if preference != ContinuityPreference::None && same_workstream {
        0
    } else {
        1
    }
}

/// Deterministic claim tiebreak after [`claim_selection_rank`] ties: oldest
/// availability first (`created_at` is the defensive fallback for a NULL
/// `available_since`), then lowest LogicalAgent ID.
pub fn claim_tiebreak<'a>(
    available_since: Option<f64>,
    created_at: f64,
    id: &'a str,
) -> (f64, &'a str) {
    (available_since.unwrap_or(created_at), id)
}

/// Only a terminal observation constitutes durable quiescence proof. A
/// nonterminal record must persist zero proof bits AND must not unlock writer
/// replacement: after a crash there is no live lease left to re-run the
/// writer-safety gate over an execution whose own durable truth says
/// quiescence-unknown.
pub fn durable_quiescence(terminal_confirmed: bool, quiescent_confirmed: bool) -> bool {
    terminal_confirmed && quiescent_confirmed
}

/// Mechanical failure class carried by a suspension of active authority: the
/// observed class when replacement is safe, otherwise the physical-safety
/// obligation that blocks automatic recovery.
pub fn suspension_failure_class(writer_safe: bool, observed: FailureClass) -> FailureClass {
    if writer_safe {
        observed
    } else {
        FailureClass::WriterQuiescenceUnknown
    }
}

/// Aggregate Batch state recomputed after any member-task transition. A
/// suspended or cancelled member suspends the aggregate; otherwise completion
/// of every member completes the batch.
pub fn batch_next_state(
    has_suspended: bool,
    has_cancelled: bool,
    all_completed: bool,
) -> BatchState {
    if has_suspended || has_cancelled {
        BatchState::Suspended
    } else if all_completed {
        BatchState::Completed
    } else {
        BatchState::Active
    }
}

/// Disposition of one member when partition capacity shrinks below population.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcessDisposition {
    /// Unassigned INITIALIZING / READY / REVIVING members retire directly,
    /// fencing their live physical presence in the same transaction.
    RetireDirectly,
    /// Everyone else (or anyone holding a Task) drains under retirement.
    DrainForRetirement,
}

pub fn excess_disposition(state: LogicalAgentState, assigned_to_task: bool) -> ExcessDisposition {
    let idle_unassigned = matches!(
        state,
        LogicalAgentState::Ready | LogicalAgentState::Initializing | LogicalAgentState::Reviving
    ) && !assigned_to_task;
    if idle_unassigned {
        ExcessDisposition::RetireDirectly
    } else {
        ExcessDisposition::DrainForRetirement
    }
}

/// What an Incarnation does when an Execution reports a physical observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceAction {
    /// Physical proof of life (RUNNING) or a declared-reusable quiet end
    /// promotes the presence back to WARM (with `ended_at` cleared).
    PromoteWarm,
    /// Terminal + confirmed-quiet end (from anything other than LOST).
    FenceTerminated,
    /// Everything unproven fences the presence LOST while it was live.
    FenceLost,
    /// Non-terminal observations leave an already-fenced presence untouched.
    Ignore,
}

pub fn incarnation_presence(
    execution_state: ExecutionState,
    terminal_confirmed: bool,
    quiescent_confirmed: bool,
    incarnation_reusable: bool,
) -> PresenceAction {
    match execution_state {
        ExecutionState::Running => return PresenceAction::PromoteWarm,
        ExecutionState::Starting | ExecutionState::Unknown => {
            return PresenceAction::Ignore
        }
        _ => {}
    }
    if execution_state == ExecutionState::Terminated
        || execution_state == ExecutionState::Lost
        || execution_state == ExecutionState::Failed
        || execution_state == ExecutionState::Succeeded
    {
        if incarnation_reusable && terminal_confirmed && quiescent_confirmed {
            return PresenceAction::PromoteWarm;
        }
        let confirmed_end =
            execution_state != ExecutionState::Lost && terminal_confirmed && quiescent_confirmed;
        if confirmed_end {
            return PresenceAction::FenceTerminated;
        }
        return PresenceAction::FenceLost;
    }
    PresenceAction::Ignore
}

/// Whether an attempt may mechanically retry under the Task's frozen policy.
pub fn retry_allowed(policy: &RetryPolicy, class: FailureClass, attempt_number: u32) -> bool {
    policy.allows(class, attempt_number)
}

/// Backoff delay applied before the next attempt becomes eligible.
pub fn retry_backoff_seconds(policy: &RetryPolicy, attempt_number: u32) -> f64 {
    policy.delay_for_attempt(attempt_number)
}
