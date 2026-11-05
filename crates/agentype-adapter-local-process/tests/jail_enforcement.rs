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
use std::path::{Path, PathBuf};
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

fn run_probes(request: &EnvironmentStartRequest) -> (Option<String>, Vec<Value>, String, String) {
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
    let probes: Vec<Value> = out_text
        .lines()
        .filter(|line| line.contains("\"probe\""))
        .map(|line| serde_json::from_str(line).expect("probe line is JSON"))
        .collect();
    assert!(
        !probes.is_empty(),
        "no probe line in captured stdout: {out_text:?} stderr: {err_text:?}"
    );
    (
        Some(start.runtime_handle.0.to_string()),
        probes,
        out_text,
        err_text,
    )
}

fn run_probe(request: &EnvironmentStartRequest) -> (Option<String>, Value, String, String) {
    let (handle, mut probes, out_text, err_text) = run_probes(request);
    let probe = probes.remove(0);
    (handle, probe, out_text, err_text)
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

fn jail_workspace(jail_root: &Path, request: &EnvironmentStartRequest) -> PathBuf {
    jail_root
        .join(format!("agentype-jail-{}", request.request_id().as_str()))
        .join("workspace")
}

#[test]
fn expired_start_deadline_creates_no_jail_directory() {
    let jail_root = unique_dir("deadline-expired");
    let descriptor = descriptor(&jail_root, &["--write-probe", "inside.txt"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let expired = AdapterDeadline::from_instant(std::time::Instant::now());
    let err = adapter
        .start_execution(&request, &expired)
        .expect_err("an already-expired start deadline must not begin a physical phase");
    assert_eq!(err.kind(), AdapterErrorKind::DeadlineExceeded);
    assert!(
        !jail_root
            .join(format!("agentype-jail-{}", request.request_id().as_str()))
            .exists(),
        "no jail side effect may occur after the absolute deadline"
    );
}

/// P1-5: the worker must not inherit the daemon environment, but the minimal
/// OS allowlist must still provide necessary, non-sensitive variables.
#[test]
fn worker_does_not_inherit_daemon_secrets_but_keeps_runtime_variables() {
    const SENTINEL: &str = "AGENTYPE_TEST_DAEMON_SENTINEL_SECRET";
    let secret = format!("sentinel-{}", std::process::id());
    std::env::set_var(SENTINEL, &secret);
    let jail_root = unique_dir("env-isolation");
    let descriptor = descriptor(
        &jail_root,
        &["--env-probe", SENTINEL, "--env-probe", "PATH"],
    );
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (_handle, probes, out_text, _err) = run_probes(&request);
    let by_name = |name: &str| {
        probes
            .iter()
            .find(|probe| probe["probe"]["name"] == name)
            .unwrap_or_else(|| panic!("probe for {name} missing: {probes:?}"))
    };
    assert_eq!(
        by_name(SENTINEL)["probe"]["present"],
        Value::Bool(false),
        "the daemon-only secret must be absent from the worker environment"
    );
    assert_eq!(
        by_name("PATH")["probe"]["present"],
        Value::Bool(true),
        "the minimal allowlist must still provide PATH"
    );
    assert!(
        !out_text.contains(&secret),
        "the sentinel secret value must never reach the worker"
    );
    std::env::remove_var(SENTINEL);
}

/// P1-3: an ACE for Execution A's per-execution restricting SID must not let
/// Execution B write A's jail, whichever mode B was admitted under.
#[cfg(windows)]
fn write_a_cannot_write_b_jail(mode_b: WorkspaceMode, tag: &str) {
    let jail_root = unique_dir(tag);
    let a_descriptor = descriptor(&jail_root, &["--write-probe", "a-own.txt"]);
    let a_request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(a_descriptor.encode()),
    );
    let (_handle, a_probe, _out, _err) = run_probe(&a_request);
    assert!(
        probe_ok(&a_probe),
        "Execution A must be able to write its own Write jail: {a_probe}"
    );

    let a_workspace = jail_workspace(&jail_root, &a_request);
    let target = a_workspace.join("from-b.txt");
    let target_arg = target.to_string_lossy().into_owned();
    let b_descriptor = descriptor(&jail_root, &["--write-probe", &target_arg]);
    let b_request = request_with(
        mode_b,
        NetworkEnforcement::Enabled,
        false,
        Some(b_descriptor.encode()),
    );
    let (_handle, b_probe, _out, _err) = run_probe(&b_request);
    assert!(
        !probe_ok(&b_probe),
        "Execution B ({mode_b:?}) must not write Execution A's jail: {b_probe}"
    );
    assert!(
        !target.exists(),
        "cross-execution write created {}",
        target.display()
    );
}

#[cfg(windows)]
#[test]
fn write_a_cannot_write_read_only_b_jail() {
    write_a_cannot_write_b_jail(WorkspaceMode::ReadOnly, "cross-read-only");
}

#[cfg(windows)]
#[test]
fn write_a_cannot_write_write_b_jail() {
    write_a_cannot_write_b_jail(WorkspaceMode::Write, "cross-write");
}

/// P1-2: on a Landlock ABI >= 3 host `ReadOnly` and bounded `Write` must deny
/// both `ftruncate` and the `open(O_RDONLY|O_TRUNC)` bypass outside the jail.
#[cfg(target_os = "linux")]
fn assert_truncate_outside_denied(mode: WorkspaceMode, flag: &str, tag: &str) {
    let jail_root = unique_dir(tag);
    let outside_dir = unique_dir(&format!("{tag}-target"));
    let outside = outside_dir.join("keep.txt");
    fs::write(&outside, b"do-not-truncate").unwrap();
    let descriptor = descriptor(&jail_root, &[flag, &outside.to_string_lossy()]);
    let request = request_with(
        mode,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (_handle, probe, _out, _err) = run_probe(&request);
    assert!(
        !probe_ok(&probe),
        "{mode:?} must deny {flag} outside the jail: {probe}"
    );
    assert_eq!(
        fs::read(&outside).unwrap(),
        b"do-not-truncate",
        "{flag} modified a file outside the jail"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_read_only_denies_truncate_outside_jail() {
    assert_truncate_outside_denied(WorkspaceMode::ReadOnly, "--truncate-probe", "ro-trunc-out");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_write_denies_truncate_outside_jail() {
    assert_truncate_outside_denied(WorkspaceMode::Write, "--truncate-probe", "wr-trunc-out");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_read_only_denies_open_truncate_outside_jail() {
    assert_truncate_outside_denied(
        WorkspaceMode::ReadOnly,
        "--open-truncate-probe",
        "ro-opentrunc-out",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_write_denies_open_truncate_outside_jail() {
    assert_truncate_outside_denied(
        WorkspaceMode::Write,
        "--open-truncate-probe",
        "wr-opentrunc-out",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_write_allows_truncate_inside_jail() {
    let jail_root = unique_dir("wr-trunc-in");
    let descriptor = descriptor(
        &jail_root,
        &[
            "--write-probe",
            "inside.txt",
            "--truncate-probe",
            "inside.txt",
        ],
    );
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let (_handle, probes, _out, _err) = run_probes(&request);
    let truncate = probes
        .iter()
        .find(|probe| probe["probe"]["label"] == "truncate")
        .expect("truncate probe ran");
    assert!(
        probe_ok(truncate),
        "Write mode must allow truncation inside the jail: {truncate}"
    );
    let written = jail_workspace(&jail_root, &request).join("inside.txt");
    assert_eq!(fs::metadata(&written).unwrap().len(), 0);
}
