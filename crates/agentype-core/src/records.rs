//! Domain records and command payloads. These are semantic, not SQLite rows.

use crate::ids::*;
use crate::states::*;
use crate::UnixTime;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub retry_classes: Vec<FailureClass>,
    pub base_backoff_seconds: f64,
    pub max_backoff_seconds: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            retry_classes: vec![
                FailureClass::TransientExternal,
                FailureClass::Timeout,
                FailureClass::ExecutionLost,
                FailureClass::ResourceUnavailable,
            ],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 60.0,
        }
    }
}

impl RetryPolicy {
    pub fn delay_for_attempt(&self, attempt_number: u32) -> f64 {
        let exponent = attempt_number.saturating_sub(1);
        (self.base_backoff_seconds * 2f64.powi(exponent as i32)).min(self.max_backoff_seconds)
    }

    pub fn allows(&self, class: FailureClass, attempt_number: u32) -> bool {
        self.retry_classes.contains(&class) && attempt_number < self.max_attempts
    }
}

#[derive(Clone, Debug)]
pub struct TaskSpec {
    pub name: String,
    pub payload: Value,
    pub acceptance: Value,
    pub partition: PartitionId,
    pub workstream_id: Option<WorkstreamId>,
    pub continuity: ContinuityPreference,
    pub affinity_tags: Vec<String>,
    pub workspace_mode: WorkspaceMode,
    pub dependencies: Vec<String>,
    pub priority: i64,
    pub retry_policy: RetryPolicy,
    pub supersedes_task_id: Option<TaskId>,
    pub task_id: Option<TaskId>,
}

impl TaskSpec {
    pub fn new(name: impl Into<String>, payload: Value) -> Self {
        Self {
            name: name.into(),
            payload,
            acceptance: Value::Object(Default::default()),
            partition: PartitionId::new("general"),
            workstream_id: None,
            continuity: ContinuityPreference::None,
            affinity_tags: Vec::new(),
            workspace_mode: WorkspaceMode::ReadOnly,
            dependencies: Vec::new(),
            priority: 0,
            retry_policy: RetryPolicy::default(),
            supersedes_task_id: None,
            task_id: None,
        }
    }

    pub fn write(mut self) -> Self {
        self.workspace_mode = WorkspaceMode::Write;
        self
    }

    pub fn partition(mut self, name: impl Into<String>) -> Self {
        self.partition = PartitionId::new(name);
        self
    }

    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    pub fn depends_on(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.dependencies = names.into_iter().map(Into::into).collect();
        self
    }

    pub fn tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.affinity_tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn workstream(mut self, id: WorkstreamId) -> Self {
        self.workstream_id = Some(id);
        self
    }

    pub fn continuity(mut self, c: ContinuityPreference) -> Self {
        self.continuity = c;
        self
    }
}

#[derive(Clone, Debug)]
pub struct PartitionSpec {
    pub name: PartitionId,
    pub desired_capacity: i64,
    pub retention: Retention,
    pub execution_target: String,
    pub execution_profile: String,
    pub tags: Vec<String>,
}

