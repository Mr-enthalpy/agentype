//! M6-B.4 typed physical execution → durable ACK interface: typed admission ->
//! real B.3 matching -> real B.4
//! physical eligibility -> Attempt/Lease/ProvisioningBinding -> pure prepare ->
//! Execution/BindingSnapshot -> exact adapter start -> observation -> durable
//! Result ACK boundary.
//!
//! This drives the **production** `SchedulerDaemon` composition root. The typed
//! Task is not dispatched by a test-only helper: the daemon's `ControlLoopService`
//! enumerates it, the authoritative acquisition re-proves every conjunct, the
//! real `WorkspaceJailSourceIntegration` attests/prepares the descriptor, and the
//! real `JailedProcessAgentAdapter` physically creates and starts the jailed
//! process. The assertions read only durable Scheduler state plus the physical
//! jail directory, so a test-only shortcut cannot satisfy them.
//!
//! **ACK boundary.** The harness calls `Kernel::ack_success` directly, after
//! reading the committed Attempt/Lease/Execution. There is no production Worker
//! Result transport, worker authentication, delivery retry, or worker-originated
//! ACK in this scope (B.5 credentials and a worker data plane are deferred). What
//! this suite proves is the durable Kernel command boundary: a valid lease-epoch
//! ACK completes the Task, and an invalid writer-safety/quiescence ACK is
//! refused. It does not prove a production worker-originated ACK path.
//!
//! Linux enforcement (Landlock) and Windows enforcement (restricted token) are
//! both real; on other platforms the adapter is honestly unable to enforce and
//! the suite is skipped.
#![cfg(any(windows, target_os = "linux"))]

use agentype_adapter_api::{AdapterDeadline, ExecutionAdapter, RuntimeHandle};
use agentype_adapter_local_process::{
    JailedProcessAgentAdapter, LocalProcessAgentAdapter,
    ADAPTER_KIND as LOCAL_PROCESS_ADAPTER_KIND, JAIL_ADAPTER_KIND, JAIL_PROVISIONING_PROTOCOL,
};
use agentype_agent_contract::{
    canonical_json_body_digest, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentRequirementDraft, AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget,
    ConfigDigest, ConfigStatus, ContinuityMode, LifecycleMode, MaterializationDigest,
    NetworkPolicy, PhysicalSafety, SecurityContract, SourceConfig, SourceConfigRef, SourceStatus,
    SpawnSource, SpawnSourceRef,
};
use agentype_core::{
    ExecutionId, ExecutionState, InformationFunction, PartitionSpec, RawWorkIntent, Retention,
    SemanticInputSet, SystemClock, TaskId, TaskSpec, TaskState, WorkspaceMode,
};
use agentype_execution_config::{
    AdapterBindingKey, ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig,
};
use agentype_runtime::{
    AdapterDeadlinePolicy, AdapterRegistry, DaemonExit, DaemonPhase, NotifierBinding,
    PhysicalObserverConfig, PreparedSource, ProvisioningResolutionError, RuntimeTimingConfig,
    SchedulerDaemonBuilder, SourceConfigIntegration, SourceIntegrationRegistry,
    SqliteRuntimeConfig, TypedDispatchConfig,
};
use agentype_source_workspace_jail::{workspace_jail_config_body, WorkspaceJailSourceIntegration};
use agentype_storage_sqlite::{Kernel, SourceConfigBody, SourceConfigRevision};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static TEST_LOCK: Mutex<()> = Mutex::new(());

const MAX_BYTES: usize = 16_384;

fn fake_bin() -> String {
    env!("CARGO_BIN_EXE_fake-agent").to_string()
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn test_dir(tag: &str) -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b4-e2e-{tag}-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn timing() -> RuntimeTimingConfig {
    RuntimeTimingConfig::new(0.05, 0.5, 10.0).unwrap()
}

fn observer() -> PhysicalObserverConfig {
    PhysicalObserverConfig::new(0.15, 3.0, 8, 10.0).unwrap()
}

fn deadlines() -> AdapterDeadlinePolicy {
    AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap()
}

fn typed_config() -> TypedDispatchConfig {
    TypedDispatchConfig::new(
        8,
        Duration::from_millis(100),
        Duration::from_secs(30),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .unwrap()
}

fn execution_registry() -> ExecutionRegistry {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(ExecutionTargetConfig::new("jail", JAIL_ADAPTER_KIND, false))
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    // The legacy queue targets the ordinary local-process adapter. Its command
    // comes from target options, never from the typed BindingSnapshot.
    registry
        .register_target(
            ExecutionTargetConfig::new("local", LOCAL_PROCESS_ADAPTER_KIND, false).with_options(
                json!({
                    "command": fake_bin(),
                    "args": ["--sleep-ms", "5000"],
                }),
            ),
        )
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("legacy-default"))
        .unwrap();
    registry
}

fn safety() -> PhysicalSafety {
    PhysicalSafety::new(
        false,
        vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write],
        [NetworkPolicy::Enabled]
            .into_iter()
            .collect::<BTreeSet<_>>(),
    )
    .unwrap()
}

