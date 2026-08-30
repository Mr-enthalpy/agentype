//! Transactional M4 kernel. Semantics come from the spec; this crate persists them.

use crate::store::{json_dump, json_load, map_sqlite, query_opt, Store};
use crate::txutil::*;
use agentype_core::*;
use agentype_execution_config::{ExecutionLaunchSnapshot, FrozenPhysicalExecutionBinding};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Mechanical outcome of a supervised heartbeat renewal (M5.3 §10/§11).
///
/// `Renewed` means the Scheduler continues granting the current Attempt
/// execution authority for another lease interval. `NotRunning` means the
/// Execution exists, belongs to the Attempt, and the Attempt/Lease authority
/// was still valid — but the Execution is no longer physically RUNNING;
/// supervision ownership must be dropped and the durable physical state must
/// NOT be repaired from heartbeat code. Authority loss and storage faults
/// remain `Err(Error)` for the caller to classify: stale/invalid/not-found →
/// authority loss, anything else → fatal persistence fault (M5.3 §15).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SupervisedRenewal {
    Renewed(UnixTime),
    NotRunning,
}

/// Narrow supervision view of a lease: renewal bookkeeping only (heartbeat
/// bookkeeping, expiry, state). Test and timing assertion surface — it grants
/// no authority.
#[derive(Clone, Copy, Debug)]
pub struct LeaseSupervisionView {
    pub heartbeat_at: UnixTime,
    pub expires_at: UnixTime,
    pub state: LeaseState,
}
use std::sync::Arc;

pub struct Kernel {
    store: Store,
    clock: Arc<dyn Clock>,
    lease_seconds: f64,
    continuity_max_bytes: usize,
}

impl Kernel {
    pub fn open(
        path: impl AsRef<Path>,
        clock: Arc<dyn Clock>,
        lease_seconds: f64,
        continuity_max_bytes: usize,
    ) -> Result<Self, Error> {
        Self::from_store(
            Store::open(path)?,
            clock,
            lease_seconds,
            continuity_max_bytes,
        )
    }

    pub fn open_memory(
        clock: Arc<dyn Clock>,
        lease_seconds: f64,
        continuity_max_bytes: usize,
    ) -> Result<Self, Error> {
        Self::from_store(
            Store::open_memory()?,
            clock,
            lease_seconds,
            continuity_max_bytes,
        )
    }

    fn from_store(
        store: Store,
        clock: Arc<dyn Clock>,
        lease_seconds: f64,
        continuity_max_bytes: usize,
    ) -> Result<Self, Error> {
        // Finite-authority gate (M5.3 audit P1-4): NaN passes `<= 0.0`
        // comparisons and an infinite lease could never naturally expire,
        // so lease authority must be finite AND positive.
        if !(lease_seconds.is_finite() && lease_seconds > 0.0) {
            return Err(Error::invalid_transition(
                "lease_seconds must be finite and positive",
            ));
        }
        if continuity_max_bytes == 0 {
            return Err(Error::invalid_transition(
                "continuity_max_bytes must be positive",
            ));
        }
        Ok(Self {
            store,
            clock,
            lease_seconds,
            continuity_max_bytes,
        })
    }