impl PartitionSpec {
    pub fn new(
        name: impl Into<String>,
        desired_capacity: i64,
        retention: Retention,
        execution_target: impl Into<String>,
        execution_profile: impl Into<String>,
    ) -> Self {
        Self {
            name: PartitionId::new(name),
            desired_capacity,
            retention,
            execution_target: execution_target.into(),
            execution_profile: execution_profile.into(),
            tags: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Claim {
    pub task_id: TaskId,
    pub batch_id: BatchId,
    pub attempt_id: AttemptId,
    pub attempt_number: u32,
    pub lease_id: LeaseId,
    pub lease_epoch: LeaseEpoch,
    pub lease_expires_at: UnixTime,
    pub logical_agent_id: LogicalAgentId,
    pub incarnation_id: Option<IncarnationId>,
    pub execution_target: String,
    pub execution_profile: String,
    pub workspace_mode: WorkspaceMode,
    pub payload: Value,
    pub acceptance: Value,
    pub workstream_id: Option<WorkstreamId>,
}

#[derive(Clone, Debug)]
pub struct TaskRecord {
    pub id: TaskId,
    pub batch_id: BatchId,
    pub name: String,
    pub state: TaskState,
    pub partition: PartitionId,
    pub workspace_mode: WorkspaceMode,
    pub fencing_epoch: LeaseEpoch,
    pub current_attempt_id: Option<AttemptId>,
    pub max_attempts: u32,
    pub next_eligible_at: Option<UnixTime>,
}

#[derive(Clone, Debug)]
pub struct BatchRecord {
    pub id: BatchId,
    pub state: BatchState,
}

#[derive(Clone, Debug)]
pub struct AttemptRecord {
    pub id: AttemptId,
    pub task_id: TaskId,
    pub logical_agent_id: LogicalAgentId,
    pub incarnation_id: Option<IncarnationId>,
    pub attempt_number: u32,
    pub lease_epoch: LeaseEpoch,
    pub state: AttemptState,
    pub execution_target: String,
    pub execution_profile: String,
    pub partition_name: PartitionId,
}

#[derive(Clone, Debug)]
pub struct LeaseRecord {
    pub id: LeaseId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub epoch: LeaseEpoch,
    pub state: LeaseState,
    pub expires_at: UnixTime,
}

#[derive(Clone, Debug)]
pub struct ResultRecord {
    pub id: ResultId,
    pub task_id: TaskId,
    pub batch_id: BatchId,
    pub state: ResultState,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct LogicalAgentRecord {
    pub id: LogicalAgentId,
    pub partition: PartitionId,
    pub pending_partition: Option<PartitionId>,
    pub retention: Retention,
    pub state: LogicalAgentState,
    pub current_task_id: Option<TaskId>,
    pub retirement_requested: bool,
}

#[derive(Clone, Debug)]
pub struct IncarnationRecord {
    pub id: IncarnationId,
    pub logical_agent_id: LogicalAgentId,
    pub generation: u32,
    pub execution_target: String,
    pub state: IncarnationState,
}

#[derive(Clone, Debug)]
pub struct ExecutionRecord {
    pub id: ExecutionId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub incarnation_id: IncarnationId,
    pub execution_target: String,
    pub execution_profile: String,
    pub state: ExecutionState,
    pub attempt_isolation: bool,
    pub terminal_confirmed: bool,
    pub quiescent_confirmed: bool,
}

/// Monotonic committed continuity snapshot for an authoritative launch.
#[derive(Clone, Debug, PartialEq)]
pub struct CommittedContinuitySnapshot {
    preference: ContinuityPreference,
    version: i64,
    capsule: Value,
}

impl CommittedContinuitySnapshot {
    pub fn new(preference: ContinuityPreference, version: i64, capsule: Value) -> Self {
        Self {
            preference,
            version,
            capsule,
        }
    }

    pub fn stateless() -> Self {
        Self {
            preference: ContinuityPreference::None,
            version: 0,
            capsule: Value::Null,
        }
    }

    pub fn preference(&self) -> ContinuityPreference {
        self.preference
    }

    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn capsule(&self) -> &Value {
        &self.capsule
    }
}

/// Strongly-typed safety guarantee bound to a specific execution target and profile.
///
/// Ensures that the isolation guarantee cannot be decoupled from the target and profile
/// for which configuration resolution was performed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenExecutionSafety {
    execution_target: String,
    execution_profile: String,
    attempt_isolation: bool,
}

impl FrozenExecutionSafety {
    /// Internal constructor for authoritative configuration resolution within agentype-core.
    pub(crate) fn new(
        execution_target: impl Into<String>,
        execution_profile: impl Into<String>,
        attempt_isolation: bool,
    ) -> Self {
        Self {
            execution_target: execution_target.into(),
            execution_profile: execution_profile.into(),
            attempt_isolation,
        }
    }

    /// Safe constructor for unisolated execution environments (fail-safe default).
    pub fn unisolated(
        execution_target: impl Into<String>,
        execution_profile: impl Into<String>,
    ) -> Self {
        Self::new(execution_target, execution_profile, false)
    }

    pub fn execution_target(&self) -> &str {
        &self.execution_target
    }

    pub fn execution_profile(&self) -> &str {
        &self.execution_profile
    }

    pub fn attempt_isolation(&self) -> bool {
        self.attempt_isolation
    }
}

/// Authoritative launch snapshot reconstructed from durable Scheduler state.
///
/// This object is produced exclusively by the Execution creation transaction and
/// encapsulates all execution parameters as private, readonly fields.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionLaunchSnapshot {
    execution_id: ExecutionId,
    request_id: RequestId,
    task_id: TaskId,
    batch_id: BatchId,
    attempt_id: AttemptId,
    attempt_number: u32,
    lease_id: LeaseId,
    lease_epoch: LeaseEpoch,
    lease_expires_at: UnixTime,
    logical_agent_id: LogicalAgentId,
    incarnation_id: IncarnationId,
    incarnation_runtime_handle: Value,
    execution_target: String,
    execution_profile: String,
    workspace_mode: WorkspaceMode,
    prompt: String,
    payload: Value,
    acceptance: Value,
    workstream_id: Option<WorkstreamId>,
    continuity: CommittedContinuitySnapshot,
    safety: FrozenExecutionSafety,
}

impl ExecutionLaunchSnapshot {
    /// Internal storage-level constructor for Kernel execution creation transactions.
    ///
    /// # Safety
    /// Caller MUST be a fenced Kernel database transaction that has atomically validated
    /// the Attempt, Lease, Task, Agent, and Incarnation records from durable storage.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn from_persisted_kernel_authority(
        execution_id: ExecutionId,
        request_id: RequestId,
        task_id: TaskId,
        batch_id: BatchId,
        attempt_id: AttemptId,
        attempt_number: u32,
        lease_id: LeaseId,
        lease_epoch: LeaseEpoch,
        lease_expires_at: UnixTime,
        logical_agent_id: LogicalAgentId,
        incarnation_id: IncarnationId,
        incarnation_runtime_handle: Value,
        execution_target: String,
        execution_profile: String,
        workspace_mode: WorkspaceMode,
        prompt: String,
        payload: Value,
        acceptance: Value,
        workstream_id: Option<WorkstreamId>,
        continuity: CommittedContinuitySnapshot,
        safety: FrozenExecutionSafety,
    ) -> Self {
        Self {
            execution_id,
            request_id,
            task_id,
            batch_id,
            attempt_id,
            attempt_number,
            lease_id,
            lease_epoch,
            lease_expires_at,
            logical_agent_id,
            incarnation_id,
            incarnation_runtime_handle,
            execution_target,
            execution_profile,
            workspace_mode,
            prompt,
            payload,
            acceptance,
            workstream_id,
            continuity,
            safety,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::too_many_arguments)]
    pub fn for_testing(
        execution_id: ExecutionId,
        request_id: RequestId,
        task_id: TaskId,
        batch_id: BatchId,
        attempt_id: AttemptId,
        attempt_number: u32,
        lease_id: LeaseId,
        lease_epoch: LeaseEpoch,
        lease_expires_at: UnixTime,
        logical_agent_id: LogicalAgentId,
        incarnation_id: IncarnationId,
        incarnation_runtime_handle: Value,
        execution_target: String,
        execution_profile: String,
        workspace_mode: WorkspaceMode,
        prompt: String,
        payload: Value,
        acceptance: Value,
        workstream_id: Option<WorkstreamId>,
        continuity: CommittedContinuitySnapshot,
        safety: FrozenExecutionSafety,
    ) -> Self {
        Self {
            execution_id,
            request_id,
            task_id,
            batch_id,
            attempt_id,
            attempt_number,
            lease_id,
            lease_epoch,
            lease_expires_at,
            logical_agent_id,
            incarnation_id,
            incarnation_runtime_handle,
            execution_target,
            execution_profile,
            workspace_mode,
            prompt,
            payload,
            acceptance,
            workstream_id,
            continuity,
            safety,
        }
    }

    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn batch_id(&self) -> &BatchId {
        &self.batch_id
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }

    pub fn lease_id(&self) -> &LeaseId {
        &self.lease_id
    }

    pub fn lease_epoch(&self) -> LeaseEpoch {
        self.lease_epoch
    }

    pub fn lease_expires_at(&self) -> UnixTime {
        self.lease_expires_at
    }

    pub fn logical_agent_id(&self) -> &LogicalAgentId {
        &self.logical_agent_id
    }

    pub fn incarnation_id(&self) -> &IncarnationId {
        &self.incarnation_id
    }

    pub fn incarnation_runtime_handle(&self) -> &Value {
        &self.incarnation_runtime_handle
    }

    pub fn execution_target(&self) -> &str {
        &self.execution_target
    }

    pub fn execution_profile(&self) -> &str {
        &self.execution_profile
    }

    pub fn workspace_mode(&self) -> WorkspaceMode {
        self.workspace_mode
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }

    pub fn acceptance(&self) -> &Value {
        &self.acceptance
    }

    pub fn workstream_id(&self) -> Option<&WorkstreamId> {
        self.workstream_id.as_ref()
    }

    pub fn continuity(&self) -> &CommittedContinuitySnapshot {
        &self.continuity
    }

    pub fn safety(&self) -> &FrozenExecutionSafety {
        &self.safety
    }

    pub fn attempt_isolation(&self) -> bool {
        self.safety.attempt_isolation()
    }
}

#[derive(Clone, Debug)]
pub struct EscalationRecord {
    pub id: EscalationId,
    pub task_id: TaskId,
    pub batch_id: BatchId,
    pub logical_agent_id: Option<LogicalAgentId>,
    pub failure_class: FailureClass,
    pub state: EscalationState,
}

#[derive(Clone, Debug)]
pub struct OutboxEvent {
    pub id: OutboxEventId,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub state: OutboxState,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct PartitionRecord {
    pub name: PartitionId,
    pub desired_capacity: i64,
    pub retention: Retention,
    pub execution_target: String,
    pub execution_profile: String,
    pub active: bool,
    pub merged_into: Option<PartitionId>,
    pub topology_revision: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ExpireReport {
    pub retried: u32,
    pub suspended: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ReconcileReport {
    pub born: u32,
    pub retired: u32,
    pub draining: u32,
}

pub const BATCH_RESULTS_READY: &str = "BATCH_RESULTS_READY";
pub const DECISION_REQUIRED: &str = "DECISION_REQUIRED";

pub const CONTINUITY_KEYS: &[&str] = &[
    "INVARIANTS",
    "DECISIONS",
    "CURRENT DESIGN",
    "REJECTED ALTERNATIVES",
    "OPEN QUESTIONS",
    "KNOWN FAILURES",
    "CURRENT CHECKPOINT",
    "NEXT LIKELY STEPS",
];