/// The harness owns the daemon, the store, the physical jail base, and the exact
/// catalog identities the typed Task is pinned to.
struct Harness {
    daemon: Option<agentype_runtime::RunningSchedulerDaemon>,
    kernel: Kernel,
    db: PathBuf,
    jail_root: PathBuf,
    source_ref: SpawnSourceRef,
    policy_ref: AdapterPolicyRef,
    binding_key: AdapterBindingKey,
}

impl Harness {
    /// Start the production daemon with one exact source integration. The catalog
    /// is published afterwards so no test-only dispatch path can observe a
    /// pre-seeded execution.
    fn start(tag: &str, integration: Arc<dyn SourceConfigIntegration>) -> Self {
        Self::start_at(test_dir(tag), tag, integration)
    }

    /// Start the production daemon against an explicit directory. Used by the
    /// restart test, which must reopen the same store after a real crash.
    fn start_at(dir: PathBuf, tag: &str, integration: Arc<dyn SourceConfigIntegration>) -> Self {
        let db = dir.join("scheduler.db");
        let jail_root = dir.join("jail-base");
        let source_ref = SpawnSourceRef::new(format!("source-{tag}"), 1).unwrap();

        let adapter = Arc::new(JailedProcessAgentAdapter::try_new().unwrap());
        let binding_key = adapter.binding_key().clone();
        let mut adapters = AdapterRegistry::new();
        // The policy below resolves `binding_ref = "primary"`; the import must
        // declare exactly that alias or enumeration sees no candidate.
        adapters
            .import_source_with_binding_ref(adapter, "primary", deadlines())
            .unwrap();
        // A separate local-process adapter serves the legacy queue; it shares no
        // binding key or descriptor with the typed jail path.
        adapters
            .import_source_with_binding_ref(
                Arc::new(LocalProcessAgentAdapter::try_new().unwrap()),
                "legacy",
                deadlines(),
            )
            .unwrap();

        let mut integrations = SourceIntegrationRegistry::new();
        integrations
            .register(source_ref.clone(), integration)
            .unwrap();

        let store = SqliteRuntimeConfig::new(&db, 10.0, MAX_BYTES).unwrap();
        let daemon = SchedulerDaemonBuilder::new(
            store,
            timing(),
            observer(),
            execution_registry(),
            adapters,
            NotifierBinding::DisabledForTests,
        )
        .unwrap()
        .typed_dispatch(integrations, typed_config())
        .start()
        .unwrap();
        assert_eq!(daemon.phase(), DaemonPhase::Ready);

        let kernel = Kernel::open(&db, Arc::new(SystemClock), 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                4,
                Retention::Ephemeral,
                "jail",
                "default",
            ))
            .unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "legacy",
                4,
                Retention::Ephemeral,
                "local",
                "legacy-default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();