    fn tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>, UnixTime) -> Result<T, Error>,
    ) -> Result<T, Error> {
        // The transaction timestamp is sampled by the store AFTER the
        // connection lock and BEGIN IMMEDIATE succeed (M5.3 audit P1-1):
        // authority validation must never run against a clock reading
        // taken before the transaction won the SQLite write
        // serialization, or a contended renewal could resurrect an
        // already-expired lease.
        self.store.with_immediate_clock(self.clock.as_ref(), f)
    }

    pub fn now(&self) -> UnixTime {
        self.clock.now()
    }

    /// The lease duration this Kernel grants on every renewal. Read-only
    /// configuration accessor so runtime composition can validate a
    /// heartbeat policy against Kernel-owned lease authority without
    /// duplicating it (M5.3 §30).
    pub fn lease_seconds(&self) -> f64 {
        self.lease_seconds
    }

    pub fn pragmas(&self) -> Result<(String, i64, i64), Error> {
        self.store.query(|conn| {
            let journal: String = conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .map_err(map_sqlite)?;
            let sync: i64 = conn
                .pragma_query_value(None, "synchronous", |r| r.get(0))
                .map_err(map_sqlite)?;
            let fk: i64 = conn
                .pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .map_err(map_sqlite)?;
            Ok((journal, sync, fk))
        })
    }

    pub fn schema_version(&self) -> Result<i64, Error> {
        self.store.query(|conn| {
            conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .map_err(map_sqlite)
        })
    }

    // ------------------------------------------------------------------ topology

    pub fn upsert_partition(&self, spec: &PartitionSpec) -> Result<i64, Error> {
        if spec.desired_capacity < 0 {
            return Err(Error::invalid_transition(
                "desired_capacity must be non-negative",
            ));
        }
        self.tx(|tx, now| {
            match query_opt(
                tx,
                "SELECT retention,execution_target,execution_profile,tags_json,active,desired_capacity,topology_revision
                 FROM pool_partitions WHERE name=?1",
                params![spec.name.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)? != 0,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                },
            )? {
                Some((retention, target, profile, tags_json, active, cap, rev)) => {
                    if !active {
                        return Err(Error::invalid_transition(
                            "pool upsert cannot reactivate an inactive partition",
                        ));
                    }
                    let mut mismatches = Vec::new();
                    if retention != spec.retention.as_sql() {
                        mismatches.push("retention");
                    }
                    if target != spec.execution_target {
                        mismatches.push("execution_target");
                    }
                    if profile != spec.execution_profile {
                        mismatches.push("execution_profile");
                    }
                    let mut existing_tags = parse_str_list(&tags_json)?;
                    existing_tags.sort();
                    let mut new_tags = spec.tags.clone();
                    new_tags.sort();
                    if existing_tags != new_tags {
                        mismatches.push("tags");
                    }
                    if !mismatches.is_empty() {
                        return Err(Error::invalid_transition(format!(
                            "pool upsert cannot mutate an existing partition's structural definition: {}",
                            mismatches.join(", ")
                        )));
                    }
                    if cap != spec.desired_capacity {
                        return Err(Error::invalid_transition(
                            "pool upsert cannot resize an existing partition; use pool resize",
                        ));
                    }
                    Ok(rev)
                }
                None => {
                    let payload = serde_json::json!({
                        "name": spec.name.as_str(),
                        "desired_capacity": spec.desired_capacity,
                        "retention": spec.retention.as_sql(),
                        "execution_target": spec.execution_target,
                        "execution_profile": spec.execution_profile,
                        "tags": spec.tags,
                    });
                    let revision = insert_revision(tx, "UPSERT", &payload, now)?;
                    tx.execute(
                        "INSERT INTO pool_partitions(name,desired_capacity,retention,execution_target,
                         execution_profile,tags_json,active,topology_revision,created_at,updated_at)
                         VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8,?8)",
                        params![
                            spec.name.as_str(),
                            spec.desired_capacity,
                            spec.retention.as_sql(),
                            spec.execution_target,
                            spec.execution_profile,
                            json_dump(&Value::Array(
                                spec.tags.iter().cloned().map(Value::String).collect()
                            )),
                            revision,
                            now
                        ],
                    )
                    .map_err(map_sqlite)?;
                    Ok(revision)
                }
            }
        })
    }

    pub fn create_workstream(
        &self,
        name: &str,
        project_state_ref: Option<&str>,
        workstream_id: Option<WorkstreamId>,
    ) -> Result<WorkstreamId, Error> {
        let id = workstream_id.unwrap_or_default();
        self.tx(|tx, now| {
            tx.execute(
                "INSERT INTO workstreams(id,name,project_state_ref,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",
                params![id.as_str(), name, project_state_ref, now],
            )
            .map_err(map_sqlite)?;
            Ok(id)
        })
    }

    pub fn resize_partition(&self, name: &str, desired_capacity: i64) -> Result<i64, Error> {
        if desired_capacity < 0 {
            return Err(Error::invalid_transition(
                "desired_capacity must be non-negative",
            ));
        }
        self.tx(|tx, now| {
            required_partition(tx, name, true)?;
            let revision = insert_revision(
                tx,
                "RESIZE",
                &serde_json::json!({"name": name, "desired_capacity": desired_capacity}),
                now,
            )?;
            tx.execute(
                "UPDATE pool_partitions SET desired_capacity=?1,topology_revision=?2,updated_at=?3 WHERE name=?4",
                params![desired_capacity, revision, now, name],
            )
            .map_err(map_sqlite)?;
            Ok(revision)
        })
    }

    pub fn reconcile_pool(&self) -> Result<ReconcileReport, Error> {
        self.tx(|tx, now| {
            let mut report = ReconcileReport::default();
            let idle_drains: Vec<String> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id FROM logical_agents WHERE state='DRAINING'
                         AND current_task_id IS NULL AND pending_partition_name IS NOT NULL
                         AND retirement_requested=0 ORDER BY id",
                    )
                    .map_err(map_sqlite)?;
                let ids = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                ids.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?
            };
            for id in idle_drains {
                release_agent(tx, &id, now)?;
            }
            let partitions: Vec<PartitionRow> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT name,desired_capacity,retention,execution_target,execution_profile,tags_json,active,merged_into,topology_revision
                         FROM pool_partitions WHERE active=1 ORDER BY name",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map([], PartitionRow::from_query)
                    .map_err(map_sqlite)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?
            };
            for partition in partitions {
                // Coarse pre-filter; shrink ordering is a core decision
                // (unassigned first, READY first, lowest id) revalidated over
                // the snapshot so SQL text cannot change the outcome.
                let members: Vec<(AgentRow, Option<f64>, f64)> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                                    pending_partition_name,retirement_requested,continuity_json,continuity_version,
                                    available_since,created_at
                             FROM logical_agents
                             WHERE COALESCE(pending_partition_name,partition_name)=?1
                             AND state IN ('INITIALIZING','READY','ASSIGNED','DRAINING','REVIVING')
                             AND NOT (state='DRAINING' AND retirement_requested=1)",
                        )
                        .map_err(map_sqlite)?;
                    let rows = stmt
                        .query_map(params![partition.name], |r| {
                            let agent = AgentRow::from_query(r)?;
                            let available_since = r.get::<_, Option<f64>>(11)?;
                            let created_at = r.get::<_, f64>(12)?;
                            Ok((agent, available_since, created_at))
                        })
                        .map_err(map_sqlite)?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(row.map_err(map_sqlite)?);
                    }
                    out
                };
                let deficit = partition.desired_capacity - members.len() as i64;
                for _ in 0..deficit.max(0) {
                    birth_agent(tx, &partition.name, None, None, now)?;
                    report.born += 1;
                }
                let excess = (-deficit).max(0) as usize;
                if excess > 0 {
                    let mut candidates: Vec<PoolMemberSnapshot> = Vec::with_capacity(members.len());
                    for (a, available_since, created_at) in &members {
                        candidates.push(PoolMemberSnapshot {
                            id: a.id.clone(),
                            state: LogicalAgentState::parse_sql(&a.state)?,
                            assigned_to_task: a.current_task_id.is_some(),
                            retirement_requested: false,
                            available_since: *available_since,
                            created_at: *created_at,
                        });
                    }
                    sort_excess_candidates(&mut candidates);
                    for candidate in candidates.into_iter().take(excess) {
                        match excess_disposition(candidate.state, candidate.assigned_to_task) {
                            ExcessDisposition::RetireDirectly => {
                                retire_logical_agent(tx, &candidate.id, now)?;
                                report.retired += 1;
                            }
                            ExcessDisposition::DrainForRetirement => {
                                tx.execute(
                                    "UPDATE logical_agents SET state='DRAINING',retirement_requested=1,updated_at=?1 WHERE id=?2",
                                    params![now, candidate.id],
                                )
                                .map_err(map_sqlite)?;
                                report.draining += 1;
                            }
                        }
                    }
                }
            }
            Ok(report)
        })
    }

    pub fn move_capacity(&self, source: &str, target: &str, count: i64) -> Result<i64, Error> {
        if source == target {
            return Err(Error::invalid_transition(
                "source and target partitions must differ",
            ));
        }
        if count <= 0 {
            return Err(Error::invalid_transition("count must be positive"));
        }
        self.tx(|tx, now| {
            let source_p = required_partition(tx, source, true)?;
            let target_p = required_partition(tx, target, true)?;
            if count > source_p.desired_capacity {
                return Err(Error::invalid_transition(
                    "cannot move more capacity than the source desired capacity",
                ));
            }
            let revision = insert_revision(
                tx,
                "MOVE_CAPACITY",
                &serde_json::json!({"source": source, "target": target, "count": count}),
                now,
            )?;
            tx.execute(
                "UPDATE pool_partitions SET desired_capacity=?1,topology_revision=?2,updated_at=?3 WHERE name=?4",
                params![source_p.desired_capacity - count, revision, now, source],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE pool_partitions SET desired_capacity=?1,topology_revision=?2,updated_at=?3 WHERE name=?4",
                params![target_p.desired_capacity + count, revision, now, target],
            )
            .map_err(map_sqlite)?;
            // Coarse pre-filter; candidate ordering and per-member cutover
            // planning are core decisions (spec 15).
            let members: Vec<(AgentRow, Option<f64>, f64)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                                pending_partition_name,retirement_requested,continuity_json,continuity_version,
                                available_since,created_at
                         FROM logical_agents
                         WHERE COALESCE(pending_partition_name,partition_name)=?1
                         AND state IN ('INITIALIZING','READY','ASSIGNED','DRAINING','REVIVING','SUSPENDED')
                         AND retirement_requested=0",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![source], |r| {
                        let agent = AgentRow::from_query(r)?;
                        let available_since = r.get::<_, Option<f64>>(11)?;
                        let created_at = r.get::<_, f64>(12)?;
                        Ok((agent, available_since, created_at))
                    })
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                out
            };
            let mut candidates: Vec<(PoolMemberSnapshot, usize)> = Vec::with_capacity(members.len());
            for (i, (a, available_since, created_at)) in members.iter().enumerate() {
                candidates.push((
                    PoolMemberSnapshot {
                        id: a.id.clone(),
                        state: LogicalAgentState::parse_sql(&a.state)?,
                        assigned_to_task: a.current_task_id.is_some(),
                        retirement_requested: a.retirement_requested,
                        available_since: *available_since,
                        created_at: *created_at,
                    },
                    i,
                ));
            }
            candidates.retain(|(s, _)| move_candidate_eligible(s));
            candidates.sort_by(|a, b| {
                move_rank_key(&a.0)
                    .partial_cmp(&move_rank_key(&b.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (candidate, idx) in candidates.into_iter().take(count as usize) {
                let agent = &members[idx].0;
                match plan_move_cutover(candidate.state, candidate.assigned_to_task) {
                    MoveCutoverPlan::StageDrain => {
                        tx.execute(
                            "UPDATE logical_agents SET state='DRAINING',pending_partition_name=?1,
                             available_since=NULL,updated_at=?2 WHERE id=?3",
                            params![target, now, agent.id],
                        )
                        .map_err(map_sqlite)?;
                    }
                    MoveCutoverPlan::ReconnectCutover { restore_ready } => {
                        request_partition_cutover(tx, agent, target, now)?;
                        if restore_ready {
                            tx.execute(
                                "UPDATE logical_agents SET state='READY',available_since=?1,updated_at=?1 WHERE id=?2",
                                params![now, agent.id],
                            )
                            .map_err(map_sqlite)?;
                        }
                    }
                }
            }
            Ok(revision)
        })
    }

    pub fn merge_partitions(&self, source: &str, target: &str) -> Result<i64, Error> {
        if source == target {
            return Err(Error::invalid_transition(
                "source and target partitions must differ",
            ));
        }
        self.tx(|tx, now| {
            let source_p = required_partition(tx, source, true)?;
            let target_p = required_partition(tx, target, true)?;
            let revision = insert_revision(
                tx,
                "MERGE",
                &serde_json::json!({
                    "source": source,
                    "target": target,
                    "source_capacity": source_p.desired_capacity,
                    "target_capacity": target_p.desired_capacity,
                }),
                now,
            )?;
            let merged_capacity = source_p.desired_capacity + target_p.desired_capacity;
            tx.execute(
                "UPDATE tasks SET partition_name=?1,updated_at=?2 WHERE partition_name=?3
                 AND state NOT IN ('COMPLETED','CANCELLED')",
                params![target, now, source],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE logical_agents SET pending_partition_name=?1,updated_at=?2
                 WHERE pending_partition_name=?3 AND state<>'RETIRED'",
                params![target, now, source],
            )
            .map_err(map_sqlite)?;
            let immediate: Vec<AgentRow> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                                pending_partition_name,retirement_requested,continuity_json,continuity_version
                         FROM logical_agents WHERE partition_name=?1
                         AND (state IN ('READY','INITIALIZING','REVIVING')
                              OR (state='DRAINING' AND current_task_id IS NULL))",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![source], AgentRow::from_query)
                    .map_err(map_sqlite)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?
            };
            for agent in immediate {
                let mut desired = agent
                    .pending_partition
                    .clone()
                    .unwrap_or_else(|| target.to_string());
                if desired == source {
                    desired = target.to_string();
                }
                if agent.state == "DRAINING" {
                    tx.execute(
                        "UPDATE logical_agents SET pending_partition_name=?1 WHERE id=?2",
                        params![desired, agent.id],
                    )
                    .map_err(map_sqlite)?;
                    release_agent(tx, &agent.id, now)?;
                } else {
                    request_partition_cutover(tx, &agent, &desired, now)?;
                }
            }
            tx.execute(
                "UPDATE logical_agents SET pending_partition_name=?1,updated_at=?2
                 WHERE partition_name=?3 AND state IN ('ASSIGNED','DRAINING','SUSPENDED')
                 AND (pending_partition_name IS NULL OR pending_partition_name=?3)",
                params![target, now, source],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE pool_partitions SET active=0,desired_capacity=0,merged_into=?1,
                 topology_revision=?2,updated_at=?3 WHERE name=?4",
                params![target, revision, now, source],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE pool_partitions SET desired_capacity=?1,topology_revision=?2,updated_at=?3 WHERE name=?4",
                params![merged_capacity, revision, now, target],
            )
            .map_err(map_sqlite)?;
            Ok(revision)
        })
    }

    pub fn retire_partition(&self, name: &str) -> Result<i64, Error> {
        self.tx(|tx, now| {
            required_partition(tx, name, true)?;
            if let Some((id, state)) = query_opt(
                tx,
                "SELECT id,state FROM tasks WHERE partition_name=?1
                 AND state NOT IN ('COMPLETED','CANCELLED') ORDER BY created_at,id LIMIT 1",
                params![name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )? {
                return Err(Error::invalid_transition(format!(
                    "cannot retire partition with nonterminal Task {id} in state {state}"
                )));
            }
            if let Some(id) = query_opt(
                tx,
                "SELECT id FROM logical_agents WHERE pending_partition_name=?1
                 AND state<>'RETIRED' ORDER BY id LIMIT 1",
                params![name],
                |r| r.get::<_, String>(0),
            )? {
                return Err(Error::invalid_transition(format!(
                    "cannot retire partition with desired LogicalAgent {id}"
                )));
            }
            if let Some(id) = query_opt(
                tx,
                "SELECT a.id FROM logical_agents a
                 JOIN escalations e ON e.logical_agent_id=a.id
                 WHERE a.partition_name=?1 AND a.state<>'RETIRED'
                 AND e.state='OPEN' AND e.failure_class='WRITER_QUIESCENCE_UNKNOWN'
                 ORDER BY a.id LIMIT 1",
                params![name],
                |r| r.get::<_, String>(0),
            )? {
                return Err(Error::invalid_transition(format!(
                    "cannot retire partition with open writer-safety obligation on LogicalAgent {id}"
                )));
            }
            let departing: Vec<AgentRow> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                                pending_partition_name,retirement_requested,continuity_json,continuity_version
                         FROM logical_agents WHERE partition_name=?1
                         AND pending_partition_name IS NOT NULL AND current_task_id IS NULL
                         AND state<>'RETIRED' ORDER BY id",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![name], AgentRow::from_query)
                    .map_err(map_sqlite)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?
            };
            for agent in departing {
                if agent.state == "DRAINING" {
                    release_agent(tx, &agent.id, now)?;
                } else if let Some(pending) = &agent.pending_partition {
                    request_partition_cutover(tx, &agent, pending, now)?;
                }
            }
            let revision = insert_revision(tx, "RETIRE", &serde_json::json!({"name": name}), now)?;
            let idle: Vec<String> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id FROM logical_agents WHERE partition_name=?1
                         AND pending_partition_name IS NULL
                         AND state IN ('READY','INITIALIZING','SUSPENDED','REVIVING')",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![name], |r| r.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite)?
            };
            for id in idle {
                retire_logical_agent(tx, &id, now)?;
            }
            tx.execute(
                "UPDATE logical_agents SET state='DRAINING',retirement_requested=1,updated_at=?1
                 WHERE partition_name=?2 AND pending_partition_name IS NULL AND state='ASSIGNED'",
                params![now, name],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE pool_partitions SET active=0,desired_capacity=0,topology_revision=?1,updated_at=?2 WHERE name=?3",
                params![revision, now, name],
            )
            .map_err(map_sqlite)?;
            Ok(revision)
        })
    }

    // ------------------------------------------------------------------ work

    pub fn submit_batch(
        &self,
        tasks: &[TaskSpec],
    ) -> Result<(BatchId, HashMap<String, TaskId>), Error> {
        if tasks.is_empty() {
            return Err(Error::invalid_transition(
                "a batch requires at least one task",
            ));
        }
        let mut names = HashSet::new();
        for t in tasks {
            if !names.insert(t.name.clone()) {
                return Err(Error::invalid_transition(
                    "task names must be unique within a batch",
                ));
            }
        }
        self.tx(|tx, now| {
            let batch_id = BatchId::new();
            tx.execute(
                "INSERT INTO batches(id,state,metadata_json,created_at,updated_at) VALUES(?1,'ACTIVE','{}',?2,?2)",
                params![batch_id.as_str(), now],
            )
            .map_err(map_sqlite)?;
            let mut ids: HashMap<String, TaskId> = HashMap::new();
            for t in tasks {
                ids.insert(t.name.clone(), t.task_id.clone().unwrap_or_else(TaskId::new));
            }
            let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
            for t in tasks {
                let tid = ids[&t.name].as_str().to_string();
                let mut resolved = Vec::new();
                for dep in &t.dependencies {
                    if let Some(id) = ids.get(dep) {
                        resolved.push(id.as_str().to_string());
                    } else if ids.values().any(|v| v.as_str() == dep) {
                        resolved.push(dep.clone());
                    } else {
                        return Err(Error::invalid_transition(format!(
                            "unknown dependency {dep:?} for {}",
                            t.name
                        )));
                    }
                }
                dependencies.insert(tid, resolved);
            }
            assert_acyclic(&dependencies)?;
            for t in tasks {
                required_partition(tx, t.partition.as_str(), true)?;
                let task_id = ids[&t.name].as_str();
                let state = if dependencies[task_id].is_empty() {
                    "QUEUED"
                } else {
                    "BLOCKED"
                };
                let retry_json = Value::Array(
                    t.retry_policy
                        .retry_classes
                        .iter()
                        .map(|c| Value::String(c.as_sql().to_string()))
                        .collect(),
                );
                tx.execute(
                    "INSERT INTO tasks(id,batch_id,name,payload_json,acceptance_json,partition_name,
                     workstream_id,continuity,affinity_tags_json,workspace_mode,required,priority,state,
                     max_attempts,retry_classes_json,base_backoff_seconds,max_backoff_seconds,
                     supersedes_task_id,created_at,updated_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11,?12,?13,?14,?15,?16,?17,?18,?18)",
                    params![
                        task_id,
                        batch_id.as_str(),
                        t.name,
                        json_dump(&t.payload),
                        json_dump(&t.acceptance),
                        t.partition.as_str(),
                        t.workstream_id.as_ref().map(|w| w.as_str().to_string()),
                        t.continuity.as_sql(),
                        json_dump(&Value::Array(
                            t.affinity_tags.iter().cloned().map(Value::String).collect()
                        )),
                        t.workspace_mode.as_sql(),
                        t.priority,
                        state,
                        t.retry_policy.max_attempts as i64,
                        json_dump(&retry_json),
                        t.retry_policy.base_backoff_seconds,
                        t.retry_policy.max_backoff_seconds,
                        t.supersedes_task_id.as_ref().map(|s| s.as_str().to_string()),
                        now
                    ],
                )
                .map_err(map_sqlite)?;
            }
            for (task_id, deps) in &dependencies {
                for dep in deps {
                    tx.execute(
                        "INSERT INTO task_dependencies(task_id,depends_on_task_id) VALUES(?1,?2)",
                        params![task_id, dep],
                    )
                    .map_err(map_sqlite)?;
                }
            }
            Ok((batch_id, ids))
        })
    }

    pub fn claim_next_available(&self) -> Result<Option<Claim>, Error> {
        let lease_seconds = self.lease_seconds;
        self.tx(|tx, now| {
            // Coarse pre-filter only (performance); semantic eligibility and
            // ordering are re-decided by core::decisions below so that query
            // text alone can never change scheduler behavior (spec 15).
            let tasks: Vec<(TaskRow, i64, f64, bool, String)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT t.id,t.batch_id,t.name,t.payload_json,t.acceptance_json,t.partition_name,t.workstream_id,
                                t.continuity,t.affinity_tags_json,t.workspace_mode,t.state,t.max_attempts,t.retry_classes_json,
                                t.base_backoff_seconds,t.max_backoff_seconds,t.next_eligible_at,t.current_attempt_id,t.fencing_epoch,
                                t.priority,t.created_at,p.active,b.state
                         FROM tasks t JOIN batches b ON b.id=t.batch_id
                         JOIN pool_partitions p ON p.name=t.partition_name
                         WHERE t.state='QUEUED'",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![], |r| {
                        Ok((
                            TaskRow::from_query(r)?,
                            r.get::<_, i64>(18)?,
                            r.get::<_, f64>(19)?,
                            r.get::<_, i64>(20)? != 0,
                            r.get::<_, String>(21)?,
                        ))
                    })
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                out
            };
            // Core decides which queued tasks are claimable and in what order.
            let mut snapshots: Vec<ClaimTaskSnapshot> = Vec::with_capacity(tasks.len());
            for (t, priority, created_at, active, batch_state) in &tasks {
                snapshots.push(ClaimTaskSnapshot {
                    id: t.id.clone(),
                    state: TaskState::parse_sql(&t.state)?,
                    batch_state: BatchState::parse_sql(batch_state)?,
                    partition_active: *active,
                    next_eligible_at: t.next_eligible_at,
                    priority: *priority,
                    created_at: *created_at,
                });
            }
            for task_id in order_claim_tasks(&snapshots, now) {
                let (task, _, _, _, _) =
                    match tasks.iter().find(|(t, _, _, _, _)| t.id == task_id) {
                        Some(found) => found,
                        None => continue,
                    };
                // Coarse pre-filter (partition + cheap readiness predicate);
                // selection semantics live in core::select_claim_agent.
                let agents: Vec<(AgentRow, Option<f64>, f64)> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                                    pending_partition_name,retirement_requested,continuity_json,continuity_version,
                                    available_since,created_at
                             FROM logical_agents WHERE partition_name=?1 AND state='READY'
                             AND current_task_id IS NULL",
                        )
                        .map_err(map_sqlite)?;
                    let rows = stmt
                        .query_map(params![task.partition], |r| {
                            let agent = AgentRow::from_query(r)?;
                            let available_since = r.get::<_, Option<f64>>(11)?;
                            let created_at = r.get::<_, f64>(12)?;
                            Ok((agent, available_since, created_at))
                        })
                        .map_err(map_sqlite)?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(row.map_err(map_sqlite)?);
                    }
                    out
                };
                let task_tags = parse_str_list(&task.affinity_tags_json)?;
                let continuity = ContinuityPreference::parse_sql(&task.continuity)?;
                let intent = ClaimIntent {
                    partition: &task.partition,
                    required_tags: &task_tags,
                    workstream_id: task.workstream_id.as_deref(),
                    continuity,
                };
                let mut agent_snaps: Vec<ClaimAgentSnapshot> = Vec::with_capacity(agents.len());
                for (a, available_since, created_at) in &agents {
                    agent_snaps.push(ClaimAgentSnapshot {
                        id: a.id.clone(),
                        state: LogicalAgentState::parse_sql(&a.state)?,
                        assigned_to_task: a.current_task_id.is_some(),
                        partition: a.partition.clone(),
                        workstream_id: a.workstream_id.clone(),
                        tags: parse_str_list(&a.tags_json)?,
                        available_since: *available_since,
                        created_at: *created_at,
                    });
                }
                if let Some(picked) = select_claim_agent(&agent_snaps, &intent) {
                    let (agent, _, _) =
                        agents.iter().find(|(a, _, _)| a.id == picked.id).expect("picked from loaded set");
                    let partition = required_partition(tx, &task.partition, true)?;
                    return Ok(Some(claim_selected(
                        tx,
                        agent,
                        &partition,
                        task,
                        now,
                        lease_seconds,
                    )?));
                }
            }
            Ok(None)
        })
    }

    /// Validate claim authority in a short transaction and derive the durable
    /// execution binding used as the configuration-resolution key.
    ///
    /// Authority precedence: the resolution key (execution_target /
    /// execution_profile) comes from the frozen Attempt row, never from the
    /// Claim DTO's redundant copies. A stale or expired Claim fails authority
    /// validation (StaleAuthority) and a Claim whose copies disagree with the
    /// durable Attempt is rejected (InvalidAuthority) — both BEFORE any
    /// configuration resolution, so a forged claim cannot turn a fully
    /// configured Task into a RESOURCE_UNAVAILABLE preparation failure.
    pub fn resolve_execution_binding(
        &self,
        claim: &Claim,
    ) -> Result<AuthoritativeExecutionBinding, Error> {
        self.tx(|tx, now| {
            let (attempt, _lease, _task) =
                validate_authority_tx(tx, claim.attempt_id.as_str(), claim.lease_epoch.get(), now)?;
            if claim.task_id.as_str() != attempt.task_id {
                return Err(Error::invalid_authority(
                    "claim task_id does not match authoritative attempt",
                ));
            }
            if claim.logical_agent_id.as_str() != attempt.logical_agent_id {
                return Err(Error::invalid_authority(
                    "claim logical_agent_id does not match authoritative attempt",
                ));
            }
            if claim.execution_target != attempt.execution_target {
                return Err(Error::invalid_authority(
                    "claim execution_target does not match authoritative attempt",
                ));
            }
            if claim.execution_profile != attempt.execution_profile {
                return Err(Error::invalid_authority(
                    "claim execution_profile does not match authoritative attempt",
                ));
            }
            Ok(AuthoritativeExecutionBinding {
                attempt_id: claim.attempt_id.clone(),
                lease_epoch: claim.lease_epoch,
                execution_target: attempt.execution_target.clone(),
                execution_profile: attempt.execution_profile.clone(),
            })
        })
    }

    pub fn create_execution(
        &self,
        claim: &Claim,
        physical_binding: FrozenPhysicalExecutionBinding,
    ) -> Result<ExecutionLaunchSnapshot, Error> {
        let safety = physical_binding.safety();
        // Kernel invariant check (M5.3 §36): the frozen adapter routing
        // identity is required for every execution commitment — defense in
        // depth behind the typed constructor validation.
        if physical_binding.adapter_kind().trim().is_empty() {
            return Err(Error::invalid_authority(
                "execution commitment requires a non-blank adapter routing identity",
            ));
        }
        self.tx(|tx, now| {
            let (attempt, lease, task) =
                validate_authority_tx(tx, claim.attempt_id.as_str(), claim.lease_epoch.get(), now)?;
            if claim.task_id.as_str() != attempt.task_id {
                return Err(Error::invalid_authority(
                    "claim task_id does not match authoritative attempt",
                ));
            }
            if claim.logical_agent_id.as_str() != attempt.logical_agent_id {
                return Err(Error::invalid_authority(
                    "claim logical_agent_id does not match authoritative attempt",
                ));
            }
            if claim.execution_target != attempt.execution_target {
                return Err(Error::invalid_authority(
                    "claim execution_target does not match authoritative attempt",
                ));
            }
            if claim.execution_profile != attempt.execution_profile {
                return Err(Error::invalid_authority(
                    "claim execution_profile does not match authoritative attempt",
                ));
            }
            // Attempt-bound proof: a safety fact minted for a different
            // attempt (or a different lease epoch) is rejected even when the
            // target and profile names coincide, closing cross-attempt
            // replay of a stale isolated proof.
            if safety.attempt_id().as_str() != attempt.id {
                return Err(Error::invalid_authority(format!(
                    "safety proof is bound to attempt {} but the authoritative attempt is {}",
                    safety.attempt_id().as_str(),
                    attempt.id
                )));
            }
            if safety.lease_epoch() != claim.lease_epoch {
                return Err(Error::invalid_authority(
                    "safety proof is bound to a different lease epoch",
                ));
            }
            if safety.execution_target() != attempt.execution_target {
                return Err(Error::invalid_authority(format!(
                    "safety proof target '{}' does not match authoritative attempt target '{}'",
                    safety.execution_target(),
                    attempt.execution_target
                )));
            }
            if safety.execution_profile() != attempt.execution_profile {
                return Err(Error::invalid_authority(format!(
                    "safety proof profile '{}' does not match authoritative attempt profile '{}'",
                    safety.execution_profile(),
                    attempt.execution_profile
                )));
            }
            let (incarnation_id, incarnation_handle_json) = match attempt.incarnation_id {
                Some(id) => {
                    let handle: String = tx
                        .query_row(
                            "SELECT runtime_handle_json FROM incarnations WHERE id=?1",
                            params![id],
                            |r| r.get(0),
                        )
                        .map_err(map_sqlite)?;
                    (id, handle)
                }
                None => {
                    let id = ensure_incarnation(
                        tx,
                        &attempt.logical_agent_id,
                        &attempt.execution_target,
                        now,
                    )?;
                    let handle: String = tx
                        .query_row(
                            "SELECT runtime_handle_json FROM incarnations WHERE id=?1",
                            params![id],
                            |r| r.get(0),
                        )
                        .map_err(map_sqlite)?;
                    (id, handle)
                }
            };
            let busy: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM executions WHERE incarnation_id=?1
                     AND state IN ('STARTING','RUNNING','UNKNOWN') LIMIT 1",
                    params![incarnation_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            if busy.is_some() {
                return Err(Error::invalid_transition(format!(
                    "incarnation {incarnation_id} already owns an active Execution"
                )));
            }
            tx.execute(
                "UPDATE attempts SET incarnation_id=?1 WHERE id=?2 AND incarnation_id IS NULL",
                params![incarnation_id, attempt.id],
            )
            .map_err(map_sqlite)?;
            let execution_id = ExecutionId::new();
            let request_id = RequestId::new();
            let attempt_isolation = safety.attempt_isolation();
            tx.execute(
                "INSERT INTO executions(id,request_id,task_id,attempt_id,incarnation_id,execution_target,
                 execution_profile,adapter_kind,attempt_isolation,state,started_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'STARTING',?10,?10)",
                params![
                    execution_id.as_str(),
                    request_id.as_str(),
                    attempt.task_id,
                    attempt.id,
                    incarnation_id,
                    attempt.execution_target,
                    attempt.execution_profile,
                    physical_binding.adapter_kind(),
                    attempt_isolation as i64,
                    now
                ],
            )
            .map_err(map_sqlite)?;

            let payload = json_load(&task.payload_json)?;
            let acceptance = json_load(&task.acceptance_json)?;
            let workspace_mode = WorkspaceMode::parse_sql(&task.workspace_mode)?;

            let agent = required_agent(tx, &attempt.logical_agent_id)?;
            let continuity_capsule = json_load(&agent.continuity_json)?;
            let continuity_pref = ContinuityPreference::parse_sql(&task.continuity)?;
            let continuity = CommittedContinuitySnapshot::new(
                continuity_pref,
                agent.continuity_version,
                continuity_capsule,
            );
            let incarnation_runtime_handle = json_load(&incarnation_handle_json)?;

            // SAFETY: Atomically validated and reconstructed from durable storage within the
            // Kernel execution creation transaction.
            Ok(unsafe {
                ExecutionLaunchSnapshot::from_persisted_kernel_authority(
                    execution_id,
                    request_id,
                    TaskId::from_string(&attempt.task_id),
                    BatchId::from_string(&task.batch_id),
                    AttemptId::from_string(&attempt.id),
                    attempt.attempt_number as u32,
                    LeaseId::from_string(&lease.id),
                    LeaseEpoch(lease.epoch),
                    lease.expires_at,
                    LogicalAgentId::from_string(&attempt.logical_agent_id),
                    IncarnationId::from_string(&incarnation_id),
                    incarnation_runtime_handle,
                    attempt.execution_target,
                    attempt.execution_profile,
                    workspace_mode,
                    task.name,
                    payload,
                    acceptance,
                    task.workstream_id.map(WorkstreamId::from_string),
                    continuity,
                    safety.clone(),
                )
            })
        })
    }

    pub fn confirm_running_and_renew(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
        execution_id: &ExecutionId,
        runtime_handle: &Value,
    ) -> Result<UnixTime, Error> {
        let lease_seconds = self.lease_seconds;
        self.tx(|tx, now| {
            let (attempt, lease, task) =
                validate_authority_tx(tx, attempt_id.as_str(), lease_epoch.get(), now)?;
            let execution = required_execution(tx, execution_id.as_str())?;
            if execution.attempt_id != attempt.id {
                return Err(Error::stale("execution does not belong to current attempt"));
            }
            if !matches!(execution.state.as_str(), "RUNNING" | "STARTING" | "UNKNOWN") {
                return Err(Error::invalid_transition(format!(
                    "execution {} cannot enter RUNNING from {}",
                    execution.id, execution.state
                )));
            }
            let expires_at = now + lease_seconds;
            tx.execute(
                "UPDATE executions SET state='RUNNING',runtime_handle_json=?1,updated_at=?2 WHERE id=?3",
                params![json_dump(runtime_handle), now, execution.id],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE tasks SET state='RUNNING',updated_at=?1 WHERE id=?2 AND current_attempt_id=?3",
                params![now, task.id, attempt.id],
            )
            .map_err(map_sqlite)?;
            if let Some(inc) = &attempt.incarnation_id {
                tx.execute(
                    "UPDATE incarnations SET state='WARM',runtime_handle_json=?1 WHERE id=?2",
                    params![json_dump(runtime_handle), inc],
                )
                .map_err(map_sqlite)?;
            }
            let n = tx
                .execute(
                    "UPDATE leases SET heartbeat_at=?1,expires_at=?2 WHERE id=?3 AND state='ACTIVE' AND expires_at>?1",
                    params![now, expires_at, lease.id],
                )
                .map_err(map_sqlite)?;
            if n != 1 {
                return Err(Error::stale(
                    "lease expired before RUNNING supervision was established",
                ));
            }
            Ok(expires_at)
        })
    }

    pub fn record_physical_outcome(
        &self,
        execution_id: &ExecutionId,
        state: ExecutionState,
        runtime_handle: Option<&Value>,
        payload: Option<&Value>,
        failure_class: Option<FailureClass>,
        terminal_confirmed: bool,
        quiescent_confirmed: bool,
    ) -> Result<(), Error> {
        // Spec 16 §A / 14: a nonterminal authoritative observation MUST NOT
        // carry (or inherit) terminal/quiescence proof. Durable proof may only
        // be persisted together with a terminal physical state; entering an
        // unresolved state supersedes and clears any earlier stored proof.
        let unresolved = matches!(state, ExecutionState::Unknown | ExecutionState::Lost);
        if unresolved && (terminal_confirmed || quiescent_confirmed) {
            return Err(Error::invalid_transition(
                "an unresolved physical outcome cannot carry terminal or quiescence proof",
            ));
        }
        self.tx(|tx, now| {
            let execution = required_execution(tx, execution_id.as_str())?;
            let from = ExecutionState::parse_sql(&execution.state)?;
            require_physical_transition(from, state)?;
            let handle_json = runtime_handle.map(json_dump);
            let outcome_json = payload.map(json_dump);
            tx.execute(
                "UPDATE executions SET state=?1,runtime_handle_json=COALESCE(?2,runtime_handle_json),
                 outcome_json=COALESCE(?3,outcome_json),
                 failure_class=COALESCE(?4,failure_class),
                 terminal_confirmed=CASE WHEN ?7 THEN 0 ELSE MAX(terminal_confirmed,?5) END,
                 quiescent_confirmed=CASE WHEN ?7 THEN 0 ELSE MAX(quiescent_confirmed,?6) END,
                 updated_at=?8,
                 ended_at=CASE WHEN ?5 THEN ?8 ELSE ended_at END WHERE id=?9",
                params![
                    state.as_sql(),
                    handle_json,
                    outcome_json,
                    failure_class.map(|c| c.as_sql().to_string()),
                    terminal_confirmed as i64,
                    quiescent_confirmed as i64,
                    unresolved as i64,
                    now,
                    execution.id
                ],
            )
            .map_err(map_sqlite)?;
            if let Some(h) = &handle_json {
                tx.execute(
                    "UPDATE incarnations SET runtime_handle_json=?1 WHERE id=?2",
                    params![h, execution.incarnation_id],
                )
                .map_err(map_sqlite)?;
            }
            record_incarnation_presence(
                tx,
                Some(&execution.incarnation_id),
                state,
                terminal_confirmed,
                quiescent_confirmed,
                false,
                now,
            )?;
            Ok(())
        })
    }

    pub fn ack_success(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
        execution_id: Option<&ExecutionId>,
        payload: &Value,
        summary: Option<&str>,
        quiescent_confirmed: bool,
        incarnation_reusable: bool,
    ) -> Result<Option<ResultId>, Error> {
        self.tx(|tx, now| {
            let (attempt, lease, task) =
                validate_authority_tx(tx, attempt_id.as_str(), lease_epoch.get(), now)?;
            let execution = execution_for_attempt(
                tx,
                &attempt.id,
                execution_id.map(|e| e.as_str()),
            )?;
            let resolved_id = execution.as_ref().map(|e| e.id.clone());
            let write = task.workspace_mode == "write";
            if write
                && execution.is_some()
                && !writer_is_safe_to_replace(
                    true,
                    true,
                    quiescent_confirmed,
                    execution.as_ref().map(|e| e.attempt_isolation).unwrap_or(false),
                )
            {
                if let Some(eid) = &resolved_id {
                    tx.execute(
                        "UPDATE executions SET state='SUCCEEDED',outcome_json=?1,terminal_confirmed=1,
                         quiescent_confirmed=0,updated_at=?2,ended_at=?2 WHERE id=?3",
                        params![json_dump(payload), now, eid],
                    )
                    .map_err(map_sqlite)?;
                }
                record_incarnation_presence(
                    tx,
                    attempt.incarnation_id.as_deref(),
                    ExecutionState::Succeeded,
                    true,
                    false,
                    false,
                    now,
                )?;
                record_failure(
                    tx,
                    &task.id,
                    Some(&attempt.id),
                    resolved_id.as_deref(),
                    FailureClass::WriterQuiescenceUnknown,
                    Some("WRITER_SUCCESS_NOT_QUIESCENT"),
                    Some("WRITER_SUCCESS_NOT_QUIESCENT"),
                    Some("writer reported success but physical quiescence is unknown"),
                    now,
                )?;
                suspend_current(
                    tx,
                    &attempt,
                    &lease,
                    &task,
                    FailureClass::WriterQuiescenceUnknown,
                    Some("WRITER_SUCCESS_NOT_QUIESCENT"),
                    Some("writer reported success but physical quiescence is unknown"),
                    now,
                )?;
                return Ok(None);
            }
            if let Some(exec) = &execution {
                if !matches!(exec.state.as_str(), "STARTING" | "RUNNING" | "UNKNOWN") {
                    return Err(Error::invalid_transition(format!(
                        "execution {} cannot succeed from {}",
                        exec.id, exec.state
                    )));
                }
                tx.execute(
                    "UPDATE executions SET state='SUCCEEDED',outcome_json=?1,terminal_confirmed=1,
                     quiescent_confirmed=?2,updated_at=?3,ended_at=?3 WHERE id=?4",
                    params![json_dump(payload), quiescent_confirmed as i64, now, exec.id],
                )
                .map_err(map_sqlite)?;
            }
            record_incarnation_presence(
                tx,
                attempt.incarnation_id.as_deref(),
                ExecutionState::Succeeded,
                true,
                quiescent_confirmed,
                incarnation_reusable,
                now,
            )?;
            tx.execute(
                "UPDATE attempts SET state='SUCCEEDED',ended_at=?1 WHERE id=?2",
                params![now, attempt.id],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE leases SET state='RELEASED',ended_at=?1 WHERE id=?2",
                params![now, lease.id],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE tasks SET state='COMPLETED',current_attempt_id=NULL,next_eligible_at=NULL,updated_at=?1 WHERE id=?2",
                params![now, task.id],
            )
            .map_err(map_sqlite)?;
            let result_id = ResultId::new();
            tx.execute(
                "INSERT INTO results(id,task_id,batch_id,attempt_id,logical_agent_id,execution_id,
                 payload_json,summary,checkpoint_id,workspace_state_ref,state,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,NULL,'AVAILABLE',?9)",
                params![
                    result_id.as_str(),
                    task.id,
                    task.batch_id,
                    attempt.id,
                    attempt.logical_agent_id,
                    resolved_id,
                    json_dump(payload),
                    summary,
                    now
                ],
            )
            .map_err(map_sqlite)?;
            release_agent(tx, &attempt.logical_agent_id, now)?;
            release_dependencies(tx, &task.batch_id, now)?;
            recompute_batch(tx, &task.batch_id, now)?;
            Ok(Some(result_id))
        })
    }

    pub fn nack(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
        failure_class: FailureClass,
        execution_id: Option<&ExecutionId>,
        terminal_confirmed: bool,
        quiescent_confirmed: bool,
        incarnation_reusable: bool,
    ) -> Result<TaskState, Error> {
        self.tx(|tx, now| {
            let (attempt, lease, task) =
                validate_authority_tx(tx, attempt_id.as_str(), lease_epoch.get(), now)?;
            let execution = execution_for_attempt(
                tx,
                &attempt.id,
                execution_id.map(|e| e.as_str()),
            )?;
            let resolved_id = execution.as_ref().map(|e| e.id.clone());
            record_failure(
                tx,
                &task.id,
                Some(&attempt.id),
                resolved_id.as_deref(),
                failure_class,
                None,
                None,
                None,
                now,
            )?;
            if let Some(exec) = &execution {
                if !matches!(exec.state.as_str(), "STARTING" | "RUNNING" | "UNKNOWN") {
                    return Err(Error::invalid_transition(format!(
                        "execution {} cannot fail from {}",
                        exec.id, exec.state
                    )));
                }
                let next = if terminal_confirmed { "FAILED" } else { "UNKNOWN" };
                tx.execute(
                    "UPDATE executions SET state=?1,failure_class=?2,terminal_confirmed=?3,
                     quiescent_confirmed=CASE WHEN ?3 THEN ?4 ELSE 0 END,
                     updated_at=?5,ended_at=CASE WHEN ?3 THEN ?5 ELSE ended_at END WHERE id=?6",
                    params![
                        next,
                        failure_class.as_sql(),
                        terminal_confirmed as i64,
                        quiescent_confirmed as i64,
                        now,
                        exec.id
                    ],
                )
                .map_err(map_sqlite)?;
            }
            let presence = if failure_class == FailureClass::ExecutionLost {
                ExecutionState::Lost
            } else {
                ExecutionState::Failed
            };
            // One normalized truth for persistence AND policy (core::
            // durable_quiescence): only a terminal observation constitutes
            // durable quiescence proof. A caller claiming quiescence without
            // terminality is a nonterminal observation (UNKNOWN, zero proof
            // bits below) and must not unlock writer replacement — otherwise
            // a crash after the fact would dispatch a duplicate writer the
            // durable state itself records as unproven.
            let durable_quiescent = durable_quiescence(terminal_confirmed, quiescent_confirmed);
            record_incarnation_presence(
                tx,
                attempt.incarnation_id.as_deref(),
                presence,
                terminal_confirmed,
                durable_quiescent,
                incarnation_reusable,
                now,
            )?;
            let policy = task_retry_policy(&task)?;
            let retry_allowed = retry_allowed(&policy, failure_class, attempt.attempt_number as u32);
            let writer_safe = writer_is_safe_to_replace(
                task.workspace_mode == "write",
                execution.is_some(),
                durable_quiescent,
                execution.as_ref().map(|e| e.attempt_isolation).unwrap_or(false),
            );
            if retry_allowed && writer_safe {
                let delay =
                    retry_backoff_seconds(&policy, attempt.attempt_number as u32);
                tx.execute(
                    "UPDATE attempts SET state='FAILED',ended_at=?1 WHERE id=?2",
                    params![now, attempt.id],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE leases SET state='RELEASED',ended_at=?1 WHERE id=?2",
                    params![now, lease.id],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE tasks SET state='RETRY_WAIT',current_attempt_id=NULL,next_eligible_at=?1,updated_at=?2 WHERE id=?3",
                    params![now + delay, now, task.id],
                )
                .map_err(map_sqlite)?;
                release_agent(tx, &attempt.logical_agent_id, now)?;
                return Ok(TaskState::RetryWait);
            }
            let suspension = suspension_failure_class(writer_safe, failure_class);
            suspend_current(tx, &attempt, &lease, &task, suspension, None, None, now)?;
            Ok(TaskState::Suspended)
        })
    }

    pub fn cancel_task(&self, task_id: &TaskId, quiescence_confirmed: bool) -> Result<(), Error> {
        self.tx(|tx, now| {
            let task = required_task(tx, task_id.as_str())?;
            if matches!(task.state.as_str(), "COMPLETED" | "CANCELLED") {
                return Ok(());
            }
            if let Some(attempt_id) = &task.current_attempt_id {
                let attempt = required_attempt(tx, attempt_id)?;
                let execution = execution_for_attempt(tx, &attempt.id, None)?;
                let row_durable_quiescent = execution
                    .as_ref()
                    .map(|e| durable_quiescence(e.terminal_confirmed, e.quiescent_confirmed))
                    .unwrap_or(false);
                let writer_unknown = task.workspace_mode == "write"
                    && execution.is_some()
                    && !writer_is_safe_to_replace(
                        true,
                        true,
                        quiescence_confirmed || row_durable_quiescent,
                        execution.as_ref().map(|e| e.attempt_isolation).unwrap_or(false),
                    );
                let physical_quiescent =
                    execution.is_none() || quiescence_confirmed || row_durable_quiescent;
                record_incarnation_presence(
                    tx,
                    attempt.incarnation_id.as_deref(),
                    if physical_quiescent {
                        ExecutionState::Terminated
                    } else {
                        ExecutionState::Lost
                    },
                    physical_quiescent,
                    physical_quiescent,
                    false,
                    now,
                )?;
                tx.execute(
                    "UPDATE attempts SET state='CANCELLED',ended_at=?1 WHERE id=?2 AND state='ACTIVE'",
                    params![now, attempt.id],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE leases SET state='REVOKED',ended_at=?1 WHERE attempt_id=?2 AND state='ACTIVE'",
                    params![now, attempt.id],
                )
                .map_err(map_sqlite)?;
                if let Some(eid) = execution.as_ref().map(|e| e.id.as_str()) {
                    let next = if physical_quiescent {
                        "TERMINATED"
                    } else {
                        "LOST"
                    };
                    tx.execute(
                        "UPDATE executions SET state=?1,ended_at=COALESCE(ended_at,?2),updated_at=?2
                         WHERE id=?3 AND state IN ('STARTING','RUNNING','UNKNOWN')",
                        params![next, now, eid],
                    )
                    .map_err(map_sqlite)?;
                }
                if writer_unknown {
                    tx.execute(
                        "UPDATE logical_agents SET state='SUSPENDED',current_task_id=NULL,
                         available_since=NULL,updated_at=?1 WHERE id=?2 AND state<>'RETIRED'",
                        params![now, attempt.logical_agent_id],
                    )
                    .map_err(map_sqlite)?;
                    create_escalation(
                        tx,
                        &task.id,
                        &task.batch_id,
                        Some(&attempt.logical_agent_id),
                        task.workstream_id.as_deref(),
                        FailureClass::WriterQuiescenceUnknown,
                        Some("CANCELLED_WRITER_NOT_QUIESCENT"),
                        Some("scheduler authority cancelled but physical writer quiescence is unknown"),
                        now,
                    )?;
                } else {
                    release_agent(tx, &attempt.logical_agent_id, now)?;
                }
            }
            tx.execute(
                "UPDATE tasks SET state='CANCELLED',current_attempt_id=NULL,updated_at=?1 WHERE id=?2",
                params![now, task.id],
            )
            .map_err(map_sqlite)?;
            recompute_batch(tx, &task.batch_id, now)?;
            Ok(())
        })
    }

    /// Spec 03 §Batch: OPEN/ACTIVE/SUSPENDED -> cancel -> CANCELLED. Cancelling
    /// a batch closes every nonterminal Task and every active Attempt/Lease
    /// under writer-quiescence rules; an open WRITER_QUIESCENCE_UNKNOWN
    /// obligation survives (spec 03: cancellation is not quiescence proof).
    pub fn cancel_batch(&self, batch_id: &BatchId) -> Result<(), Error> {
        self.tx(|tx, now| {
            let state: String = tx
                .query_row(
                    "SELECT state FROM batches WHERE id=?1",
                    params![batch_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        Error::not_found(format!("batches {:?}", batch_id.as_str()))
                    }
                    other => map_sqlite(other),
                })?;
            if state == "COMPLETED" {
                return Err(Error::invalid_transition(
                    "a terminal COMPLETED batch cannot be cancelled",
                ));
            }
            if state == "CANCELLED" {
                return Ok(());
            }
            tx.execute(
                "UPDATE escalations SET state='CANCELLED',resolved_at=?1
                 WHERE batch_id=?2 AND state='OPEN' AND failure_class<>'WRITER_QUIESCENCE_UNKNOWN'",
                params![now, batch_id.as_str()],
            )
            .map_err(map_sqlite)?;
            let active: Vec<(String, String, Option<String>, String, String, Option<String>)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT a.id,a.logical_agent_id,a.incarnation_id,t.id,t.workspace_mode,t.workstream_id
                         FROM attempts a JOIN tasks t ON t.id=a.task_id
                         WHERE t.batch_id=?1 AND a.state='ACTIVE' ORDER BY a.id",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt.query_map(params![batch_id.as_str()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })
                .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                out
            };
            for (attempt_id, agent_id, incarnation_id, task_id, workspace_mode, workstream_id) in
                active
            {
                let execution = execution_for_attempt(tx, &attempt_id, None)?;
                let (row_quiescent, row_isolation) = execution
                    .as_ref()
                    .map(|e| {
                        (
                            durable_quiescence(e.terminal_confirmed, e.quiescent_confirmed),
                            e.attempt_isolation,
                        )
                    })
                    .unwrap_or((false, false));
                let writer_unknown =
                    workspace_mode == "write" && execution.is_some() && !(row_quiescent || row_isolation);
                let physical_quiescent = execution.is_none() || row_quiescent;
                record_incarnation_presence(
                    tx,
                    incarnation_id.as_deref(),
                    if physical_quiescent {
                        ExecutionState::Terminated
                    } else {
                        ExecutionState::Lost
                    },
                    physical_quiescent,
                    physical_quiescent,
                    false,
                    now,
                )?;
                tx.execute(
                    "UPDATE attempts SET state='CANCELLED',ended_at=?1 WHERE id=?2 AND state='ACTIVE'",
                    params![now, attempt_id],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE leases SET state='REVOKED',ended_at=?1 WHERE attempt_id=?2 AND state='ACTIVE'",
                    params![now, attempt_id],
                )
                .map_err(map_sqlite)?;
                if writer_unknown {
                    tx.execute(
                        "UPDATE logical_agents SET state='SUSPENDED',current_task_id=NULL,
                         available_since=NULL,updated_at=?1 WHERE id=?2 AND state<>'RETIRED'",
                        params![now, agent_id],
                    )
                    .map_err(map_sqlite)?;
                    create_escalation(
                        tx,
                        &task_id,
                        batch_id.as_str(),
                        Some(&agent_id),
                        workstream_id.as_deref(),
                        FailureClass::WriterQuiescenceUnknown,
                        Some("CANCELLED_WRITER_NOT_QUIESCENT"),
                        Some("batch cancelled but physical writer quiescence is unknown"),
                        now,
                    )?;
                } else {
                    release_agent(tx, &agent_id, now)?;
                }
            }
            tx.execute(
                "UPDATE tasks SET state='CANCELLED',current_attempt_id=NULL,updated_at=?1
                 WHERE batch_id=?2 AND state NOT IN ('COMPLETED','CANCELLED')",
                params![now, batch_id.as_str()],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE batches SET state='CANCELLED',updated_at=?1 WHERE id=?2",
                params![now, batch_id.as_str()],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn expire_leases(&self, recover_unstarted: bool) -> Result<ExpireReport, Error> {
        self.tx(|tx, now| {
            let mut report = ExpireReport::default();
            let mut stmt = tx
                .prepare(
                    "SELECT l.id,l.attempt_id,l.task_id,a.logical_agent_id,a.attempt_number,a.incarnation_id,
                            t.batch_id,t.workspace_mode,t.max_attempts,t.retry_classes_json,
                            t.base_backoff_seconds,t.max_backoff_seconds,t.workstream_id,
                            e.id,e.attempt_isolation,e.terminal_confirmed,e.quiescent_confirmed
                     FROM leases l JOIN attempts a ON a.id=l.attempt_id JOIN tasks t ON t.id=l.task_id
                     LEFT JOIN executions e ON e.attempt_id=a.id
                     WHERE l.state='ACTIVE' AND (l.expires_at<=?1 OR (?2=1 AND e.id IS NULL))
                     ORDER BY l.expires_at",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![now, recover_unstarted as i64], |r| {
                    Ok(ExpireRow {
                        lease_id: r.get(0)?,
                        attempt_id: r.get(1)?,
                        task_id: r.get(2)?,
                        logical_agent_id: r.get(3)?,
                        attempt_number: r.get(4)?,
                        incarnation_id: r.get(5)?,
                        batch_id: r.get(6)?,
                        workspace_mode: r.get(7)?,
                        max_attempts: r.get(8)?,
                        retry_classes_json: r.get(9)?,
                        base_backoff_seconds: r.get(10)?,
                        max_backoff_seconds: r.get(11)?,
                        workstream_id: r.get(12)?,
                        execution_id: r.get(13)?,
                        attempt_isolation: r.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0,
                        terminal_confirmed: r.get::<_, Option<i64>>(15)?.unwrap_or(0) != 0,
                        quiescent_confirmed: r.get::<_, Option<i64>>(16)?.unwrap_or(0) != 0,
                    })
                })
                .map_err(map_sqlite)?;
            let collected: Vec<ExpireRow> = rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)?;
            drop(stmt);
            let mut seen = HashSet::new();
            for row in collected {
                if !seen.insert(row.attempt_id.clone()) {
                    continue;
                }
                tx.execute(
                    "UPDATE leases SET state='EXPIRED',ended_at=?1 WHERE id=?2 AND state='ACTIVE'",
                    params![now, row.lease_id],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE attempts SET state='EXPIRED',ended_at=?1 WHERE id=?2 AND state='ACTIVE'",
                    params![now, row.attempt_id],
                )
                .map_err(map_sqlite)?;
                if let Some(inc) = &row.incarnation_id {
                    tx.execute(
                        "UPDATE incarnations SET state='LOST',ended_at=?1 WHERE id=?2
                         AND state IN ('STARTING','WARM','COLD')",
                        params![now, inc],
                    )
                    .map_err(map_sqlite)?;
                }
                let orphaned = row.execution_id.is_none();
                let failure_code = if orphaned {
                    "CLAIM_ORPHANED"
                } else {
                    "LEASE_EXPIRED"
                };
                record_failure(
                    tx,
                    &row.task_id,
                    Some(&row.attempt_id),
                    row.execution_id.as_deref(),
                    FailureClass::ExecutionLost,
                    Some(failure_code),
                    None,
                    Some(if orphaned {
                        "scheduler recovery found an active claim without an Execution"
                    } else {
                        "lease expired before authoritative completion"
                    }),
                    now,
                )?;
                let durable_quiescent =
                    durable_quiescence(row.terminal_confirmed, row.quiescent_confirmed);
                let writer_safe = writer_is_safe_to_replace(
                    row.workspace_mode == "write",
                    row.execution_id.is_some(),
                    durable_quiescent,
                    row.attempt_isolation,
                );
                // Frozen retry semantics come from core's RetryPolicy; storage
                // only decodes the durable columns into it.
                let policy = RetryPolicy {
                    max_attempts: row.max_attempts as u32,
                    retry_classes: parse_failure_classes(&row.retry_classes_json)?,
                    base_backoff_seconds: row.base_backoff_seconds,
                    max_backoff_seconds: row.max_backoff_seconds,
                };
                let retry_allowed =
                    retry_allowed(&policy, FailureClass::ExecutionLost, row.attempt_number as u32);
                if retry_allowed && writer_safe {
                    release_agent(tx, &row.logical_agent_id, now)?;
                    let delay =
                        retry_backoff_seconds(&policy, row.attempt_number as u32);
                    tx.execute(
                        "UPDATE tasks SET state='RETRY_WAIT',current_attempt_id=NULL,next_eligible_at=?1,updated_at=?2 WHERE id=?3",
                        params![now + delay, now, row.task_id],
                    )
                    .map_err(map_sqlite)?;
                    report.retried += 1;
                } else {
                    let failure_class =
                        suspension_failure_class(writer_safe, FailureClass::ExecutionLost);
                    tx.execute(
                        "UPDATE tasks SET state='SUSPENDED',current_attempt_id=NULL,updated_at=?1 WHERE id=?2",
                        params![now, row.task_id],
                    )
                    .map_err(map_sqlite)?;
                    if !writer_safe {
                        tx.execute(
                            "UPDATE logical_agents SET state='SUSPENDED',current_task_id=NULL,
                             available_since=NULL,updated_at=?1 WHERE id=?2 AND state<>'RETIRED'",
                            params![now, row.logical_agent_id],
                        )
                        .map_err(map_sqlite)?;
                    } else {
                        release_agent(tx, &row.logical_agent_id, now)?;
                    }
                    create_escalation(
                        tx,
                        &row.task_id,
                        &row.batch_id,
                        Some(&row.logical_agent_id),
                        row.workstream_id.as_deref(),
                        failure_class,
                        Some(failure_code),
                        Some(if writer_safe {
                            "retry unavailable"
                        } else {
                            "writer quiescence unknown"
                        }),
                        now,
                    )?;
                    recompute_batch(tx, &row.batch_id, now)?;
                    report.suspended += 1;
                }
            }
            Ok(report)
        })
    }

    pub fn promote_retry_wait(&self) -> Result<u32, Error> {
        self.tx(|tx, now| {
            let n = tx
                .execute(
                    "UPDATE tasks SET state='QUEUED',next_eligible_at=NULL,updated_at=?1
                     WHERE state='RETRY_WAIT' AND next_eligible_at<=?1",
                    params![now],
                )
                .map_err(map_sqlite)?;
            Ok(n as u32)
        })
    }

    pub fn ack_result(&self, result_id: &ResultId, consumer_ref: &str) -> Result<(), Error> {
        self.tx(|tx, now| {
            let state: String = tx
                .query_row(
                    "SELECT state FROM results WHERE id=?1",
                    params![result_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        Error::not_found(format!("results {:?}", result_id.as_str()))
                    }
                    other => map_sqlite(other),
                })?;
            if state == "ACKED" {
                return Ok(());
            }
            tx.execute(
                "UPDATE results SET state='ACKED',consumed_at=?1,consumer_ref=?2,disposition='consumed' WHERE id=?3",
                params![now, consumer_ref, result_id.as_str()],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn ack_outbox(&self, event_id: &OutboxEventId) -> Result<OutboxState, Error> {
        self.tx(|tx, now| {
            let state: String = tx
                .query_row(
                    "SELECT state FROM notification_outbox WHERE id=?1",
                    params![event_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        Error::not_found(format!("outbox {:?}", event_id.as_str()))
                    }
                    other => map_sqlite(other),
                })?;
            if state == "ACKED" {
                return Ok(OutboxState::Acked);
            }
            if !matches!(state.as_str(), "PENDING" | "DELIVERED") {
                return Err(Error::invalid_transition(format!(
                    "outbox {state} cannot be acknowledged"
                )));
            }
            tx.execute(
                "UPDATE notification_outbox SET state='ACKED',acknowledged_at=?1 WHERE id=?2",
                params![now, event_id.as_str()],
            )
            .map_err(map_sqlite)?;
            Ok(OutboxState::Acked)
        })
    }

    pub fn mark_outbox_delivered(&self, event_id: &OutboxEventId) -> Result<OutboxState, Error> {
        self.tx(|tx, now| {
            let n = tx
                .execute(
                    "UPDATE notification_outbox SET state='DELIVERED',delivered_at=?1,
                     delivery_attempts=delivery_attempts+1 WHERE id=?2 AND state='PENDING'",
                    params![now, event_id.as_str()],
                )
                .map_err(map_sqlite)?;
            if n == 0 {
                let state: String = tx
                    .query_row(
                        "SELECT state FROM notification_outbox WHERE id=?1",
                        params![event_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(map_sqlite)?;
                return OutboxState::parse_sql(&state);
            }
            Ok(OutboxState::Delivered)
        })
    }

    /// LEGACY M4 primitive (frozen test surface only). This renewal is
    /// fenced by attempt_id + lease_epoch alone: it takes no execution
    /// identity and is NOT wired to supervision admission. Production
    /// periodic renewal MUST use `renew_supervised_execution`, which is
    /// fenced by the positively admitted execution identity. Visibility
    /// reduction is scheduled for the M5.8 composition freeze.
    pub fn heartbeat(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
    ) -> Result<UnixTime, Error> {
        let lease_seconds = self.lease_seconds;
        self.tx(|tx, now| {
            let (_attempt, lease, _) =
                validate_authority_tx(tx, attempt_id.as_str(), lease_epoch.get(), now)?;
            let running: Option<String> = tx
                .query_row(
                    "SELECT id FROM executions WHERE attempt_id=?1 AND state='RUNNING' LIMIT 1",
                    params![attempt_id.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            if running.is_none() {
                return Err(Error::stale(
                    "attempt has no active Execution eligible for heartbeat",
                ));
            }
            let expires_at = now + lease_seconds;
            tx.execute(
                "UPDATE leases SET heartbeat_at=?1,expires_at=?2 WHERE id=?3",
                params![now, expires_at, lease.id],
            )
            .map_err(map_sqlite)?;
            Ok(expires_at)
        })
    }

    /// Fenced periodic renewal for a positively admitted Execution (M5.3).
    ///
    /// Every call revalidates current durable authority: the Attempt is
    /// ACTIVE, the Lease is ACTIVE and unexpired, the fencing epoch matches,
    /// `task.current_attempt_id` matches, and the named Execution belongs to
    /// the Attempt and is physically RUNNING. Renewal is fenced by
    /// `attempt_id` + `lease_epoch` + `execution_id` — never by TaskId or
    /// LogicalAgentId alone. An already-expired lease fails stale; a renewal
    /// can never revive expired authority.
    pub fn renew_supervised_execution(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
        execution_id: &ExecutionId,
    ) -> Result<SupervisedRenewal, Error> {
        let lease_seconds = self.lease_seconds;
        self.tx(|tx, now| {
            let (attempt, lease, _) =
                validate_authority_tx(tx, attempt_id.as_str(), lease_epoch.get(), now)?;
            let execution = required_execution(tx, execution_id.as_str())?;
            if execution.attempt_id != attempt.id {
                return Err(Error::stale("execution does not belong to current attempt"));
            }
            if execution.state != "RUNNING" {
                return Ok(SupervisedRenewal::NotRunning);
            }
            let expires_at = now + lease_seconds;
            tx.execute(
                "UPDATE leases SET heartbeat_at=?1,expires_at=?2 WHERE id=?3",
                params![now, expires_at, lease.id],
            )
            .map_err(map_sqlite)?;
            Ok(SupervisedRenewal::Renewed(expires_at))
        })
    }

    pub fn lease_supervision_view(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<LeaseSupervisionView, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT heartbeat_at,expires_at,state FROM leases WHERE attempt_id=?1",
                params![attempt_id.as_str()],
                |r| {
                    Ok((
                        r.get::<_, UnixTime>(0)?,
                        r.get::<_, UnixTime>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(map_sqlite)
            .and_then(|(heartbeat_at, expires_at, state)| {
                Ok(LeaseSupervisionView {
                    heartbeat_at,
                    expires_at,
                    state: LeaseState::parse_sql(&state)?,
                })
            })
        })
    }

    pub fn promote_checkpoint(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
        capsule: &Value,
    ) -> Result<CheckpointId, Error> {
        let max_bytes = self.continuity_max_bytes;
        self.tx(|tx, now| {
            let (attempt, _, task) =
                validate_authority_tx(tx, attempt_id.as_str(), lease_epoch.get(), now)?;
            let id = promote_checkpoint(
                tx,
                &attempt,
                &task,
                lease_epoch.get(),
                capsule,
                None,
                max_bytes,
                now,
            )?;
            Ok(CheckpointId::from_string(id))
        })
    }

    pub fn resolve_escalation(
        &self,
        escalation_id: &EscalationId,
        operation: &str,
        quiescence_confirmed: bool,
    ) -> Result<(), Error> {
        let op = EscalationOperation::parse(operation)?;
        self.tx(|tx, now| {
            let (task_id, batch_id, logical_agent_id, failure_class, state): (
                String,
                String,
                Option<String>,
                String,
                String,
            ) = tx
                .query_row(
                    "SELECT task_id,batch_id,logical_agent_id,failure_class,state FROM escalations WHERE id=?1",
                    params![escalation_id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .map_err(map_sqlite)?;
            let task = required_task(tx, &task_id)?;
            let latest = query_opt(
                tx,
                "SELECT a.incarnation_id,e.id,e.attempt_isolation FROM attempts a
                 LEFT JOIN executions e ON e.attempt_id=a.id WHERE a.task_id=?1
                 ORDER BY a.attempt_number DESC LIMIT 1",
                params![task.id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                    ))
                },
            )?;
            let (incarnation_id, execution_id, frozen_isolation) =
                latest.unwrap_or((None, None, false));

            let snap = EscalationResolutionSnapshot {
                escalation_is_open: state == "OPEN",
                failure_class: FailureClass::parse_sql(&failure_class)?,
                task_state: TaskState::parse_sql(&task.state)?,
                workspace_mode: WorkspaceMode::parse_sql(&task.workspace_mode)?,
                frozen_isolation,
                has_agent: logical_agent_id.is_some(),
            };

            let plan = plan_escalation_resolution(&snap, op, quiescence_confirmed)?;
            match plan {
                EscalationResolutionPlan::ReleaseCancelledWriter {
                    writer_presence,
                    revive_agent,
                    resolve_escalation,
                } => {
                    if writer_presence == EscalatedWriterPresenceAction::FinalizePresence {
                        finalize_escalated_writer_presence(
                            tx,
                            execution_id.as_deref(),
                            incarnation_id.as_deref(),
                            frozen_isolation,
                            quiescence_confirmed,
                            now,
                        )?;
                    }
                    if revive_agent {
                        if let Some(agent_id) = &logical_agent_id {
                            prepare_agent_revival_after_safety(tx, agent_id, now)?;
                        }
                    }
                    if resolve_escalation {
                        tx.execute(
                            "UPDATE escalations SET state='RESOLVED',resolved_at=?1 WHERE id=?2",
                            params![now, escalation_id.as_str()],
                        )
                        .map_err(map_sqlite)?;
                    }
                    recompute_batch(tx, &batch_id, now)?;
                }
                EscalationResolutionPlan::Retry {
                    next_task_state,
                    reactivate_batch,
                    writer_presence,
                    revive_agent,
                    resolve_escalation,
                } => {
                    tx.execute(
                        "UPDATE tasks SET state=?1,next_eligible_at=NULL,updated_at=?2 WHERE id=?3",
                        params![next_task_state.as_sql(), now, task.id],
                    )
                    .map_err(map_sqlite)?;
                    if reactivate_batch {
                        tx.execute(
                            "UPDATE batches SET state='ACTIVE',updated_at=?1 WHERE id=?2 AND state='SUSPENDED'",
                            params![now, batch_id],
                        )
                        .map_err(map_sqlite)?;
                    }
                    if writer_presence == EscalatedWriterPresenceAction::FinalizePresence {
                        finalize_escalated_writer_presence(
                            tx,
                            execution_id.as_deref(),
                            incarnation_id.as_deref(),
                            frozen_isolation,
                            quiescence_confirmed,
                            now,
                        )?;
                    }
                    if revive_agent {
                        if let Some(agent_id) = &logical_agent_id {
                            prepare_agent_revival_after_safety(tx, agent_id, now)?;
                        }
                    }
                    if resolve_escalation {
                        tx.execute(
                            "UPDATE escalations SET state='RESOLVED',resolved_at=?1 WHERE id=?2",
                            params![now, escalation_id.as_str()],
                        )
                        .map_err(map_sqlite)?;
                    }
                    recompute_batch(tx, &batch_id, now)?;
                }
                EscalationResolutionPlan::CancelTask {
                    next_task_state,
                    resolve_escalation,
                    recompute_batch_only,
                } => {
                    tx.execute(
                        "UPDATE tasks SET state=?1,updated_at=?2 WHERE id=?3",
                        params![next_task_state.as_sql(), now, task.id],
                    )
                    .map_err(map_sqlite)?;
                    if resolve_escalation {
                        tx.execute(
                            "UPDATE escalations SET state='RESOLVED',resolved_at=?1 WHERE id=?2",
                            params![now, escalation_id.as_str()],
                        )
                        .map_err(map_sqlite)?;
                    }
                    if recompute_batch_only || resolve_escalation {
                        recompute_batch(tx, &batch_id, now)?;
                    }
                }
            }
            Ok(())
        })
    }

    pub fn report_configuration_unavailable(
        &self,
        attempt_id: &AttemptId,
        lease_epoch: LeaseEpoch,
        detail: &str,
    ) -> Result<TaskState, Error> {
        let _ = detail;
        // Configuration unavailability says nothing about the physical state
        // of an existing writer. It MUST NOT fabricate terminal/quiescence
        // proof (spec 02: cancellation/config events are not quiescence).
        self.nack(
            attempt_id,
            lease_epoch,
            unavailable_configuration_failure(),
            None,
            false,
            false,
            false,
        )
    }

    pub fn revive_agent(
        &self,
        logical_agent_id: &LogicalAgentId,
        execution_target: &str,
    ) -> Result<(), Error> {
        self.tx(|tx, now| {
            let agent = required_agent(tx, logical_agent_id.as_str())?;
            if agent.state == "RETIRED" {
                return Err(Error::invalid_transition(
                    "a semantically retired LogicalAgent cannot revive",
                ));
            }
            let blocked: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM escalations WHERE logical_agent_id=?1 AND state='OPEN'
                     AND failure_class='WRITER_QUIESCENCE_UNKNOWN' LIMIT 1",
                    params![logical_agent_id.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            if blocked.is_some() {
                return Err(Error::invalid_transition(
                    "writer physical-safety obligation must be resolved before revival",
                ));
            }
            let active: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM attempts WHERE logical_agent_id=?1 AND state='ACTIVE' LIMIT 1",
                    params![logical_agent_id.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            if agent.current_task_id.is_some() || active.is_some() {
                return Err(Error::invalid_transition(
                    "an assigned LogicalAgent must close or fence its active Attempt before revival",
                ));
            }
            let partition = required_partition(tx, &agent.partition, true)?;
            if partition.execution_target != execution_target {
                return Err(Error::invalid_transition(
                    "revival target must match the active partition",
                ));
            }
            tx.execute(
                "UPDATE logical_agents SET state='READY',available_since=?1,updated_at=?1 WHERE id=?2",
                params![now, logical_agent_id.as_str()],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn revive_eligible_agents(&self) -> Result<u32, Error> {
        let candidates: Vec<(String, String)> = self.store.query(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT a.id,p.execution_target FROM logical_agents a
                     JOIN pool_partitions p ON p.name=a.partition_name
                     WHERE a.state='REVIVING' AND p.active=1 ORDER BY a.id",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(map_sqlite)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite)
        })?;
        let mut n = 0u32;
        for (id, target) in candidates {
            self.revive_agent(&LogicalAgentId::from_string(id), &target)?;
            n += 1;
        }
        Ok(n)
    }

    pub fn recover_authority(&self) -> Result<ExpireReport, Error> {
        let expired = self.expire_leases(true)?;
        self.promote_retry_wait()?;
        self.reconcile_pool()?;
        self.revive_eligible_agents()?;
        Ok(expired)
    }

    // ------------------------------------------------------------------ getters

    pub fn task(&self, id: &TaskId) -> Result<TaskRecord, Error> {
        self.store.query(|conn| {
            let row = required_task_conn(conn, id.as_str())?;
            Ok(TaskRecord {
                id: TaskId::from_string(&row.id),
                batch_id: BatchId::from_string(&row.batch_id),
                name: row.name,
                state: TaskState::parse_sql(&row.state)?,
                partition: PartitionId::new(&row.partition),
                workspace_mode: WorkspaceMode::parse_sql(&row.workspace_mode)?,
                fencing_epoch: LeaseEpoch(row.fencing_epoch),
                current_attempt_id: row.current_attempt_id.map(AttemptId::from_string),
                max_attempts: row.max_attempts as u32,
                next_eligible_at: row.next_eligible_at,
            })
        })
    }

    pub fn batch(&self, id: &BatchId) -> Result<BatchRecord, Error> {
        self.store.query(|conn| {
            let state: String = conn
                .query_row(
                    "SELECT state FROM batches WHERE id=?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        Error::not_found(format!("batches {:?}", id.as_str()))
                    }
                    other => map_sqlite(other),
                })?;
            Ok(BatchRecord {
                id: id.clone(),
                state: BatchState::parse_sql(&state)?,
            })
        })
    }

    pub fn result_for_task(&self, task_id: &TaskId) -> Result<ResultRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,task_id,batch_id,state,payload_json FROM results WHERE task_id=?1",
                params![task_id.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    Error::not_found(format!("results for {:?}", task_id.as_str()))
                }
                other => map_sqlite(other),
            })
            .and_then(|(id, tid, bid, state, payload)| {
                Ok(ResultRecord {
                    id: ResultId::from_string(id),
                    task_id: TaskId::from_string(tid),
                    batch_id: BatchId::from_string(bid),
                    state: ResultState::parse_sql(&state)?,
                    payload: json_load(&payload)?,
                })
            })
        })
    }

    pub fn attempt(&self, id: &AttemptId) -> Result<AttemptRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,task_id,logical_agent_id,incarnation_id,attempt_number,lease_epoch,state,
                        execution_target,execution_profile,partition_name
                 FROM attempts WHERE id=?1",
                params![id.as_str()],
                AttemptRow::from_row,
            )
            .map_err(map_sqlite)
            .and_then(|row| {
                Ok(AttemptRecord {
                    id: AttemptId::from_string(&row.id),
                    task_id: TaskId::from_string(&row.task_id),
                    logical_agent_id: LogicalAgentId::from_string(&row.logical_agent_id),
                    incarnation_id: row.incarnation_id.map(IncarnationId::from_string),
                    attempt_number: row.attempt_number as u32,
                    lease_epoch: LeaseEpoch(row.lease_epoch),
                    state: AttemptState::parse_sql(&row.state)?,
                    execution_target: row.execution_target,
                    execution_profile: row.execution_profile,
                    partition_name: PartitionId::new(row.partition_name),
                })
            })
        })
    }

    pub fn lease_for_attempt(&self, attempt_id: &AttemptId) -> Result<LeaseRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,task_id,attempt_id,epoch,state,expires_at FROM leases WHERE attempt_id=?1",
                params![attempt_id.as_str()],
                LeaseRow::from_row,
            )
            .map_err(map_sqlite)
            .and_then(|row| {
                Ok(LeaseRecord {
                    id: LeaseId::from_string(&row.id),
                    task_id: TaskId::from_string(&row.task_id),
                    attempt_id: AttemptId::from_string(&row.attempt_id),
                    epoch: LeaseEpoch(row.epoch),
                    state: LeaseState::parse_sql(&row.state)?,
                    expires_at: row.expires_at,
                })
            })
        })
    }

    pub fn logical_agent(&self, id: &LogicalAgentId) -> Result<LogicalAgentRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                        pending_partition_name,retirement_requested,continuity_json,continuity_version
                 FROM logical_agents WHERE id=?1",
                params![id.as_str()],
                AgentRow::from_query,
            )
            .map_err(map_sqlite)
            .and_then(|row| {
                Ok(LogicalAgentRecord {
                    id: LogicalAgentId::from_string(&row.id),
                    partition: PartitionId::new(&row.partition),
                    pending_partition: row.pending_partition.map(PartitionId::new),
                    retention: Retention::parse_sql(&row.retention)?,
                    state: LogicalAgentState::parse_sql(&row.state)?,
                    current_task_id: row.current_task_id.map(TaskId::from_string),
                    retirement_requested: row.retirement_requested,
                })
            })
        })
    }

    pub fn ready_agent(&self, partition: &str) -> Result<LogicalAgentId, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id FROM logical_agents WHERE partition_name=?1 AND state='READY' ORDER BY available_since,id LIMIT 1",
                params![partition],
                |r| r.get::<_, String>(0),
            )
            .map(LogicalAgentId::from_string)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Error::not_found("no READY agent"),
                other => map_sqlite(other),
            })
        })
    }

    pub fn incarnation(&self, id: &IncarnationId) -> Result<IncarnationRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,logical_agent_id,generation,execution_target,state FROM incarnations WHERE id=?1",
                params![id.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(map_sqlite)
            .and_then(|(id, agent, gen, target, state)| {
                Ok(IncarnationRecord {
                    id: IncarnationId::from_string(id),
                    logical_agent_id: LogicalAgentId::from_string(agent),
                    generation: gen as u32,
                    execution_target: target,
                    state: IncarnationState::parse_sql(&state)?,
                })
            })
        })
    }

    pub fn execution(&self, id: &ExecutionId) -> Result<ExecutionRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,task_id,attempt_id,incarnation_id,execution_target,execution_profile,
                        state,attempt_isolation,terminal_confirmed,quiescent_confirmed
                 FROM executions WHERE id=?1",
                params![id.as_str()],
                ExecutionRow::from_row,
            )
            .map_err(map_sqlite)
            .and_then(|row| {
                Ok(ExecutionRecord {
                    id: ExecutionId::from_string(&row.id),
                    task_id: TaskId::from_string(&row.task_id),
                    attempt_id: AttemptId::from_string(&row.attempt_id),
                    incarnation_id: IncarnationId::from_string(&row.incarnation_id),
                    execution_target: row.execution_target,
                    execution_profile: row.execution_profile,
                    state: ExecutionState::parse_sql(&row.state)?,
                    attempt_isolation: row.attempt_isolation,
                    terminal_confirmed: row.terminal_confirmed,
                    quiescent_confirmed: row.quiescent_confirmed,
                })
            })
        })
    }

    /// Durable runtime handle of an Execution (M5.2 physical-history
    /// verification reader).
    ///
    /// Narrow persistence-level reader for verifying that unresolved start
    /// observations keep their observed adapter handle. The full M5.4
    /// reconciliation identity reader (request_id + handle by attempt) is
    /// deliberately deferred to M5.4.
    pub fn execution_runtime_handle(&self, id: &ExecutionId) -> Result<Value, Error> {
        self.tx(|tx, _| {
            let handle: String = tx
                .query_row(
                    "SELECT runtime_handle_json FROM executions WHERE id=?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            json_load(&handle)
        })
    }

    pub fn open_escalation_for_task(&self, task_id: &TaskId) -> Result<EscalationRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT id,task_id,batch_id,logical_agent_id,failure_class,state FROM escalations
                 WHERE task_id=?1 AND state='OPEN'",
                params![task_id.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Error::not_found("open escalation"),
                other => map_sqlite(other),
            })
            .and_then(|(id, tid, bid, agent, class, state)| {
                Ok(EscalationRecord {
                    id: EscalationId::from_string(id),
                    task_id: TaskId::from_string(tid),
                    batch_id: BatchId::from_string(bid),
                    logical_agent_id: agent.map(LogicalAgentId::from_string),
                    failure_class: FailureClass::parse_sql(&class)?,
                    state: EscalationState::parse_sql(&state)?,
                })
            })
        })
    }

    pub fn outbox_for_batch(
        &self,
        batch_id: &BatchId,
        event_type: &str,
    ) -> Result<Vec<OutboxEvent>, Error> {
        self.store.query(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id,event_type,aggregate_type,aggregate_id,payload_json,state
                     FROM notification_outbox WHERE aggregate_id=?1 AND event_type=?2 ORDER BY created_at",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![batch_id.as_str(), event_type], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                let (id, et, at, aid, payload, state) = row.map_err(map_sqlite)?;
                out.push(OutboxEvent {
                    id: OutboxEventId::from_string(id),
                    event_type: et,
                    aggregate_type: at,
                    aggregate_id: aid,
                    state: OutboxState::parse_sql(&state)?,
                    payload: json_load(&payload)?,
                });
            }
            Ok(out)
        })
    }

    pub fn partition(&self, name: &str) -> Result<PartitionRecord, Error> {
        self.store.query(|conn| {
            conn.query_row(
                "SELECT name,desired_capacity,retention,execution_target,execution_profile,tags_json,active,merged_into,topology_revision
                 FROM pool_partitions WHERE name=?1",
                params![name],
                PartitionRow::from_query,
            )
            .map_err(map_sqlite)
            .and_then(|row| {
                Ok(PartitionRecord {
                    name: PartitionId::new(&row.name),
                    desired_capacity: row.desired_capacity,
                    retention: Retention::parse_sql(&row.retention)?,
                    execution_target: row.execution_target,
                    execution_profile: row.execution_profile,
                    active: row.active,
                    merged_into: row.merged_into.map(PartitionId::new),
                    topology_revision: row.topology_revision,
                })
            })
        })
    }
}

