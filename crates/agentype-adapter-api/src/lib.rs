//! Generic ExecutionAdapter traits and opaque handles.
//!
//! Core MUST NOT import vendor names. Adapters own process/session mapping.
//! M4 ships the trait surface and an in-memory fake; a reference adapter is M5.

use agentype_core::{
    ExecutionId, ExecutionLaunchSnapshot, ExecutionState, FailureClass, RequestId, WorkspaceMode,
};
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
            Self::DeadlineExceeded(m) => write!(f, "adapter deadline: {m}"),
            Self::Protocol(m) => write!(f, "adapter protocol: {m}"),
            Self::Unavailable(m) => write!(f, "adapter unavailable: {m}"),
            Self::Other(m) => write!(f, "adapter: {m}"),
        }
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = Result<T, AdapterError>;

/// Opaque runtime handle. Core stores JSON; it MUST NOT interpret vendor keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeHandle(pub Value);

#[derive(Clone, Debug)]
pub struct ExecutionRequest {
    pub request_id: RequestId,
    pub execution_id: ExecutionId,
    pub execution_target: String,
    pub execution_profile: String,
    pub workspace_mode: WorkspaceMode,
    pub prompt: String,
    pub payload: Value,
    pub incarnation_runtime_handle: RuntimeHandle,
}

impl ExecutionRequest {
    pub fn from_launch(
        launch: &ExecutionLaunchSnapshot,
        incarnation_runtime_handle: RuntimeHandle,
    ) -> Self {
        Self {
            request_id: launch.request_id.clone(),
            execution_id: launch.execution_id.clone(),
            execution_target: launch.execution_target.clone(),
            execution_profile: launch.execution_profile.clone(),
            workspace_mode: launch.workspace_mode,
            prompt: launch.prompt.clone(),
            payload: launch.payload.clone(),
            incarnation_runtime_handle,
        }
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
                request.execution_target
            )));
        }
        let handle = RuntimeHandle(serde_json::json!({
            "fake": true,
            "request_id": request.request_id.as_str(),
        }));
        g.by_request
            .insert(request.request_id.as_str().to_string(), handle.clone());
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
mod tests {
    use super::*;

    #[test]
    fn fake_does_not_invent_quiescence_from_enum_names() {
        let fake = FakeAdapter::new();
        let req = ExecutionRequest {
            request_id: RequestId::new(),
            execution_id: ExecutionId::new(),
            execution_target: "local".into(),
            execution_profile: "default".into(),
            workspace_mode: WorkspaceMode::ReadOnly,
            prompt: "hi".into(),
            payload: Value::Null,
            incarnation_runtime_handle: RuntimeHandle::default(),
        };
        let start = fake.start_execution(&req).unwrap();
        assert!(!start.terminal_confirmed);
        assert!(!start.quiescent_confirmed);
        let rec = fake
            .reconcile_start(&req.request_id, Some(&start.runtime_handle))
            .unwrap();
        assert!(rec.ambiguous);
        assert!(!rec.quiescent_confirmed);
    }

    #[test]
    fn reconcile_can_restore_handle_by_request_id_alone() {
        let fake = FakeAdapter::new();
        let req = ExecutionRequest {
            request_id: RequestId::new(),
            execution_id: ExecutionId::new(),
            execution_target: "local".into(),
            execution_profile: "default".into(),
            workspace_mode: WorkspaceMode::ReadOnly,
            prompt: "hi".into(),
            payload: Value::Null,
            incarnation_runtime_handle: RuntimeHandle::default(),
        };
        let start = fake.start_execution(&req).unwrap();

        // Ambiguous start: scheduler lost the handle, but the start request
        // identity was persisted. Reconciliation must locate the runtime by
        // request identity alone (spec 07: reconcile_start is UNCHANGED from
        // V0.1 and takes request_id + optional persisted handle).
        let rec = fake.reconcile_start(&req.request_id, None).unwrap();
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
        let launch = ExecutionLaunchSnapshot {
            execution_id: ExecutionId::new(),
            request_id: RequestId::new(),
            task_id: agentype_core::TaskId::new(),
            batch_id: agentype_core::BatchId::new(),
            attempt_id: agentype_core::AttemptId::new(),
            attempt_number: 1,
            lease_id: agentype_core::LeaseId::new(),
            lease_epoch: agentype_core::LeaseEpoch(1),
            lease_expires_at: 100.0,
            logical_agent_id: agentype_core::LogicalAgentId::new(),
            incarnation_id: agentype_core::IncarnationId::new(),
            execution_target: "local".to_string(),
            execution_profile: "default".to_string(),
            workspace_mode: WorkspaceMode::ReadOnly,
            prompt: "task-prompt".to_string(),
            payload: serde_json::json!({"key": "val"}),
            acceptance: serde_json::json!({}),
            workstream_id: None,
            attempt_isolation: false,
        };
        let handle = RuntimeHandle(serde_json::json!({"proc": 42}));
        let req = ExecutionRequest::from_launch(&launch, handle.clone());
        assert_eq!(req.request_id, launch.request_id);
        assert_eq!(req.execution_id, launch.execution_id);
        assert_eq!(req.execution_target, "local");
        assert_eq!(req.execution_profile, "default");
        assert_eq!(req.workspace_mode, WorkspaceMode::ReadOnly);
        assert_eq!(req.prompt, "task-prompt");
        assert_eq!(req.payload, serde_json::json!({"key": "val"}));
        assert_eq!(req.incarnation_runtime_handle, handle);
    }
}