        Self {
            daemon: Some(daemon),
            kernel,
            db,
            jail_root,
            source_ref,
            policy_ref: AdapterPolicyRef::new(format!("policy-{tag}"), 1).unwrap(),
            binding_key,
        }
    }

    /// Publish the complete immutable catalog for one AgentType revision.
    fn publish(
        &self,
        agent_type_id: &str,
        requires_attempt_isolation: bool,
        binding_ref: &str,
        config: &serde_json::Value,
    ) -> AgentTypeRef {
        let policy = AdapterBindingPolicy {
            policy_ref: self.policy_ref.clone(),
            adapter_kind: JAIL_ADAPTER_KIND.into(),
            binding_ref: binding_ref.into(),
            required_safety: safety(),
            status: ConfigStatus::Active,
        };
        self.kernel.publish_adapter_binding_policy(&policy).unwrap();

        let source = SpawnSource {
            source_ref: self.source_ref.clone(),
            adapter_policy: policy.policy_ref.clone(),
            lifecycle_modes: [LifecycleMode::Ephemeral].into_iter().collect(),
            continuity_modes: [ContinuityMode::None].into_iter().collect(),
            functional_envelope: BTreeMap::new(),
            claims: Vec::new(),
            status: SourceStatus::Active,
        };
        self.kernel.publish_spawn_source(&source).unwrap();

        let digest = ConfigDigest::new(canonical_json_body_digest(config)).unwrap();
        let source_config = SourceConfig {
            config_ref: SourceConfigRef::new(
                source.source_ref.clone(),
                format!("config-{agent_type_id}"),
                1,
            )
            .unwrap(),
            config_digest: digest,
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: Vec::new(),
            claims: Vec::new(),
            status: ConfigStatus::Active,
        };
        self.kernel
            .publish_source_config(
                &source_config,
                &SourceConfigBody::OpaqueJson(config.clone()),
            )
            .unwrap();

        let pin = AgentTypeRef::new(agent_type_id, 1).unwrap();
        let agent = AgentType {
            type_ref: pin.clone(),
            based_on: None,
            contract: AgentTypeContract {
                allowed_information_functions: vec![InformationFunction::Expand],
                required_capabilities: BTreeMap::new(),
                affinity: AffinityConstraint::Any,
                budget_ceiling: Budget::new(100.0).unwrap(),
                security: SecurityContract {
                    // A Write ceiling still permits a ReadOnly Task; the physical
                    // jail proves whichever mode the Task actually requests.
                    workspace: WorkspaceMode::Write,
                    network: NetworkPolicy::Enabled,
                    requires_attempt_isolation,
                },
                lifecycle: [LifecycleMode::Ephemeral].into_iter().collect(),
                continuity: ContinuityMode::None,
                sandbox_policy: None,
                anchor_constraint: None,
            },
        };
        self.kernel.publish_agent_type(&agent).unwrap();
        pin
    }

    /// Publish the real workspace-jail source config for the fixture worker.
    fn publish_workspace_jail(
        &self,
        agent_type_id: &str,
        requires_attempt_isolation: bool,
        binding_ref: &str,
        extra_args: &[&str],
    ) -> AgentTypeRef {
        let mut args = Vec::new();
        if !extra_args.contains(&"--sleep-ms") {
            args.push("--sleep-ms".to_string());
            args.push("5000".to_string());
        }
        args.extend(extra_args.iter().map(|arg| (*arg).to_string()));
        let body = workspace_jail_config_body(
            fake_bin(),
            args,
            self.jail_root.to_string_lossy().to_string(),
        );
        self.publish(
            agent_type_id,
            requires_attempt_isolation,
            binding_ref,
            &body,
        )
    }

    fn admit(
        &self,
        pin: &AgentTypeRef,
        spec: TaskSpec,
        required_network: NetworkPolicy,
    ) -> agentype_core::TaskId {
        let generation = self.kernel.create_generation(json!({})).unwrap();
        let intent = RawWorkIntent {
            raw_intent_key: format!("typed-{}", nanos()),
            objective: "typed end-to-end".into(),
            information_function: InformationFunction::Expand,
            semantic_input_set: SemanticInputSet::new(),
            rationale: None,
            suggested_task_spec: Some(spec),
        };
        let proposal = self
            .kernel
            .compile_root_intent(&generation.generation_id, intent, "session", 1)
            .unwrap();
        let draft = AgentRequirementDraft {
            required_type: AgentTypeSelector::Exact(pin.clone()),
            required_capabilities: BTreeMap::new(),
            required_network,
            required_attempt_isolation: false,
            sandbox_policy: None,
            required_anchor: None,
            budget: Budget::new(50.0).unwrap(),
        };
        self.kernel
            .admit_typed_proposal(&proposal.proposal_id, 0, None, draft)
            .unwrap()
    }

    /// Admit a legacy (no typed requirement) Task. Uses the same duplicate
    /// generation/admission primitives as a real legacy front door.
    fn admit_legacy(&self, spec: TaskSpec) -> agentype_core::TaskId {
        let generation = self.kernel.create_generation(json!({})).unwrap();
        let intent = RawWorkIntent {
            raw_intent_key: format!("legacy-{}", nanos()),
            objective: "legacy shared queue".into(),
            information_function: InformationFunction::Expand,
            semantic_input_set: SemanticInputSet::new(),
            rationale: None,
            suggested_task_spec: Some(spec),
        };
        let proposal = self
            .kernel
            .compile_root_intent(&generation.generation_id, intent, "session", 1)
            .unwrap();
        self.kernel
            .admit_proposal(&proposal.proposal_id, 0, None)
            .unwrap()
    }

    fn snapshots(&self) -> Vec<agentype_storage_sqlite::ExecutionReconciliationSnapshot> {
        self.kernel.reconciliation_candidates().unwrap()
    }

    fn only_snapshot(&self) -> agentype_storage_sqlite::ExecutionReconciliationSnapshot {
        let snaps = self.snapshots();
        assert_eq!(snaps.len(), 1, "expected exactly one Execution");
        snaps.into_iter().next().unwrap()
    }

    fn row_count(&self, table: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.busy_timeout(Duration::from_secs(10)).unwrap();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
    }

    fn jail_execution_dir(&self, request_id: &str) -> PathBuf {
        self.jail_root
            .join(format!("agentype-jail-{}", safe_component(request_id)))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.join();
        }
    }
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out after {timeout:?}");
}

fn wait_running(kernel: &Kernel, task: &agentype_core::TaskId) {
    wait_until(Duration::from_secs(15), || {
        kernel
            .reconciliation_candidates()
            .unwrap()
            .iter()
            .any(|snap| {
                snap.task_id() == task
                    && kernel.execution(snap.execution_id()).unwrap().state
                        == ExecutionState::Running
            })
    });
}