fn claim_selected(
    tx: &rusqlite::Transaction<'_>,
    agent: &AgentRow,
    partition: &PartitionRow,
    task: &TaskRow,
    now: UnixTime,
    lease_seconds: f64,
) -> Result<Claim, Error> {
    let epoch = task.fencing_epoch + 1;
    let attempt_number: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM attempts WHERE task_id=?1",
            params![task.id],
            |r| r.get(0),
        )
        .map_err(map_sqlite)?;
    let attempt_number = attempt_number + 1;
    let attempt_id = AttemptId::new();
    let lease_id = LeaseId::new();
    let expires_at = now + lease_seconds;
    tx.execute(
        "INSERT INTO attempts(id,task_id,logical_agent_id,incarnation_id,attempt_number,lease_epoch,state,
                              execution_target,execution_profile,partition_name,created_at)
         VALUES(?1,?2,?3,NULL,?4,?5,'ACTIVE',?6,?7,?8,?9)",
        params![
            attempt_id.as_str(),
            task.id,
            agent.id,
            attempt_number,
            epoch as i64,
            partition.execution_target,
            partition.execution_profile,
            partition.name,
            now
        ],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "INSERT INTO leases(id,task_id,attempt_id,epoch,state,expires_at,heartbeat_at,created_at)
         VALUES(?1,?2,?3,?4,'ACTIVE',?5,?6,?6)",
        params![
            lease_id.as_str(),
            task.id,
            attempt_id.as_str(),
            epoch as i64,
            expires_at,
            now
        ],
    )
    .map_err(map_sqlite)?;
    let n = tx
        .execute(
            "UPDATE tasks SET state='LEASED',current_attempt_id=?1,fencing_epoch=?2,updated_at=?3
             WHERE id=?4 AND state='QUEUED'",
            params![attempt_id.as_str(), epoch as i64, now, task.id],
        )
        .map_err(map_sqlite)?;
    if n != 1 {
        return Err(Error::conflict("task was not QUEUED at claim"));
    }
    tx.execute(
        "UPDATE logical_agents SET state='ASSIGNED',current_task_id=?1,
         workstream_id=COALESCE(?2,workstream_id),available_since=NULL,updated_at=?3
         WHERE id=?4 AND state='READY'",
        params![task.id, task.workstream_id, now, agent.id],
    )
    .map_err(map_sqlite)?;
    Ok(Claim {
        task_id: TaskId::from_string(&task.id),
        batch_id: BatchId::from_string(&task.batch_id),
        attempt_id,
        attempt_number: attempt_number as u32,
        lease_id,
        lease_epoch: LeaseEpoch(epoch),
        lease_expires_at: expires_at,
        logical_agent_id: LogicalAgentId::from_string(&agent.id),
        incarnation_id: None,
        execution_target: partition.execution_target.clone(),
        execution_profile: partition.execution_profile.clone(),
        workspace_mode: WorkspaceMode::parse_sql(&task.workspace_mode)?,
        payload: json_load(&task.payload_json)?,
        acceptance: json_load(&task.acceptance_json)?,
        workstream_id: task.workstream_id.as_ref().map(WorkstreamId::from_string),
    })
}

