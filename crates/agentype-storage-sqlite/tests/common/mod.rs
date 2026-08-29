//! Shared M4 test support. Storage-level fixtures construct persisted history
//! below the domain API boundary; every transition under test goes back
//! through the Kernel public API. Fixtures start from a real `Kernel::open`
//! schema and never hand-write DDL.

#![allow(dead_code)]

use agentype_core::*;
use agentype_execution_config::{
    resolve_execution_environment, ExecutionProfileConfig, ExecutionRegistry,
    ExecutionResolutionMode, ExecutionTargetConfig, FrozenExecutionSafety,
};
use agentype_storage_sqlite::Kernel;
use rusqlite::Connection;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Temporary directory holding a file-backed scheduler database.
pub struct FixtureDb {
    pub dir: PathBuf,
    pub path: PathBuf,
}

impl FixtureDb {
    pub fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("agentype-{tag}-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        Self { dir, path }
    }
}

impl Drop for FixtureDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub const CONTINUITY_MAX_BYTES: usize = 16_384;

pub struct Env {
    pub k: Kernel,
    pub clock: Arc<ManualClock>,
}

impl Env {
    fn build(store: impl FnOnce(Arc<dyn Clock>) -> Result<Kernel, Error>) -> Self {
        let clock = Arc::new(ManualClock::new(1_000_000.0));
        let k = store(clock.clone() as Arc<dyn Clock>).unwrap();
        k.upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
        k.reconcile_pool().unwrap();
        Self { k, clock }
    }
}

/// In-memory environment (kernel-level conformance tests).
pub fn memory_env() -> Env {
    Env::build(|clock| Kernel::open_memory(clock, 10.0, CONTINUITY_MAX_BYTES))
}

/// File-backed environment; required for storage-level fixtures.
pub fn file_env(db: &FixtureDb) -> Env {
    let path = db.path.clone();
    Env::build(move |clock| Kernel::open(&path, clock, 10.0, CONTINUITY_MAX_BYTES))
}

/// Re-open an existing file-backed database (restart simulation).
pub fn reopen(env: &Env, db: &FixtureDb) -> Kernel {
    let clock = env.clock.clone() as Arc<dyn Clock>;
    Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES).unwrap()
}

pub fn read_task(name: &str) -> TaskSpec {
    TaskSpec::new(name, json!({"objective": name}))
}

pub fn write_task(name: &str) -> TaskSpec {
    TaskSpec::new(name, json!({"objective": name})).write()
}

pub fn retryable_write(name: &str) -> TaskSpec {
    write_task(name).retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![
            FailureClass::ExecutionLost,
            FailureClass::Timeout,
            FailureClass::TransientExternal,
        ],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    })
}

pub fn retryable_read(name: &str) -> TaskSpec {
    read_task(name).retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![FailureClass::ExecutionLost, FailureClass::Timeout],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    })
}

pub fn run_claim(
    k: &Kernel,
    spec: TaskSpec,
    isolation: bool,
) -> (BatchId, TaskId, Claim, ExecutionId) {
    let (batch, ids) = k.submit_batch(std::slice::from_ref(&spec)).unwrap();
    let claim = k.claim_next_available().unwrap().expect("claim");
    let safety = if isolation {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new(
                &claim.execution_target,
                "test",
                true,
            ))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new(&claim.execution_profile))
            .unwrap();
        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            &claim.execution_target,
            &claim.execution_profile,
        )
        .unwrap();
        env.safety()
    } else {
        FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile)
    };
    let launch = k.create_execution(&claim, safety).unwrap();
    let execution_id = launch.execution_id().clone();
    k.confirm_running_and_renew(
        &claim.attempt_id,
        claim.lease_epoch,
        &execution_id,
        &json!({}),
    )
    .unwrap();
    (
        batch,
        ids.values().next().unwrap().clone(),
        claim,
        execution_id,
    )
}

// ---------------------------------------------------------------- fixtures
// Short-lived direct connections under the persistence boundary. They only
// fabricate historical/crash-leftover state; the operation under test is
// always invoked through the Kernel afterwards.

fn connect(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn
}

pub fn fixture_agent_state(db: &FixtureDb, agent: &LogicalAgentId, state: &str, now: f64) {
    let conn = connect(&db.path);
    let n = conn
        .execute(
            "UPDATE logical_agents SET state=?1,updated_at=?2 WHERE id=?3",
            rusqlite::params![state, now, agent.as_str()],
        )
        .unwrap();
    assert_eq!(n, 1, "fixture agent row missing");
}

/// Rewrite an agent's availability timestamp (matching-order fixture).
pub fn fixture_agent_available(db: &FixtureDb, agent: &LogicalAgentId, available_since: f64) {
    let conn = connect(&db.path);
    let n = conn
        .execute(
            "UPDATE logical_agents SET available_since=?1,updated_at=?2 WHERE id=?3",
            rusqlite::params![available_since, available_since, agent.as_str()],
        )
        .unwrap();
    assert_eq!(n, 1, "fixture agent row missing");
}

/// Corrupt an authoritative durable document (fail-closed fixture).
pub fn fixture_corrupt_json(db: &FixtureDb, table: &str, column: &str, id: &str) {
    let conn = connect(&db.path);
    let n = conn
        .execute(
            &format!("UPDATE {table} SET {column}='not-json{{' WHERE id=?1"),
            rusqlite::params![id],
        )
        .unwrap();
    assert_eq!(n, 1, "fixture row missing");
}

/// Insert a physical Incarnation in an arbitrary persisted state.
pub fn fixture_incarnation(
    db: &FixtureDb,
    agent: &LogicalAgentId,
    generation: i64,
    target: &str,
    state: &str,
) -> String {
    let id = format!("fix-inc-{}-{}", generation, nanos());
    let conn = connect(&db.path);
    conn.execute(
        "INSERT INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
         VALUES(?1,?2,?3,?4,?5,1.0)",
        rusqlite::params![id, agent.as_str(), generation, target, state],
    )
    .unwrap();
    id
}

pub fn fixture_execution(
    db: &FixtureDb,
    exec: &ExecutionId,
    state: &str,
    terminal_confirmed: bool,
    quiescent_confirmed: bool,
) {
    let conn = connect(&db.path);
    let n = conn
        .execute(
            "UPDATE executions SET state=?1,terminal_confirmed=?2,quiescent_confirmed=?3,updated_at=?4
             WHERE id=?5",
            rusqlite::params![
                state,
                terminal_confirmed as i64,
                quiescent_confirmed as i64,
                1.0,
                exec.as_str()
            ],
        )
        .unwrap();
    assert_eq!(n, 1, "fixture execution row missing");
}