fn assert_no_authority(harness: &Harness, task: &agentype_core::TaskId) {
    assert_eq!(
        harness.row_count("attempts"),
        0,
        "an unprovisionable typed Task must not fabricate an Attempt"
    );
    assert!(
        harness.snapshots().is_empty(),
        "an unprovisionable typed Task must not fabricate an Execution"
    );
    for table in [
        "attempts",
        "leases",
        "executions",
        "binding_snapshots",
        "failures",
    ] {
        assert_eq!(
            harness.row_count(table),
            0,
            "{table} must stay empty before the typed Task is provisionable"
        );
    }
    assert_eq!(
        harness.kernel.task(task).unwrap().state,
        TaskState::Queued,
        "typed unavailability before Attempt is a deferral, not a Task failure"
    );
    assert!(harness.kernel.result_for_task(task).is_err());
    assert!(harness.kernel.open_escalation_for_task(task).is_err());
}

const CRASH_CHILD_ENV: &str = "AGENTYPE_TYPED_DAEMON_CRASH_CHILD";

/// Child half of `typed_daemon_restart_recovers_the_same_physical_instance`.
///
/// This test is a no-op unless the parent invokes the same test binary with an
/// explicit environment variable. When invoked, it starts the real production
/// daemon, admits and starts one typed Task, waits until the jailed worker has
/// proven it is fully initialized, records the exact physical identity, and then
/// calls `abort()`: a genuine process crash with no Drop, shutdown, or lease
/// expiry.
#[test]
fn typed_daemon_crash_child_host() {
    let Ok(root) = std::env::var(CRASH_CHILD_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let harness = Harness::start_at(
        root.clone(),
        "crash-child",
        Arc::new(WorkspaceJailSourceIntegration::new()),
    );
    let pin = harness.publish_workspace_jail(
        "crash-worker",
        false,
        "primary",
        &["--sleep-ms", "0", "--ready-probe", "ready.txt", "--hang"],
    );
    let agent = harness.kernel.ready_agent("general").unwrap();
    harness
        .kernel
        .bind_logical_agent_type(&agent, &pin)
        .unwrap();
    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-crash", json!({"goal": "crash"}))
            .partition("general")
            .write(),
        NetworkPolicy::Enabled,
    );

    wait_running(&harness.kernel, &task);
    let snap = harness.only_snapshot();
    let jail_dir = harness.jail_execution_dir(snap.request_id().as_str());
    let ready = jail_dir.join("workspace").join("ready.txt");
    wait_until(Duration::from_secs(20), || ready.is_file());

    let info = json!({
        "task": task.as_str(),
        "execution": snap.execution_id().as_str(),
        "request": snap.request_id().as_str(),
        "pid": snap.runtime_handle()["pid"],
        "jail_dir": jail_dir.to_string_lossy(),
    });
    std::fs::write(root.join("child-info.json"), info.to_string()).unwrap();

    // The worker wrote its ready marker, so user32/DLL initialization is
    // complete. Now crash the daemon for real.
    std::process::abort();
}

/// A real typed daemon crash after Execution commitment: the parent reopens the
/// same store and must recover the *same* physical worker, not respawn.
#[test]
fn typed_daemon_restart_recovers_the_same_physical_instance() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let dir = test_dir("restart");

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "typed_daemon_crash_child_host", "--nocapture"])
        .env(CRASH_CHILD_ENV, &dir)
        .output()
        .expect("the crash host must start");
    assert!(
        !output.status.success(),
        "the child daemon must die by abort, not by a clean shutdown: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let info: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("child-info.json"))
            .expect("the crashed child must have recorded its physical identity"),
    )
    .expect("child info is JSON");
    let task = TaskId::from_string(info["task"].as_str().unwrap());
    let execution_id = ExecutionId::from_string(info["execution"].as_str().unwrap());
    let crashed_pid = info["pid"].as_u64().expect("child recorded an exact pid");
    assert_ne!(crashed_pid, 0);

    // Reopen the same SQLite store with a fresh daemon. Its startup recovery
    // must adopt the durable locator and the same live physical process.
    let mut harness = Harness::start_at(
        dir.clone(),
        "restart-parent",
        Arc::new(WorkspaceJailSourceIntegration::new()),
    );
    wait_until(Duration::from_secs(20), || {
        harness
            .kernel
            .execution(&execution_id)
            .map(|execution| execution.state == ExecutionState::Running)
            .unwrap_or(false)
    });
    assert_eq!(
        harness.kernel.execution(&execution_id).unwrap().state,
        ExecutionState::Running,
        "restart recovery must re-adopt the same running physical execution"
    );
    assert_eq!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Running
    );

    let snap = harness
        .snapshots()
        .into_iter()
        .find(|snap| snap.execution_id() == &execution_id)
        .expect("the recovered Execution must remain a reconciliation candidate");
    assert_eq!(
        snap.runtime_handle()["pid"].as_u64(),
        Some(crashed_pid),
        "recovery must not substitute or respawn a different physical process: {}",
        snap.runtime_handle()
    );
    assert_eq!(
        harness.snapshots().len(),
        1,
        "restart recovery must not fabricate a second Execution"
    );

    // Non-impersonating witness: a fresh adapter (no in-memory state from the
    // crashed daemon) observes the recovered handle as the same running process.
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let deadline = AdapterDeadline::after(Duration::from_secs(5)).unwrap();
    let handle = RuntimeHandle(snap.runtime_handle().clone());
    let observed = adapter
        .observe_execution(&handle, &deadline)
        .expect("the recovered handle must be physically observable");
    assert_eq!(observed.state, ExecutionState::Running);
    let terminated = adapter
        .terminate_execution(&handle, &deadline)
        .expect("the recovered process must remain controllable");
    assert_eq!(terminated.state, ExecutionState::Terminated);

    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// With typed dispatch enabled, a legacy Task and a typed Task queued at the
