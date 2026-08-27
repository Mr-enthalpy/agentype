//! Pure scheduling / transition decisions.
//!
//! Spec 15 (normative): storage persists core state; it MUST NOT define
//! scheduling semantics. Every scheduling decision lives here as a
//! deterministic, SQLite-free function. `agentype-storage-sqlite` loads
//! authoritative rows, invokes these functions inside its transaction, and
//! persists the results atomically.

use crate::records::RetryPolicy;
use crate::states::{
    BatchState, ContinuityPreference, ExecutionState, FailureClass, LogicalAgentState, TaskState,
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

// ---------------------------------------------------------------------------
// Claim selection over snapshots (spec 15: storage loads rows; core decides
// eligibility, ordering, and selection — coarse SQL filtering is a performance
// optimization only and MUST NOT change scheduler behavior).
// ---------------------------------------------------------------------------

/// Task-side snapshot for claim selection.
#[derive(Clone, Debug)]
pub struct ClaimTaskSnapshot {
    pub id: String,
    pub state: TaskState,
    pub batch_state: BatchState,
    pub partition_active: bool,
    pub next_eligible_at: Option<f64>,
    pub priority: i64,
    pub created_at: f64,
}

/// Semantic claimability of a Task. Storage may pre-filter cheaply; this is
/// the authoritative re-check so that query text alone cannot change behavior.
pub fn claim_task_eligible(s: &ClaimTaskSnapshot, now: f64) -> bool {
    s.state == TaskState::Queued
        && s.batch_state == BatchState::Active
        && s.partition_active
        && s.next_eligible_at.map_or(true, |t| t <= now)
}

/// Frozen task order for claiming: highest priority, oldest submission, then
/// lowest Task ID. Returns ids in deterministic visiting order.
pub fn order_claim_tasks(tasks: &[ClaimTaskSnapshot], now: f64) -> Vec<String> {
    let mut eligible: Vec<&ClaimTaskSnapshot> =
        tasks.iter().filter(|t| claim_task_eligible(t, now)).collect();
    eligible.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.created_at.partial_cmp(&b.created_at).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.id.cmp(&b.id))
    });
    eligible.into_iter().map(|t| t.id.clone()).collect()
}

/// Agent-side snapshot for claim selection.
#[derive(Clone, Debug)]
pub struct ClaimAgentSnapshot {
    pub id: String,
    pub state: LogicalAgentState,
    pub assigned_to_task: bool,
    pub partition: String,
    pub workstream_id: Option<String>,
    pub tags: Vec<String>,
    pub available_since: Option<f64>,
    pub created_at: f64,
}

/// What a claimable Task demands from its consumer at selection time.
#[derive(Clone, Debug)]
pub struct ClaimIntent<'a> {
    pub partition: &'a str,
    pub required_tags: &'a [String],
    pub workstream_id: Option<&'a str>,
    pub continuity: ContinuityPreference,
}

/// Select the single best consumer for a task per the frozen matching rules:
/// exact partition, READY + unassigned, tag subset, continuity gate, then
/// rank (workstream-aware placement) with availability/id tiebreaks.
pub fn select_claim_agent<'a>(
    agents: &'a [ClaimAgentSnapshot],
    intent: &ClaimIntent<'_>,
) -> Option<&'a ClaimAgentSnapshot> {
    agents
        .iter()
        .filter(|a| {
            a.state == LogicalAgentState::Ready
                && !a.assigned_to_task
                && a.partition == intent.partition
                && crate::authority::tags_match(intent.required_tags, &a.tags)
        })
        .filter(|a| {
            let same_ws = intent.workstream_id.is_some()
                && Some(intent.workstream_id.unwrap()) == a.workstream_id.as_deref();
            !(intent.continuity == ContinuityPreference::Required && !same_ws)
        })
        .min_by(|a, b| {
            let same_a = intent.workstream_id.is_some()
                && Some(intent.workstream_id.unwrap()) == a.workstream_id.as_deref();
            let same_b = intent.workstream_id.is_some()
                && Some(intent.workstream_id.unwrap()) == b.workstream_id.as_deref();
            claim_selection_rank(intent.continuity, same_a)
                .cmp(&claim_selection_rank(intent.continuity, same_b))
                .then_with(|| {
                    let (ka, ia) = claim_tiebreak(a.available_since, a.created_at, &a.id);
                    let (kb, ib) = claim_tiebreak(b.available_since, b.created_at, &b.id);
                    ka.partial_cmp(&kb)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(ia.cmp(ib))
                })
        })
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

