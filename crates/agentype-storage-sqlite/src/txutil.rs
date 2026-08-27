use crate::store::{json_dump, json_load, map_sqlite, query_opt};
use agentype_core::*;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::HashMap;

pub fn required_task(tx: &Transaction<'_>, id: &str) -> Result<TaskRow, Error> {
    query_opt(
        tx,
        "SELECT id,batch_id,name,payload_json,acceptance_json,partition_name,workstream_id,
                continuity,affinity_tags_json,workspace_mode,state,max_attempts,retry_classes_json,
                base_backoff_seconds,max_backoff_seconds,next_eligible_at,current_attempt_id,
                fencing_epoch FROM tasks WHERE id=?1",
        params![id],
        TaskRow::from_row,
    )?
    .ok_or_else(|| Error::not_found(format!("tasks {id:?} not found")))
}

pub fn required_agent(tx: &Transaction<'_>, id: &str) -> Result<AgentRow, Error> {
    query_opt(
        tx,
        "SELECT id,partition_name,retention,state,workstream_id,tags_json,current_task_id,
                pending_partition_name,retirement_requested,continuity_json,continuity_version
         FROM logical_agents WHERE id=?1",
        params![id],
        AgentRow::from_row,
    )?
    .ok_or_else(|| Error::not_found(format!("logical_agents {id:?} not found")))
}

pub fn required_partition(tx: &Transaction<'_>, name: &str, active: bool) -> Result<PartitionRow, Error> {
    let sql = if active {
        "SELECT name,desired_capacity,retention,execution_target,execution_profile,tags_json,active,merged_into,topology_revision
         FROM pool_partitions WHERE name=?1 AND active=1"
    } else {
        "SELECT name,desired_capacity,retention,execution_target,execution_profile,tags_json,active,merged_into,topology_revision
         FROM pool_partitions WHERE name=?1"
    };
    query_opt(tx, sql, params![name], PartitionRow::from_row)?
        .ok_or_else(|| Error::not_found(format!("pool_partitions {name:?} not found")))
}

pub fn required_attempt(tx: &Transaction<'_>, id: &str) -> Result<AttemptRow, Error> {
    query_opt(
        tx,
        "SELECT id,task_id,logical_agent_id,incarnation_id,attempt_number,lease_epoch,state
         FROM attempts WHERE id=?1",
        params![id],
        AttemptRow::from_row,
    )?
    .ok_or_else(|| Error::not_found(format!("attempts {id:?} not found")))
}

pub fn required_execution(tx: &Transaction<'_>, id: &str) -> Result<ExecutionRow, Error> {
    query_opt(
        tx,
        "SELECT id,task_id,attempt_id,incarnation_id,state,attempt_isolation,terminal_confirmed,quiescent_confirmed
         FROM executions WHERE id=?1",
        params![id],
        ExecutionRow::from_row,
    )?
    .ok_or_else(|| Error::not_found(format!("executions {id:?} not found")))
}

pub fn validate_authority_tx(
    tx: &Transaction<'_>,
    attempt_id: &str,
    lease_epoch: u64,
    now: UnixTime,
) -> Result<(AttemptRow, LeaseRow, TaskRow), Error> {
    let attempt = required_attempt(tx, attempt_id)?;
    let task = required_task(tx, &attempt.task_id)?;
    let lease = query_opt(
        tx,
        "SELECT id,task_id,attempt_id,epoch,state,expires_at FROM leases WHERE attempt_id=?1",
        params![attempt_id],
        LeaseRow::from_row,
    )?
    .ok_or_else(|| Error::stale("attempt has no lease"))?;
    let snap = AuthoritySnapshot {
        attempt_id: AttemptId::from_string(&attempt.id),
        attempt_state: AttemptState::parse_sql(&attempt.state)?,
        lease_state: LeaseState::parse_sql(&lease.state)?,
        lease_epoch: LeaseEpoch(attempt.lease_epoch),
        lease_expires_at: lease.expires_at,
        task_current_attempt_id: task
            .current_attempt_id
            .as_ref()
            .map(AttemptId::from_string),
        task_fencing_epoch: LeaseEpoch(task.fencing_epoch),
    };
    validate_authority(&snap, LeaseEpoch(lease_epoch), now)?;
    if lease.epoch != lease_epoch {
        return Err(Error::stale("attempt no longer owns authoritative task state"));
    }
    Ok((attempt, lease, task))
}