/// same time must both be dispatched. Neither queue may starve the other.
#[test]
fn legacy_and_typed_tasks_are_both_dispatched_from_a_shared_queue() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start(
        "mixed-queue",
        Arc::new(WorkspaceJailSourceIntegration::new()),
    );
    let pin = harness.publish_workspace_jail("mixed-typed", false, "primary", &[]);

    // The legacy Task takes the existing unbound READY pool agent. The typed
    // Task deliberately uses the newborn path, so both are immediately
    // provisionable at the same time.
    let legacy =
        harness.admit_legacy(TaskSpec::new("legacy-shared-queue", json!({})).partition("legacy"));
    let typed = harness.admit(
        &pin,
        TaskSpec::new("typed-shared-queue", json!({})).partition("general"),
        NetworkPolicy::Enabled,
    );

    let queue = [legacy.clone(), typed.clone()];
    wait_until(Duration::from_secs(20), || {
        queue.iter().all(|task| {
            matches!(
                harness.kernel.task(task).unwrap().state,
                TaskState::Leased | TaskState::Running
            )
        })
    });
    wait_until(Duration::from_secs(20), || harness.snapshots().len() == 2);
    let dispatched: BTreeSet<TaskId> = harness
        .snapshots()
        .iter()
        .map(|snap| snap.task_id().clone())
        .collect();
    assert_eq!(
        dispatched,
        [legacy.clone(), typed.clone()].into_iter().collect(),
        "both queued sides must receive exactly one Execution"
    );

    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// The full happy path on a ReadOnly Task: production daemon, real source
