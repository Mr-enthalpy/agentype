//! M5.7 reference-adapter conformance: a real OS process attached to the
//! frozen M5.6 ExecutionAdapter contract. Not a model, provider, or harness.

#![allow(unsafe_code)]

use agentype_adapter_api::{
    AdapterDeadline, AdapterError, AdapterErrorKind, ExecutionAdapter, ExecutionObservation,
    ExecutionRequest, RuntimeHandle, StartObservation,
};
use agentype_adapter_local_process::{LocalProcessAgentAdapter, ADAPTER_KIND};
use agentype_core::{
    AttemptId, AuthoritativeExecutionBinding, BatchId, CommittedContinuitySnapshot, ExecutionId,
    ExecutionState, FailureClass, IncarnationId, LeaseEpoch, LeaseId, LogicalAgentId, RequestId,
    TaskId, WorkspaceMode,
};
use agentype_execution_config::{
    ExecutionLaunchSnapshot, ExecutionProfileConfig, ExecutionRegistry, ExecutionResolutionMode,
    ExecutionTargetConfig,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const SECRET: &str = "super-secret-token-do-not-leak";

fn fake_bin() -> String {
    env!("CARGO_BIN_EXE_fake-agent").to_string()
}

fn long_deadline() -> AdapterDeadline {
    AdapterDeadline::after(Duration::from_secs(8)).unwrap()
}

fn short_deadline() -> AdapterDeadline {
    AdapterDeadline::after(Duration::from_millis(300)).unwrap()
}

fn expired_deadline() -> AdapterDeadline {
    AdapterDeadline::from_instant(Instant::now() - Duration::from_secs(1))
}

fn assert_by_deadline(started: Instant, budget: Duration) {
    let elapsed = started.elapsed();
    assert!(
        elapsed < budget + Duration::from_secs(2),
        "call ignored deadline: elapsed {elapsed:?} budget {budget:?}"
    );
}

fn assert_no_secret(err: &AdapterError) {
    let shown = format!("{err}");
    assert!(
        !shown.contains(SECRET),
        "adapter diagnostic leaked secret: {shown}"
    );
    if let Some(d) = err.diagnostic() {
        assert!(!d.contains(SECRET), "diagnostic leaked secret: {d}");
    }
}

fn assert_no_quiescence_start(obs: &StartObservation) {
    assert!(!obs.quiescent_confirmed);
    assert!(!obs.terminal_confirmed);
}

fn assert_no_quiescence_obs(obs: &ExecutionObservation) {
    assert!(!obs.quiescent_confirmed);
}

struct AgentSpec {
    env: Vec<(String, String)>,
    extra_options: Value,
    payload: Value,
}

impl Default for AgentSpec {
    fn default() -> Self {
        Self {
            env: Vec::new(),
            extra_options: json!({}),
            payload: json!({"k": "v"}),
        }
    }
}

impl AgentSpec {
    fn with_flag(name: &str) -> Self {
        Self::default().env_pair(name, "1")
    }

    fn env_pair(mut self, k: &str, v: &str) -> Self {
        self.env.push((k.to_string(), v.to_string()));
        self
    }

    fn extra(mut self, extra: Value) -> Self {
        self.extra_options = extra;
        self
    }

    fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
}

fn request(spec: AgentSpec) -> ExecutionRequest {
    let mut env_map = serde_json::Map::new();
    for (k, v) in spec.env {
        env_map.insert(k, Value::String(v));
    }
    // Sanitization fixture: present on every child so every path can assert
    // it never appears in AdapterError diagnostics.
    env_map.insert(
        "FAKE_AGENT_STDERR_SECRET".into(),
        Value::String(SECRET.into()),
    );

    let mut options = json!({
        "command": fake_bin(),
        "args": [],
        "env": env_map,
    });
    if let Some(obj) = spec.extra_options.as_object() {
        for (k, v) in obj {
            options[k] = v.clone();
        }
    }

    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(
            ExecutionTargetConfig::new("local_process", ADAPTER_KIND, false).with_options(options),
        )
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();

    let attempt_id = AttemptId::new();
    let binding = AuthoritativeExecutionBinding {
        attempt_id: attempt_id.clone(),
        lease_epoch: LeaseEpoch(1),
        execution_target: "local_process".to_string(),
        execution_profile: "default".to_string(),
    };
    let environment = agentype_execution_config::resolve_execution_environment(
        ExecutionResolutionMode::Authoritative(&registry),
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
            "fixture".to_string(),
            spec.payload,
            json!({"ok": true}),
            None,
            CommittedContinuitySnapshot::stateless(),
            environment.safety().clone(),
        )
    };
    ExecutionRequest::from_launch(&snapshot, &environment).unwrap()
}

