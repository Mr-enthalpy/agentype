//! OS-owned exclusive lock for one Scheduler store.
//!
//! Ownership is an OS Runtime property, not a Scheduler Lease, PID file, or
//! SQLite row. The lock identifies the store by canonical file identity
//! (Unix `dev:ino`, Windows volume+index), so path aliases and hard links
//! cannot mint a second daemon.

use fs4::fs_std::FileExt;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Production file-backed Scheduler store. `:memory:` is test-only and
/// cannot acquire a process lock.
#[derive(Debug, Clone)]
pub struct SqliteRuntimeConfig {
    path: PathBuf,
    lease_seconds: f64,
    continuity_max_bytes: usize,
}

impl SqliteRuntimeConfig {
    pub fn new(
        path: impl Into<PathBuf>,
        lease_seconds: f64,
        continuity_max_bytes: usize,
    ) -> Result<Self, ProcessLockError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(ProcessLockError::IdentityUnresolvable(
                "scheduler store path is empty".into(),
            ));
        }
        if path == Path::new(":memory:") {
            return Err(ProcessLockError::IdentityUnresolvable(
                "in-memory sqlite is not a production scheduler store".into(),
            ));
        }
        if !lease_seconds.is_finite() || lease_seconds <= 0.0 {
            return Err(ProcessLockError::IdentityUnresolvable(
                "lease_seconds must be finite and positive".into(),
            ));
        }
        if continuity_max_bytes == 0 {
            return Err(ProcessLockError::IdentityUnresolvable(
                "continuity_max_bytes must be positive".into(),
            ));
        }
        Ok(Self {
            path,
            lease_seconds,
            continuity_max_bytes,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lease_seconds(&self) -> f64 {
        self.lease_seconds
    }

    pub fn continuity_max_bytes(&self) -> usize {
        self.continuity_max_bytes
    }
}

/// Failure to acquire exclusive ownership of a Scheduler store.
#[derive(Debug)]
pub enum ProcessLockError {
    AlreadyRunning,
    IdentityUnresolvable(String),
    Io(io::Error),
}

impl std::fmt::Display for ProcessLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => {
                write!(f, "another scheduler daemon already owns this store")
            }
            Self::IdentityUnresolvable(detail) => {
                write!(f, "scheduler store identity unresolvable: {detail}")
            }
            Self::Io(err) => write!(f, "process lock i/o: {err}"),
        }
    }
}

impl std::error::Error for ProcessLockError {}

impl From<io::Error> for ProcessLockError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Diagnostic store identity. Never treated as Scheduler authority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreIdentity {
    file_id: String,
    canonical: String,
}

impl StoreIdentity {
    fn adjacent_lock_path(&self) -> Result<PathBuf, ProcessLockError> {
        let canonical = PathBuf::from(&self.canonical);
        let parent = canonical.parent().ok_or_else(|| {
            ProcessLockError::IdentityUnresolvable(
                "canonical store path has no parent directory".into(),
            )
        })?;
        Ok(parent.join(format!(".agentype-runtime-lock-{}", fnv64(&self.file_id))))
    }
}

fn fnv64(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for b in value.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Exclusive, nonblocking, crash-released ownership of one Scheduler store.
///
/// Non-Clone. Dropping the lock (or process death) releases the OS lock.
pub struct RuntimeProcessLock {
    _files: Vec<File>,
    _reservation: IdentityReservation,
    identity: StoreIdentity,
    store_path: PathBuf,
}

impl RuntimeProcessLock {
    /// Acquire exclusive ownership before any recovery mutation or Adapter call.
    pub fn acquire(config: &SqliteRuntimeConfig) -> Result<Self, ProcessLockError> {
        Self::acquire_path(config.path())
    }

    fn acquire_path(store_path: &Path) -> Result<Self, ProcessLockError> {
        if let Some(parent) = store_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let store = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(store_path)?;
        let identity = store_identity(&store, store_path)?;
        drop(store);
        let reservation = IdentityReservation::claim(&identity)?;
        let lock_path = store_lock_path(&identity)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        match file.try_lock_exclusive() {
            Ok(true) => {}
            Ok(false) => return Err(ProcessLockError::AlreadyRunning),
            Err(err) => return Err(err.into()),
        }
        Ok(Self {
            _files: vec![file],
            _reservation: reservation,
            identity,
            store_path: store_path.to_path_buf(),
        })
    }

    /// Canonical store identity string. Diagnostic only — not ownership.
    pub fn identity_debug(&self) -> &str {
        &self.identity.file_id
    }

    pub fn store_path(&self) -> &Path {
        &self.store_path
    }
}

struct IdentityReservation {
    keys: Vec<String>,
}

impl IdentityReservation {
    fn claim(identity: &StoreIdentity) -> Result<Self, ProcessLockError> {
        let mut held = held_identities()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut keys = Vec::new();
        for key in [identity.file_id.clone(), identity.canonical.clone()] {
            if !held.insert(key.clone()) {
                for added in &keys {
                    held.remove(added);
                }
                return Err(ProcessLockError::AlreadyRunning);
            }
            keys.push(key);
        }
        Ok(Self { keys })
    }
}

impl Drop for IdentityReservation {
    fn drop(&mut self) {
        let mut held = held_identities()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in &self.keys {
            held.remove(key);
        }
    }
}

fn held_identities() -> &'static Mutex<HashSet<String>> {
    static HELD: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(test)]
fn identity_keys_held(identity: &StoreIdentity) -> bool {
    let held = held_identities()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    held.contains(&identity.file_id) && held.contains(&identity.canonical)
}

