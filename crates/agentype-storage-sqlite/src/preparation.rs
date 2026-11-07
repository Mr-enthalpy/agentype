//! Durable pre-Execution control-plane facts. These are neither worker failures
//! nor physical outcome observations; recovery must not invent either one.

use crate::store::map_sqlite;
#[cfg(any(test, feature = "runtime-internal"))]
use crate::txutil::validate_preparation_fact_tx;
use agentype_core::{AttemptId, Error, EscalationCause, FailureClass, PreparationFaultKind};
#[cfg(any(test, feature = "runtime-internal"))]
use agentype_core::{Claim, UnixTime};
use rusqlite::Transaction;
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) const PREPARATION_ESCALATION: &str = "PREPARATION_CONTROL_PLANE";

pub(crate) fn get_fault(
    conn: &Connection,
    attempt_id: &AttemptId,
) -> Result<Option<PreparationFaultKind>, Error> {
    conn.query_row(
        "SELECT kind FROM preparation_faults WHERE attempt_id=?1",
        params![attempt_id.as_str()],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(map_sqlite)?
    .map(|kind| PreparationFaultKind::parse_sql(&kind))
    .transpose()
}

#[cfg(any(test, feature = "runtime-internal"))]
pub(crate) fn record_fault(
    tx: &Transaction<'_>,
    now: UnixTime,
    claim: &Claim,
    kind: PreparationFaultKind,
) -> Result<(), Error> {
    let (attempt, lease, task) =
        validate_preparation_fact_tx(tx, claim.attempt_id.as_str(), claim.lease_epoch.get())?;
    if attempt.task_id != claim.task_id.as_str()
        || attempt.logical_agent_id != claim.logical_agent_id.as_str()
        || attempt.execution_target != claim.execution_target
        || attempt.execution_profile != claim.execution_profile
        || lease.id != claim.lease_id.as_str()
    {
        return Err(Error::invalid_authority(
            "preparation fault claim identity mismatch",
        ));
    }
    if crate::requirement::task_agent_requirement_mode(tx, &claim.task_id)? != "TYPED"
        || attempt.incarnation_id.is_some()
    {
        return Err(Error::invalid_transition(
            "preparation faults require an unstarted typed claim",
        ));
    }
    if task.state != "LEASED" {
        return Err(Error::invalid_transition(
            "preparation fault requires a leased Task",
        ));
    }
    // First confirmed fact is immutable. A repeated control-plane observation
    // cannot replace its category or remove the recovery fence.
    if get_fault(tx, &claim.attempt_id)?.is_none() {
        tx.execute(
            "INSERT INTO preparation_faults(attempt_id,kind,created_at) VALUES(?1,?2,?3)",
            params![claim.attempt_id.as_str(), kind.as_sql(), now],
        )
        .map_err(map_sqlite)?;
    }
    Ok(())
}

fn validated_fault(
    conn: &Connection,
    attempt_id: &AttemptId,
    task_id: &str,
) -> Result<PreparationFaultKind, Error> {
    let (owner, incarnation, execution): (String, Option<String>, Option<String>) = conn.query_row(
        "SELECT a.task_id,a.incarnation_id,e.id FROM attempts a LEFT JOIN executions e ON e.attempt_id=a.id WHERE a.id=?1",
        [attempt_id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(map_sqlite)?;
    if owner != task_id || incarnation.is_some() || execution.is_some() {
        return Err(Error::invariant(
            "preparation decision has incompatible claim/physical authority",
        ));
    }
    get_fault(conn, attempt_id)?
        .ok_or_else(|| Error::invariant("preparation decision lacks its immutable fault"))
}

pub(crate) fn escalation_cause(
    conn: &Connection,
    escalation_id: &str,
    task_id: &str,
    tag: &str,
) -> Result<EscalationCause, Error> {
    let child: Option<String> = conn
        .query_row(
            "SELECT attempt_id FROM preparation_escalations WHERE escalation_id=?1",
            [escalation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    if tag == PREPARATION_ESCALATION {
        let attempt_id = AttemptId::from_string(
            child.ok_or_else(|| Error::invariant("preparation escalation child missing"))?,
        );
        let kind = validated_fault(conn, &attempt_id, task_id)?;
        Ok(EscalationCause::PreparationFault { attempt_id, kind })
    } else {
        if child.is_some() {
            return Err(Error::invariant(
                "Task failure escalation has a preparation child",
            ));
        }
        Ok(EscalationCause::TaskFailure(FailureClass::parse_sql(tag)?))
    }
}

pub(crate) fn attach_escalation(
    tx: &Transaction<'_>,
    escalation_id: &str,
    task_id: &str,
    attempt_id: &AttemptId,
    kind: PreparationFaultKind,
) -> Result<(), Error> {
    if validated_fault(tx, attempt_id, task_id)? != kind {
        return Err(Error::invariant(
            "preparation escalation category differs from immutable fact",
        ));
    }
    tx.execute(
        "INSERT INTO preparation_escalations(escalation_id,attempt_id) VALUES(?1,?2)",
        params![escalation_id, attempt_id.as_str()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// Prior PR-era v8 stores may already contain a suspended faulted Task without
/// a Root decision. Repair only the latest closed, unstarted faulted attempt;
/// never reopen resolved/cancelled decisions or resurrect a historical claim.
#[cfg(any(test, feature = "runtime-internal"))]
pub(crate) fn restore_missing_escalations(
    tx: &Transaction<'_>,
    now: UnixTime,
) -> Result<(), Error> {
    let mut statement = tx.prepare("SELECT a.id,t.id,t.batch_id,a.logical_agent_id,t.workstream_id
        FROM preparation_faults f JOIN attempts a ON a.id=f.attempt_id JOIN tasks t ON t.id=a.task_id
        WHERE t.state='SUSPENDED' AND t.current_attempt_id IS NULL AND a.state<>'ACTIVE'
          AND a.id=(SELECT id FROM attempts WHERE task_id=t.id ORDER BY attempt_number DESC LIMIT 1)
          AND NOT EXISTS(SELECT 1 FROM preparation_escalations p WHERE p.attempt_id=a.id)").map_err(map_sqlite)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(map_sqlite)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_sqlite)?;
    drop(statement);
    for (attempt, task, batch, agent, workstream) in rows {
        let attempt_id = AttemptId::from_string(attempt);
        let kind = validated_fault(tx, &attempt_id, &task)?;
        crate::txutil::create_escalation_with_cause(
            tx,
            &task,
            &batch,
            Some(&agent),
            workstream.as_deref(),
            &EscalationCause::PreparationFault { attempt_id, kind },
            None,
            None,
            now,
        )?;
        crate::txutil::recompute_batch(tx, &batch, now)?;
    }
    Ok(())
}
