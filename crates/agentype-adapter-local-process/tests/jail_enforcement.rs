//! Real physical enforcement tests for the workspace-jail adapter.
//!
//! These are not mock tests: each case spawns the real `fake-agent` binary under
//! a real OS sandbox (Windows restricted token or Linux Landlock) and observes a
//! real write attempt. A missing enforcement mechanism shows up as a successful
//! write, so the test fails closed rather than trusting a declared capability.

use agentype_adapter_api::{
    AdapterDeadline, AdapterErrorKind, EnvironmentStartRequest, ExecutionAdapter,
};
use agentype_adapter_local_process::{
    JailLaunchDescriptor, JailedProcessAgentAdapter, JAIL_ADAPTER_KIND,
};
use agentype_core::{
    AttemptId, AuthoritativeExecutionBinding, BatchId, CommittedContinuitySnapshot, ExecutionId,
    IncarnationId, LeaseEpoch, LeaseId, LogicalAgentId, RequestId, TaskId, WorkspaceMode,
};
use agentype_execution_config::{
    resolve_execution_environment, ExecutionLaunchSnapshot, ExecutionProfileConfig,
    ExecutionRegistry, ExecutionResolutionMode, ExecutionTargetConfig, NetworkEnforcement,
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn fake_bin() -> String {
    env!("CARGO_BIN_EXE_fake-agent").to_string()
}

fn long_deadline() -> AdapterDeadline {
    AdapterDeadline::after(Duration::from_secs(15)).unwrap()
}

fn unique_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agentype-jail-{label}-{nanos}"));
    fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("cannot create {}: {err}", dir.display()));
    dir
}

fn descriptor(jail_root: &PathBuf, args: &[&str]) -> JailLaunchDescriptor {
    JailLaunchDescriptor::new(
        fake_bin(),
        args.iter().map(|arg| arg.to_string()).collect(),
        jail_root,
    )
    .unwrap()
}

fn request_with(
    mode: WorkspaceMode,
    network: NetworkEnforcement,
    attempt_isolation: bool,
    descriptor: Option<String>,
) -> EnvironmentStartRequest {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(
            ExecutionTargetConfig::new("jail", JAIL_ADAPTER_KIND, attempt_isolation)
                .with_options(json!({"command": fake_bin(), "args": [], "env": {}})),
        )
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();

    let attempt_id = AttemptId::new();
    let binding = AuthoritativeExecutionBinding {
        attempt_id: attempt_id.clone(),
        lease_epoch: LeaseEpoch(1),
        execution_target: "jail".to_string(),
        execution_profile: "default".to_string(),
    };
    let environment =
        resolve_execution_environment(ExecutionResolutionMode::Authoritative(&registry), &binding)
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
            mode,
            network,
            "jail-enforcement".to_string(),
            json!({}),
            json!({}),
            None,
            CommittedContinuitySnapshot::stateless(),
            environment.safety().clone(),
            descriptor.clone(),
        )
    };
    EnvironmentStartRequest::from_launch_with_descriptor(
        &snapshot,
        &environment,
        descriptor.unwrap_or_default(),
    )
    .unwrap()
}

fn run_probe(request: &EnvironmentStartRequest) -> (Option<String>, Value, String, String) {
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let start = adapter
        .start_execution(request, &long_deadline())
        .expect("jail start must succeed for an enforceable request");
    let outcome = adapter
        .collect_outcome(&start.runtime_handle, &long_deadline())
        .expect("collect must observe the ended jailed process");
    let refs = outcome.artifact_refs.expect("collect exposes artifacts");
    let stdout = refs
        .get("stdout")
        .and_then(Value::as_str)
        .expect("collect exposes the captured stdout path")
        .to_string();
    let stderr = refs
        .get("stderr")
        .and_then(Value::as_str)
        .expect("collect exposes the captured stderr path")
        .to_string();
    let out_text = fs::read_to_string(&stdout).unwrap_or_default();
    let err_text = fs::read_to_string(&stderr).unwrap_or_default();
    let line = out_text
        .lines()
        .find(|line| line.contains("\"probe\""))
        .unwrap_or_else(|| {
            panic!("no probe line in captured stdout: {out_text:?} stderr: {err_text:?}")
        });
    let probe: Value = serde_json::from_str(line).expect("probe line is JSON");
    (
        Some(start.runtime_handle.0.to_string()),
        probe,
        out_text,
        err_text,
    )
}

