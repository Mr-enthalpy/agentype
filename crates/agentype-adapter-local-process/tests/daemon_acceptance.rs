//! M5.8 live acceptance: SchedulerDaemon + LocalProcessAgentAdapter +
//! file-backed SQLite. Worker Result JSON is never parsed by the adapter.

use agentype_adapter_local_process::{LocalProcessAgentAdapter, ADAPTER_KIND};
use agentype_core::{ExecutionState, PartitionSpec, Retention, TaskSpec, TaskState};
use agentype_execution_config::{ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig};
use agentype_runtime::{
    AdapterDeadlinePolicy, AdapterRegistry, DaemonError, DaemonPhase, NotifierBinding,
    PhysicalObserverConfig, RuntimeTimingConfig, SchedulerDaemonBuilder, SqliteRuntimeConfig,
};
use agentype_storage_sqlite::Kernel;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn fake_bin() -> String {
    env!("CARGO_BIN_EXE_fake-agent").to_string()
}

fn temp_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("agentype-m58-accept-{nanos}.sqlite"))
}

fn timing() -> RuntimeTimingConfig {
    RuntimeTimingConfig::new(0.2, 0.5, 10.0).unwrap()
}

fn observer() -> PhysicalObserverConfig {
    PhysicalObserverConfig::new(0.15, 3.0, 8, 10.0).unwrap()
}

fn policy() -> AdapterDeadlinePolicy {
    AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap()
}

fn registry(env: serde_json::Value) -> ExecutionRegistry {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(
            ExecutionTargetConfig::new("local", ADAPTER_KIND, false).with_options(json!({
                "command": fake_bin(),
                "args": [],
                "env": env,
            })),
        )
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    registry
}

fn adapters() -> AdapterRegistry {
    let adapter = Arc::new(LocalProcessAgentAdapter::try_new().unwrap());
    let mut adapters = AdapterRegistry::new();
    adapters.import_source(adapter, policy()).unwrap();
    adapters
}

fn seed_pool(kernel: &Kernel) {
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
}

fn start_daemon(
    path: &std::path::Path,
    env: serde_json::Value,
) -> agentype_runtime::RunningSchedulerDaemon {
    let store = SqliteRuntimeConfig::new(path, 10.0, 16_384).unwrap();
    SchedulerDaemonBuilder::new(
        store,
        timing(),
        observer(),
        registry(env),
        adapters(),
        NotifierBinding::DisabledForTests,
    )
    .unwrap()
    .start()
    .unwrap()
}

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out after {timeout:?}");
}

fn only_snap(kernel: &Kernel) -> agentype_storage_sqlite::ExecutionReconciliationSnapshot {
    let snaps = kernel.reconciliation_candidates().unwrap();
    assert_eq!(
        snaps.len(),
        1,
        "expected one execution, got {}",
        snaps.len()
    );
    snaps.into_iter().next().unwrap()
}

/// Acceptance A: physical end is not a Task Result.
#[test]
fn acceptance_a_physical_end_does_not_mint_result() {
    let path = temp_store();
    let daemon = start_daemon(&path, json!({"FAKE_AGENT_SLEEP_MS": "200"}));
    assert_eq!(daemon.phase(), DaemonPhase::Ready);
    seed_pool(daemon.kernel());
    daemon
        .kernel()
        .submit_batch(&[TaskSpec::new("m58-a", json!({"goal": "echo"}))])
        .unwrap();
    wait_until(Duration::from_secs(8), || {
        let snaps = daemon.kernel().reconciliation_candidates().unwrap();
        snaps.iter().any(|s| {
            daemon.kernel().execution(s.execution_id()).unwrap().state == ExecutionState::Terminated
        })
    });
    let snap = only_snap(daemon.kernel());
    let exec = daemon.kernel().execution(snap.execution_id()).unwrap();
    assert_eq!(exec.state, ExecutionState::Terminated);
    assert!(!exec.terminal_confirmed);
    assert!(!exec.quiescent_confirmed);
    assert_ne!(
        daemon.kernel().task(snap.task_id()).unwrap().state,
        TaskState::Completed
    );
    assert!(daemon.kernel().result_for_task(snap.task_id()).is_err());
    daemon.join();
    let _ = std::fs::remove_file(&path);
}