/// integration, real jail, exact committed binding, and a durable Result ACK
/// (harness-injected `Kernel::ack_success`, not a worker transport) that
/// completes the Task while the physical execution is still RUNNING.
#[test]
fn typed_read_only_task_runs_end_to_end_and_acks_through_the_daemon() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start("readonly", Arc::new(WorkspaceJailSourceIntegration::new()));
    let pin = harness.publish_workspace_jail("reviewer", false, "primary", &[]);

    // Existing-agent path: bind the pool's READY agent to the exact revision.
    let agent = harness.kernel.ready_agent("general").unwrap();
    harness
        .kernel
        .bind_logical_agent_type(&agent, &pin)
        .unwrap();

    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-read-only", json!({"goal": "jail"})).partition("general"),
        NetworkPolicy::Enabled,
    );

    wait_running(&harness.kernel, &task);
    let snap = harness.only_snapshot();

    // Durable provenance: exactly the imported (kind, key), the committed
    // descriptor, and the frozen source digest.
    assert_eq!(snap.adapter_kind(), JAIL_ADAPTER_KIND);
    assert_eq!(snap.adapter_binding_key(), harness.binding_key.as_str());

    let execution_id = snap.execution_id().clone();
    let request_id = snap.request_id().as_str().to_string();
    let snapshot = harness
        .kernel
        .get_binding_snapshot(&execution_id)
        .unwrap()
        .expect("typed Execution must carry a BindingSnapshot");
    assert_eq!(snapshot.adapter_kind, JAIL_ADAPTER_KIND);
    assert_eq!(snapshot.adapter_binding_key, harness.binding_key.as_str());
    assert_eq!(snapshot.spawn_source, harness.source_ref);
    assert_eq!(snapshot.source_config_digest.as_str(), {
        let body = workspace_jail_config_body(
            fake_bin(),
            vec!["--sleep-ms".into(), "5000".into()],
            harness.jail_root.to_string_lossy().to_string(),
        );
        canonical_json_body_digest(&body)
    });
    let descriptor: serde_json::Value =
        serde_json::from_str(&snapshot.launch_descriptor).expect("descriptor is JSON");
    assert_eq!(
        descriptor["protocol"],
        serde_json::Value::String(JAIL_PROVISIONING_PROTOCOL.into())
    );
    assert_eq!(
        descriptor["jail_root"],
        serde_json::Value::String(harness.jail_root.to_string_lossy().into_owned())
    );

    // Physical evidence: the jailed process exists under the exact
    // request-derived root, and the runtime handle names the jail adapter.
    let jail_dir = harness.jail_execution_dir(&request_id);
    assert!(
        jail_dir.is_dir(),
        "the adapter must create the per-execution jail"
    );
    assert!(jail_dir.join("workspace").is_dir());
    assert_eq!(
        snap.runtime_handle()["kind"],
        serde_json::Value::String("jailed_process".into())
    );

    // Exactly one ProvisioningBinding, Immutable and bound to the same physical
    // domain the Execution froze.
    let binding = harness
        .kernel
        .get_provisioning_binding(snap.incarnation_id())
        .unwrap()
        .expect("typed acquisition must persist its provisioning binding");
    assert_eq!(binding.adapter_kind, JAIL_ADAPTER_KIND);
    assert_eq!(binding.adapter_binding_key, harness.binding_key.as_str());
    assert_eq!(binding.provisioning_protocol, JAIL_PROVISIONING_PROTOCOL);
    assert_eq!(binding.logical_agent_id, agent);
    eprintln!(
        "typed-e2e trace: task={task} execution={execution_id} request={request_id} \
         adapter={}/{} protocol={} jail={} agent={agent}",
        snap.adapter_kind(),
        snap.adapter_binding_key(),
        binding.provisioning_protocol,
        jail_dir.display()
    );

    // ACK while the worker is still physically RUNNING: this exercises the
    // durable ACK interface directly, not a worker-originated transport.
    assert_eq!(
        harness.kernel.execution(&execution_id).unwrap().state,
        ExecutionState::Running
    );
    let attempt = snap.attempt_id().clone();
    let epoch = snap.lease_epoch();
    harness
        .kernel
        .ack_success(
            &attempt,
            epoch,
            Some(&execution_id),
            &json!({"ok": true, "outcome": "typed-e2e"}),
            Some("typed e2e"),
            false,
            false,
        )
        .unwrap()
        .expect("a ReadOnly ACK must produce a Result");

    assert_eq!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Completed
    );
    let result = harness.kernel.result_for_task(&task).unwrap();
    assert_eq!(result.payload, json!({"ok": true, "outcome": "typed-e2e"}));
    eprintln!(
        "typed-e2e ack: task={} result={} physical_state={:?}",
        harness.kernel.task(&task).unwrap().state,
        result.id,
        harness.kernel.execution(&execution_id).unwrap().state
    );

    // Physical end after the ACK is history only; it must not mint a second
    // Result or reopen Task authority.
    wait_until(Duration::from_secs(20), || {
        harness.kernel.execution(&execution_id).unwrap().state == ExecutionState::Terminated
    });
    assert_eq!(
        harness.kernel.result_for_task(&task).unwrap().id,
        result.id,
        "late physical end must not replace the durable ACK Result"
    );
    assert_eq!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Completed
    );

    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// The newborn-agent path: an unbound pool agent is skipped by B.3 matching and
/// the Scheduler births + binds a new agent from the eligible source.
#[test]
fn typed_newborn_agent_is_provisioned_and_bound_by_the_scheduler() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start("newborn", Arc::new(WorkspaceJailSourceIntegration::new()));
    let pin = harness.publish_workspace_jail("newborn-reviewer", false, "primary", &[]);

    // Deliberately leave the pool agent unbound.
    let pool_agent = harness.kernel.ready_agent("general").unwrap();
    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-newborn", json!({"goal": "jail"})).partition("general"),
        NetworkPolicy::Enabled,
    );

    wait_running(&harness.kernel, &task);
    let snap = harness.only_snapshot();

    // The matcher skips the unbound pool agent; the Scheduler births a
    // *different* LogicalAgent and binds it to the exact revision.
    let binding = harness
        .kernel
        .get_provisioning_binding(snap.incarnation_id())
        .unwrap()
        .expect("typed acquisition must persist its provisioning binding");
    assert_ne!(binding.logical_agent_id, pool_agent);
    assert_eq!(
        harness
            .kernel
            .get_logical_agent_type_binding(&binding.logical_agent_id)
            .unwrap()
            .expect("the newborn agent is bound to the required type"),
        pin
    );
    // Acquisition commits LEASED; the immediately following observation may
    // have already advanced the Task to RUNNING by the time we read it.
    assert!(matches!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Leased | TaskState::Running
    ));

    let execution_id = snap.execution_id().clone();
    let attempt = snap.attempt_id().clone();
    let epoch = snap.lease_epoch();
    harness
        .kernel
        .ack_success(
            &attempt,
            epoch,
            Some(&execution_id),
            &json!({"ok": true}),
            None,
            false,
            false,
        )
        .unwrap()
        .expect("ReadOnly ACK produces a Result");
    assert_eq!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Completed
    );

    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// A real Write Task: the jailed worker physically writes inside its per-
