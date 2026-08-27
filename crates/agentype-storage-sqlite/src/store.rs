//! SQLite connection, WAL / synchronous=FULL, and IMMEDIATE transactions.

use crate::schema::{SCHEMA_SQL, SCHEMA_VERSION};
use agentype_core::{Error, UnixTime};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let conn = Connection::open(path.as_ref())
            .map_err(|e| Error::invariant(format!("open sqlite: {e}")))?;
        configure(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn open_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()
            .map_err(|e| Error::invariant(format!("open memory sqlite: {e}")))?;
        configure(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn initialize(&self) -> Result<(), Error> {
        self.with_immediate(|tx, now| {
            tx.execute_batch(SCHEMA_SQL)
                .map_err(|e| Error::invariant(format!("schema: {e}")))?;
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
        // Clock is supplied by Kernel; store-level helpers used only at init
        // pass 0. Kernel wraps this with the real clock.
        self.with_immediate_at(0.0, f)
    }

    pub fn with_immediate_at<T>(
        &self,
        now: UnixTime,
        f: impl FnOnce(&Transaction<'_>, UnixTime) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| Error::invariant("sqlite mutex poisoned"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        match f(&tx, now) {
            Ok(value) => {
                tx.commit().map_err(map_sqlite)?;
                Ok(value)
            }
            Err(err) => {
                let _ = tx.rollback();
                Err(err)
            }
        }
    }

    pub fn query<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::invariant("sqlite mutex poisoned"))?;
        f(&conn)
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
        _ => Error::invariant(format!("sqlite: {err}")),
    }
}

pub fn json_dump(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

/// Authoritative persisted state must decode strictly: a corrupted durable
/// document is an invariant violation, never a silent alternative schedule.
pub fn json_load(s: &str) -> Result<serde_json::Value, Error> {
    serde_json::from_str(s)
        .map_err(|e| Error::invariant(format!("corrupted durable JSON: {e}")))
}



pub fn query_opt<T>(
    tx: &Transaction<'_>,
    sql: &str,
    params: impl rusqlite::Params,
    f: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Option<T>, Error> {
    tx.query_row(sql, params, f).optional().map_err(map_sqlite)
}
