//! Canonical M5.6→M5.7 path: Dispatcher / Recovery through a real
//! LocalProcessAgentAdapter. Runtime does not depend on this crate in
//! production; this integration test is the only wiring.

use agentype_adapter_api::{AdapterDeadline, ExecutionAdapter, RuntimeHandle};
use agentype_adapter_local_process::{LocalProcessAgentAdapter, ADAPTER_KIND};
use agentype_core::{Clock, ManualClock, PartitionSpec, Retention, TaskSpec};
use agentype_execution_config::{
    AdapterBindingKey, ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig,
};
use agentype_runtime::{
    recover_runtime_without_notifier, AdapterDeadlinePolicy, AdapterRegistry, DispatchOneOutcome,
    Dispatcher, RuntimeTimingConfig,
};
use agentype_storage_sqlite::Kernel;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn fake_bin() -> String {
    env!("CARGO_BIN_EXE_fake-agent").to_string()
}

fn timing() -> RuntimeTimingConfig {
    RuntimeTimingConfig::new(1.0, 2.0, 10.0).unwrap()
}

fn policy() -> AdapterDeadlinePolicy {
    AdapterDeadlinePolicy::uniform(Duration::from_secs(8)).unwrap()
}

fn kernel() -> (Arc<ManualClock>, Kernel) {
    let clock = Arc::new(ManualClock::new(1_000.0));
    let kernel = Kernel::open_memory(clock.clone() as Arc<dyn Clock>, 10.0, 16_384).unwrap();
    kernel
        .upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    kernel.reconcile_pool().unwrap();
    (clock, kernel)
}

fn registry_with(command_env: serde_json::Value) -> ExecutionRegistry {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(
            ExecutionTargetConfig::new("local", ADAPTER_KIND, false).with_options(json!({
                "command": fake_bin(),
                "args": [],
                "env": command_env,
            })),
        )
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    registry
}

fn only_execution_id(kernel: &Kernel) -> agentype_core::ExecutionId {
    let snaps = kernel.reconciliation_candidates().unwrap();
    assert_eq!(snaps.len(), 1);
    snaps[0].execution_id().clone()
}

#[test]
fn dispatcher_starts_and_collects_real_fake_agent() {
    let (_clock, kernel) = kernel();
    let adapter = Arc::new(LocalProcessAgentAdapter::new());
    let mut adapters = AdapterRegistry::new();
    adapters
        .register(
            ADAPTER_KIND,
            adapter.binding_key().clone(),
            adapter,
            policy(),
        )
        .unwrap();
    let registry = registry_with(json!({}));
    kernel
        .submit_batch(&[TaskSpec::new("real-collect", json!({"goal": "echo"}))])
        .unwrap();
    let d = Dispatcher::new(&kernel, &registry, &adapters);
    match d.dispatch_one().unwrap() {
        DispatchOneOutcome::TaskCompleted { .. } => {}
        DispatchOneOutcome::RunningAdmitted { admission } => {
            drop(admission);
            let execution_id = only_execution_id(&kernel);
            let handle = RuntimeHandle(kernel.execution_runtime_handle(&execution_id).unwrap());
            let binding = adapters.resolve_unique(ADAPTER_KIND).unwrap();
            let collected = binding.collect_outcome(&handle).unwrap();
            assert_eq!(collected.state, agentype_core::ExecutionState::Succeeded);
        }
        other => panic!("expected TaskCompleted or RunningAdmitted, got {other:?}"),
    }
}