/// execution workspace, and a durable ACK without a quiescence witness is
/// refused rather than fabricated.
#[test]
fn typed_write_task_writes_inside_the_jail_and_cannot_fake_quiescence() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start("write", Arc::new(WorkspaceJailSourceIntegration::new()));
    let pin =
        harness.publish_workspace_jail("writer", false, "primary", &["--write-probe", "probe.txt"]);
    let agent = harness.kernel.ready_agent("general").unwrap();
    harness
        .kernel
        .bind_logical_agent_type(&agent, &pin)
        .unwrap();

    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-write", json!({"goal": "jail"}))
            .partition("general")
            .write(),
        NetworkPolicy::Enabled,
    );

    wait_running(&harness.kernel, &task);
    let snap = harness.only_snapshot();
    let execution_id = snap.execution_id().clone();
    let jail_dir = harness.jail_execution_dir(snap.request_id().as_str());

    // The worker runs as a restricted token; this file appearing proves the
    // adapter granted exactly the jail root and the process could use it.
    let probe = jail_dir.join("workspace").join("probe.txt");
    wait_until(Duration::from_secs(20), || probe.is_file());
    assert_eq!(std::fs::read(&probe).unwrap(), b"agentype-write-probe");

    let attempt = snap.attempt_id().clone();
    let epoch = snap.lease_epoch();
    let acked = harness
        .kernel
        .ack_success(
            &attempt,
            epoch,
            Some(&execution_id),
            &json!({"ok": true}),
            None,
            false,
            false,
        )
        .unwrap();
    assert!(
        acked.is_none(),
        "a Write ACK without a quiescence witness must not mint a Result"
    );
    assert_ne!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Completed,
        "writer safety must not be weakened to complete the Task"
    );
    assert!(harness.kernel.result_for_task(&task).is_err());
    assert_eq!(harness.row_count("failures"), 1);
    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// Unavailable before Attempt: the only source cannot enforce the required
/// isolation, so the Task stays QUEUED with no Attempt, Lease, Execution,
/// BindingSnapshot, failure, or escalation. The legacy claim path must not pick
/// it up either.
#[test]
fn unprovisionable_typed_task_creates_no_authority_and_no_nack() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start(
        "unavailable",
        Arc::new(WorkspaceJailSourceIntegration::new()),
    );
    // The AgentType demands attempt isolation; the jail adapter honestly does
    // not provide it, so B.4 must reject the only source.
    let pin = harness.publish_workspace_jail("isolated", true, "primary", &[]);
    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-unavailable", json!({})).partition("general"),
        NetworkPolicy::Enabled,
    );

    thread::sleep(Duration::from_millis(800));
    assert_no_authority(&harness, &task);
    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// Exact binding is mandatory: a policy alias that no installed adapter declares
/// yields no candidate. There is no fallback to a same-kind binding.
#[test]
fn typed_task_requires_the_exact_adapter_binding_alias() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start(
        "missing-binding",
        Arc::new(WorkspaceJailSourceIntegration::new()),
    );
    // The adapter was imported as `primary`; the policy asks for `missing`.
    let pin = harness.publish_workspace_jail("missing-binding", false, "missing", &[]);
    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-missing-binding", json!({})).partition("general"),
        NetworkPolicy::Enabled,
    );

    thread::sleep(Duration::from_millis(800));
    assert_no_authority(&harness, &task);
    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// Post-commit preparation drift: attestation succeeds, the authority
/// transaction commits Attempt/Lease/ProvisioningBinding, then the integration's
/// prepared digest differs. No Execution may be fabricated, and the durable
/// consequence is a mechanical RESOURCE_UNAVAILABLE NACK of the committed claim.
#[test]
fn prepared_digest_drift_nacks_the_committed_claim_without_an_execution() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start("drift", Arc::new(DriftingIntegration));

    let body = workspace_jail_config_body(
        fake_bin(),
        vec!["--sleep-ms".into(), "50".into()],
        harness.jail_root.to_string_lossy().to_string(),
    );
    let pin = harness.publish("drifting", false, "primary", &body);
    let agent = harness.kernel.ready_agent("general").unwrap();
    harness
        .kernel
        .bind_logical_agent_type(&agent, &pin)
        .unwrap();
    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-drift", json!({})).partition("general"),
        NetworkPolicy::Enabled,
    );

    wait_until(Duration::from_secs(15), || {
        harness.row_count("failures") >= 1
    });
    assert!(
        harness.snapshots().is_empty(),
        "a digest mismatch must not commit an Execution"
    );
    assert_eq!(harness.row_count("executions"), 0);
    assert_eq!(harness.row_count("binding_snapshots"), 0);
    assert!(
        harness.row_count("attempts") >= 1,
        "the authority transaction committed before preparation"
    );
    assert!(
        harness.row_count("leases") >= 1,
        "the authority transaction committed before preparation"
    );
    assert!(
        harness.row_count("provisioning_bindings") >= 1,
        "the immutable provisioning binding was committed"
    );
    assert!(harness.kernel.result_for_task(&task).is_err());
    assert_ne!(
        harness.kernel.task(&task).unwrap().state,
        TaskState::Completed
    );

    // A candidate-local preparation failure is settled, not a control-plane
    // fatal: the daemon stays healthy and keeps servicing the store.
    assert_eq!(harness.daemon.as_ref().unwrap().phase(), DaemonPhase::Ready);
    assert!(matches!(
        harness.daemon.take().unwrap().join(),
        DaemonExit::Stopped
    ));
}

