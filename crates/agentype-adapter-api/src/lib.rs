//! Generic ExecutionAdapter traits and opaque handles.
//!
//! Core MUST NOT import vendor names. Adapters own process/session mapping.
//! M4 ships the trait surface and an in-memory fake; a reference adapter is M5.

#![deny(unsafe_code)]

use agentype_core::{
    AttemptId, BatchId, CommittedContinuitySnapshot, ExecutionId, ExecutionState, FailureClass,
    IncarnationId, LeaseEpoch, LeaseId, LogicalAgentId, RequestId, TaskId, WorkspaceMode,
    WorkstreamId,
};
use agentype_execution_config::{ExecutionLaunchSnapshot, ResolvedExecutionEnvironment};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    DeadlineExceeded(String),
    Protocol(String),
    Unavailable(String),
    Other(String),
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded(m) => write!(f, "execution deadline exceeded: {m}"),
            Self::Protocol(m) => write!(f, "adapter protocol violation: {m}"),
            Self::Unavailable(m) => write!(f, "adapter runtime unavailable: {m}"),
            Self::Other(m) => write!(f, "adapter error: {m}"),
        }
    }
}

impl std::error::Error for AdapterError {}

/// The launch snapshot and the resolved environment handed to
/// `ExecutionRequest::from_launch` do not describe the same attempt identity.
/// Mixing scheduler semantics from one attempt with runtime configuration
/// from another is rejected fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEnvironmentMismatch {
    pub detail: String,
}

impl std::fmt::Display for LaunchEnvironmentMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "launch/environment pairing mismatch: {}", self.detail)
    }
}

impl std::error::Error for LaunchEnvironmentMismatch {}

pub type AdapterResult<T> = Result<T, AdapterError>;

/// Opaque JSON-serializable runtime handle stored by the scheduler across crash/reconciliation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeHandle(pub Value);

/// Deterministic, provider-neutral worker protocol derived from an
/// authoritative launch snapshot.
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

/// Complete structured execution request passed to an ExecutionAdapter.
///
/// Encapsulates all execution metadata as private fields with readonly getters.
/// Constructible exclusively from the two authoritative sources:
///
/// - the `ExecutionLaunchSnapshot` — durable Scheduler semantics (identities,
///   workspace, payload, acceptance, continuity, incarnation binding);
/// - the `ResolvedExecutionEnvironment` — authoritative runtime configuration
///   (target options, profile options, configured timeout inputs).
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
    prompt: String,
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
    /// resolved environment (which can only be produced by authoritative
    /// configuration resolution — its fields are private, so callers cannot
    /// inject fabricated options). The worker prompt is deterministically
    /// derived from the snapshot as the provider-neutral V0.1 worker protocol
    /// (see `RenderedWorkerPrompt`).
    ///
    /// Fail-closed pairing: the snapshot and the environment must describe
    /// the same attempt identity (attempt_id, lease_epoch, execution_target,
    /// execution_profile) and the same attempt_isolation fact; mixing launch
    /// semantics from one attempt with runtime configuration from another —
    /// including a same-named target re-registered with different isolation
    /// in a different registry — is rejected.
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
            prompt: RenderedWorkerPrompt::from_launch(launch).protocol,
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

    /// Deterministically derived worker protocol (V0.1 task protocol), never
    /// the Task name and never caller-supplied text.
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

    pub fn incarnation_runtime_handle(&self) -> &RuntimeHandle {
        &self.incarnation_runtime_handle
    }

    /// Authoritative target options resolved from the ExecutionRegistry
    /// (host/endpoint settings of the requested environment).
    pub fn target_options(&self) -> &Value {
        &self.target_options
    }

    /// Authoritative profile options resolved from the ExecutionRegistry
    /// (model settings / provider-neutral tuning of the requested profile).
    pub fn profile_options(&self) -> &Value {
        &self.profile_options
    }

    /// Configured profile timeout input (seconds). Deadline *enforcement* is
    /// a later-milestone adapter concern; this is the configured input only.
    pub fn profile_timeout_seconds(&self) -> Option<f64> {
        self.profile_timeout_seconds
    }
}