fn start_hang(adapter: &LocalProcessAgentAdapter) -> (ExecutionRequest, StartObservation) {
    let req = request(AgentSpec::with_flag("FAKE_AGENT_HANG"));
    let start = adapter.start_execution(&req, &long_deadline()).unwrap();
    assert_eq!(start.state, ExecutionState::Running);
    assert_no_quiescence_start(&start);
    (req, start)
}

fn handle_fields(handle: &RuntimeHandle) -> (u32, String) {
    let obj = handle.0.as_object().expect("handle object");
    assert_eq!(obj.get("kind").and_then(Value::as_str), Some(ADAPTER_KIND));
    assert_eq!(obj.get("v").and_then(Value::as_i64), Some(1));
    let pid = obj.get("pid").and_then(Value::as_u64).expect("pid") as u32;
    let request_id = obj
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(obj.get("stdout").and_then(Value::as_str).is_some());
    assert!(obj.get("stderr").and_then(Value::as_str).is_some());
    (pid, request_id)
}

// --- §16 Creation ---

#[test]
fn start_creates_environment_and_returns_persisted_handle() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::default());
    let start = adapter.start_execution(&req, &long_deadline()).unwrap();
    assert_no_quiescence_start(&start);
    let (pid, request_id) = handle_fields(&start.runtime_handle);
    assert!(pid > 0);
    assert_eq!(request_id, req.request_id().as_str());

    let text = serde_json::to_string(&start.runtime_handle.0).unwrap();
    let restored = RuntimeHandle(serde_json::from_str(&text).unwrap());
    assert_eq!(restored, start.runtime_handle);

    let outcome = adapter
        .collect_outcome(&restored, &long_deadline())
        .unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded);
    assert!(outcome.terminal_confirmed);
    assert!(!outcome.quiescent_confirmed);
    assert_eq!(outcome.summary.as_deref(), Some("fake-agent"));
}

// --- §16 Observation ---

#[test]
fn observe_running_and_exited_environments() {
    let adapter = LocalProcessAgentAdapter::new();
    let (_req, start) = start_hang(&adapter);
    let obs = adapter
        .observe_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(obs.state, ExecutionState::Running);
    assert_no_quiescence_obs(&obs);
    assert!(!obs.terminal_confirmed);

    adapter
        .terminate_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
    let after = adapter
        .observe_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
    // Process death is UNKNOWN, never SUCCEEDED and never quiescence.
    assert_eq!(after.state, ExecutionState::Unknown);
    assert_no_quiescence_obs(&after);
    assert!(!after.terminal_confirmed);
}

#[test]
fn observe_unknown_handle_is_protocol() {
    let adapter = LocalProcessAgentAdapter::new();
    let err = adapter
        .observe_execution(
            &RuntimeHandle(
                json!({"v": 1, "kind": "not_local", "pid": 1, "stdout": "a", "stderr": "b"}),
            ),
            &long_deadline(),
        )
        .unwrap_err();
    assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    assert_no_secret(&err);
}

// --- §16 Deadline / M5.6 §51 ---

#[test]
fn expired_deadline_is_rejected_on_all_six_operations() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::default());
    let expired = expired_deadline();
    let dummy = RuntimeHandle(json!({
        "v": 1,
        "kind": ADAPTER_KIND,
        "pid": 1,
        "request_id": req.request_id().as_str(),
        "stdout": "stdout.txt",
        "stderr": "stderr.txt",
    }));

    let start_err = adapter.start_execution(&req, &expired).unwrap_err();
    assert_eq!(start_err.kind(), AdapterErrorKind::DeadlineExceeded);
    assert_no_secret(&start_err);

    let observe_err = adapter.observe_execution(&dummy, &expired).unwrap_err();
    assert_eq!(observe_err.kind(), AdapterErrorKind::DeadlineExceeded);

    let interrupt_err = adapter.interrupt_execution(&dummy, &expired).unwrap_err();
    assert_eq!(interrupt_err.kind(), AdapterErrorKind::DeadlineExceeded);

    let terminate_err = adapter.terminate_execution(&dummy, &expired).unwrap_err();
    assert_eq!(terminate_err.kind(), AdapterErrorKind::DeadlineExceeded);

    let collect_err = adapter.collect_outcome(&dummy, &expired).unwrap_err();
    assert_eq!(collect_err.kind(), AdapterErrorKind::DeadlineExceeded);

    let reconcile_err = adapter
        .reconcile_start(req.request_id(), Some(&dummy), &expired)
        .unwrap_err();
    assert_eq!(reconcile_err.kind(), AdapterErrorKind::DeadlineExceeded);
}