#[test]
fn recovery_readmits_same_binding_key_after_new_adapter_instance() {
    let (_clock, kernel) = kernel();
    let kernel = Arc::new(kernel);
    let adapter = Arc::new(LocalProcessAgentAdapter::new());
    let key = adapter.binding_key().clone();
    let mut adapters = AdapterRegistry::new();
    adapters
        .register(ADAPTER_KIND, key.clone(), adapter, policy())
        .unwrap();
    let registry = registry_with(json!({"FAKE_AGENT_HANG": "1"}));
    kernel
        .submit_batch(&[TaskSpec::new("hang-recover", json!({}))])
        .unwrap();
    let d = Dispatcher::new(&kernel, &registry, &adapters);
    match d.dispatch_one().unwrap() {
        DispatchOneOutcome::RunningAdmitted { admission } => drop(admission),
        other => panic!("expected RunningAdmitted, got {other:?}"),
    }
    let execution_id = only_execution_id(&kernel);
    let handle = RuntimeHandle(kernel.execution_runtime_handle(&execution_id).unwrap());

    drop(adapters);

    let adapter2 = Arc::new(LocalProcessAgentAdapter::new());
    assert_eq!(adapter2.binding_key(), &key);
    let mut adapters2 = AdapterRegistry::new();
    adapters2
        .register(
            ADAPTER_KIND,
            adapter2.binding_key().clone(),
            adapter2.clone(),
            policy(),
        )
        .unwrap();
    let recovered = recover_runtime_without_notifier(kernel.clone(), &adapters2, timing()).unwrap();
    assert!(
        recovered.runner().contains(&execution_id),
        "same boot/domain key must allow reconcile RUNNING"
    );

    adapter2
        .terminate_execution(
            &handle,
            &AdapterDeadline::after(Duration::from_secs(8)).unwrap(),
        )
        .unwrap();
}

#[test]
fn recovery_does_not_readmit_foreign_binding_key() {
    let (_clock, kernel) = kernel();
    let kernel = Arc::new(kernel);
    let adapter = Arc::new(LocalProcessAgentAdapter::new());
    let mut adapters = AdapterRegistry::new();
    adapters
        .register(
            ADAPTER_KIND,
            AdapterBindingKey::for_tests(),
            adapter,
            policy(),
        )
        .unwrap();
    let registry = registry_with(json!({"FAKE_AGENT_HANG": "1"}));
    kernel
        .submit_batch(&[TaskSpec::new("hang-foreign", json!({}))])
        .unwrap();
    let d = Dispatcher::new(&kernel, &registry, &adapters);
    match d.dispatch_one().unwrap() {
        DispatchOneOutcome::RunningAdmitted { admission } => drop(admission),
        other => panic!("expected RunningAdmitted, got {other:?}"),
    }
    let execution_id = only_execution_id(&kernel);
    let handle = RuntimeHandle(kernel.execution_runtime_handle(&execution_id).unwrap());

    drop(adapters);

    let adapter2 = Arc::new(LocalProcessAgentAdapter::new());
    assert_ne!(
        adapter2.binding_key().as_str(),
        AdapterBindingKey::for_tests().as_str()
    );
    let mut adapters2 = AdapterRegistry::new();
    adapters2
        .register(
            ADAPTER_KIND,
            adapter2.binding_key().clone(),
            adapter2.clone(),
            policy(),
        )
        .unwrap();
    let recovered = recover_runtime_without_notifier(kernel.clone(), &adapters2, timing()).unwrap();
    assert!(
        !recovered.runner().contains(&execution_id),
        "foreign domain key must not re-admit"
    );

    adapter2
        .terminate_execution(
            &handle,
            &AdapterDeadline::after(Duration::from_secs(8)).unwrap(),
        )
        .ok();
}

#[test]
fn isolated_target_cannot_use_local_process_adapter() {
    let (_clock, kernel) = kernel();
    let adapter = Arc::new(LocalProcessAgentAdapter::new());
    let mut adapters = AdapterRegistry::new();
    adapters
        .register(
            ADAPTER_KIND,
            adapter.binding_key().clone(),
            adapter,
            policy(),
        )
        .unwrap();
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(
            ExecutionTargetConfig::new("local", ADAPTER_KIND, true).with_options(json!({
                "command": fake_bin(),
                "args": [],
                "env": {},
            })),
        )
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    kernel
        .submit_batch(&[TaskSpec::new("isolated-local", json!({}))])
        .unwrap();
    let d = Dispatcher::new(&kernel, &registry, &adapters);
    match d.dispatch_one().unwrap() {
        DispatchOneOutcome::ConfigurationUnavailable { .. } => {}
        other => panic!("expected ConfigurationUnavailable, got {other:?}"),
    }
    assert!(kernel.reconciliation_candidates().unwrap().is_empty());
}
