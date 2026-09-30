//! M5.8 audit round 5 P1: the production Scheduler store contract.
//!
//! `RuntimeProcessLock` resolves store identity from a filesystem handle,
//! while SQLite resolves the same string its own way. If the two disagree,
//! two daemons can each hold a lock and still share one store. These tests
//! pin the contract that makes them agree: the production store is a literal
//! file-backed filesystem path, and SQLite URI interpretation is off.

use agentype_core::{Clock, ManualClock};
use agentype_storage_sqlite::{is_uri_filename, Kernel, SCHEMA_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CONTINUITY_MAX_BYTES: usize = 16_384;

/// A writable directory that lives under the workspace build directory, so
/// these tests do not depend on a writable system temp directory.
fn scratch(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let dir = base.join(format!("store-contract-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(1_000.0))
}

/// The interpreter predicate: only the literal `file:` prefix is a SQLite URI
/// filename. A Windows drive letter and ordinary relative paths are not.
#[test]
fn only_the_literal_file_scheme_is_a_uri_filename() {
    for uri in [
        "file:scheduler.sqlite",
        "file:./scheduler.sqlite",
        "file:/var/lib/agentype/scheduler.sqlite",
        "file:///var/lib/agentype/scheduler.sqlite",
        "file::memory:?cache=shared",
        "file:scheduler.sqlite?mode=rwc",
    ] {
        assert!(
            is_uri_filename(Path::new(uri)),
            "{uri} must be a URI filename"
        );
    }

    for literal in [
        "scheduler.sqlite",
        "./scheduler.sqlite",
        "scheduler.sqlite?mode=rwc",
        "FILE:scheduler.sqlite",
        "/var/lib/agentype/scheduler.sqlite",
        "C:\\agentype\\scheduler.sqlite",
    ] {
        assert!(
            !is_uri_filename(Path::new(literal)),
            "{literal} is a literal filesystem path"
        );
    }
}

/// The production store must still open normally, and it must be genuinely
/// file-backed: two connections over the same path see the same database.
#[test]
fn literal_path_stays_file_backed_and_reopenable() {
    let dir = scratch("literal");
    let path = dir.join("scheduler.sqlite");

    {
        let kernel = Kernel::open(&path, clock(), 10.0, CONTINUITY_MAX_BYTES).unwrap();
        assert_eq!(kernel.schema_version().unwrap(), SCHEMA_VERSION);
    }
    assert!(path.exists(), "a literal path must create a real file");
    assert!(
        !dir.join("file:scheduler.sqlite").exists(),
        "no URI-scheme file may be created"
    );

    // A second open sees the same durable schema identity, so this really is
    // the file SQLite opened rather than a private in-memory database.
    let reopened = Kernel::open(&path, clock(), 10.0, CONTINUITY_MAX_BYTES).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), SCHEMA_VERSION);

    let _ = std::fs::remove_dir_all(dir);
}

/// A URI filename must be refused by the store boundary itself, before any
/// database file is created. This is the layer that actually opens SQLite, so
/// the rejection cannot be bypassed by a caller that never built a
/// `SqliteRuntimeConfig`.
///
/// The paths are used as written rather than joined onto a directory: neither
/// `dir/file:name` nor `dir\file:name` is a URI filename, and on Windows the
/// first is not a legal file name at all. The alias the URI would resolve to
/// is checked directly, so no illegal name is ever created.
#[test]
fn uri_filename_is_refused_by_the_store_boundary() {
    for uri in [
        "file:scheduler.sqlite",
        "file::memory:?cache=shared",
        "file:scheduler.sqlite?mode=rwc",
    ] {
        let path = Path::new(uri);
        let Err(err) = Kernel::open(path, clock(), 10.0, CONTINUITY_MAX_BYTES) else {
            panic!("{uri} must not open as a production store");
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("URI"),
            "{uri} must be refused for being a URI filename, got {rendered}"
        );
        assert!(
            !path.exists(),
            "{uri} must be refused before any database file is created"
        );
    }

    // The alias the relative URI would have resolved to must not exist. It is
    // named but never created, so this stays portable.
    let alias = Path::new("scheduler.sqlite");
    if alias.exists() {
        let _ = std::fs::remove_file(alias);
    }
    if Kernel::open("file:scheduler.sqlite", clock(), 10.0, CONTINUITY_MAX_BYTES).is_ok() {
        panic!("the URI alias must not open");
    }
    assert!(
        !alias.exists(),
        "SQLite's URI path component must never be opened"
    );
}
