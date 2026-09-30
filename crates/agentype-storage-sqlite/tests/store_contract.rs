//! M5.8 store contract: the production Scheduler store.
//!
//! `RuntimeProcessLock` resolves store identity from a filesystem handle,
//! while SQLite resolves the same string with its own special-filename rules.
//! If the two disagree, two daemons can each hold a lock and still share one
//! store. These tests pin the contract that makes them agree: a production
//! store is a literal file-backed filesystem path, and every SQLite
//! special-filename spelling — URI, `:memory:`, and the empty filename — is
//! refused before SQLite is asked to open anything.
//!
//! The classifier is also the one shared table that `SqliteRuntimeConfig`
//! applies, so the two layers cannot drift apart.

use agentype_core::{Clock, ManualClock};
use agentype_storage_sqlite::{classify_store_path, Kernel, StorePathKind, SCHEMA_VERSION};
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

/// The classification table. Every kind other than `LiteralFile` is a SQLite
/// special filename that must never back a production store.
///
/// `:memory:` and the empty filename are special even with URI processing
/// completely off, which is why the contract is a positive check on the kind
/// rather than a list of URI rejections.
#[test]
fn classification_table_names_every_sqlite_special_filename() {
    for uri in [
        "file:scheduler.sqlite",
        "file:./scheduler.sqlite",
        "file:/var/lib/agentype/scheduler.sqlite",
        "file:///var/lib/agentype/scheduler.sqlite",
        "file::memory:?cache=shared",
        "file:scheduler.sqlite?mode=rwc",
    ] {
        assert_eq!(
            classify_store_path(Path::new(uri)),
            StorePathKind::SqliteUri,
            "{uri} must classify as a URI filename"
        );
    }

    assert_eq!(
        classify_store_path(Path::new(":memory:")),
        StorePathKind::Memory,
        "bare `:memory:` is the in-memory spelling"
    );

    // Bare `:memory:` is special only when there is no additional text. The
    // shared-cache spelling is a URI filename, and a bare `:memory:` with a
    // tail is an ordinary disk filename — it is *not* the memory database.
    assert_eq!(
        classify_store_path(Path::new(":memory:?cache=shared")),
        StorePathKind::LiteralFile,
        "`:memory:` with a query tail is an ordinary disk filename"
    );
    assert_eq!(
        classify_store_path(Path::new("file::memory:?cache=shared")),
        StorePathKind::SqliteUri,
        "the shared-cache memory spelling is a URI filename"
    );

    assert_eq!(
        classify_store_path(Path::new("")),
        StorePathKind::Temporary,
        "the empty filename is SQLite's temporary-database spelling"
    );

    for literal in [
        "scheduler.sqlite",
        "./scheduler.sqlite",
        "scheduler.sqlite?mode=rwc",
        "FILE:scheduler.sqlite",
        "/var/lib/agentype/scheduler.sqlite",
        "C:\\agentype\\scheduler.sqlite",
        // SQLite's own documented escape hatch for a real file named
        // `:memory:`: the leading `./` makes it a literal path.
        "./:memory:",
    ] {
        assert_eq!(
            classify_store_path(Path::new(literal)),
            StorePathKind::LiteralFile,
            "{literal} is a literal filesystem path"
        );
    }
}