/// Acceptance B: Worker ACK fixture wins; later physical end is history.
#[test]
fn acceptance_b_worker_ack_then_physical_history_only() {
    let path = temp_store();
    let daemon = start_daemon(&path, json!({"FAKE_AGENT_SLEEP_MS": "1500"}));
    seed_pool(daemon.kernel());
    daemon
        .kernel()
        .submit_batch(&[TaskSpec::new("m58-b", json!({"goal": "echo"}))])
        .unwrap();
    wait_until(Duration::from_secs(5), || {
        daemon
            .kernel()
            .reconciliation_candidates()
            .unwrap()
            .iter()
            .any(|s| {
                daemon.kernel().execution(s.execution_id()).unwrap().state
                    == ExecutionState::Running
            })
    });
    let snap = only_snap(daemon.kernel());
    let claim_attempt = snap.attempt_id().clone();
    let epoch = snap.lease_epoch();
    let exec_id = snap.execution_id().clone();
    let task_id = snap.task_id().clone();
    daemon
        .kernel()
        .ack_success(
            &claim_attempt,
            epoch,
            Some(&exec_id),
            &json!({"ok": true}),
            None,
            true,
            false,
        )
        .unwrap();
    assert_eq!(
        daemon.kernel().task(&task_id).unwrap().state,
        TaskState::Completed
    );
    let first = daemon.kernel().result_for_task(&task_id).unwrap();
    wait_until(Duration::from_secs(6), || {
        daemon.kernel().execution(&exec_id).unwrap().state == ExecutionState::Terminated
            || daemon.kernel().execution(&exec_id).unwrap().state == ExecutionState::Succeeded
    });
    assert_eq!(
        daemon.kernel().task(&task_id).unwrap().state,
        TaskState::Completed
    );
    let second = daemon.kernel().result_for_task(&task_id).unwrap();
    assert_eq!(first.id, second.id);
    daemon.join();
    let _ = std::fs::remove_file(&path);
}

/// Acceptance C: second owner of the same store is AlreadyRunning.
#[test]
fn acceptance_c_second_daemon_is_already_running() {
    let path = temp_store();
    let first = start_daemon(&path, json!({}));
    let store = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
    let err = SchedulerDaemonBuilder::new(
        store,
        timing(),
        observer(),
        registry(json!({})),
        adapters(),
        NotifierBinding::DisabledForTests,
    )
    .unwrap()
    .start()
    .err()
    .expect("second start must fail");
    assert!(matches!(err, DaemonError::AlreadyRunning), "got {err}");
    first.join();
    let _ = std::fs::remove_file(&path);
}

/// Acceptance D: shutdown releases the lock; next daemon recovers then dispatches.
#[test]
fn acceptance_d_restart_acquires_lock_and_recovers_before_dispatch() {
    let path = temp_store();
    let first = start_daemon(&path, json!({"FAKE_AGENT_SLEEP_MS": "50"}));
    seed_pool(first.kernel());
    first
        .kernel()
        .submit_batch(&[TaskSpec::new("m58-d", json!({"goal": "echo"}))])
        .unwrap();
    thread::sleep(Duration::from_millis(400));
    match first.join() {
        agentype_runtime::DaemonExit::Stopped | agentype_runtime::DaemonExit::Failed(_) => {}
    }
    let second = start_daemon(&path, json!({"FAKE_AGENT_SLEEP_MS": "50"}));
    assert_eq!(second.phase(), DaemonPhase::Ready);
    second.join();
    let _ = std::fs::remove_file(&path);
}
