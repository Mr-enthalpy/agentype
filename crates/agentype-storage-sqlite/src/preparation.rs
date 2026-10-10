//! Durable pre-Execution control-plane facts. These are neither worker failures
//! nor physical outcome observations; recovery must not invent either one.

use crate::store::map_sqlite;
#[cfg(any(test, feature = "runtime-internal"))]
use crate::txutil::validate_preparation_fact_tx;
use agentype_core::{AttemptId, Error};
#[cfg(any(test, feature = "runtime-internal"))]
use agentype_core::{Claim, UnixTime};
#[cfg(any(test, feature = "runtime-internal"))]
use rusqlite::Transaction;
use rusqlite::{params, Connection, OptionalExtension};

/// Closed, payload-free categories for a confirmed fatal preparation fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationFaultKind {
    StorageFailure,
    InvariantViolation,
    RecoveryRequired,
    MissingRevision,
    InvalidContract,
    Other,
}

impl PreparationFaultKind {
    #[cfg(any(test, feature = "runtime-internal"))]
    fn as_sql(self) -> &'static str {
        match self {
            Self::StorageFailure => "STORAGE_FAILURE",
            Self::InvariantViolation => "INVARIANT_VIOLATION",
            Self::RecoveryRequired => "RECOVERY_REQUIRED",
            Self::MissingRevision => "MISSING_REVISION",
            Self::InvalidContract => "INVALID_CONTRACT",
            Self::Other => "OTHER",
        }
    }

    fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "STORAGE_FAILURE" => Ok(Self::StorageFailure),
            "INVARIANT_VIOLATION" => Ok(Self::InvariantViolation),
            "RECOVERY_REQUIRED" => Ok(Self::RecoveryRequired),
            "MISSING_REVISION" => Ok(Self::MissingRevision),
            "INVALID_CONTRACT" => Ok(Self::InvalidContract),
            "OTHER" => Ok(Self::Other),
            _ => Err(Error::invariant(
                "invalid durable preparation fault category",
            )),
        }
    }
}

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
    .map(|kind| PreparationFaultKind::parse(&kind))
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