#[derive(Clone, Debug)]
pub struct StartObservation {
    pub state: ExecutionState,
    pub runtime_handle: RuntimeHandle,
    pub ambiguous: bool,
    pub failure_class: Option<FailureClass>,
    pub detail: Option<String>,
    pub terminal_confirmed: bool,
    pub quiescent_confirmed: bool,
}

#[derive(Clone, Debug)]
pub struct ExecutionObservation {
    pub state: ExecutionState,
    pub terminal_confirmed: bool,
    pub quiescent_confirmed: bool,
    pub detail: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ExecutionOutcome {
    pub state: ExecutionState,
    pub payload: Option<Value>,
    pub summary: Option<String>,
    pub failure_class: Option<FailureClass>,
    pub terminal_confirmed: bool,
    pub quiescent_confirmed: bool,
    pub incarnation_reusable: bool,
}

pub trait ExecutionAdapter: Send + Sync {
    fn start_execution(&self, request: &ExecutionRequest) -> AdapterResult<StartObservation>;
    fn observe_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation>;
    fn interrupt_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation>;
    fn terminate_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation>;
    fn collect_outcome(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionOutcome>;
    /// Spec 07: narrow interface UNCHANGED from V0.1. Reconciliation is keyed
    /// by the stable start request identity because an ambiguous start may
    /// leave the scheduler without a complete runtime handle; the persisted
    /// handle, when present, is only a hint for the adapter.
    fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
    ) -> AdapterResult<StartObservation>;
}

/// In-memory fake used by M4 tests and M5.2 dispatch tests. No process, no
/// vendor protocol. Deterministic controls let tests assert invocation
/// counts, inspect the exact request received, and inject outcomes/errors.
#[derive(Clone, Default)]
pub struct FakeAdapter {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    by_request: HashMap<String, RuntimeHandle>,
    next_start: Option<StartObservation>,
    next_start_error: Option<AdapterError>,
    next_observe: Option<ExecutionObservation>,
    next_outcome: Option<ExecutionOutcome>,
    next_collect_error: Option<AdapterError>,
    next_reconcile: Option<StartObservation>,
    next_reconcile_error: Option<AdapterError>,
    unavailable: bool,
    start_call_count: usize,
    reconcile_call_count: usize,
    collect_call_count: usize,
    last_request: Option<ExecutionRequest>,
    last_reconcile_request_id: Option<RequestId>,
}

impl FakeAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        self.inner.lock().expect("fake adapter").unavailable = unavailable;
    }

    pub fn set_next_start(&self, obs: StartObservation) {
        self.inner.lock().expect("fake adapter").next_start = Some(obs);
    }

    /// Inject an error for the next `start_execution` call (after this one
    /// call the injection is consumed).
    pub fn set_next_start_error(&self, err: AdapterError) {
        self.inner.lock().expect("fake adapter").next_start_error = Some(err);
    }

    pub fn set_next_outcome(&self, outcome: ExecutionOutcome) {
        self.inner.lock().expect("fake adapter").next_outcome = Some(outcome);
    }

    /// How many times `start_execution` was invoked in total.
    pub fn start_call_count(&self) -> usize {
        self.inner.lock().expect("fake adapter").start_call_count
    }

    /// The request received by the most recent `start_execution` call.
    pub fn last_request(&self) -> Option<ExecutionRequest> {
        self.inner
            .lock()
            .expect("fake adapter")
            .last_request
            .clone()
    }

    /// Inject the next `reconcile_start` observation (consumed once).
    pub fn set_next_reconcile(&self, obs: StartObservation) {
        self.inner.lock().expect("fake adapter").next_reconcile = Some(obs);
    }

    /// Inject an error for the next `reconcile_start` call (consumed once).
    pub fn set_next_reconcile_error(&self, err: AdapterError) {
        self.inner
            .lock()
            .expect("fake adapter")
            .next_reconcile_error = Some(err);
    }

    /// Inject an error for the next `collect_outcome` call (consumed once).
    pub fn set_next_collect_error(&self, err: AdapterError) {
        self.inner.lock().expect("fake adapter").next_collect_error = Some(err);
    }

    pub fn reconcile_call_count(&self) -> usize {
        self.inner
            .lock()
            .expect("fake adapter")
            .reconcile_call_count
    }

    pub fn collect_call_count(&self) -> usize {
        self.inner.lock().expect("fake adapter").collect_call_count
    }

    pub fn last_reconcile_request_id(&self) -> Option<RequestId> {
        self.inner
            .lock()
            .expect("fake adapter")
            .last_reconcile_request_id
            .clone()
    }
}