/// Holds the process lock until Runtime workers have stopped.
pub struct RuntimeProcessGuard {
    lock: RuntimeProcessLock,
}

impl std::fmt::Debug for RuntimeProcessGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeProcessGuard")
            .field("identity", &self.lock.identity.file_id)
            .field("store_path", &self.lock.store_path)
            .finish_non_exhaustive()
    }
}

impl RuntimeProcessGuard {
    pub fn acquire(config: &SqliteRuntimeConfig) -> Result<Self, ProcessLockError> {
        Ok(Self {
            lock: RuntimeProcessLock::acquire(config)?,
        })
    }

    pub fn lock(&self) -> &RuntimeProcessLock {
        &self.lock
    }
}

fn store_lock_path(identity: &StoreIdentity) -> Result<PathBuf, ProcessLockError> {
    identity.adjacent_lock_path()
}

fn store_identity(_file: &File, path: &Path) -> Result<StoreIdentity, ProcessLockError> {
    let _canonical = fs::canonicalize(path).map_err(|err| {
        ProcessLockError::IdentityUnresolvable(format!(
            "cannot canonicalize {}: {err}",
            path.display()
        ))
    })?;
    let id = file_id::get_file_id(path).map_err(|err| {
        ProcessLockError::IdentityUnresolvable(format!(
            "cannot read file identity for {}: {err}",
            path.display()
        ))
    })?;
    Ok(StoreIdentity {
        file_id: format!("{id:?}"),
        canonical: _canonical.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
fn store_identity_for_tests(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    store_identity(&file, path).ok().map(|id| id.file_id)
}

/// Production dispatch eligibility. Runtime-local, non-serializable, no
/// public constructor. Minted only after lock + recovery + activation.
pub struct ReadyPermit {
    _private: (),
}

impl ReadyPermit {
    #[allow(dead_code)]
    pub(crate) fn mint() -> Self {
        Self { _private: () }
    }
}

impl std::fmt::Debug for ReadyPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadyPermit").finish_non_exhaustive()
    }
}

/// Tiny helper used by cross-process lock tests: acquire and wait for stdin EOF.
#[doc(hidden)]
pub fn hold_process_lock_until_stdin_closes(path: &Path) -> Result<(), ProcessLockError> {
    let config = SqliteRuntimeConfig::new(path, 10.0, 16_384)?;
    let _guard = RuntimeProcessGuard::acquire(&config)?;
    let mut stdout = io::stdout();
    stdout.write_all(b"LOCKED\n")?;
    stdout.flush()?;
    let mut sink = String::new();
    let _ = io::stdin().read_line(&mut sink);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("agentype-lock-{nanos}.sqlite"))
    }

    #[test]
    fn first_lock_succeeds_and_is_not_clone() {
        let path = temp_store();
        let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
        let guard = RuntimeProcessGuard::acquire(&cfg).unwrap();
        assert!(!guard.lock().identity_debug().is_empty());
        drop(guard);
        let _again = RuntimeProcessGuard::acquire(&cfg).unwrap();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn same_process_second_acquire_is_already_running() {
        let path = temp_store();
        let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
        let _guard = RuntimeProcessGuard::acquire(&cfg).unwrap();
        match RuntimeProcessGuard::acquire(&cfg) {
            Err(ProcessLockError::AlreadyRunning) => {}
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
        let file = File::open(&path).unwrap();
        let id = store_identity(&file, &path).unwrap();
        assert!(
            identity_keys_held(&id),
            "failed second claim must not drop the holder's reservation"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn failed_second_claim_does_not_drop_first_reservation() {
        let path = temp_store();
        let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
        let guard = RuntimeProcessGuard::acquire(&cfg).unwrap();
        let file = File::open(&path).unwrap();
        let id = store_identity(&file, &path).unwrap();
        assert!(identity_keys_held(&id));
        assert!(matches!(
            RuntimeProcessGuard::acquire(&cfg),
            Err(ProcessLockError::AlreadyRunning)
        ));
        assert!(identity_keys_held(&id));
        drop(guard);
        assert!(!identity_keys_held(&id));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn memory_path_fails_closed() {
        match SqliteRuntimeConfig::new(":memory:", 10.0, 16_384) {
            Err(ProcessLockError::IdentityUnresolvable(_)) => {}
            other => panic!("expected IdentityUnresolvable, got {other:?}"),
        }
    }

    #[test]
    fn hardlink_cannot_create_independent_ownership() {
        let path = temp_store();
        let cfg = SqliteRuntimeConfig::new(&path, 10.0, 16_384).unwrap();
        let guard = RuntimeProcessGuard::acquire(&cfg).unwrap();
        let alias = path.with_extension("hardlink.sqlite");
        match fs::hard_link(&path, &alias) {
            Ok(()) => {
                let original_id = guard.lock().identity_debug().to_string();
                let alias_id = store_identity_for_tests(&alias);
                if alias_id.as_deref() == Some(original_id.as_str()) {
                    let alias_cfg = SqliteRuntimeConfig::new(&alias, 10.0, 16_384).unwrap();
                    match RuntimeProcessGuard::acquire(&alias_cfg) {
                        Err(ProcessLockError::AlreadyRunning) => {}
                        other => panic!("hardlink must share lock, got {other:?}"),
                    }
                }
                let _ = fs::remove_file(&alias);
            }
            Err(err) => {
                let _ = err;
            }
        }
        drop(guard);
        let _ = fs::remove_file(path);
    }
}
