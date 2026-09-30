//! V0.1 worker-plane compatibility types.
//!
//! These are **not** `ExecutionAdapter` DTOs. The canonical adapter surface
//! is `EnvironmentStartRequest` / `StartObservation` / `ExecutionObservation`
//! / `PhysicalExecutionOutcome`. Task payload, acceptance, and result JSON
//! belong to a separate Worker data plane.

use crate::{LaunchEnvironmentMismatch, RuntimeHandle};
use agentype_core::{
    AttemptId, BatchId, CommittedContinuitySnapshot, ExecutionId, ExecutionState, IncarnationId,
    LeaseEpoch, LeaseId, LogicalAgentId, RequestId, TaskId, WorkspaceMode, WorkstreamId,
};
use agentype_execution_config::{ExecutionLaunchSnapshot, ResolvedExecutionEnvironment};
use serde_json::Value;

/// Deterministic V0.1 worker-plane protocol. Not part of `ExecutionAdapter`.
///
/// Given the same `ExecutionLaunchSnapshot`, the scheduler worker instruction
/// is uniquely determined: the V0.1 task protocol (`LOCAL AGENT SCHEDULER
/// TASK` / `TASK_ID` / `ATTEMPT_ID` / `LEASE_EPOCH` / `WORKSTREAM` /
/// `OBJECTIVE` = payload / `ACCEPTANCE` / `COMMITTED CONTINUITY`, plus
/// `WRITER RECOVERY RULES` for WRITE tasks and a closing `RETURN` section).
/// There is no constructor accepting arbitrary text, so the instruction
/// traveling to a worker can never be substituted away from the Task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedWorkerPrompt {
    protocol: String,
}

impl RenderedWorkerPrompt {
    /// The only construction path: derive the protocol from the launch snapshot.
    pub fn from_launch(launch: &ExecutionLaunchSnapshot) -> Self {
        Self {
            protocol: render_worker_protocol(launch),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.protocol
    }
}

fn render_worker_protocol(launch: &ExecutionLaunchSnapshot) -> String {
    let mut sections = vec![
        "LOCAL AGENT SCHEDULER TASK".to_string(),
        format!("TASK_ID\n{}", launch.task_id().as_str()),
        format!("ATTEMPT_ID\n{}", launch.attempt_id().as_str()),
        format!("LEASE_EPOCH\n{}", launch.lease_epoch()),
        format!(
            "WORKSTREAM\n{}",
            match launch.workstream_id() {
                Some(w) => w.as_str().to_string(),
                None => "none".to_string(),
            }
        ),
        format!("OBJECTIVE\n{}", python_canonical_json(launch.payload())),
        format!("ACCEPTANCE\n{}", python_canonical_json(launch.acceptance())),
        format!(
            "COMMITTED CONTINUITY\n{}",
            python_canonical_json(launch.continuity().capsule())
        ),
    ];
    if matches!(launch.workspace_mode(), WorkspaceMode::Write) {
        sections.push(
            "WRITER RECOVERY RULES\n\
             The current workspace is authoritative. Inspect assignment-scoped state and diff \
             before writing; continue idempotently; do not revert unrelated work."
                .to_string(),
        );
    }
    sections.push(
        "RETURN\nReturn the authoritative result only when acceptance is satisfied. \
         Do not claim Scheduler ACK; the Scheduler validates the current lease separately."
            .to_string(),
    );
    sections.join("\n\n")
}

/// Canonical JSON rendering matching the V0.1 oracle's
/// `json.dumps(value, ensure_ascii=False, sort_keys=True)` (sorted object
/// keys, `", "` / `": "` separators, non-ASCII kept literal).
fn python_canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => python_json_string(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(python_canonical_json).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}: {}",
                        python_json_string(k),
                        python_canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn python_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Worker-plane request. Not an `ExecutionAdapter` input.
///
/// Encapsulates Task payload, acceptance, workstream, and continuity for a
/// separate Worker data plane. Constructible exclusively from the two
/// authoritative sources:
///
/// - the `ExecutionLaunchSnapshot` — durable Scheduler semantics;
/// - the `ResolvedExecutionEnvironment` — authoritative runtime configuration.
///
/// The Claim is part of neither source and can never reach this request.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionRequest {
    request_id: RequestId,
    execution_id: ExecutionId,
    task_id: TaskId,
    batch_id: BatchId,
    attempt_id: AttemptId,
    attempt_number: u32,
    lease_id: LeaseId,
    lease_epoch: LeaseEpoch,
    logical_agent_id: LogicalAgentId,
    incarnation_id: IncarnationId,
    execution_target: String,
    execution_profile: String,
    workspace_mode: WorkspaceMode,
    payload: Value,
    acceptance: Value,
    workstream_id: Option<WorkstreamId>,
    continuity: CommittedContinuitySnapshot,
    incarnation_runtime_handle: RuntimeHandle,
    target_options: Value,
    profile_options: Value,
    profile_timeout_seconds: Option<f64>,
}

