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
