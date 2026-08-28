//! Pure scheduling / transition decisions.
//!
//! Spec 15 (normative): storage persists core state; it MUST NOT define
//! scheduling semantics. Every scheduling decision lives here as a
//! deterministic, SQLite-free function. `agentype-storage-sqlite` loads
//! authoritative rows, invokes these functions inside its transaction, and
//! persists the results atomically.

use crate::errors::Error;
use crate::records::RetryPolicy;
use crate::states::{
    AttemptState, BatchState, ContinuityPreference, ExecutionState, FailureClass, LeaseState,
    LogicalAgentState, Retention, TaskState, WorkspaceMode,
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
pub fn claim_tiebreak(available_since: Option<f64>, created_at: f64, id: &str) -> (f64, &str) {
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
        && s.next_eligible_at.is_none_or(|t| t <= now)
}

/// Frozen task order for claiming: highest priority, oldest submission, then
/// lowest Task ID. Returns ids in deterministic visiting order.
pub fn order_claim_tasks(tasks: &[ClaimTaskSnapshot], now: f64) -> Vec<String> {
    let mut eligible: Vec<&ClaimTaskSnapshot> = tasks
        .iter()
        .filter(|t| claim_task_eligible(t, now))
        .collect();
    eligible.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(
                a.created_at
                    .partial_cmp(&b.created_at)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
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
            intent.continuity != ContinuityPreference::Required || same_ws
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
        ExecutionState::Starting | ExecutionState::Unknown => return PresenceAction::Ignore,
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
    (
        s.assigned_to_task,
        s.state != LogicalAgentState::Ready,
        s.id.as_str(),
    )
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
    let idle_ready = s.state == LogicalAgentState::Ready && !s.assigned_to_task;
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

// ---------------------------------------------------------------------------
// Cross-target execution safety (spec 15: pure safety predicate).
// ---------------------------------------------------------------------------

/// Execution snapshot used to evaluate cross-target cutover / abandonment safety.
#[derive(Clone, Debug, PartialEq)]
pub struct CrossTargetExecutionSnapshot {
    pub attempt_state: AttemptState,
    pub lease_state: Option<LeaseState>,
    pub lease_expires_at: Option<f64>,
    pub workspace_mode: WorkspaceMode,
    pub attempt_isolation: bool,
    pub quiescent_confirmed: bool,
}

/// Cross-target safety gate: whether an execution on a foreign target is safe
/// to abandon. An execution is safe if its authority is definitively closed
/// (non-ACTIVE attempt, or absent/non-ACTIVE/expired lease) AND it is safe for
/// writers (read-only workspace, isolated attempt, or confirmed quiescent).
pub fn is_cross_target_execution_safe(exec: &CrossTargetExecutionSnapshot, now: f64) -> bool {
    let authority_closed = exec.attempt_state != AttemptState::Active
        && (exec.lease_state.is_none()
            || exec.lease_state != Some(LeaseState::Active)
            || exec.lease_expires_at.unwrap_or(0.0) <= now);
    let writer_ok = exec.workspace_mode == WorkspaceMode::ReadOnly
        || exec.attempt_isolation
        || exec.quiescent_confirmed;
    authority_closed && writer_ok
}

/// Evaluates whether a set of cross-target executions are ALL safe to abandon.
/// Returns `true` if safe, `false` if any execution is unsafe.
pub fn cross_target_cutover_safety(executions: &[CrossTargetExecutionSnapshot], now: f64) -> bool {
    executions
        .iter()
        .all(|e| is_cross_target_execution_safe(e, now))
}

// ---------------------------------------------------------------------------
// Partition cutover disposition / planning.
// ---------------------------------------------------------------------------

/// The topology disposition for a partition cutover request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionCutoverDisposition {
    /// Cutover can commit immediately: update partition, retention, clear pending, lose foreign incarnations.
    Commit,
    /// Unsafe execution on suspended idle agent: stage pending destination until safety resolution.
    StagePendingDestination,
    /// Agent currently has active attempt: must drain via task completion boundary.
    RejectAssignedDrainRequired,
    /// Unsafe physical execution cannot be abandoned immediately.
    RejectUnsafeExecution,
}

/// Decides how a partition cutover request should proceed based on agent state,
/// active assignment, and physical cross-target execution safety.
pub fn partition_cutover_plan(
    agent_state: LogicalAgentState,
    has_active_attempt: bool,
    has_current_task: bool,
    cross_target_safe: bool,
) -> PartitionCutoverDisposition {
    if has_active_attempt {
        return PartitionCutoverDisposition::RejectAssignedDrainRequired;
    }
    if !cross_target_safe {
        if agent_state == LogicalAgentState::Suspended && !has_current_task {
            return PartitionCutoverDisposition::StagePendingDestination;
        }
        return PartitionCutoverDisposition::RejectUnsafeExecution;
    }
    PartitionCutoverDisposition::Commit
}

// ---------------------------------------------------------------------------
// Agent lifecycle release & post-safety revival policy.
// ---------------------------------------------------------------------------

/// Lifecycle disposition when releasing a LogicalAgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentReleaseDisposition {
    Retire,
    BecomeReady,
}

/// Determines the lifecycle transition when releasing a LogicalAgent.
pub fn agent_release_disposition(
    retirement_requested: bool,
    target_retention: Retention,
) -> AgentReleaseDisposition {
    if retirement_requested || target_retention == Retention::Ephemeral {
        AgentReleaseDisposition::Retire
    } else {
        AgentReleaseDisposition::BecomeReady
    }
}

/// Disposition of an agent following safety resolution (e.g. escalation resolve).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostSafetyAgentDisposition {
    /// Agent is already RETIRED; no further transition.
    NoAction,
    /// Agent becomes READY per release policy and should be promoted to REVIVING.
    Revive,
    /// Agent is retired per release policy (e.g. ephemeral or retirement_requested).
    Retire,
}