fn assert_acyclic(graph: &HashMap<String, Vec<String>>) -> Result<(), Error> {
    fn visit(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) -> Result<(), Error> {
        if visiting.contains(node) {
            return Err(Error::invalid_transition(
                "task dependency graph contains a cycle",
            ));
        }
        if visited.contains(node) {
            return Ok(());
        }
        visiting.insert(node.to_string());
        if let Some(deps) = graph.get(node) {
            for d in deps {
                visit(d, graph, visiting, visited)?;
            }
        }
        visiting.remove(node);
        visited.insert(node.to_string());
        Ok(())
    }
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for node in graph.keys() {
        visit(node, graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}

struct ExpireRow {
    lease_id: String,
    attempt_id: String,
    task_id: String,
    logical_agent_id: String,
    attempt_number: i64,
    incarnation_id: Option<String>,
    batch_id: String,
    workspace_mode: String,
    max_attempts: i64,
    retry_classes_json: String,
    base_backoff_seconds: f64,
    max_backoff_seconds: f64,
    workstream_id: Option<String>,
    execution_id: Option<String>,
    attempt_isolation: bool,
    terminal_confirmed: bool,
    quiescent_confirmed: bool,
}

fn required_task_conn(conn: &rusqlite::Connection, id: &str) -> Result<TaskRow, Error> {
    conn.query_row(
        "SELECT id,batch_id,name,payload_json,acceptance_json,partition_name,workstream_id,
                continuity,affinity_tags_json,workspace_mode,state,max_attempts,retry_classes_json,
                base_backoff_seconds,max_backoff_seconds,next_eligible_at,current_attempt_id,fencing_epoch
         FROM tasks WHERE id=?1",
        params![id],
        TaskRow::from_query,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Error::not_found(format!("tasks {id:?} not found")),
        other => map_sqlite(other),
    })
}

impl TaskRow {
    pub(crate) fn from_query(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            batch_id: r.get(1)?,
            name: r.get(2)?,
            payload_json: r.get(3)?,
            acceptance_json: r.get(4)?,
            partition: r.get(5)?,
            workstream_id: r.get(6)?,
            continuity: r.get(7)?,
            affinity_tags_json: r.get(8)?,
            workspace_mode: r.get(9)?,
            state: r.get(10)?,
            max_attempts: r.get(11)?,
            retry_classes_json: r.get(12)?,
            base_backoff_seconds: r.get(13)?,
            max_backoff_seconds: r.get(14)?,
            next_eligible_at: r.get(15)?,
            current_attempt_id: r.get(16)?,
            fencing_epoch: r.get::<_, i64>(17)? as u64,
        })
    }
}

impl AgentRow {
    pub(crate) fn from_query(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            partition: r.get(1)?,
            retention: r.get(2)?,
            state: r.get(3)?,
            workstream_id: r.get(4)?,
            tags_json: r.get(5)?,
            current_task_id: r.get(6)?,
            pending_partition: r.get(7)?,
            retirement_requested: r.get::<_, i64>(8)? != 0,
            continuity_json: r.get(9)?,
            continuity_version: r.get(10)?,
        })
    }
}

impl PartitionRow {
    pub(crate) fn from_query(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            name: r.get(0)?,
            desired_capacity: r.get(1)?,
            retention: r.get(2)?,
            execution_target: r.get(3)?,
            execution_profile: r.get(4)?,
            tags_json: r.get(5)?,
            active: r.get::<_, i64>(6)? != 0,
            merged_into: r.get(7)?,
            topology_revision: r.get(8)?,
        })
    }
}