impl ExecutionAdapter for FakeAdapter {
    fn start_execution(&self, request: &ExecutionRequest) -> AdapterResult<StartObservation> {
        let mut g = self.inner.lock().expect("fake adapter");
        g.start_call_count += 1;
        g.last_request = Some(request.clone());
        if g.unavailable {
            return Err(AdapterError::Unavailable(format!(
                "target {} unavailable",
                request.execution_target()
            )));
        }
        if let Some(err) = g.next_start_error.take() {
            return Err(err);
        }
        let handle = RuntimeHandle(serde_json::json!({
            "fake": true,
            "request_id": request.request_id().as_str(),
        }));
        g.by_request
            .insert(request.request_id().as_str().to_string(), handle.clone());
        Ok(g.next_start.take().unwrap_or(StartObservation {
            state: ExecutionState::Running,
            runtime_handle: handle,
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
        }))
    }

    fn observe_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation> {
        let mut g = self.inner.lock().expect("fake adapter");
        if g.unavailable {
            return Err(AdapterError::Unavailable("adapter unavailable".into()));
        }
        Ok(g.next_observe.take().unwrap_or(ExecutionObservation {
            state: ExecutionState::Running,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: Some(handle.0.to_string()),
        }))
    }

    fn interrupt_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation> {
        self.observe_execution(handle)
    }

    fn terminate_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation> {
        let _ = handle;
        Ok(ExecutionObservation {
            state: ExecutionState::Terminated,
            terminal_confirmed: true,
            quiescent_confirmed: true,
            detail: None,
        })
    }