impl ExecutionRequest {
    /// Assemble the worker request from the authoritative launch snapshot and
    /// the authoritative resolved environment.
    ///
    /// Two-source rule: scheduler semantics come exclusively from the
    /// snapshot; physical runtime configuration comes exclusively from the
    /// resolved environment. A V0.1 text protocol MAY be derived from the
    /// snapshot via `RenderedWorkerPrompt`; it is not a field of this type.
    pub fn from_launch(
        launch: &ExecutionLaunchSnapshot,
        environment: &ResolvedExecutionEnvironment,
    ) -> Result<Self, LaunchEnvironmentMismatch> {
        let safety = environment.safety();
        let mut mismatched: Vec<&'static str> = Vec::new();
        if launch.safety().attempt_id().as_str() != safety.attempt_id().as_str() {
            mismatched.push("attempt_id");
        }
        if launch.safety().lease_epoch() != safety.lease_epoch() {
            mismatched.push("lease_epoch");
        }
        if launch.execution_target() != safety.execution_target() {
            mismatched.push("execution_target");
        }
        if launch.execution_profile() != safety.execution_profile() {
            mismatched.push("execution_profile");
        }
        if launch.safety().attempt_isolation() != safety.attempt_isolation() {
            mismatched.push("attempt_isolation");
        }
        if !mismatched.is_empty() {
            return Err(LaunchEnvironmentMismatch {
                detail: format!(
                    "launch snapshot and resolved environment describe different attempts: {mismatched:?}"
                ),
            });
        }
        Ok(Self {
            request_id: launch.request_id().clone(),
            execution_id: launch.execution_id().clone(),
            task_id: launch.task_id().clone(),
            batch_id: launch.batch_id().clone(),
            attempt_id: launch.attempt_id().clone(),
            attempt_number: launch.attempt_number(),
            lease_id: launch.lease_id().clone(),
            lease_epoch: launch.lease_epoch(),
            logical_agent_id: launch.logical_agent_id().clone(),
            incarnation_id: launch.incarnation_id().clone(),
            execution_target: launch.execution_target().to_string(),
            execution_profile: launch.execution_profile().to_string(),
            workspace_mode: launch.workspace_mode(),
            payload: launch.payload().clone(),
            acceptance: launch.acceptance().clone(),
            workstream_id: launch.workstream_id().cloned(),
            continuity: launch.continuity().clone(),
            incarnation_runtime_handle: RuntimeHandle(launch.incarnation_runtime_handle().clone()),
            target_options: environment.target().options.clone(),
            profile_options: environment.profile().options.clone(),
            profile_timeout_seconds: environment.profile().timeout_seconds,
        })
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
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

    pub fn logical_agent_id(&self) -> &LogicalAgentId {
        &self.logical_agent_id
    }

    pub fn incarnation_id(&self) -> &IncarnationId {
        &self.incarnation_id
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

    pub fn incarnation_runtime_handle(&self) -> &RuntimeHandle {
        &self.incarnation_runtime_handle
    }

    pub fn target_options(&self) -> &Value {
        &self.target_options
    }

    pub fn profile_options(&self) -> &Value {
        &self.profile_options
    }

    /// Configured profile timeout input (seconds). This is execution/profile
    /// configuration ONLY (M5.6 §5): it MUST NOT be auto-copied onto any
    /// Scheduler-facing operation deadline.
    pub fn profile_timeout_seconds(&self) -> Option<f64> {
        self.profile_timeout_seconds
    }
}

/// Worker-plane outcome JSON. Not an `ExecutionAdapter` collect result.
#[derive(Clone, Debug)]
pub struct ExecutionOutcome {
    pub state: ExecutionState,
    pub payload: Option<Value>,
    pub summary: Option<String>,
    pub terminal_confirmed: bool,
    pub quiescent_confirmed: bool,
    pub incarnation_reusable: bool,
}
