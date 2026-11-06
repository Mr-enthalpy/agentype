//! Real physical enforcement tests for the workspace-jail adapter.
//!
//! These are not mock tests: each case spawns the real `fake-agent` binary under
//! a real OS sandbox (Windows restricted token or Linux Landlock) and observes a
//! real write attempt. A missing enforcement mechanism shows up as a successful
//! write, so the test fails closed rather than trusting a declared capability.

use agentype_adapter_api::{
    AdapterDeadline, AdapterErrorKind, EnvironmentStartRequest, ExecutionAdapter, RuntimeHandle,
};
use agentype_adapter_local_process::{
    JailLaunchDescriptor, JailedProcessAgentAdapter, JAIL_ADAPTER_KIND,
};
use agentype_core::{
    AttemptId, AuthoritativeExecutionBinding, BatchId, CommittedContinuitySnapshot, ExecutionId,
    ExecutionState, IncarnationId, LeaseEpoch, LeaseId, LogicalAgentId, RequestId, TaskId,
    WorkspaceMode,
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
        "no probe line in captured stdout: {out_text:?} stderr: {err_text:?} exit: {:?}",
        outcome.exit_status
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

#[cfg(windows)]
#[test]
fn worker_token_is_minimal_and_desktop_is_private() {
    let jail_root = unique_dir("token-isolation");
    let descriptor = descriptor(&jail_root, &["--isolation-probe"]);
    let request = request_with(
        WorkspaceMode::ReadOnly,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (_handle, probe, out, err) = run_probe(&request);
    let probe = probe.get("probe").expect("isolation probe object");
    assert_eq!(probe.get("ok").and_then(Value::as_bool), Some(true));
    let privileges: Vec<&str> = probe
        .get("privileges")
        .and_then(Value::as_array)
        .expect("privileges array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        privileges.contains(&"SeChangeNotifyPrivilege"),
        "the traversal privilege must survive: {privileges:?}"
    );
    for host in [
        "SeShutdownPrivilege",
        "SeUndockPrivilege",
        "SeIncreaseWorkingSetPrivilege",
        "SeTimeZonePrivilege",
    ] {
        assert!(
            !privileges.contains(&host),
            "host privilege {host} leaked into the worker token: {privileges:?} stdout={out:?} stderr={err:?}"
        );
    }

    let desktop = probe
        .get("desktop")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        desktop.starts_with("agentype-desktop-"),
        "worker must run on the per-execution desktop, got {desktop:?}"
    );
    assert_eq!(
        probe.get("default_desktop_open").and_then(Value::as_bool),
        Some(false),
        "worker must not open the interactive Default desktop"
    );
    assert_eq!(
        probe.get("input_desktop_open").and_then(Value::as_bool),
        Some(false),
        "worker must not open the input desktop"
    );

    let access = probe
        .get("winsta_access")
        .expect("the isolation probe reports real window-station access checks");
    for allowed in ["enumdesktops", "readattributes", "enumerate"] {
        assert_eq!(
            access.get(allowed).and_then(Value::as_bool),
            Some(true),
            "the worker must retain {allowed} for Win32 initialization: {out:?} {err:?}"
        );
    }
    // The token is write-restricted: the restricting SID controls write-shaped
    // window-station access. None of these may be openable.
    for forbidden in [
        "createdesktop",
        "writeattributes",
        "accessclipboard",
        "standard_rights_required",
        "all_access",
    ] {
        assert_eq!(
            access.get(forbidden).and_then(Value::as_bool),
            Some(false),
            "the restricting SID must not be able to open the window station with {forbidden}: {out:?} {err:?}"
        );
    }
    // A write right must stay denied even when requested together with a
    // read right the interactive session already holds.
    assert_eq!(
        access
            .get("readscreen_plus_createdesktop")
            .and_then(Value::as_bool),
        Some(false),
        "requesting a read right must not smuggle in createdesktop: {out:?} {err:?}"
    );
    // These rights are read-shaped. A write-restricted token evaluates them
    // with the normal SIDs against WinSta0's existing DACL, so the interactive
    // session's default ACEs allow them. The adapter adds no ACE for the
    // restricting SID, and this adapter does not claim same-user read
    // confidentiality. The probe records them so the documented claim stays
    // honest instead of implying the adapter denies them.
    for preexisting in [
        "enumdesktops",
        "readattributes",
        "enumerate",
        "accessglobalatoms",
        "exitwindows",
        "readscreen",
    ] {
        assert_eq!(
            access.get(preexisting).and_then(Value::as_bool),
            Some(true),
            "the probe must record the station's pre-existing {preexisting} capability so the claim stays honest: {out:?} {err:?}"
        );
    }
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

/// Landlock mediates content writes, creation, deletion, rename, and truncate,
/// but it does not mediate metadata syscalls such as `chmod`, `utimensat`, or
/// `setxattr`. The report and ADR therefore contract the Linux jail to a
/// content-write boundary rather than full filesystem immutability. This test
/// keeps that contract honest with a real OS counterexample: while the content
/// write outside the jail is denied, the metadata change succeeds.
#[cfg(target_os = "linux")]
fn assert_metadata_outside_is_not_landlock_mediated(mode: WorkspaceMode, tag: &str) {
    use std::os::unix::fs::PermissionsExt;

    let jail_root = unique_dir(tag);
    let outside_dir = unique_dir(&format!("{tag}-target"));
    let outside = outside_dir.join("owned.txt");
    fs::write(&outside, b"do-not-change").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
    let outside_arg = outside.to_string_lossy().into_owned();
    let descriptor = descriptor(
        &jail_root,
        &[
            "--write-probe",
            &outside_arg,
            "--metadata-probe",
            &outside_arg,
        ],
    );
    let request = request_with(
        mode,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );

    let (_handle, probes, out, err) = run_probes(&request);
    let label = |name: &str| {
        probes
            .iter()
            .find(|probe| probe["probe"]["label"] == name)
            .unwrap_or_else(|| panic!("{name} probe did not run: {out:?} {err:?}"))
    };

    let write = label("write");
    assert!(
        !probe_ok(write),
        "{mode:?} must still deny content writes outside the jail: {write}"
    );
    assert_eq!(
        fs::read(&outside).unwrap(),
        b"do-not-change",
        "the content write must not have modified the outside file"
    );

    let chmod = label("chmod");
    assert!(
        probe_ok(chmod),
        "{mode:?} Landlock does not mediate chmod; the documented content-write boundary expects this metadata change to succeed: {chmod}"
    );
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o600,
        "the counterexample must actually observe the metadata change"
    );

    let utimensat = label("utimensat");
    assert!(
        probe_ok(utimensat),
        "{mode:?} Landlock does not mediate utimensat; the documented content-write boundary expects this metadata change to succeed: {utimensat}"
    );

    let setxattr = label("setxattr");
    if !probe_ok(setxattr) {
        let errno = setxattr["probe"]["errno"].as_i64().unwrap_or(0);
        assert!(
            errno == 95 || errno == 38,
            "{mode:?} setxattr failed for an unexpected reason, which would hide the documented gap: {setxattr}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_read_only_landlock_is_a_content_write_boundary_not_metadata_immutability() {
    assert_metadata_outside_is_not_landlock_mediated(WorkspaceMode::ReadOnly, "ro-metadata");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_write_landlock_is_a_content_write_boundary_not_metadata_immutability() {
    assert_metadata_outside_is_not_landlock_mediated(WorkspaceMode::Write, "wr-metadata");
}

fn jail_exec_dir(request: &EnvironmentStartRequest) -> PathBuf {
    std::env::temp_dir().join(format!(
        "agentype-jail-exec-{}",
        request.request_id().as_str()
    ))
}

fn read_locator(request: &EnvironmentStartRequest) -> Value {
    let path = jail_exec_dir(request).join("locator.json");
    serde_json::from_str(
        &fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display())),
    )
    .expect("locator.json is JSON")
}

fn start_hanging(request: &EnvironmentStartRequest) -> (JailedProcessAgentAdapter, RuntimeHandle) {
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let start = adapter
        .start_execution(request, &long_deadline())
        .expect("jail start must succeed");
    assert_eq!(start.state, ExecutionState::Running);
    (adapter, start.runtime_handle)
}

#[test]
fn reconcile_without_persisted_handle_recovers_running_instance() {
    let jail_root = unique_dir("reconcile-running");
    let descriptor = descriptor(&jail_root, &["--hang"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let (_first, started) = start_hanging(&request);

    // A restarted daemon has no in-memory child map and no persisted handle.
    // The durable locator written immediately after spawn is the only evidence.
    // Keep the original adapter alive so the parent-side desktop handle does not
    // destroy the live desktop during this in-process restart simulation.
    let second = JailedProcessAgentAdapter::try_new().unwrap();
    let observation = second
        .reconcile_start(request.request_id(), None, &long_deadline())
        .expect("reconcile must use the durable locator");
    assert_eq!(observation.state, ExecutionState::Running);
    assert!(!observation.ambiguous);
    let handle = &observation.runtime_handle.0;
    assert_eq!(
        handle.get("pid").and_then(Value::as_u64),
        started.0.get("pid").and_then(Value::as_u64),
        "reconcile must recover the same physical pid"
    );
    assert!(
        handle.get("birth").and_then(Value::as_u64).is_some(),
        "reconcile must observe the real birth identity instead of leaving a locator: {handle}"
    );
    let terminated = second
        .terminate_execution(&observation.runtime_handle, &long_deadline())
        .expect("the recovered instance must be controllable");
    assert_eq!(terminated.state, ExecutionState::Terminated);
}

#[test]
fn durable_locator_controls_the_process_after_adapter_restart() {
    let jail_root = unique_dir("locator-control");
    let descriptor = descriptor(&jail_root, &["--hang"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let (_first, _started) = start_hanging(&request);

    let locator = read_locator(&request);
    assert_eq!(
        locator.get("identity_complete").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        locator.get("reconnectable").and_then(Value::as_bool),
        Some(false)
    );
    let locator_handle = RuntimeHandle(locator);

    let second = JailedProcessAgentAdapter::try_new().unwrap();
    let observed = second
        .observe_execution(&locator_handle, &long_deadline())
        .expect("a locator is enough to observe the physical instance");
    assert_eq!(observed.state, ExecutionState::Running);
    let terminated = second
        .terminate_execution(&locator_handle, &long_deadline())
        .expect("a locator is enough to terminate the physical instance");
    assert_eq!(terminated.state, ExecutionState::Terminated);
}

#[test]
fn born_less_locator_is_recovered_after_the_spawn_identity_window() {
    let jail_root = unique_dir("locator-no-birth");
    let descriptor = descriptor(&jail_root, &["--hang"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let (_first, _started) = start_hanging(&request);
    // Keep the original adapter alive so the parent-side private-desktop handle
    // stays open while the second adapter performs the recovery.
    // A crash between spawn and the birth probe leaves exactly this durable
    // state: the locator exists, but the identity upgrade has not happened.
    let mut locator = read_locator(&request);
    locator
        .as_object_mut()
        .expect("locator object")
        .remove("birth");
    fs::write(
        jail_exec_dir(&request).join("locator.json"),
        locator.to_string(),
    )
    .unwrap();

    let second = JailedProcessAgentAdapter::try_new().unwrap();
    let observation = second
        .reconcile_start(request.request_id(), None, &long_deadline())
        .expect("the locator must survive the missing birth identity");
    assert_eq!(observation.state, ExecutionState::Running);
    assert!(
        observation
            .runtime_handle
            .0
            .get("birth")
            .and_then(Value::as_u64)
            .is_some(),
        "recovery must persist the observed birth identity: {}",
        observation.runtime_handle.0
    );
    let terminated = second
        .terminate_execution(&observation.runtime_handle, &long_deadline())
        .expect("recovered born-less instance must be controllable");
    assert_eq!(terminated.state, ExecutionState::Terminated);
}

#[test]
fn reconcile_without_any_physical_evidence_is_ambiguous_not_respawned() {
    let jail_root = unique_dir("reconcile-none");
    let descriptor = descriptor(&jail_root, &["--hang"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let observation = adapter
        .reconcile_start(request.request_id(), None, &long_deadline())
        .expect("a missing witness is an ambiguous observation, not an error");
    assert_eq!(observation.state, ExecutionState::Unknown);
    assert!(observation.ambiguous);
    assert!(
        observation.runtime_handle.0.is_null()
            || observation
                .runtime_handle
                .0
                .as_object()
                .map(|object| object.is_empty())
                .unwrap_or(false),
        "no synthetic handle may be minted without physical evidence"
    );
}

#[cfg(windows)]
#[test]
fn born_less_locator_cannot_impersonate_an_unrelated_pid() {
    let jail_root = unique_dir("locator-impersonation");
    let descriptor = descriptor(&jail_root, &["--hang"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let exec_dir = jail_exec_dir(&request);
    fs::create_dir_all(&exec_dir).unwrap();
    let request_id = request.request_id().as_str();
    let locator = json!({
        "v": 1,
        "kind": JAIL_ADAPTER_KIND,
        "pid": std::process::id(),
        "inst": "not-this-execution",
        "request_id": request_id,
        "stdout": exec_dir.join("stdout.txt").to_string_lossy(),
        "stderr": exec_dir.join("stderr.txt").to_string_lossy(),
        "identity_complete": false,
        "reconnectable": false,
        "spawn_after_ms": 1,
    });
    fs::write(exec_dir.join("locator.json"), locator.to_string()).unwrap();

    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let observation = adapter
        .reconcile_start(request.request_id(), None, &long_deadline())
        .expect("a mismatched witness is a terminal physical observation");
    assert_eq!(
        observation.state,
        ExecutionState::Terminated,
        "a numeric pid without the per-execution restricting SID must not be adopted: {:?}",
        observation.detail
    );
}

/// A process that was really started but whose locator cannot be persisted must
/// not be reported as a clean start or assumed absent. The adapter returns an
/// unknown-state boundary carrying the in-memory locator, and a restarted
/// adapter can still observe and terminate that exact process through the hint.
#[test]
fn locator_write_failure_leaves_the_started_process_unknown_but_controllable() {
    let jail_root = unique_dir("locator-write-failure");
    let descriptor = descriptor(&jail_root, &["--hang"]);
    let request = request_with(
        WorkspaceMode::Write,
        NetworkEnforcement::Enabled,
        false,
        Some(descriptor.encode()),
    );
    let exec_dir = jail_exec_dir(&request);
    fs::create_dir_all(&exec_dir).unwrap();
    // A real write failure after spawn: the atomic replacement cannot rename a
    // regular file over a directory. The child is already running at this point.
    fs::create_dir(exec_dir.join("locator.json")).unwrap();

    let adapter = JailedProcessAgentAdapter::try_new().unwrap();
    let err = adapter
        .start_execution(&request, &long_deadline())
        .expect_err("an unpersistable locator cannot be reported as a clean start");
    assert_eq!(err.kind(), AdapterErrorKind::Unavailable);
    let hint = err
        .runtime_handle_hint()
        .expect("the in-memory locator must travel with the unknown-state error")
        .clone();
    let locator = hint.0.as_object().expect("locator object");
    assert_eq!(
        locator.get("request_id").and_then(Value::as_str),
        Some(request.request_id().as_str())
    );
    assert_eq!(
        locator.get("identity_complete").and_then(Value::as_bool),
        Some(false)
    );
    let pid = locator
        .get("pid")
        .and_then(Value::as_u64)
        .expect("locator pid") as u32;
    assert_ne!(pid, 0);

    let second = JailedProcessAgentAdapter::try_new().unwrap();
    let observed = second
        .observe_execution(&hint, &long_deadline())
        .expect("the locator hint must let a fresh adapter observe the live process");
    assert_eq!(observed.state, ExecutionState::Running);
    let terminated = second
        .terminate_execution(&hint, &long_deadline())
        .expect("the locator hint must let a fresh adapter terminate the live process");
    assert_eq!(terminated.state, ExecutionState::Terminated);
}

/// The adapter creates a private desktop per Execution but must never write to
/// the shared window station's DACL: the station outlives every Execution, so
/// per-Execution ACEs would accumulate for the scheduler's lifetime with no
/// revocation point that is safe for still-running workers. This runs three
/// complete Execution lifecycles and compares the live WinSta0 ACL.
#[cfg(windows)]
#[test]
fn shared_window_station_acl_does_not_grow_across_execution_lifecycles() {
    let before = window_station_ace_count();
    for index in 0..3 {
        let jail_root = unique_dir(&format!("station-acl-{index}"));
        let descriptor = descriptor(&jail_root, &["--isolation-probe"]);
        let request = request_with(
            WorkspaceMode::ReadOnly,
            NetworkEnforcement::Enabled,
            false,
            Some(descriptor.encode()),
        );
        let (_handle, probe, out, err) = run_probe(&request);
        assert!(
            probe_ok(&probe),
            "execution {index} must run inside the jail: {out:?} {err:?}"
        );
    }
    let after = window_station_ace_count();
    assert_eq!(
        after, before,
        "starting and ending Executions must not change the shared window station DACL"
    );
}

#[cfg(windows)]
fn window_station_ace_count() -> u32 {
    type Handle = *mut core::ffi::c_void;
    type Dword = u32;
    type Bool = i32;
    const SE_WINDOW_OBJECT: i32 = 7;
    const DACL_SECURITY_INFORMATION: Dword = 0x0000_0004;
    const ACL_SIZE_INFORMATION: i32 = 2;

    #[repr(C)]
    struct AclSizeInformation {
        ace_count: Dword,
        acl_bytes_in_use: Dword,
        acl_bytes_free: Dword,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> Handle;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn GetSecurityInfo(
            handle: Handle,
            object_type: i32,
            security_info: Dword,
            owner: *mut Handle,
            group: *mut Handle,
            dacl: *mut Handle,
            sacl: *mut Handle,
            descriptor: *mut Handle,
        ) -> Dword;
        fn GetAclInformation(
            acl: *mut core::ffi::c_void,
            info: *mut core::ffi::c_void,
            len: Dword,
            class: i32,
        ) -> Bool;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: Handle) -> Handle;
    }

    // SAFETY: GetProcessWindowStation returns a process-owned handle.
    let station = unsafe { GetProcessWindowStation() };
    assert!(
        !station.is_null(),
        "the test process must have a window station"
    );
    let mut dacl: Handle = std::ptr::null_mut();
    let mut descriptor: Handle = std::ptr::null_mut();
    // SAFETY: station is live; the out pointers are owned by this frame.
    let read = unsafe {
        GetSecurityInfo(
            station,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(read, 0, "cannot read the shared window station DACL");
    assert!(!dacl.is_null(), "the window station must have a DACL");
    let mut size = AclSizeInformation {
        ace_count: 0,
        acl_bytes_in_use: 0,
        acl_bytes_free: 0,
    };
    // SAFETY: dacl is a valid ACL; size is an out parameter.
    let ok = unsafe {
        GetAclInformation(
            dacl,
            (&mut size as *mut AclSizeInformation).cast(),
            std::mem::size_of::<AclSizeInformation>() as Dword,
            ACL_SIZE_INFORMATION,
        )
    };
    assert_ne!(ok, 0, "cannot size the shared window station DACL");
    // SAFETY: GetSecurityInfo returned this descriptor for LocalFree.
    unsafe {
        LocalFree(descriptor);
    }
    size.ace_count
}