    fn collect_outcome(&self, _handle: &RuntimeHandle) -> AdapterResult<ExecutionOutcome> {
        let mut g = self.inner.lock().expect("fake adapter");
        g.collect_call_count += 1;
        if g.unavailable {
            return Err(AdapterError::Unavailable("adapter unavailable".into()));
        }
        if let Some(err) = g.next_collect_error.take() {
            return Err(err);
        }
        Ok(g.next_outcome.take().unwrap_or(ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload: Some(serde_json::json!({"ok": true})),
            summary: Some("fake".into()),
            failure_class: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        }))
    }

    fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
    ) -> AdapterResult<StartObservation> {
        let mut g = self.inner.lock().expect("fake adapter");
        g.reconcile_call_count += 1;
        g.last_reconcile_request_id = Some(request_id.clone());
        if g.unavailable {
            return Err(AdapterError::Unavailable("adapter unavailable".into()));
        }
        if let Some(err) = g.next_reconcile_error.take() {
            return Err(err);
        }
        let non_empty = |h: &RuntimeHandle| {
            h.0.as_object()
                .map(|o| !o.is_empty())
                .unwrap_or(!h.0.is_null())
        };
        let handle = match persisted_handle {
            Some(h) if non_empty(h) => h.clone(),
            _ => g
                .by_request
                .get(request_id.as_str())
                .cloned()
                .unwrap_or_default(),
        };
        Ok(g.next_reconcile.take().unwrap_or(StartObservation {
            state: ExecutionState::Unknown,
            runtime_handle: handle,
            ambiguous: true,
            failure_class: None,
            detail: Some("fake reconcile is identity-preserving by request".into()),
            terminal_confirmed: false,
            quiescent_confirmed: false,
        }))
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use agentype_execution_config::FrozenExecutionSafety;

    /// Coherent synthetic launch fixture: the snapshot's attempt identity and
    /// its Attempt-bound safety proof share one `AuthoritativeExecutionBinding`
    /// (review §21 fixture hygiene — never hand a snapshot a safety proof
    /// minted for a different synthetic attempt).
    struct MockLaunch {
        snapshot: ExecutionLaunchSnapshot,
        binding: agentype_core::AuthoritativeExecutionBinding,
        environment: ResolvedExecutionEnvironment,
    }

    fn mock_launch() -> MockLaunch {
        let attempt_id = agentype_core::AttemptId::new();
        let binding = agentype_core::AuthoritativeExecutionBinding {
            attempt_id: attempt_id.clone(),
            lease_epoch: LeaseEpoch(1),
            execution_target: "local".to_string(),
            execution_profile: "default".to_string(),
        };
        let environment = agentype_execution_config::resolve_execution_environment(
            agentype_execution_config::ExecutionResolutionMode::DirectUnconfigured,
            &binding,
        )
        .unwrap();
        let snapshot = unsafe {
            ExecutionLaunchSnapshot::from_persisted_kernel_authority(
                ExecutionId::new(),
                RequestId::new(),
                TaskId::new(),
                BatchId::new(),
                attempt_id,
                1,
                LeaseId::new(),
                LeaseEpoch(1),
                100.0,
                LogicalAgentId::new(),
                IncarnationId::new(),
                Value::Null,
                binding.execution_target.clone(),
                binding.execution_profile.clone(),
                WorkspaceMode::ReadOnly,
                "hi".to_string(),
                Value::Null,
                Value::Null,
                None,
                CommittedContinuitySnapshot::stateless(),
                FrozenExecutionSafety::unisolated(binding.clone()),
            )
        };
        MockLaunch {
            snapshot,
            binding,
            environment,
        }
    }

    #[test]
    fn fake_does_not_invent_quiescence_from_enum_names() {
        let fake = FakeAdapter::new();
        let mock = mock_launch();
        let launch = mock.snapshot;
        let req = ExecutionRequest::from_launch(&launch, &mock.environment).unwrap();
        let start = fake.start_execution(&req).unwrap();
        assert!(!start.terminal_confirmed);
        assert!(!start.quiescent_confirmed);
        let rec = fake
            .reconcile_start(req.request_id(), Some(&start.runtime_handle))
            .unwrap();
        assert!(rec.ambiguous);
        assert!(!rec.quiescent_confirmed);
    }

    #[test]
    fn reconcile_can_restore_handle_by_request_id_alone() {
        let fake = FakeAdapter::new();
        let mock = mock_launch();
        let launch = mock.snapshot;
        let req = ExecutionRequest::from_launch(&launch, &mock.environment).unwrap();
        let start = fake.start_execution(&req).unwrap();

        // Ambiguous start: scheduler lost the handle, but the start request
        // identity was persisted. Reconciliation must locate the runtime by
        // request identity alone (spec 07: reconcile_start is UNCHANGED from
        // V0.1 and takes request_id + optional persisted handle).
        let rec = fake.reconcile_start(req.request_id(), None).unwrap();
        assert_eq!(rec.runtime_handle, start.runtime_handle);
        assert!(rec.ambiguous);
        assert!(!rec.terminal_confirmed);
        assert!(!rec.quiescent_confirmed);
    }

    #[test]
    fn unknown_request_reconciles_ambiguous_without_proof() {
        let fake = FakeAdapter::new();
        let rec = fake.reconcile_start(&RequestId::new(), None).unwrap();
        assert!(rec.ambiguous);
        assert!(!rec.terminal_confirmed);
        assert!(!rec.quiescent_confirmed);
    }

    #[test]
    fn execution_request_constructed_from_launch_snapshot() {
        let ws = WorkstreamId::new();
        let inc_id = agentype_core::IncarnationId::new();
        // §21 fixture hygiene: one attempt identity shared by the snapshot
        // and its Attempt-bound safety proof.
        let attempt_id = agentype_core::AttemptId::new();
        let binding = agentype_core::AuthoritativeExecutionBinding {
            attempt_id: attempt_id.clone(),
            lease_epoch: LeaseEpoch(1),
            execution_target: "local".to_string(),
            execution_profile: "default".to_string(),
        };
        let environment = agentype_execution_config::resolve_execution_environment(
            agentype_execution_config::ExecutionResolutionMode::DirectUnconfigured,
            &binding,
        )
        .unwrap();
        let launch = unsafe {
            ExecutionLaunchSnapshot::from_persisted_kernel_authority(
                ExecutionId::new(),
                RequestId::new(),
                agentype_core::TaskId::new(),
                agentype_core::BatchId::new(),
                attempt_id,
                1,
                agentype_core::LeaseId::new(),
                LeaseEpoch(1),
                100.0,
                agentype_core::LogicalAgentId::new(),
                inc_id.clone(),
                serde_json::json!({"proc": 42}),
                binding.execution_target.clone(),
                binding.execution_profile.clone(),
                WorkspaceMode::ReadOnly,
                "my-task".to_string(),
                serde_json::json!({"key": "val"}),
                serde_json::json!({"criterion": "pass"}),
                Some(ws.clone()),
                CommittedContinuitySnapshot::new(
                    agentype_core::ContinuityPreference::Required,
                    3,
                    serde_json::json!({"state": "saved"}),
                ),
                FrozenExecutionSafety::unisolated(binding.clone()),
            )
        };
        // The safety proof is bound to the snapshot's own attempt identity.
        assert_eq!(launch.safety().attempt_id(), launch.attempt_id());
        assert_eq!(launch.safety().lease_epoch(), launch.lease_epoch());
        let req = ExecutionRequest::from_launch(&launch, &environment).unwrap();
        assert_eq!(req.request_id(), launch.request_id());
        assert_eq!(req.execution_id(), launch.execution_id());
        assert_eq!(req.task_id(), launch.task_id());
        assert_eq!(req.batch_id(), launch.batch_id());
        assert_eq!(req.attempt_id(), launch.attempt_id());
        assert_eq!(req.attempt_number(), 1);
        assert_eq!(req.lease_id(), launch.lease_id());
        assert_eq!(req.lease_epoch(), LeaseEpoch(1));
        assert_eq!(req.logical_agent_id(), launch.logical_agent_id());
        assert_eq!(req.incarnation_id(), &inc_id);
        assert_eq!(req.execution_target(), "local");
        assert_eq!(req.execution_profile(), "default");
        assert_eq!(req.workspace_mode(), WorkspaceMode::ReadOnly);
        // The snapshot carries the durable Task label; the request prompt is
        // deterministically derived from the protocol — never the label.
        assert_eq!(launch.task_name(), "my-task");
        assert_eq!(
            req.prompt(),
            RenderedWorkerPrompt::from_launch(&launch).as_str()
        );
        assert!(req.prompt().contains("OBJECTIVE\n{\"key\": \"val\"}"));
        assert_eq!(req.payload(), &serde_json::json!({"key": "val"}));
        assert_eq!(req.acceptance(), &serde_json::json!({"criterion": "pass"}));
        assert_eq!(req.workstream_id(), Some(&ws));
        assert_eq!(req.continuity().version(), 3);
        assert_eq!(
            req.continuity().capsule(),
            &serde_json::json!({"state": "saved"})
        );
        assert_eq!(
            req.continuity().preference(),
            agentype_core::ContinuityPreference::Required
        );
        assert_eq!(
            req.incarnation_runtime_handle(),
            &RuntimeHandle(serde_json::json!({"proc": 42}))
        );
        // Two-source rule: runtime configuration comes from the resolved
        // environment, never from the snapshot or the Claim.
        assert_eq!(req.target_options(), &environment.target().options);
        assert_eq!(req.profile_options(), &environment.profile().options);
        assert_eq!(
            req.profile_timeout_seconds(),
            environment.profile().timeout_seconds
        );
    }

    /// Review P1: the worker instruction is a pure function of the launch
    /// snapshot. There is no API path to inject arbitrary text, so the same
    /// durable facts always produce the same protocol.
    #[test]
    fn worker_prompt_is_deterministic_and_cannot_be_injected() {
        let mock = mock_launch();
        let launch = mock.snapshot;
        let first = ExecutionRequest::from_launch(&launch, &mock.environment).unwrap();
        let second = ExecutionRequest::from_launch(&launch, &mock.environment).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.prompt(),
            RenderedWorkerPrompt::from_launch(&launch).as_str()
        );
        assert!(first
            .prompt()
            .starts_with("LOCAL AGENT SCHEDULER TASK\n\nTASK_ID\n"));
        // The mock snapshot is read-only: no writer instructions may appear.
        assert!(!first.prompt().contains("WRITER RECOVERY RULES"));
    }

    /// §21 fixture hygiene regression: the synthetic snapshot and its
    /// Attempt-bound safety proof share one attempt identity.
    #[test]
    fn fixture_safety_is_bound_to_the_snapshot_attempt_identity() {
        let mock = mock_launch();
        let (launch, binding) = (&mock.snapshot, &mock.binding);
        assert_eq!(launch.attempt_id(), &binding.attempt_id);
        assert_eq!(launch.safety().attempt_id(), launch.attempt_id());
        assert_eq!(launch.safety().lease_epoch(), launch.lease_epoch());
        assert_eq!(launch.execution_target(), binding.execution_target.as_str());
        assert_eq!(
            launch.execution_profile(),
            binding.execution_profile.as_str()
        );
    }

    /// §30: deterministic FakeAdapter controls for dispatch tests.
    #[test]
    fn fake_adapter_records_invocation_count_and_last_request() {
        let fake = FakeAdapter::new();
        assert_eq!(fake.start_call_count(), 0);
        let mock = mock_launch();
        let launch = mock.snapshot;
        let req = ExecutionRequest::from_launch(&launch, &mock.environment).unwrap();
        let _ = fake.start_execution(&req).unwrap();
        let _ = fake.start_execution(&req).unwrap();
        assert_eq!(fake.start_call_count(), 2);
        assert_eq!(fake.last_request().as_ref(), Some(&req));

        // An injected start error is consumed exactly once.
        fake.set_next_start_error(AdapterError::DeadlineExceeded("cleanup".into()));
        assert!(fake.start_execution(&req).is_err());
        assert_eq!(fake.start_call_count(), 3);
        assert!(fake.start_execution(&req).is_ok());
    }

    /// Audit P2: from_launch fails closed when the launch snapshot and the
    /// resolved environment do not describe the same attempt identity —
    /// scheduler semantics from one attempt can never be combined with
    /// runtime configuration from another.
    #[test]
    fn from_launch_rejects_mixed_launch_environment_pairs() {
        let mock = mock_launch();

        // A different attempt identity.
        let foreign_binding = agentype_core::AuthoritativeExecutionBinding {
            attempt_id: agentype_core::AttemptId::new(),
            lease_epoch: LeaseEpoch(1),
            execution_target: "local".to_string(),
            execution_profile: "default".to_string(),
        };
        let foreign_env = agentype_execution_config::resolve_execution_environment(
            agentype_execution_config::ExecutionResolutionMode::DirectUnconfigured,
            &foreign_binding,
        )
        .unwrap();
        let err = ExecutionRequest::from_launch(&mock.snapshot, &foreign_env).unwrap_err();
        assert!(err.detail.contains("attempt_id"), "got: {err:?}");

        // The same attempt, but a different configured target.
        let wrong_target_binding = agentype_core::AuthoritativeExecutionBinding {
            attempt_id: mock.binding.attempt_id.clone(),
            lease_epoch: LeaseEpoch(1),
            execution_target: "elsewhere".to_string(),
            execution_profile: "default".to_string(),
        };
        let wrong_target_env = agentype_execution_config::resolve_execution_environment(
            agentype_execution_config::ExecutionResolutionMode::DirectUnconfigured,
            &wrong_target_binding,
        )
        .unwrap();
        let err = ExecutionRequest::from_launch(&mock.snapshot, &wrong_target_env).unwrap_err();
        assert!(err.detail.contains("execution_target"), "got: {err:?}");

        // The same attempt identity and target/profile names, but a
        // differently-isolated environment (audit P1: same-named targets can
        // be re-registered with different isolation in another registry).
        let mut isolated_registry = agentype_execution_config::ExecutionRegistry::new();
        isolated_registry
            .register_target(agentype_execution_config::ExecutionTargetConfig::new(
                "local", "process", true,
            ))
            .unwrap();
        isolated_registry
            .register_profile(agentype_execution_config::ExecutionProfileConfig::new(
                "default",
            ))
            .unwrap();
        let isolated_env = agentype_execution_config::resolve_execution_environment(
            agentype_execution_config::ExecutionResolutionMode::Authoritative(&isolated_registry),
            &mock.binding,
        )
        .unwrap();
        assert!(isolated_env.attempt_isolation());
        let err = ExecutionRequest::from_launch(&mock.snapshot, &isolated_env).unwrap_err();
        assert!(err.detail.contains("attempt_isolation"), "got: {err:?}");
    }
}
