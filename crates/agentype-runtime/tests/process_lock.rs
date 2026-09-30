//! Cross-process RuntimeProcessLock tests. Two threads are not evidence.

use agentype_runtime::{ProcessLockError, RuntimeProcessGuard, SqliteRuntimeConfig};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_store() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("agentype-xproc-{nanos}.sqlite"))
}

fn helper() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hold-process-lock"))
}

#[test]
fn second_os_process_fails_before_recovery() {
    let path = temp_store();
    let mut child = helper()
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock helper");
    let stdout = child.stdout.take().expect("helper stdout");
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next().and_then(Result::ok);
    assert_eq!(first.as_deref(), Some("LOCKED"));

    let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
    match RuntimeProcessGuard::acquire(&cfg) {
        Err(ProcessLockError::AlreadyRunning) => {}
        other => panic!("second process must not acquire, got {other:?}"),
    }

    drop(child.stdin.take());
    let status = child.wait().expect("helper exit");
    assert!(status.success(), "helper failed: {status}");

    let _released = RuntimeProcessGuard::acquire(&cfg).expect("lock released after helper exit");
    let _ = std::fs::remove_file(path);
}

#[test]
fn clean_shutdown_releases_lock_for_next_process() {
    let path = temp_store();
    let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
    {
        let _guard = RuntimeProcessGuard::acquire(&cfg).unwrap();
    }
    let mut child = helper()
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock helper");
    let stdout = child.stdout.take().expect("helper stdout");
    let first = BufReader::new(stdout).lines().next().and_then(Result::ok);
    assert_eq!(first.as_deref(), Some("LOCKED"));
    let mut stdin = child.stdin.take().expect("helper stdin");
    let _ = stdin.write_all(b"done\n");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let _ = std::fs::remove_file(path);
}

#[test]
fn different_process_temp_dir_cannot_split_store_ownership() {
    let path = temp_store();
    let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
    let _guard = RuntimeProcessGuard::acquire(&cfg).unwrap();

    let alien_tmp = std::env::temp_dir().join(format!(
        "agentype-alien-tmp-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&alien_tmp).unwrap();
    let mut child = helper()
        .env("TMPDIR", &alien_tmp)
        .env("TMP", &alien_tmp)
        .env("TEMP", &alien_tmp)
        .env("XDG_RUNTIME_DIR", &alien_tmp)
        .env("HOME", &alien_tmp)
        .env("LOCALAPPDATA", &alien_tmp)
        .env("PROGRAMDATA", &alien_tmp)
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lock helper with alien temp dir");
    let stdout = child.stdout.take().expect("helper stdout");
    let first = BufReader::new(stdout).lines().next().and_then(Result::ok);
    assert_ne!(
        first.as_deref(),
        Some("LOCKED"),
        "helper with a different XDG_RUNTIME_DIR/HOME/LOCALAPPDATA/TMPDIR must not lock the same store"
    );
    drop(child.stdin.take());
    let status = child.wait().expect("helper exit");
    assert!(!status.success(), "helper must fail closed, got {status}");
    drop(_guard);
    let _ = std::fs::remove_dir_all(alien_tmp);
    let _ = std::fs::remove_file(path);
}

/// Same file, new directory. The identity lock must not follow the old parent.
#[test]
fn cross_directory_rename_cannot_split_store_ownership() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("agentype-rename-{nanos}"));
    let source = root.join("a").join("scheduler.sqlite");
    let dest = root.join("b").join("scheduler.sqlite");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

    let mut child = helper()
        .arg(&source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock helper");
    let stdout = child.stdout.take().expect("helper stdout");
    let first = BufReader::new(stdout).lines().next().and_then(Result::ok);
    assert_eq!(first.as_deref(), Some("LOCKED"));

    std::fs::rename(&source, &dest).expect("rename open store");
    // Re-open and close in this process. That must not drop the child's flock.
    drop(std::fs::File::open(&dest).unwrap());
    let cfg = SqliteRuntimeConfig::new(&dest, 10.0, 16_384).unwrap();
    match RuntimeProcessGuard::acquire(&cfg) {
        Err(ProcessLockError::AlreadyRunning) => {}
        other => panic!("renamed store must stay owned, got {other:?}"),
    }

    drop(child.stdin.take());
    let status = child.wait().expect("helper exit");
    assert!(status.success(), "helper failed: {status}");
    let _released = RuntimeProcessGuard::acquire(&cfg).expect("lock released after helper exit");
    let _ = std::fs::remove_dir_all(root);
}