// ---------------------------------------------------------------------------
// Pool reconciliation / MOVE_CAPACITY selection and planning.
// ---------------------------------------------------------------------------

/// Membership snapshot used by pool/topology ranking. `retirement_requested`
/// members never participate in either ranking (drain-retire is already
/// underway); storage may pre-filter, core revalidates.
#[derive(Clone, Debug)]
pub struct PoolMemberSnapshot {
    pub id: String,
    pub state: LogicalAgentState,
    pub assigned_to_task: bool,
    pub retirement_requested: bool,
    pub available_since: Option<f64>,
    pub created_at: f64,
}

/// Excess-shrink ordering (V0.1 parity): prefer to retire unassigned members,
/// READY before the other live states, then lowest ID as the final tiebreak.
pub fn excess_rank_key(s: &PoolMemberSnapshot) -> (bool, bool, &str) {
    (s.assigned_to_task, s.state != LogicalAgentState::Ready, s.id.as_str())
}

/// Sort excess candidates in place into visit order for capacity shrink.
pub fn sort_excess_candidates(members: &mut [PoolMemberSnapshot]) {
    members.sort_by(|a, b| excess_rank_key(a).cmp(&excess_rank_key(b)));
}

/// MOVE Capacity candidate eligibility (semantic re-check over the coarse SQL
/// pre-filter): a member counts while no drain-retirement was requested.
pub fn move_candidate_eligible(s: &PoolMemberSnapshot) -> bool {
    !s.retirement_requested
}

/// MOVE ordering (V0.1 parity): idle READY members move first (zero-cost
/// cutover), then oldest availability, then lowest ID.
pub fn move_rank_key(s: &PoolMemberSnapshot) -> (i32, f64, &str) {
    let idle_ready =
        s.state == LogicalAgentState::Ready && !s.assigned_to_task;
    (
        if idle_ready { 0 } else { 1 },
        s.available_since.unwrap_or(s.created_at),
        s.id.as_str(),
    )
}

/// Sort move candidates in place into visit order for capacity transfer.
pub fn sort_move_candidates(members: &mut [PoolMemberSnapshot]) {
    members.sort_by(|a, b| {
        move_rank_key(a)
            .partial_cmp(&move_rank_key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// The topology action planned for one moved member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveCutoverPlan {
    /// Member holds (or is transitioning out of) a Task: stage the desired
    /// partition under DRAINING; cutover commits at the release boundary.
    StageDrain,
    /// Member has nothing in flight: cut over immediately, restoring READY
    /// availability when it had been mid-drain with an idle hand-off.
    ReconnectCutover { restore_ready: bool },
}

pub fn plan_move_cutover(state: LogicalAgentState, assigned_to_task: bool) -> MoveCutoverPlan {
    if assigned_to_task || state == LogicalAgentState::Assigned {
        MoveCutoverPlan::StageDrain
    } else {
        MoveCutoverPlan::ReconnectCutover {
            restore_ready: state == LogicalAgentState::Draining,
        }
    }
}

/// Whether an attempt may mechanically retry under the Task's frozen policy.
pub fn retry_allowed(policy: &RetryPolicy, class: FailureClass, attempt_number: u32) -> bool {
    policy.allows(class, attempt_number)
}

/// Backoff delay applied before the next attempt becomes eligible.
pub fn retry_backoff_seconds(policy: &RetryPolicy, attempt_number: u32) -> f64 {
    policy.delay_for_attempt(attempt_number)
}