#[test]
fn collect_blocked_response_respects_deadline_and_does_not_prove_state() {
    let adapter = LocalProcessAgentAdapter::new();
    let (_req, start) = start_hang(&adapter);
    let budget = Duration::from_millis(300);
    let started = Instant::now();
    let err = adapter
        .collect_outcome(&start.runtime_handle, &short_deadline())
        .unwrap_err();
    assert_by_deadline(started, budget);
    assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    assert!(err.runtime_handle_hint().is_some());
    assert_no_secret(&err);
    // Timeout is not TERMINATED / quiescence. The process may have been
    // kill-sent as cleanup; that is still not a terminal observation.
}

#[test]
fn start_stdin_timeout_returns_partial_locator() {
    // Unread stdin + payload large enough to fill the OS pipe buffer.
    let payload = json!({"pad": "x".repeat(2 * 1024 * 1024)});
    let req = request(AgentSpec::with_flag("FAKE_AGENT_HANG").payload(payload));
    let adapter = LocalProcessAgentAdapter::new();
    let budget = Duration::from_millis(400);
    let started = Instant::now();
    let err = adapter
        .start_execution(&req, &AdapterDeadline::after(budget).unwrap())
        .unwrap_err();
    assert_by_deadline(started, budget);
    assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    assert!(
        err.runtime_handle_hint().is_some(),
        "partial locator must survive stdin timeout"
    );
    assert_no_secret(&err);
}

#[test]
fn missing_command_is_unavailable_not_scheduler_failure() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::default().extra(json!({
        "command": "agentype-definitely-not-a-real-executable-xyz"
    })));
    let err = adapter.start_execution(&req, &long_deadline()).unwrap_err();
    assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
    assert_no_secret(&err);
}

// --- §16 Control ---

#[test]
fn interrupt_is_observation_not_cancellation() {
    let adapter = LocalProcessAgentAdapter::new();
    let (_req, start) = start_hang(&adapter);
    let obs = adapter
        .interrupt_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(obs.state, ExecutionState::Running);
    assert_no_quiescence_obs(&obs);
    assert!(!obs.terminal_confirmed);
    adapter
        .terminate_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
}

#[test]
fn terminate_kill_is_not_quiescence_or_task_cancel() {
    let adapter = LocalProcessAgentAdapter::new();
    let (_req, start) = start_hang(&adapter);
    let obs = adapter
        .terminate_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(obs.state, ExecutionState::Terminated);
    assert!(!obs.terminal_confirmed);
    assert!(!obs.quiescent_confirmed);
}

#[test]
fn terminate_timeout_does_not_imply_termination() {
    let adapter = LocalProcessAgentAdapter::new();
    let (_req, start) = start_hang(&adapter);
    let err = adapter
        .terminate_execution(&start.runtime_handle, &expired_deadline())
        .unwrap_err();
    assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    // Still running: expired deadline abandoned the call without proving death.
    let obs = adapter
        .observe_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(obs.state, ExecutionState::Running);
    adapter
        .terminate_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
}

// --- §16 Collection ---

#[test]
fn collect_successful_outcome() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::default());
    let start = adapter.start_execution(&req, &long_deadline()).unwrap();
    let out = adapter
        .collect_outcome(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(out.state, ExecutionState::Succeeded);
    assert_eq!(out.payload, Some(json!({"echo": true})));
    assert!(out.terminal_confirmed);
    assert!(!out.quiescent_confirmed);
    assert!(!out.incarnation_reusable);
}

