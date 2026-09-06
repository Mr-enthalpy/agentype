//! Linux pidfd liveness must not follow a recycled numeric PID.

#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
#[test]
fn pidfd_liveness_never_follows_recycled_numeric_pid() {
    let bin = env!("CARGO_BIN_EXE_pidfd-recycle");
    let status = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--kill-child",
        ])
        .arg(bin)
        .status()
        .expect("unshare must exist on Linux CI");
    assert!(
        status.success(),
        "PID-namespace recycle must prove pidfd ignores the new occupant; {status}"
    );
}
