//! Integration tests for RootSemanticControl.

use agentype_adapter_api::{
    AdapterDeadline, AdapterResult, EnvironmentStartRequest, ExecutionAdapter,
    ExecutionObservation, FakeAdapter, ImportableAdapter, PhysicalExecutionOutcome, RuntimeHandle,
    StartObservation,
};
use agentype_core::{
    GenerationState, InformationFunction, PartitionSpec, RawWorkIntent, Retention, SemanticInputSet,
};
use agentype_execution_config::AdapterBindingKey;
use agentype_root_bridge::RecordingRootBridge;
use agentype_runtime::{
    AdapterDeadlinePolicy, AdapterRegistry, DaemonPhase, ExecutionProfileConfig, ExecutionRegistry,
    ExecutionTargetConfig, NotifierBinding, NotifierConfig, NotifierRetryPolicy,
    PhysicalObserverConfig, RuntimeTimingConfig, SchedulerDaemonBuilder, SqliteRuntimeConfig,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agentype_core::RequestId;

struct TestImportableAdapter(Arc<FakeAdapter>);

impl ExecutionAdapter for TestImportableAdapter {
    fn start_execution(
        &self,
        request: &EnvironmentStartRequest,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        self.0.start_execution(request, deadline)
    }

    fn observe_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        self.0.observe_execution(handle, deadline)
    }

    fn interrupt_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        self.0.interrupt_execution(handle, deadline)
    }

    fn terminate_execution(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<ExecutionObservation> {
        self.0.terminate_execution(handle, deadline)
    }

    fn collect_outcome(
        &self,
        handle: &RuntimeHandle,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<PhysicalExecutionOutcome> {
        self.0.collect_outcome(handle, deadline)
    }

    fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
        deadline: &AdapterDeadline,
    ) -> AdapterResult<StartObservation> {
        self.0
            .reconcile_start(request_id, persisted_handle, deadline)
    }
}

impl ImportableAdapter for TestImportableAdapter {
    fn import_kind(&self) -> &str {
        "process"
    }

    fn import_binding_key(&self) -> AdapterBindingKey {
        AdapterBindingKey::new("test-key").unwrap()
    }

    fn import_attempt_isolation(&self) -> bool {
        false
    }
}

fn test_store(name: &str) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/tmp"));
    let _ = std::fs::create_dir_all(&base);
    let path = base.join(format!("{name}.sqlite"));
    let _ = std::fs::remove_file(&path);
    path
}

fn fake_composition() -> (ExecutionRegistry, AdapterRegistry) {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(ExecutionTargetConfig::new("local", "process", false))
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();

    let fake = Arc::new(TestImportableAdapter(Arc::new(FakeAdapter::new())));
    let mut adapters = AdapterRegistry::new();
    adapters
        .import_source(
            fake,
            AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap(),
        )
        .unwrap();
    (registry, adapters)
}

#[test]
fn test_semantic_control_e2e_through_daemon() {
    let path = test_store("sem_e2e");
    let store_cfg = SqliteRuntimeConfig::new(&path, 30.0, 16_384).unwrap();
    let (registry, adapters) = fake_composition();

    let retry_policy = NotifierRetryPolicy::new(0.1, 1.0).unwrap();
    let notifier = NotifierBinding::Enabled {
        config: NotifierConfig::new(0.5, 3, retry_policy).unwrap(),
        bridge: Arc::new(RecordingRootBridge::new()),
    };

    let daemon = SchedulerDaemonBuilder::new(
        store_cfg,
        RuntimeTimingConfig::new(1.0, 2.0, 30.0).unwrap(),
        PhysicalObserverConfig::new(1.0, 4.0, 8, 30.0).unwrap(),
        registry,
        adapters,
        notifier,
    )
    .unwrap()
    .start()
    .unwrap();

    assert_eq!(daemon.phase(), DaemonPhase::Ready);

    let control = daemon.control();
    control
        .upsert_partition(&PartitionSpec::new(
            "default",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();

    let sem = daemon.semantic_control();
    let gen = sem
        .create_generation(serde_json::json!({"objective": "investigate_performance"}))
        .unwrap();
    assert_eq!(gen.state, GenerationState::Open);

    let intent = RawWorkIntent {
        raw_intent_key: "profile_cpu".into(),
        objective: "Capture CPU profile".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(
            agentype_core::TaskSpec::new("profile_cpu", serde_json::json!({})).partition("default"),
        ),
    };

    let prop = sem
        .compile_intent(&gen.generation_id, intent, "root", "root_cli", 1)
        .unwrap();

    let task_id = sem.admit_proposal(&prop.proposal_id, 0, None).unwrap();

    let view = sem.read_generation_view(&gen.generation_id).unwrap();
    assert_eq!(view.admitted_task_ids, vec![task_id.clone()]);
    assert!(!view.is_settled);

    sem.freeze_generation(&gen.generation_id, 0).unwrap();

    // Cancel task to make terminal
    control.cancel_task(&task_id).unwrap();

    let view_settled = sem.read_generation_view(&gen.generation_id).unwrap();
    assert!(view_settled.is_settled);

    sem.close_generation(&gen.generation_id, 1).unwrap();

    let view_closed = sem.read_generation_view(&gen.generation_id).unwrap();
    assert_eq!(view_closed.generation.state, GenerationState::Closed);

    daemon.join();
    let _ = std::fs::remove_file(path);
}
