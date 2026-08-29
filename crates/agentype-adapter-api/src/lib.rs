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
use agentype_execution_config::ExecutionLaunchSnapshot;
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
/// Constructible exclusively from an authoritative `ExecutionLaunchSnapshot`.
///
/// `prompt` is deterministically derived from the snapshot (see
/// `RenderedWorkerPrompt`); it is not a parameter and cannot be injected.
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
}

impl ExecutionRequest {
    /// Assemble the worker request from an authoritative launch snapshot.
    ///
    /// The worker prompt is not a parameter: it is deterministically derived
    /// from the snapshot as the provider-neutral V0.1 worker protocol (see
    /// `RenderedWorkerPrompt`). Every caller receives the same instruction
    /// for the same durable launch facts; there is no path to inject
    /// arbitrary text between the scheduler and the worker.
    pub fn from_launch(launch: &ExecutionLaunchSnapshot) -> Self {
        Self {
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
        }
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
    unavailable: bool,
    start_call_count: usize,
    last_request: Option<ExecutionRequest>,
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
        let g = self.inner.lock().expect("fake adapter");
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
        Ok(StartObservation {
            state: ExecutionState::Unknown,
            runtime_handle: handle,
            ambiguous: true,
            failure_class: None,
            detail: Some("fake reconcile is identity-preserving by request".into()),
            terminal_confirmed: false,
            quiescent_confirmed: false,
        })
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
    fn mock_launch_fixture() -> (
        ExecutionLaunchSnapshot,
        agentype_core::AuthoritativeExecutionBinding,
    ) {
        let attempt_id = agentype_core::AttemptId::new();
        let binding = agentype_core::AuthoritativeExecutionBinding {
            attempt_id: attempt_id.clone(),
            lease_epoch: LeaseEpoch(1),
            execution_target: "local".to_string(),
            execution_profile: "default".to_string(),
        };
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
        (snapshot, binding)
    }

    fn mock_launch_snapshot() -> ExecutionLaunchSnapshot {
        mock_launch_fixture().0
    }

    #[test]
    fn fake_does_not_invent_quiescence_from_enum_names() {
        let fake = FakeAdapter::new();
        let launch = mock_launch_snapshot();
        let req = ExecutionRequest::from_launch(&launch);
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
        let launch = mock_launch_snapshot();
        let req = ExecutionRequest::from_launch(&launch);
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
                FrozenExecutionSafety::unisolated(binding),
            )
        };
        // The safety proof is bound to the snapshot's own attempt identity.
        assert_eq!(launch.safety().attempt_id(), launch.attempt_id());
        assert_eq!(launch.safety().lease_epoch(), launch.lease_epoch());
        let req = ExecutionRequest::from_launch(&launch);
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
    }

    /// Review P1: the worker instruction is a pure function of the launch
    /// snapshot. There is no API path to inject arbitrary text, so the same
    /// durable facts always produce the same protocol.
    #[test]
    fn worker_prompt_is_deterministic_and_cannot_be_injected() {
        let launch = mock_launch_snapshot();
        let first = ExecutionRequest::from_launch(&launch);
        let second = ExecutionRequest::from_launch(&launch);
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
        let (launch, binding) = mock_launch_fixture();
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
        let launch = mock_launch_snapshot();
        let req = ExecutionRequest::from_launch(&launch);
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
}