/// Determines the disposition of an agent following safety resolution.
pub fn post_safety_agent_disposition(
    current_state: LogicalAgentState,
    retirement_requested: bool,
    target_retention: Retention,
) -> PostSafetyAgentDisposition {
    if current_state == LogicalAgentState::Retired {
        return PostSafetyAgentDisposition::NoAction;
    }
    match agent_release_disposition(retirement_requested, target_retention) {
        AgentReleaseDisposition::Retire => PostSafetyAgentDisposition::Retire,
        AgentReleaseDisposition::BecomeReady => PostSafetyAgentDisposition::Revive,
    }
}

// ---------------------------------------------------------------------------
// Dependency scheduling (spec 15: pure dependency evaluation).
// ---------------------------------------------------------------------------

/// Snapshot of a BLOCKED task and the states of its prerequisite parent tasks.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockedTaskSnapshot {
    pub task_id: String,
    pub parent_states: Vec<TaskState>,
}

/// Evaluates whether a BLOCKED task's dependencies are all satisfied (all parents are COMPLETED).
pub fn dependency_release_decision(parent_states: &[TaskState]) -> bool {
    parent_states
        .iter()
        .all(|&state| state == TaskState::Completed)
}

/// Given candidate blocked tasks and their dependency parent states, returns the task IDs
/// that are unblocked and eligible to transition from BLOCKED to QUEUED.
pub fn plan_dependency_releases(blocked_tasks: &[BlockedTaskSnapshot]) -> Vec<String> {
    blocked_tasks
        .iter()
        .filter(|t| dependency_release_decision(&t.parent_states))
        .map(|t| t.task_id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Escalation resolution scheduling (spec 15: pure escalation recovery plan).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscalationOperation {
    Retry,
    CancelTask,
    ReleaseCancelledWriter,
}

impl EscalationOperation {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "retry" => Ok(Self::Retry),
            "cancel_task" => Ok(Self::CancelTask),
            "release_cancelled_writer" => Ok(Self::ReleaseCancelledWriter),
            other => Err(Error::invalid_transition(format!(
                "unsupported escalation operation '{other}'; supported operations are retry, cancel_task, and release_cancelled_writer"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationResolutionSnapshot {
    pub escalation_is_open: bool,
    pub failure_class: FailureClass,
    pub task_state: TaskState,
    pub workspace_mode: WorkspaceMode,
    pub frozen_isolation: bool,
    pub has_agent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscalatedWriterPresenceAction {
    None,
    FinalizePresence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationResolutionPlan {
    ReleaseCancelledWriter {
        writer_presence: EscalatedWriterPresenceAction,
        revive_agent: bool,
        resolve_escalation: bool,
    },
    Retry {
        next_task_state: TaskState,
        reactivate_batch: bool,
        writer_presence: EscalatedWriterPresenceAction,
        revive_agent: bool,
        resolve_escalation: bool,
    },
    CancelTask {
        next_task_state: TaskState,
        resolve_escalation: bool,
        recompute_batch_only: bool,
    },
}

pub fn plan_escalation_resolution(
    snap: &EscalationResolutionSnapshot,
    op: EscalationOperation,
    quiescence_confirmed: bool,
) -> Result<EscalationResolutionPlan, Error> {
    if !snap.escalation_is_open {
        return Err(Error::invalid_transition("escalation is not open"));
    }
    let is_writer_safe = quiescence_confirmed || snap.frozen_isolation;
    match op {
        EscalationOperation::ReleaseCancelledWriter => {
            if snap.task_state != TaskState::Cancelled
                || snap.failure_class != FailureClass::WriterQuiescenceUnknown
                || !is_writer_safe
            {
                return Err(Error::invalid_transition(
                    "cancelled writer release requires confirmed quiescence or attempt isolation",
                ));
            }
            Ok(EscalationResolutionPlan::ReleaseCancelledWriter {
                writer_presence: EscalatedWriterPresenceAction::FinalizePresence,
                revive_agent: snap.has_agent,
                resolve_escalation: true,
            })
        }
        EscalationOperation::Retry => {
            if snap.task_state != TaskState::Suspended {
                return Err(Error::invalid_transition(
                    "only a suspended task can be retried",
                ));
            }
            if snap.workspace_mode == WorkspaceMode::Write
                && snap.failure_class == FailureClass::WriterQuiescenceUnknown
                && !is_writer_safe
            {
                return Err(Error::invalid_transition(
                    "writer retry requires confirmed quiescence or attempt isolation",
                ));
            }
            let is_writer_unknown = snap.failure_class == FailureClass::WriterQuiescenceUnknown;
            Ok(EscalationResolutionPlan::Retry {
                next_task_state: TaskState::Queued,
                reactivate_batch: true,
                writer_presence: if is_writer_unknown {
                    EscalatedWriterPresenceAction::FinalizePresence
                } else {
                    EscalatedWriterPresenceAction::None
                },
                revive_agent: is_writer_unknown && snap.has_agent,
                resolve_escalation: true,
            })
        }
        EscalationOperation::CancelTask => {
            let is_writer_unknown = snap.failure_class == FailureClass::WriterQuiescenceUnknown;
            Ok(EscalationResolutionPlan::CancelTask {
                next_task_state: TaskState::Cancelled,
                resolve_escalation: !is_writer_unknown,
                recompute_batch_only: is_writer_unknown,
            })
        }
    }
}
