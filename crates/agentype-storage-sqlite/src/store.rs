//! SQLite connection, WAL / synchronous=FULL, IMMEDIATE transactions, and
//! Rust-era schema-family identity (fail-closed on foreign lineages).

use crate::schema::{SCHEMA_SQL, SCHEMA_VERSION};
use agentype_core::{Clock, Error, UnixTime};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// Durable lineage marker. Written into `scheduler_meta` on first creation.
/// This value is part of the persisted identity contract: once published it
/// MUST NOT change, or every existing Rust-era database becomes unrecognizable.
pub const IMPLEMENTATION_LINE: &str = "rust-v0.2";
const IDENTITY_KEY: &str = "implementation_line";

pub struct Store {
    conn: Mutex<Connection>,
}

/// True when `path` would be read as a SQLite **URI filename** rather than as
/// a filesystem path.
///
/// SQLite interprets a filename as a URI exactly when it begins with the
/// literal, case-sensitive prefix `file:`. Nothing else triggers URI
/// processing, so every other string — including a Windows drive letter such
/// as `C:\...` — is opened as a literal filesystem path.
///
/// This predicate exists because the Runtime's process lock resolves store
/// identity from a filesystem handle while SQLite resolves the *same* string
/// its own way. The two resolutions must agree, so a URI filename is not part
/// of the production store contract and is rejected before either side acts.
pub fn is_uri_filename(path: &Path) -> bool {
    path.as_os_str()
        .to_str()
        .is_some_and(|text| text.starts_with("file:"))
}

/// SQLite open flags for the production file-backed store.
///
/// `rusqlite::Connection::open` implies `SQLITE_OPEN_URI`, which is what makes
/// the `file:` prefix a URI. Production opens the Scheduler store with these
/// flags instead, so the filename SQLite opens is byte-for-byte the filesystem
/// path the process lock already owns.
const FILE_BACKED_OPEN_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
    .union(OpenFlags::SQLITE_OPEN_CREATE)
    .union(OpenFlags::SQLITE_OPEN_NO_MUTEX);