/// A fatal preparation fault (durable invariant violation) must fail the
/// control plane closed: the daemon stops, no Task NACK is fabricated, and
/// recovery turns the durable preparation fence into exactly one root decision.
#[test]
fn fatal_preparation_fault_stops_the_daemon_and_leaves_one_root_decision() {
    let _serial = TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut harness = Harness::start("fatal", Arc::new(FatalIntegration));

    let body = workspace_jail_config_body(
        fake_bin(),
        vec!["--sleep-ms".into(), "50".into()],
        harness.jail_root.to_string_lossy().to_string(),
    );
    let pin = harness.publish("fatal", false, "primary", &body);
    let agent = harness.kernel.ready_agent("general").unwrap();
    harness
        .kernel
        .bind_logical_agent_type(&agent, &pin)
        .unwrap();
    let task = harness.admit(
        &pin,
        TaskSpec::new("typed-fatal", json!({})).partition("general"),
        NetworkPolicy::Enabled,
    );

    let daemon = harness.daemon.take().unwrap();
    wait_until(Duration::from_secs(15), || {
        daemon.phase() == DaemonPhase::Failed
    });
    assert!(
        matches!(daemon.join(), DaemonExit::Failed(_)),
        "a fatal preparation fault must fail the daemon closed"
    );

    // Recovery (not the failing cycle) owns the Task-level decision.
    harness.kernel.recover_authority().unwrap();
    assert!(harness.snapshots().is_empty());
    assert_eq!(harness.row_count("binding_snapshots"), 0);
    assert_eq!(
        harness.row_count("escalations"),
        1,
        "exactly one durable root decision must be opened"
    );
    let escalation = harness.kernel.open_escalation_for_task(&task).unwrap();
    assert!(matches!(
        escalation.cause,
        agentype_core::EscalationCause::PreparationFault { .. }
    ));
}

// ---------------------------------------------------------------------------
// Deliberately dishonest source integrations.
//
// These implement the *public* integration trait exactly like a real source
// would. They let the E2E prove the runtime's defense-in-depth boundaries
// (materialization receipt and fatal classification) without adding any
// test-only branch to production code.
// ---------------------------------------------------------------------------

fn digest(byte: char) -> MaterializationDigest {
    MaterializationDigest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

struct DriftingIntegration;

impl SourceConfigIntegration for DriftingIntegration {
    fn protocol(&self) -> &str {
        JAIL_PROVISIONING_PROTOCOL
    }

    fn attest(
        &self,
        _revision: &SourceConfigRevision,
        _adapter_kind: &str,
        _adapter_binding_key: &str,
        _deadline: &AdapterDeadline,
    ) -> Result<Option<MaterializationDigest>, ProvisioningResolutionError> {
        Ok(Some(digest('a')))
    }

    fn prepare(
        &self,
        _revision: &SourceConfigRevision,
        _adapter_kind: &str,
        _adapter_binding_key: &str,
        _deadline: &AdapterDeadline,
    ) -> Result<PreparedSource, ProvisioningResolutionError> {
        Ok(PreparedSource {
            digest: digest('b').as_str().to_string(),
            descriptor: "{}".into(),
        })
    }
}

struct FatalIntegration;

impl SourceConfigIntegration for FatalIntegration {
    fn protocol(&self) -> &str {
        JAIL_PROVISIONING_PROTOCOL
    }

    fn attest(
        &self,
        _revision: &SourceConfigRevision,
        _adapter_kind: &str,
        _adapter_binding_key: &str,
        _deadline: &AdapterDeadline,
    ) -> Result<Option<MaterializationDigest>, ProvisioningResolutionError> {
        Ok(Some(digest('a')))
    }

    fn prepare(
        &self,
        _revision: &SourceConfigRevision,
        _adapter_kind: &str,
        _adapter_binding_key: &str,
        _deadline: &AdapterDeadline,
    ) -> Result<PreparedSource, ProvisioningResolutionError> {
        // A durable invariant violation is a control-plane fault, never
        // ordinary availability.
        Err(ProvisioningResolutionError::Storage(
            agentype_core::Error::invariant("fatal preparation conformance fault"),
        ))
    }
}

// ---------------------------------------------------------------------------