/// M5.8 audit round 7 P1: the classifier must work on the filename *bytes*
/// SQLite receives, not on a Rust `str`. On Unix, rusqlite hands SQLite
/// `OsStrExt::as_bytes()` unchanged and SQLite's URI test is a raw `memcmp`
/// against `b"file:"`, so a URI filename that is not valid UTF-8 would slip
/// past a `str`-based test and reproduce the lock/store identity split.
///
/// The other half is deliberate: a non-UTF-8 filename that is *not* a URI is
/// refused as `UnsupportedEncoding` rather than accepted, because the
/// production store contract is a UTF-8 representable path. The Runtime's
/// process-lock identity keeps its canonical pathname as a lossy UTF-8 string
/// and rebuilds the sidecar lock path from it, so an arbitrary Unix byte path
/// is not a losslessly representable Scheduler identity.
#[cfg(unix)]
#[test]
fn non_utf8_filenames_are_classified_by_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let uri = PathBuf::from(OsStr::from_bytes(b"file:agentype-\xff.sqlite"));
    assert_eq!(
        classify_store_path(&uri),
        StorePathKind::SqliteUri,
        "a `file:` prefix must be detected in raw bytes, not only in valid UTF-8"
    );

    let literal = PathBuf::from(OsStr::from_bytes(b"agentype-\xff.sqlite"));
    assert_eq!(
        classify_store_path(&literal),
        StorePathKind::UnsupportedEncoding,
        "a non-UTF-8 filename is not a UTF-8 representable production store"
    );

    // Both filenames must be refused by the boundary that actually hands them
    // to SQLite. The error text is not inspected here: it embeds the path,
    // which is not valid UTF-8.
    if Kernel::open(&uri, clock(), 10.0, CONTINUITY_MAX_BYTES).is_ok() {
        panic!("a non-UTF-8 URI filename must not open as a production store");
    }

    let dir = scratch("non-utf8");
    let path = dir.join(OsStr::from_bytes(b"agentype-\xff.sqlite"));
    if Kernel::open(&path, clock(), 10.0, CONTINUITY_MAX_BYTES).is_ok() {
        panic!("a non-UTF-8 literal filename must not open as a production store");
    }
    assert!(
        !path.exists(),
        "the refusal must happen before SQLite creates anything"
    );

    // Fully non-UTF-8 bytes behind the prefix: the byte-level URI test still
    // wins over the encoding check, so the security-relevant reason is named.
    let hidden_uri = PathBuf::from(OsStr::from_bytes(b"file:\xfe\xff"));
    assert_eq!(
        classify_store_path(&hidden_uri),
        StorePathKind::SqliteUri,
        "the byte-level `file:` test must fire regardless of UTF-8 validity"
    );

    let _ = std::fs::remove_dir_all(dir);
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

/// Every SQLite special filename must be refused by the store boundary itself,
/// before SQLite is asked to open anything. This is the layer that actually
/// hands a filename to SQLite, so the contract cannot be bypassed by a caller
/// that never built a `SqliteRuntimeConfig`.
///
/// The paths are used as written rather than joined onto a directory: neither
/// `dir/file:name` nor `dir\file:name` is a special filename, and on Windows
/// the first is not a legal file name at all.
#[test]
fn sqlite_special_filenames_are_refused_by_the_store_boundary() {
    for (name, expected) in [
        ("file:scheduler.sqlite", "SqliteUri"),
        ("file::memory:?cache=shared", "SqliteUri"),
        ("file:scheduler.sqlite?mode=rwc", "SqliteUri"),
        (":memory:", "Memory"),
        ("", "Temporary"),
    ] {
        let path = Path::new(name);
        let Err(err) = Kernel::open(path, clock(), 10.0, CONTINUITY_MAX_BYTES) else {
            panic!("{name} must not open as a production store");
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains(expected),
            "{name} must be refused as {expected}, got {rendered}"
        );
        assert!(
            !path.exists(),
            "{name} must be refused before any database file is created"
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

    // A literal file whose *name* is `:memory:` stays reachable through
    // SQLite's documented `./:memory:` escape hatch. It is only asserted at
    // the classifier level above: Windows forbids `:` in a file name, so there
    // is no portable positive open test for it.
}

/// An explicitly ephemeral store stays available, so the memory path is a
/// deliberate choice rather than a filename accident.
#[test]
fn explicit_memory_store_still_works() {
    let memory = Kernel::open_memory(clock(), 10.0, CONTINUITY_MAX_BYTES)
        .expect("open_memory is the explicit ephemeral path");
    assert_eq!(memory.schema_version().unwrap(), SCHEMA_VERSION);
}