impl Store {
    /// Open the production file-backed Scheduler store.
    ///
    /// The URI-filename rejection is repeated here on purpose: this is the
    /// boundary that actually opens the database, so the invariant holds even
    /// for a caller that reached `Kernel::open` without going through
    /// `SqliteRuntimeConfig`. URI interpretation is disabled by flags rather
    /// than by the guard alone, so the guard is defence in depth.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if is_uri_filename(path) {
            return Err(Error::storage_failure(format!(
                "sqlite URI filenames are not a production Scheduler store: {}",
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(path, FILE_BACKED_OPEN_FLAGS)
            .map_err(|e| Error::storage_failure(format!("open sqlite: {e}")))?;
        verify_lineage_before_configure(&conn)?;
        configure(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn open_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()
            .map_err(|e| Error::storage_failure(format!("open memory sqlite: {e}")))?;
        verify_lineage_before_configure(&conn)?;
        configure(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn initialize(&self) -> Result<(), Error> {
        self.with_immediate(|tx, now| {
            // Idempotent DDL (CREATE TABLE IF NOT EXISTS)
            tx.execute_batch(SCHEMA_SQL)
                .map_err(|e| Error::storage_failure(format!("schema: {e}")))?;
            // Ensure identity marker exists for fresh databases
            let identity: Option<String> = tx
                .query_row(
                    "SELECT value_json FROM scheduler_meta WHERE key=?1",
                    rusqlite::params![IDENTITY_KEY],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            match identity.as_deref() {
                Some(line) if line == IMPLEMENTATION_LINE => {}
                Some(_) => {
                    return Err(Error::invariant(format!(
                        "database belongs to an unsupported implementation lineage ({identity:?})"
                    )));
                }
                None => {
                    tx.execute(
                        "INSERT INTO scheduler_meta(key,value_json,updated_at) VALUES(?1,?2,?3)",
                        rusqlite::params![IDENTITY_KEY, IMPLEMENTATION_LINE, now],
                    )
                    .map_err(map_sqlite)?;
                }
            }
            let current: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            if current > SCHEMA_VERSION {
                return Err(Error::invariant(format!(
                    "database schema {current} is newer than supported {SCHEMA_VERSION}"
                )));
            }
            if current == 0 {
                tx.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                    rusqlite::params![SCHEMA_VERSION, now],
                )
                .map_err(map_sqlite)?;
            } else if current < SCHEMA_VERSION {
                return Err(Error::invariant(format!(
                    "refusing to guess a repair for schema {current}; Rust-era schema is {SCHEMA_VERSION}"
                )));
            }
            Ok(())
        })
    }

    pub fn with_immediate<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>, UnixTime) -> Result<T, Error>,
    ) -> Result<T, Error> {
        // Store-level init helper (schema/identity bootstrap): no clock is
        // available yet, so bootstrap timestamps are 0.0.
        self.begin_immediate(|tx| f(tx, 0.0))
    }

    /// Run one IMMEDIATE transaction, sampling the authoritative time
    /// AFTER serialization is acquired (M5.3 audit P1-1).
    ///
    /// The timestamp handed to the transaction body is read only after the
    /// connection mutex is held and BEGIN IMMEDIATE has succeeded — i.e.
    /// after the transaction has actually won the SQLite write
    /// serialization. Sampling earlier would let a caller that waited for a
    /// contended writer lock validate authority against a stale reading and
    /// resurrect a lease that already expired in real time, breaking the
    /// frozen `now >= expires_at -> stale` boundary.
    pub fn with_immediate_clock<T>(
        &self,
        clock: &dyn Clock,
        f: impl FnOnce(&Transaction<'_>, UnixTime) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.begin_immediate(|tx| {
            let now = clock.now();
            f(tx, now)
        })
    }

    fn begin_immediate<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| Error::invariant("sqlite mutex poisoned"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        match f(&tx) {
            Ok(val) => {
                tx.commit().map_err(map_sqlite)?;
                Ok(val)
            }
            Err(err) => {
                let _ = tx.rollback();
                Err(err)
            }
        }
    }

    pub fn query<T>(&self, f: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::invariant("sqlite mutex poisoned"))?;
        f(&conn)
    }
}

/// Probe BEFORE configure: whether this database already contains user
/// tables decides "brand new" vs "existing lineage". Refusing foreign lineages
/// before setting PRAGMA journal_mode=WAL ensures fail-closed behavior without
/// mutating database headers or creating sidecars.
fn verify_lineage_before_configure(conn: &Connection) -> Result<(), Error> {
    let user_table_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .map_err(map_sqlite)?;
    if user_table_count == 0 {
        return Ok(());
    }
    let has_scheduler_meta: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='scheduler_meta')",
            [],
            |r| r.get::<_, i64>(0).map(|v| v != 0),
        )
        .map_err(map_sqlite)?;
    if !has_scheduler_meta {
        return Err(Error::invariant(
            "database contains existing tables but no Rust-era identity; \
             foreign or un-migrated databases are not importable (D-DB-MIGRATE unresolved); \
             refusing to open",
        ));
    }
    let identity: Option<String> = conn
        .query_row(
            "SELECT value_json FROM scheduler_meta WHERE key=?1",
            rusqlite::params![IDENTITY_KEY],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    match identity.as_deref() {
        Some(line) if line == IMPLEMENTATION_LINE => {}
        Some(_) => {
            return Err(Error::invariant(format!(
                "database belongs to an unsupported implementation lineage (implementation_line={identity:?}); refusing to open"
            )));
        }
        None => {
            return Err(Error::invariant(
                "database contains scheduler_meta but no implementation_line identity marker; \
                 foreign databases are not importable (D-DB-MIGRATE unresolved); \
                 refusing to open",
            ));
        }
    }
    let has_migrations: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |r| r.get::<_, i64>(0).map(|v| v != 0),
        )
        .map_err(map_sqlite)?;
    if !has_migrations {
        return Err(Error::invariant(
            "database has scheduler_meta but no schema_migrations table; refusing to open",
        ));
    }
    let current_version: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(map_sqlite)?;
    match current_version {
        Some(v) if v == SCHEMA_VERSION => Ok(()),
        Some(v) => Err(Error::invariant(format!(
            "database schema version {v} does not match expected {SCHEMA_VERSION}; refusing to open"
        ))),
        None => Err(Error::invariant(
            "database schema_migrations is empty; refusing to open",
        )),
    }
}

fn configure(conn: &Connection) -> Result<(), Error> {
    conn.busy_timeout(Duration::from_secs(30))
        .map_err(map_sqlite)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(map_sqlite)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(map_sqlite)?;
    Ok(())
}

pub fn map_sqlite(err: rusqlite::Error) -> Error {
    match &err {
        rusqlite::Error::SqliteFailure(e, Some(msg))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Error::conflict(msg.clone())
        }
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Error::conflict(err.to_string())
        }
        rusqlite::Error::QueryReturnedNoRows => Error::not_found("row"),
        _ => Error::storage_failure(format!("sqlite: {err}")),
    }
}

pub fn json_dump(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

/// Authoritative persisted state must decode strictly: a corrupted durable
/// document is an invariant violation, never a silent alternative schedule.
pub fn json_load(s: &str) -> Result<serde_json::Value, Error> {
    serde_json::from_str(s).map_err(|e| Error::invariant(format!("corrupted durable JSON: {e}")))
}

pub fn query_opt<T>(
    tx: &Transaction<'_>,
    sql: &str,
    params: impl rusqlite::Params,
    f: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Option<T>, Error> {
    tx.query_row(sql, params, f).optional().map_err(map_sqlite)
}
