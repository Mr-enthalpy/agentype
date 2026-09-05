//! Cross-process adapter_binding_key identity. In-process OnceLock is not
//! evidence that a restarted Scheduler on the same boot recomputes the
//! same domain key.

use agentype_adapter_local_process::LocalProcessAgentAdapter;
use std::process::Command;

#[test]
fn domain_key_is_stable_across_processes() {
    let parent = LocalProcessAgentAdapter::new().binding_key().clone();
    let output = Command::new(env!("CARGO_BIN_EXE_print-binding-key"))
        .output()
        .expect("spawn print-binding-key");
    assert!(
        output.status.success(),
        "print-binding-key failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let child = String::from_utf8(output.stdout).expect("utf-8 domain key");
    assert_eq!(parent.as_str(), child.trim());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_domain_key_includes_pid_and_mount_namespaces() {
    let key = LocalProcessAgentAdapter::try_new()
        .expect("domain identity")
        .binding_key()
        .as_str()
        .to_string();
    let pid_ns = std::fs::read_link("/proc/self/ns/pid").expect("read pid ns");
    let mnt_ns = std::fs::read_link("/proc/self/ns/mnt").expect("read mnt ns");
    let pid_ns = pid_ns.to_str().expect("pid ns utf-8");
    let mnt_ns = mnt_ns.to_str().expect("mnt ns utf-8");
    assert!(
        key.starts_with("linux:"),
        "linux key must start with linux:, got {key}"
    );
    assert!(
        key.contains(pid_ns),
        "linux key {key} must include pid namespace {pid_ns}"
    );
    assert!(
        key.contains(mnt_ns),
        "linux key {key} must include mount namespace {mnt_ns}"
    );
    assert!(!key.contains("unknown"));
}

#[cfg(windows)]
#[test]
fn windows_domain_key_uses_boot_guid_not_tick_epoch() {
    let key = LocalProcessAgentAdapter::new()
        .binding_key()
        .as_str()
        .to_string();
    assert!(
        key.starts_with("win:"),
        "windows key must start with win:, got {key}"
    );
    let boot = key.rsplit(':').next().expect("boot token");
    assert_eq!(
        boot.len(),
        36,
        "boot token must be a GUID, not a millisecond epoch: {boot}"
    );
    let parts: Vec<&str> = boot.split('-').collect();
    assert_eq!(parts.len(), 5, "GUID shape 8-4-4-4-12, got {boot}");
    assert!(
        parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit())),
        "GUID hex, got {boot}"
    );
    assert_eq!(parts[0].len(), 8);
    assert_eq!(parts[1].len(), 4);
    assert_eq!(parts[2].len(), 4);
    assert_eq!(parts[3].len(), 4);
    assert_eq!(parts[4].len(), 12);
}
