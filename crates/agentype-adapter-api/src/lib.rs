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

/// Complete structured execution request passed to an ExecutionAdapter.
///
/// Encapsulates all execution metadata as private fields with readonly getters.
/// Constructible exclusively from an authoritative `ExecutionLaunchSnapshot`.
///
/// `prompt` is the derived worker-protocol representation (task protocol
/// sections: IDs, epoch, workstream, objective, acceptance, committed
/// continuity, and writer rules for WRITE tasks). It is NOT the Task name.
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
    /// `prompt` MUST be the runtime-rendered worker protocol produced by
    /// `agentype_runtime::render_worker_prompt(&launch)`. Adapters and other
    /// consumers MUST NOT compose scheduler semantics into the prompt
    /// themselves; the runtime is the single composition point.
    pub fn from_launch(launch: &ExecutionLaunchSnapshot, prompt: String) -> Self {
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
            prompt,
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

    /// Derived worker-protocol representation, as supplied to `from_launch`.
    /// Produced by `agentype_runtime::render_worker_prompt`; never the Task name.
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

/// In-memory fake used by M4 tests. No process, no vendor protocol.
#[derive(Clone, Default)]
pub struct FakeAdapter {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    by_request: HashMap<String, RuntimeHandle>,
    next_start: Option<StartObservation>,
    next_observe: Option<ExecutionObservation>,
    next_outcome: Option<ExecutionOutcome>,
    unavailable: bool,
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

    pub fn set_next_outcome(&self, outcome: ExecutionOutcome) {
        self.inner.lock().expect("fake adapter").next_outcome = Some(outcome);
    }
}

impl ExecutionAdapter for FakeAdapter {
    fn start_execution(&self, request: &ExecutionRequest) -> AdapterResult<StartObservation> {
        let mut g = self.inner.lock().expect("fake adapter");
        if g.unavailable {
            return Err(AdapterError::Unavailable(format!(
                "target {} unavailable",
                request.execution_target()
            )));
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

    fn mock_launch_snapshot() -> ExecutionLaunchSnapshot {
        unsafe {
            ExecutionLaunchSnapshot::from_persisted_kernel_authority(
                ExecutionId::new(),
                RequestId::new(),
                TaskId::new(),
                BatchId::new(),
                AttemptId::new(),
                1,
                LeaseId::new(),
                LeaseEpoch(1),
                100.0,
                LogicalAgentId::new(),
                IncarnationId::new(),
                Value::Null,
                "local".to_string(),
                "default".to_string(),
                WorkspaceMode::ReadOnly,
                "hi".to_string(),
                Value::Null,
                Value::Null,
                None,
                CommittedContinuitySnapshot::stateless(),
                FrozenExecutionSafety::unisolated("local", "default"),
            )
        }
    }

    #[test]
    fn fake_does_not_invent_quiescence_from_enum_names() {
        let fake = FakeAdapter::new();
        let launch = mock_launch_snapshot();
        let req = ExecutionRequest::from_launch(&launch, "rendered".to_string());
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
        let req = ExecutionRequest::from_launch(&launch, "rendered".to_string());
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
        let launch = unsafe {
            ExecutionLaunchSnapshot::from_persisted_kernel_authority(
                ExecutionId::new(),
                RequestId::new(),
                agentype_core::TaskId::new(),
                agentype_core::BatchId::new(),
                agentype_core::AttemptId::new(),
                1,
                agentype_core::LeaseId::new(),
                agentype_core::LeaseEpoch(1),
                100.0,
                agentype_core::LogicalAgentId::new(),
                inc_id.clone(),
                serde_json::json!({"proc": 42}),
                "local".to_string(),
                "default".to_string(),
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
                FrozenExecutionSafety::unisolated("local", "default"),
            )
        };
        let req = ExecutionRequest::from_launch(&launch, "RENDERED WORKER PROTOCOL".to_string());
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
        // whatever the runtime rendered — never conflated with the label.
        assert_eq!(launch.task_name(), "my-task");
        assert_eq!(req.prompt(), "RENDERED WORKER PROTOCOL");
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
}
