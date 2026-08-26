//! Generic ExecutionAdapter traits and opaque handles.
//!
//! Core MUST NOT import vendor names. Adapters own process/session mapping.
//! M4 ships the trait surface and an in-memory fake; a reference adapter is M5.

use agentype_core::{ExecutionId, ExecutionState, FailureClass, RequestId, WorkspaceMode};
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
    fn observe_execution(
        &self,
        handle: &RuntimeHandle,
    ) -> AdapterResult<ExecutionObservation>;
    fn interrupt_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation>;
    fn terminate_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation>;
    fn collect_outcome(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionOutcome>;
    fn reconcile_start(&self, handle: &RuntimeHandle) -> AdapterResult<StartObservation>;
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

    fn observe_execution(
        &self,
        handle: &RuntimeHandle,
    ) -> AdapterResult<ExecutionObservation> {
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

    fn reconcile_start(&self, handle: &RuntimeHandle) -> AdapterResult<StartObservation> {
        Ok(StartObservation {
            state: ExecutionState::Unknown,
            runtime_handle: handle.clone(),
            ambiguous: true,
            failure_class: None,
            detail: Some("fake reconcile is identity-preserving".into()),
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
        let rec = fake.reconcile_start(&start.runtime_handle).unwrap();
        assert!(rec.ambiguous);
        assert!(!rec.quiescent_confirmed);
    }
}