fn probe_ok(probe: &Value) -> bool {
    probe
        .get("probe")
        .and_then(|probe| probe.get("ok"))
        .and_then(Value::as_bool)
        .expect("probe carries an ok flag")
}

#[test]
fn write_mode_permits_write_inside_the_jail() {
    let jail_root = unique_dir("write-inside");
    let descriptor = descriptor(&jail_root, &["--write-probe", "inside.txt"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (_handle, probe, _out, _err) = run_probe(&request);
    assert!(
        probe_ok(&probe),
        "Write mode must permit writes inside the jail: {probe}"
    );

    let request_id = request.request_id().as_str();
    let written = jail_root
        .join(format!("agentype-jail-{request_id}"))
        .join("workspace")
        .join("inside.txt");
    assert!(
        written.is_file(),
        "the write must land in the per-execution workspace: {}",
        written.display()
    );
    assert_eq!(fs::read(&written).unwrap(), b"agentype-write-probe");
}

#[test]
fn write_mode_denies_write_outside_the_jail() {
    let jail_root = unique_dir("write-outside");
    let outside_dir = unique_dir("write-outside-target");
    let outside = outside_dir.join("escape.txt");
    let outside_arg = outside.to_string_lossy().into_owned();
    let descriptor = descriptor(&jail_root, &["--write-probe", &outside_arg]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (_handle, probe, _out, _err) = run_probe(&request);
    assert!(
        !probe_ok(&probe),
        "Write mode must not escape the jail root: {probe}"
    );
    assert!(
        !outside.exists(),
        "the outside write must not have created {}",
        outside.display()
    );
}

#[test]
fn read_only_mode_denies_write_inside_the_jail() {
    let jail_root = unique_dir("read-only");
    let descriptor = descriptor(&jail_root, &["--write-probe", "inside.txt"]);
    let request = request_with(
        WorkspaceMode::ReadOnly,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (handle, probe, _out, _err) = run_probe(&request);
    assert!(
        !probe_ok(&probe),
        "ReadOnly mode must deny writes inside the jail: {probe} (handle {handle:?})"
    );

    let request_id = request.request_id().as_str();
    let written = jail_root
        .join(format!("agentype-jail-{request_id}"))
        .join("workspace")
        .join("inside.txt");
    assert!(!written.exists(), "ReadOnly must not leave a written file");
}

#[test]
fn non_enabled_network_is_rejected_not_weakened() {
    let jail_root = unique_dir("network");
    let descriptor = descriptor(&jail_root, &["--write-probe", "inside.txt"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Disabled,
        false,
        Some(descriptor.encode()),
    );
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let err = adapter
        .start_execution(&request, &long_deadline())
        .expect_err("a jail that cannot enforce the network policy must be unavailable");
    assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
}

#[test]
fn attempt_isolation_demand_is_rejected_not_overclaimed() {
    let jail_root = unique_dir("isolation");
    let descriptor = descriptor(&jail_root, &["--write-probe", "inside.txt"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        true,
        Some(descriptor.encode()),
    );
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let err = adapter
        .start_execution(&request, &long_deadline())
        .expect_err("per-execution workspace must not be sold as attempt isolation");
    assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
}

#[test]
fn missing_descriptor_is_rejected() {
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        None,
    );
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let err = adapter
        .start_execution(&request, &long_deadline())
        .expect_err("a jail start without a committed descriptor must fail closed");
    assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
}
