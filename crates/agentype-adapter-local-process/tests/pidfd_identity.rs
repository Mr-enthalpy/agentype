//! Linux pidfd liveness must not follow a recycled numeric PID.

#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
#[test]
fn pidfd_liveness_never_follows_recycled_numeric_pid() {
    let bin = env!("CARGO_BIN_EXE_pidfd-recycle");
    // Hosted Ubuntu rejects unprivileged uid_map writes. PID namespace
    // recycle still requires CAP_SYS_ADMIN; passwordless sudo is the CI path.
    let status = Command::new("sudo")
        .args(["unshare", "--pid", "--fork", "--mount-proc", "--kill-child"])
        .arg(bin)
        .status()
        .expect("sudo unshare must exist on Linux CI");
    assert!(
        status.success(),
        "PID-namespace recycle must prove pidfd ignores the new occupant; {status}"
    );
}