#[test]
fn collect_failed_outcome_from_structured_json() {
    let adapter = LocalProcessAgentAdapter::new();
    let stdout = r#"{"ok":false,"summary":"agent failed","failure_class":"START_FAILURE"}"#;
    let req = request(AgentSpec::default().env_pair("FAKE_AGENT_STDOUT", stdout));
    let start = adapter.start_execution(&req, &long_deadline()).unwrap();
    let out = adapter
        .collect_outcome(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(out.state, ExecutionState::Failed);
    assert_eq!(out.failure_class, Some(FailureClass::StartFailure));
    assert_eq!(out.summary.as_deref(), Some("agent failed"));
    assert!(out.terminal_confirmed);
    assert!(!out.quiescent_confirmed);
}

#[test]
fn collect_malformed_output_is_protocol() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::with_flag("FAKE_AGENT_MALFORMED"));
    let start = adapter.start_execution(&req, &long_deadline()).unwrap();
    let err = adapter
        .collect_outcome(&start.runtime_handle, &long_deadline())
        .unwrap_err();
    assert_eq!(err.kind(), AdapterErrorKind::Protocol);
    assert_no_secret(&err);
}

// --- §16 Restart ---

#[test]
fn reconcile_reconnects_persisted_handle() {
    let adapter = LocalProcessAgentAdapter::new();
    let (req, start) = start_hang(&adapter);
    let rec = adapter
        .reconcile_start(
            req.request_id(),
            Some(&start.runtime_handle),
            &long_deadline(),
        )
        .unwrap();
    assert_eq!(rec.state, ExecutionState::Running);
    assert!(!rec.ambiguous);
    assert_eq!(rec.runtime_handle, start.runtime_handle);
    assert_no_quiescence_start(&rec);
    adapter
        .terminate_execution(&start.runtime_handle, &long_deadline())
        .unwrap();
}

#[test]
fn reconcile_failed_reconnect_is_ambiguous_unknown() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::default());
    // Not pid 1: on Linux that is init and pid_alive is true.
    let ghost = RuntimeHandle(json!({
        "v": 1,
        "kind": ADAPTER_KIND,
        "pid": u32::MAX,
        "request_id": req.request_id().as_str(),
        "stdout": "stdout.txt",
        "stderr": "stderr.txt",
    }));
    let rec = adapter
        .reconcile_start(req.request_id(), Some(&ghost), &long_deadline())
        .unwrap();
    assert_eq!(rec.state, ExecutionState::Unknown);
    assert!(rec.ambiguous);
    assert_no_quiescence_start(&rec);
}

#[test]
fn reconcile_without_handle_is_ambiguous_not_a_new_start() {
    let adapter = LocalProcessAgentAdapter::new();
    let rec = adapter
        .reconcile_start(&RequestId::new(), None, &long_deadline())
        .unwrap();
    assert_eq!(rec.state, ExecutionState::Unknown);
    assert!(rec.ambiguous);
    assert_no_quiescence_start(&rec);
}

#[test]
fn reconcile_rejects_mismatched_request_id() {
    let adapter = LocalProcessAgentAdapter::new();
    let handle = RuntimeHandle(json!({
        "v": 1,
        "kind": ADAPTER_KIND,
        "pid": 1,
        "request_id": "other",
        "stdout": "stdout.txt",
        "stderr": "stderr.txt",
    }));
    let err = adapter
        .reconcile_start(&RequestId::new(), Some(&handle), &long_deadline())
        .unwrap_err();
    assert_eq!(err.kind(), AdapterErrorKind::Protocol);
}

// --- Boundary: not a model adapter ---

#[test]
fn model_and_api_key_options_are_opaque_and_not_leaked() {
    let adapter = LocalProcessAgentAdapter::new();
    let req = request(AgentSpec::default().extra(json!({
        "model": "deepseek",
        "provider": "whoever",
        "api_key": SECRET,
    })));
    let start = adapter.start_execution(&req, &long_deadline()).unwrap();
    let out = adapter
        .collect_outcome(&start.runtime_handle, &long_deadline())
        .unwrap();
    assert_eq!(out.state, ExecutionState::Succeeded);
    let dump = format!("{out:?}");
    assert!(!dump.contains(SECRET), "outcome leaked api_key: {dump}");
}

#[test]
fn request_prompt_is_scheduler_protocol_not_caller_text() {
    let req = request(AgentSpec::default());
    assert!(req.prompt().starts_with("LOCAL AGENT SCHEDULER TASK"));
    assert!(!req.prompt().contains("deepseek"));
    assert!(!req.prompt().contains(SECRET));
}