pub fn execution_for_attempt(
    tx: &Transaction<'_>,
    attempt_id: &str,
    execution_id: Option<&str>,
) -> Result<Option<ExecutionRow>, Error> {
    let actual = query_opt(
        tx,
        "SELECT id,task_id,attempt_id,incarnation_id,state,attempt_isolation,terminal_confirmed,quiescent_confirmed
         FROM executions WHERE attempt_id=?1",
        params![attempt_id],
        ExecutionRow::from_row,
    )?;
    match execution_id {
        None => Ok(actual),
        Some(supplied_id) => {
            let supplied = required_execution(tx, supplied_id)?;
            if supplied.attempt_id != attempt_id {
                return Err(Error::stale("execution does not belong to current attempt"));
            }
            Ok(Some(supplied))
        }
    }
}

pub fn retire_logical_agent(tx: &Transaction<'_>, agent_id: &str, now: UnixTime) -> Result<(), Error> {
    tx.execute(
        "UPDATE incarnations SET state='LOST',ended_at=COALESCE(ended_at,?1)
         WHERE logical_agent_id=?2 AND state IN ('STARTING','WARM','COLD')",
        params![now, agent_id],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "UPDATE logical_agents SET state='RETIRED',current_task_id=NULL,
         pending_partition_name=NULL,retirement_requested=0,available_since=NULL,updated_at=?1
         WHERE id=?2",
        params![now, agent_id],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

pub fn canonical_partition(tx: &Transaction<'_>, partition_name: &str) -> Result<PartitionRow, Error> {
    let mut visited = std::collections::HashSet::new();
    let mut current = partition_name.to_string();
    loop {
        if !visited.insert(current.clone()) {
            return Err(Error::invalid_transition(format!(
                "partition merge cycle while resolving {partition_name:?}"
            )));
        }
        let partition = required_partition(tx, &current, false)?;
        if partition.active {
            return Ok(partition);
        }
        match partition.merged_into {
            Some(next) => current = next,
            None => {
                return Err(Error::invalid_transition(format!(
                    "desired partition {partition_name:?} is retired"
                )))
            }
        }
    }
}

pub fn unsafe_cross_target_execution(
    tx: &Transaction<'_>,
    agent_id: &str,
    target_execution_target: &str,
    now: UnixTime,
) -> Result<bool, Error> {
    let mut stmt = tx
        .prepare(
            "SELECT e.attempt_isolation,e.terminal_confirmed,e.quiescent_confirmed,a.state AS attempt_state,
                    l.state AS lease_state,l.expires_at,t.workspace_mode
             FROM executions e
             JOIN incarnations i ON i.id=e.incarnation_id
             JOIN attempts a ON a.id=e.attempt_id
             JOIN tasks t ON t.id=e.task_id
             LEFT JOIN leases l ON l.attempt_id=a.id
             WHERE i.logical_agent_id=?1 AND e.state IN ('STARTING','RUNNING','UNKNOWN')
             AND e.execution_target<>?2",
        )
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map(params![agent_id, target_execution_target], |r| {
            Ok((
                r.get::<_, i64>(0)? != 0,
                r.get::<_, i64>(1)? != 0,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .map_err(map_sqlite)?;
    let mut snapshots = Vec::new();
    for row in rows {
        let (isolation, terminal, quiescent, attempt_state, lease_state, expires_at, workspace) =
            row.map_err(map_sqlite)?;
        let durable_quiescent = durable_quiescence(terminal, quiescent);
        snapshots.push(CrossTargetExecutionSnapshot {
            attempt_state: AttemptState::parse_sql(&attempt_state)?,
            lease_state: lease_state.as_deref().map(LeaseState::parse_sql).transpose()?,
            lease_expires_at: expires_at,
            workspace_mode: WorkspaceMode::parse_sql(&workspace)?,
            attempt_isolation: isolation,
            quiescent_confirmed: durable_quiescent,
        });
    }
    let is_safe = cross_target_cutover_safety(&snapshots, now);
    Ok(!is_safe)
}

pub fn commit_partition_cutover(
    tx: &Transaction<'_>,
    agent: &AgentRow,
    target_partition: &str,
    now: UnixTime,
) -> Result<PartitionRow, Error> {
    let target = canonical_partition(tx, target_partition)?;
    let active: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM attempts WHERE logical_agent_id=?1 AND state='ACTIVE' LIMIT 1",
            params![agent.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    let unsafe_exec =
        unsafe_cross_target_execution(tx, &agent.id, &target.execution_target, now)?;
    let agent_state = LogicalAgentState::parse_sql(&agent.state)?;
    let plan = partition_cutover_plan(
        agent_state,
        active.is_some(),
        agent.current_task_id.is_some(),
        !unsafe_exec,
    );
    match plan {
        PartitionCutoverDisposition::RejectAssignedDrainRequired => {
            return Err(Error::invalid_transition(
                "an assigned LogicalAgent must use a drain boundary",
            ));
        }
        PartitionCutoverDisposition::RejectUnsafeExecution
        | PartitionCutoverDisposition::StagePendingDestination => {
            return Err(Error::invalid_transition(
                "a topology cutover cannot abandon an unsafe physical Execution",
            ));
        }
        PartitionCutoverDisposition::Commit => {}
    }
    tx.execute(
        "UPDATE incarnations SET state='LOST',ended_at=COALESCE(ended_at,?1)
         WHERE logical_agent_id=?2 AND execution_target<>?3
         AND state IN ('STARTING','WARM','COLD')",
        params![now, agent.id, target.execution_target],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "UPDATE logical_agents SET partition_name=?1,retention=?2,
         pending_partition_name=NULL,retirement_requested=0,updated_at=?3 WHERE id=?4",
        params![target.name, target.retention, now, agent.id],
    )
    .map_err(map_sqlite)?;
    Ok(target)
}

pub fn request_partition_cutover(
    tx: &Transaction<'_>,
    agent: &AgentRow,
    target_partition: &str,
    now: UnixTime,
) -> Result<PartitionRow, Error> {
    let target = canonical_partition(tx, target_partition)?;
    let active: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM attempts WHERE logical_agent_id=?1 AND state='ACTIVE' LIMIT 1",
            params![agent.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    let unsafe_exec =
        unsafe_cross_target_execution(tx, &agent.id, &target.execution_target, now)?;
    let agent_state = LogicalAgentState::parse_sql(&agent.state)?;
    let plan = partition_cutover_plan(
        agent_state,
        active.is_some(),
        agent.current_task_id.is_some(),
        !unsafe_exec,
    );
    match plan {
        PartitionCutoverDisposition::StagePendingDestination => {
            tx.execute(
                "UPDATE logical_agents SET pending_partition_name=?1,updated_at=?2 WHERE id=?3",
                params![target.name, now, agent.id],
            )
            .map_err(map_sqlite)?;
            Ok(target)
        }
        PartitionCutoverDisposition::RejectAssignedDrainRequired => Err(Error::invalid_transition(
            "an assigned LogicalAgent must use a drain boundary",
        )),
        PartitionCutoverDisposition::RejectUnsafeExecution => Err(Error::invalid_transition(
            "a topology cutover cannot abandon an unsafe physical Execution",
        )),
        PartitionCutoverDisposition::Commit => {
            commit_partition_cutover(tx, agent, &target.name, now)
        }
    }
}

pub fn release_agent(tx: &Transaction<'_>, agent_id: &str, now: UnixTime) -> Result<(), Error> {
    let agent = required_agent(tx, agent_id)?;
    if agent.retirement_requested {
        return retire_logical_agent(tx, agent_id, now);
    }
    let target_name = agent
        .pending_partition
        .clone()
        .unwrap_or_else(|| agent.partition.clone());
    let target = commit_partition_cutover(tx, &agent, &target_name, now)?;
    let target_retention = Retention::parse_sql(&target.retention)?;
    let disposition = agent_release_disposition(false, target_retention);
    match disposition {
        AgentReleaseDisposition::Retire => retire_logical_agent(tx, agent_id, now),
        AgentReleaseDisposition::BecomeReady => {
            tx.execute(
                "UPDATE logical_agents SET state='READY',current_task_id=NULL,
                 pending_partition_name=NULL,available_since=?1,updated_at=?1 WHERE id=?2",
                params![now, agent_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        }
    }
}

pub fn prepare_agent_revival_after_safety(
    tx: &Transaction<'_>,
    agent_id: &str,
    now: UnixTime,
) -> Result<(), Error> {
    let agent = required_agent(tx, agent_id)?;
    let agent_state = LogicalAgentState::parse_sql(&agent.state)?;
    let target_name = agent
        .pending_partition
        .as_deref()
        .unwrap_or(&agent.partition);
    let target = canonical_partition(tx, target_name)?;
    let target_retention = Retention::parse_sql(&target.retention)?;
    let disposition = post_safety_agent_disposition(
        agent_state,
        agent.retirement_requested,
        target_retention,
    );
    match disposition {
        PostSafetyAgentDisposition::NoAction => Ok(()),
        PostSafetyAgentDisposition::Retire => release_agent(tx, agent_id, now),
        PostSafetyAgentDisposition::Revive => {
            release_agent(tx, agent_id, now)?;
            let released = required_agent(tx, agent_id)?;
            if released.state == "READY" {
                tx.execute(
                    "UPDATE logical_agents SET state='REVIVING',available_since=NULL,updated_at=?1
                     WHERE id=?2 AND state='READY'",
                    params![now, agent_id],
                )
                .map_err(map_sqlite)?;
            }
            Ok(())
        }
    }
}

pub fn release_dependencies(tx: &Transaction<'_>, batch_id: &str, now: UnixTime) -> Result<(), Error> {
    let mut stmt = tx
        .prepare(
            "SELECT t.id, p.state
             FROM tasks t
             LEFT JOIN task_dependencies d ON d.task_id = t.id
             LEFT JOIN tasks p ON p.id = d.depends_on_task_id
             WHERE t.batch_id = ?1 AND t.state = 'BLOCKED'
             ORDER BY t.id",
        )
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map(params![batch_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(map_sqlite)?;
    let mut task_deps: HashMap<String, Vec<TaskState>> = HashMap::new();
    for row in rows {
        let (task_id, parent_state_sql) = row.map_err(map_sqlite)?;
        let entry = task_deps.entry(task_id).or_default();
        if let Some(p_state) = parent_state_sql {
            entry.push(TaskState::parse_sql(&p_state)?);
        }
    }
    let snapshots: Vec<BlockedTaskSnapshot> = task_deps
        .into_iter()
        .map(|(task_id, parent_states)| BlockedTaskSnapshot {
            task_id,
            parent_states,
        })
        .collect();
    let to_release = plan_dependency_releases(&snapshots);
    for task_id in to_release {
        tx.execute(
            "UPDATE tasks SET state='QUEUED',updated_at=?1 WHERE id=?2 AND state='BLOCKED'",
            params![now, task_id],
        )
        .map_err(map_sqlite)?;
    }
    Ok(())
}

pub fn enqueue_event(
    tx: &Transaction<'_>,
    event_type: &str,
    aggregate_type: &str,
    aggregate_id: &str,
    payload: &Value,
    now: UnixTime,
) -> Result<String, Error> {
    let event_id = OutboxEventId::new().to_string();
    tx.execute(
        "INSERT INTO notification_outbox(id,event_type,aggregate_type,aggregate_id,payload_json,
         state,next_delivery_at,created_at) VALUES(?1,?2,?3,?4,?5,'PENDING',?6,?6)",
        params![
            event_id,
            event_type,
            aggregate_type,
            aggregate_id,
            json_dump(payload),
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(event_id)
}

pub fn recompute_batch(tx: &Transaction<'_>, batch_id: &str, now: UnixTime) -> Result<(), Error> {
    let state: String = tx
        .query_row("SELECT state FROM batches WHERE id=?1", params![batch_id], |r| {
            r.get(0)
        })
        .map_err(map_sqlite)?;
    if state == "CANCELLED" {
        return Ok(());
    }
    let (suspended, cancelled, incomplete): (i64, i64, i64) = tx
        .query_row(
            "SELECT
                SUM(CASE WHEN state='SUSPENDED' THEN 1 ELSE 0 END),
                SUM(CASE WHEN state='CANCELLED' THEN 1 ELSE 0 END),
                SUM(CASE WHEN state<>'COMPLETED' THEN 1 ELSE 0 END)
             FROM tasks WHERE batch_id=?1",
            params![batch_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(map_sqlite)?;
    // Decision lives in core (spec 15); storage only persists the outcome.
    let next = batch_next_state(suspended > 0, cancelled > 0, incomplete == 0);
    let next = next.as_sql();
    if state != next {
        tx.execute(
            "UPDATE batches SET state=?1,updated_at=?2 WHERE id=?3",
            params![next, now, batch_id],
        )
        .map_err(map_sqlite)?;
        if next == "COMPLETED" {
            enqueue_event(
                tx,
                BATCH_RESULTS_READY,
                "batch",
                batch_id,
                &serde_json::json!({"batch_id": batch_id}),
                now,
            )?;
        }
    }
    Ok(())
}

pub fn record_failure(
    tx: &Transaction<'_>,
    task_id: &str,
    attempt_id: Option<&str>,
    execution_id: Option<&str>,
    class: FailureClass,
    code: Option<&str>,
    signature: Option<&str>,
    detail: Option<&str>,
    now: UnixTime,
) -> Result<String, Error> {
    let id = FailureId::new().to_string();
    tx.execute(
        "INSERT INTO failures(id,task_id,attempt_id,execution_id,failure_class,failure_code,
         normalized_signature,detail,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            id,
            task_id,
            attempt_id,
            execution_id,
            class.as_sql(),
            code,
            signature,
            detail,
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(id)
}

pub fn create_escalation(
    tx: &Transaction<'_>,
    task_id: &str,
    batch_id: &str,
    logical_agent_id: Option<&str>,
    workstream_id: Option<&str>,
    failure_class: FailureClass,
    signature: Option<&str>,
    detail: Option<&str>,
    now: UnixTime,
) -> Result<String, Error> {
    if let Some(existing) = query_opt(
        tx,
        "SELECT id FROM escalations WHERE task_id=?1 AND state='OPEN'",
        params![task_id],
        |r| r.get::<_, String>(0),
    )? {
        return Ok(existing);
    }
    let escalation_id = EscalationId::new().to_string();
    let snapshot = serde_json::json!({
        "task_id": task_id,
        "logical_agent_id": logical_agent_id,
        "workstream_id": workstream_id,
        "failure_class": failure_class.as_sql(),
        "normalized_failure_signature": signature,
        "detail": detail,
    });
    tx.execute(
        "INSERT INTO escalations(id,task_id,batch_id,logical_agent_id,workstream_id,
         failure_class,normalized_signature,snapshot_json,decision_required,state,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'OPEN',?10)",
        params![
            escalation_id,
            task_id,
            batch_id,
            logical_agent_id,
            workstream_id,
            failure_class.as_sql(),
            signature,
            json_dump(&snapshot),
            "Root must choose an explicit recovery primitive",
            now
        ],
    )
    .map_err(map_sqlite)?;
    enqueue_event(
        tx,
        DECISION_REQUIRED,
        "escalation",
        &escalation_id,
        &serde_json::json!({
            "escalation_id": escalation_id,
            "task_id": task_id,
            "batch_id": batch_id
        }),
        now,
    )?;
    Ok(escalation_id)
}

pub fn suspend_current(
    tx: &Transaction<'_>,
    attempt: &AttemptRow,
    lease: &LeaseRow,
    task: &TaskRow,
    failure_class: FailureClass,
    signature: Option<&str>,
    detail: Option<&str>,
    now: UnixTime,
) -> Result<(), Error> {
    tx.execute(
        "UPDATE attempts SET state='FAILED',ended_at=?1 WHERE id=?2 AND state='ACTIVE'",
        params![now, attempt.id],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "UPDATE leases SET state='REVOKED',ended_at=?1 WHERE id=?2 AND state='ACTIVE'",
        params![now, lease.id],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "UPDATE tasks SET state='SUSPENDED',current_attempt_id=NULL,updated_at=?1 WHERE id=?2",
        params![now, task.id],
    )
    .map_err(map_sqlite)?;
    if failure_class == FailureClass::WriterQuiescenceUnknown {
        tx.execute(
            "UPDATE logical_agents SET state='SUSPENDED',current_task_id=NULL,
             available_since=NULL,updated_at=?1 WHERE id=?2 AND state<>'RETIRED'",
            params![now, attempt.logical_agent_id],
        )
        .map_err(map_sqlite)?;
    } else {
        release_agent(tx, &attempt.logical_agent_id, now)?;
    }
    create_escalation(
        tx,
        &task.id,
        &task.batch_id,
        Some(&attempt.logical_agent_id),
        task.workstream_id.as_deref(),
        failure_class,
        signature,
        detail,
        now,
    )?;
    recompute_batch(tx, &task.batch_id, now)?;
    Ok(())
}

pub fn record_incarnation_presence(
    tx: &Transaction<'_>,
    incarnation_id: Option<&str>,
    execution_state: ExecutionState,
    terminal_confirmed: bool,
    quiescent_confirmed: bool,
    incarnation_reusable: bool,
    now: UnixTime,
) -> Result<(), Error> {
    let Some(incarnation_id) = incarnation_id else {
        return Ok(());
    };
    // The decision lives in core (spec 15); this function only translates the
    // chosen action into the corresponding persistence statement.
    let action = incarnation_presence(
        execution_state,
        terminal_confirmed,
        quiescent_confirmed,
        incarnation_reusable,
    );
    match action {
        PresenceAction::Ignore => Ok(()),
        PresenceAction::PromoteWarm => {
            tx.execute(
                "UPDATE incarnations SET state='WARM',ended_at=NULL WHERE id=?1
                 AND state IN ('STARTING','WARM','COLD')",
                params![incarnation_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        }
        PresenceAction::FenceTerminated => {
            tx.execute(
                "UPDATE incarnations SET state='TERMINATED',ended_at=COALESCE(ended_at,?2)
                 WHERE id=?3 AND state IN ('STARTING','WARM','COLD','LOST')",
                params!["TERMINATED", now, incarnation_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        }
        PresenceAction::FenceLost => {
            tx.execute(
                "UPDATE incarnations SET state='LOST',ended_at=COALESCE(ended_at,?2)
                 WHERE id=?3 AND state IN ('STARTING','WARM','COLD')",
                params!["LOST", now, incarnation_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        }
    }
}

pub fn ensure_incarnation(
    tx: &Transaction<'_>,
    logical_agent_id: &str,
    target: &str,
    now: UnixTime,
) -> Result<String, Error> {
    let agent = required_agent(tx, logical_agent_id)?;
    if agent.state == "RETIRED" {
        return Err(Error::invalid_transition(
            "a semantically retired LogicalAgent cannot obtain a live Incarnation",
        ));
    }
    if let Some((id, exec_target)) = query_opt(
        tx,
        "SELECT id,execution_target FROM incarnations WHERE logical_agent_id=?1
         AND state IN ('STARTING','WARM','COLD') ORDER BY generation DESC LIMIT 1",
        params![logical_agent_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )? {
        if exec_target != target {
            return Err(Error::invalid_transition(format!(
                "logical agent {logical_agent_id} already has active incarnation {id} on target {exec_target}"
            )));
        }
        return Ok(id);
    }
    let generation: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(generation),0)+1 FROM incarnations WHERE logical_agent_id=?1",
            params![logical_agent_id],
            |r| r.get(0),
        )
        .map_err(map_sqlite)?;
    let incarnation_id = IncarnationId::new().to_string();
    tx.execute(
        "INSERT INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
         VALUES(?1,?2,?3,?4,'STARTING',?5)",
        params![incarnation_id, logical_agent_id, generation, target, now],
    )
    .map_err(map_sqlite)?;
    Ok(incarnation_id)
}

pub fn birth_agent(
    tx: &Transaction<'_>,
    partition_name: &str,
    workstream_id: Option<&str>,
    tags: Option<&[String]>,
    now: UnixTime,
) -> Result<String, Error> {
    let partition = required_partition(tx, partition_name, true)?;
    let agent_id = LogicalAgentId::new().to_string();
    let effective_tags = match tags {
        Some(t) => t.to_vec(),
        None => parse_str_list(&partition.tags_json)?,
    };
    tx.execute(
        "INSERT INTO logical_agents(id,partition_name,retention,state,workstream_id,tags_json,
         continuity_json,available_since,created_at,updated_at)
         VALUES(?1,?2,?3,'READY',?4,?5,'{}',?6,?6,?6)",
        params![
            agent_id,
            partition_name,
            partition.retention,
            workstream_id,
            json_dump(&Value::Array(
                effective_tags.into_iter().map(Value::String).collect()
            )),
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(agent_id)
}

pub fn promote_checkpoint(
    tx: &Transaction<'_>,
    attempt: &AttemptRow,
    task: &TaskRow,
    lease_epoch: u64,
    capsule: &Value,
    project_state_ref: Option<&str>,
    max_bytes: usize,
    now: UnixTime,
) -> Result<String, Error> {
    let obj = capsule
        .as_object()
        .ok_or_else(|| Error::invalid_transition("continuity capsule must be a JSON object"))?;
    for key in obj.keys() {
        if !CONTINUITY_KEYS.contains(&key.as_str()) {
            return Err(Error::invalid_transition(format!(
                "unknown continuity keys: {key}"
            )));
        }
    }
    let encoded = json_dump(capsule);
    if encoded.len() > max_bytes {
        return Err(Error::invalid_transition(
            "continuity capsule exceeds configured byte limit",
        ));
    }
    let agent = required_agent(tx, &attempt.logical_agent_id)?;
    let version = agent.continuity_version + 1;
    let checkpoint_id = CheckpointId::new().to_string();
    tx.execute(
        "INSERT INTO checkpoints(id,logical_agent_id,task_id,attempt_id,lease_epoch,
         continuity_version,capsule_json,project_state_ref,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            checkpoint_id,
            attempt.logical_agent_id,
            task.id,
            attempt.id,
            lease_epoch as i64,
            version,
            encoded,
            project_state_ref,
            now
        ],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "UPDATE logical_agents SET continuity_json=?1,continuity_version=?2,current_checkpoint_id=?3,
         updated_at=?4 WHERE id=?5",
        params![
            json_dump(capsule),
            version,
            checkpoint_id,
            now,
            attempt.logical_agent_id
        ],
    )
    .map_err(map_sqlite)?;
    Ok(checkpoint_id)
}

pub fn finalize_escalated_writer_presence(
    tx: &Transaction<'_>,
    execution_id: Option<&str>,
    incarnation_id: Option<&str>,
    attempt_isolation: bool,
    quiescence_confirmed: bool,
    now: UnixTime,
) -> Result<(), Error> {
    if let Some(eid) = execution_id {
        if quiescence_confirmed {
            tx.execute(
                "UPDATE executions SET state=CASE
                 WHEN state IN ('STARTING','RUNNING','UNKNOWN') THEN 'TERMINATED'
                 ELSE state END,terminal_confirmed=1,quiescent_confirmed=1,
                 updated_at=?1,ended_at=COALESCE(ended_at,?1) WHERE id=?2",
                params![now, eid],
            )
            .map_err(map_sqlite)?;
        } else if attempt_isolation {
            tx.execute(
                "UPDATE executions SET state=CASE
                 WHEN state IN ('STARTING','RUNNING','UNKNOWN') THEN 'LOST'
                 ELSE state END,updated_at=?1,ended_at=COALESCE(ended_at,?1) WHERE id=?2",
                params![now, eid],
            )
            .map_err(map_sqlite)?;
        }
    }
    if let Some(iid) = incarnation_id {
        let next = if quiescence_confirmed {
            "TERMINATED"
        } else {
            "LOST"
        };
        tx.execute(
            "UPDATE incarnations SET state=?1,ended_at=COALESCE(ended_at,?2) WHERE id=?3
             AND state IN ('STARTING','WARM','COLD','LOST')",
            params![next, now, iid],
        )
        .map_err(map_sqlite)?;
    }
    Ok(())
}

pub fn insert_revision(
    tx: &Transaction<'_>,
    operation: &str,
    payload: &Value,
    now: UnixTime,
) -> Result<i64, Error> {
    tx.execute(
        "INSERT INTO pool_topology_revisions(operation,payload_json,created_at) VALUES(?1,?2,?3)",
        params![operation, json_dump(payload), now],
    )
    .map_err(map_sqlite)?;
    Ok(tx.last_insert_rowid())
}

pub fn parse_str_list(json: &str) -> Result<Vec<String>, Error> {
    let value = json_load(json)?;
    let arr = value
        .as_array()
        .ok_or_else(|| Error::invariant("expected JSON array of strings"))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| Error::invariant("expected string elements in JSON array"))?;
        out.push(s.to_string());
    }
    Ok(out)
}

pub fn parse_failure_classes(json: &str) -> Result<Vec<FailureClass>, Error> {
    parse_str_list(json)?
        .into_iter()
        .map(|s| FailureClass::parse_sql(&s))
        .collect()
}

/// Materialize the Task's frozen retry policy (spec 15: the policy semantics —
/// class gating and exponential backoff bounds — live in core; storage only
/// decodes the durable columns into it).
pub fn task_retry_policy(task: &TaskRow) -> Result<RetryPolicy, Error> {
    Ok(RetryPolicy {
        max_attempts: task.max_attempts as u32,
        retry_classes: parse_failure_classes(&task.retry_classes_json)?,
        base_backoff_seconds: task.base_backoff_seconds,
        max_backoff_seconds: task.max_backoff_seconds,
    })
}

#[derive(Clone, Debug)]
pub struct TaskRow {
    pub id: String,
    pub batch_id: String,
    pub name: String,
    pub payload_json: String,
    pub acceptance_json: String,
    pub partition: String,
    pub workstream_id: Option<String>,
    pub continuity: String,
    pub affinity_tags_json: String,
    pub workspace_mode: String,
    pub state: String,
    pub max_attempts: i64,
    pub retry_classes_json: String,
    pub base_backoff_seconds: f64,
    pub max_backoff_seconds: f64,
    pub next_eligible_at: Option<f64>,
    pub current_attempt_id: Option<String>,
    pub fencing_epoch: u64,
}

impl TaskRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
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

#[derive(Clone, Debug)]
pub struct AgentRow {
    pub id: String,
    pub partition: String,
    pub retention: String,
    pub state: String,
    pub workstream_id: Option<String>,
    pub tags_json: String,
    pub current_task_id: Option<String>,
    pub pending_partition: Option<String>,
    pub retirement_requested: bool,
    #[allow(dead_code)]
    pub continuity_json: String,
    pub continuity_version: i64,
}

impl AgentRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
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

#[derive(Clone, Debug)]
pub struct PartitionRow {
    pub name: String,
    pub desired_capacity: i64,
    pub retention: String,
    pub execution_target: String,
    pub execution_profile: String,
    pub tags_json: String,
    pub active: bool,
    pub merged_into: Option<String>,
    pub topology_revision: i64,
}

impl PartitionRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
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

#[derive(Clone, Debug)]
pub struct AttemptRow {
    pub id: String,
    pub task_id: String,
    pub logical_agent_id: String,
    pub incarnation_id: Option<String>,
    pub attempt_number: i64,
    pub lease_epoch: u64,
    pub state: String,
}

impl AttemptRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            task_id: r.get(1)?,
            logical_agent_id: r.get(2)?,
            incarnation_id: r.get(3)?,
            attempt_number: r.get(4)?,
            lease_epoch: r.get::<_, i64>(5)? as u64,
            state: r.get(6)?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct LeaseRow {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub epoch: u64,
    pub state: String,
    pub expires_at: f64,
}

impl LeaseRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            task_id: r.get(1)?,
            attempt_id: r.get(2)?,
            epoch: r.get::<_, i64>(3)? as u64,
            state: r.get(4)?,
            expires_at: r.get(5)?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionRow {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub incarnation_id: String,
    pub state: String,
    pub attempt_isolation: bool,
    pub terminal_confirmed: bool,
    pub quiescent_confirmed: bool,
}

impl ExecutionRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            task_id: r.get(1)?,
            attempt_id: r.get(2)?,
            incarnation_id: r.get(3)?,
            state: r.get(4)?,
            attempt_isolation: r.get::<_, i64>(5)? != 0,
            terminal_confirmed: r.get::<_, i64>(6)? != 0,
            quiescent_confirmed: r.get::<_, i64>(7)? != 0,
        })
    }
}
