//! M6-B.5 B5.2 durable security-acquisition diagnostics (schema v10).
//!
//! One row per Task whose **complete** `SECURITY_V1` candidate round cannot fit
//! the fixed acquisition budget. This is a liveness diagnostic, not Task
//! semantical authority: it creates no Attempt, emits no Task NACK and never
//! selects among a compiled subset. A `BLOCKED` row is cleared only by an
//! explicit operator action or a configuration/profile change that makes the
//! complete round fit (ADR-0014 §4).
//!
//! One condition is one diagnostic. Re-recording the same
//! `(candidate domain, binding/profile)` pair is a no-op — including when the
//! row is already `RELEASED`, so a released condition cannot resurrect itself.
//! A *different* condition replaces the diagnostic fields and starts `BLOCKED`
//! again; that is a new diagnostic, not a retry of the released one.

use crate::store::{map_sqlite, query_opt};
use agentype_agent_contract::{
    AcquisitionBudget, AcquisitionCost, BudgetLimit, SecurityAcquisitionBlock,
    SecurityAcquisitionBlockStatus, SecurityAcquisitionRelease,
};
use agentype_core::{Error, GenerationId, TaskId, UnixTime};
use rusqlite::{params, Transaction};

const SELECT_BLOCK: &str = "
    SELECT task_id, generation_id, candidate_domain_digest, candidate_count,
           binding_profile_digest, cost_candidates, cost_compiles,
           budget_max_candidates, budget_max_compiles, budget_limit, status,
           created_at, released_at, release_reason
    FROM security_acquisition_blocks
    WHERE task_id=?1";

/// Record one durable diagnostic.
///
/// Insert-if-absent, no-op for the same condition, replace for a different one.
/// The block's generation must be the Task's durable dispatch generation, so a
/// diagnostic cannot name a Generation the Task is not bound to.
pub fn record_security_acquisition_block(
    tx: &Transaction<'_>,
    block: &SecurityAcquisitionBlock,
) -> Result<(), Error> {
    if !block.is_blocked() {
        return Err(Error::invalid_authority(
            "a released security acquisition block is not a current condition",
        ));
    }
    let generation = task_generation(tx, block.task_id())?;
    if &generation != block.generation_id() {
        return Err(Error::invalid_authority(
            "a security acquisition block names a different Task generation",
        ));
    }

    let existing = query_opt(
        tx,
        "SELECT candidate_domain_digest, binding_profile_digest
         FROM security_acquisition_blocks WHERE task_id=?1",
        params![block.task_id().as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;

    match existing {
        None => {
            tx.execute(
                "INSERT INTO security_acquisition_blocks
                 (task_id, generation_id, candidate_domain_digest, candidate_count,
                  binding_profile_digest, cost_candidates, cost_compiles,
                  budget_max_candidates, budget_max_compiles, budget_limit,
                  status, created_at, released_at, release_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'BLOCKED', ?11, NULL, NULL)",
                params![
                    block.task_id().as_str(),
                    block.generation_id().as_str(),
                    block.candidate_domain_digest(),
                    count_to_i64(block.candidate_count())?,
                    block.binding_profile_digest(),
                    count_to_i64(block.cost().candidates())?,
                    count_to_i64(block.cost().compiles())?,
                    count_to_i64(block.budget().max_candidates())?,
                    count_to_i64(block.budget().max_compiles())?,
                    block.limit().as_str(),
                    block.created_at(),
                ],
            )
            .map_err(map_sqlite)?;
        }
        Some((domain, binding))
            if domain == block.candidate_domain_digest()
                && binding == block.binding_profile_digest() =>
        {
            // One indistinguishable condition is one diagnostic, released or not.
        }
        Some(_) => {
            tx.execute(
                "UPDATE security_acquisition_blocks
                 SET generation_id=?2, candidate_domain_digest=?3, candidate_count=?4,
                     binding_profile_digest=?5, cost_candidates=?6, cost_compiles=?7,
                     budget_max_candidates=?8, budget_max_compiles=?9, budget_limit=?10,
                     status='BLOCKED', created_at=?11, released_at=NULL, release_reason=NULL
                 WHERE task_id=?1",
                params![
                    block.task_id().as_str(),
                    block.generation_id().as_str(),
                    block.candidate_domain_digest(),
                    count_to_i64(block.candidate_count())?,
                    block.binding_profile_digest(),
                    count_to_i64(block.cost().candidates())?,
                    count_to_i64(block.cost().compiles())?,
                    count_to_i64(block.budget().max_candidates())?,
                    count_to_i64(block.budget().max_compiles())?,
                    block.limit().as_str(),
                    block.created_at(),
                ],
            )
            .map_err(map_sqlite)?;
        }
    }
    Ok(())
}

/// The durable diagnostic of a Task, if one exists.
pub fn get_security_acquisition_block(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<Option<SecurityAcquisitionBlock>, Error> {
    let row = query_opt(tx, SELECT_BLOCK, params![task_id.as_str()], |row| {
        Ok(RawBlock {
            task_id: row.get(0)?,
            generation_id: row.get(1)?,
            candidate_domain_digest: row.get(2)?,
            candidate_count: row.get(3)?,
            binding_profile_digest: row.get(4)?,
            cost_candidates: row.get(5)?,
            cost_compiles: row.get(6)?,
            budget_max_candidates: row.get(7)?,
            budget_max_compiles: row.get(8)?,
            budget_limit: row.get(9)?,
            status: row.get(10)?,
            created_at: row.get(11)?,
            released_at: row.get(12)?,
            release_reason: row.get(13)?,
        })
    })?;
    row.map(RawBlock::into_block).transpose()
}

/// Release a Task's diagnostic.
///
/// Returns whether this call changed the row. An absent row and an
/// already-released row are both `Ok(false)`: release is idempotent.
pub fn release_security_acquisition_block(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    release: SecurityAcquisitionRelease,
    now: UnixTime,
) -> Result<bool, Error> {
    let Some(mut block) = get_security_acquisition_block(tx, task_id)? else {
        return Ok(false);
    };
    if !block.is_blocked() {
        return Ok(false);
    }
    block
        .release(release, now)
        .map_err(|error| Error::invalid_authority(error.to_string()))?;
    let released_at = block
        .released_at()
        .ok_or_else(|| Error::invariant("a released block is missing its timestamp"))?;
    let reason = block
        .release_reason()
        .ok_or_else(|| Error::invariant("a released block is missing its reason"))?;
    let changed = tx
        .execute(
            "UPDATE security_acquisition_blocks
             SET status='RELEASED', released_at=?2, release_reason=?3
             WHERE task_id=?1 AND status='BLOCKED'",
            params![task_id.as_str(), released_at, reason.as_str()],
        )
        .map_err(map_sqlite)?;
    Ok(changed == 1)
}

fn task_generation(tx: &Transaction<'_>, task_id: &TaskId) -> Result<GenerationId, Error> {
    query_opt(
        tx,
        "SELECT generation_id FROM generation_task_bindings WHERE task_id=?1",
        params![task_id.as_str()],
        |row| row.get::<_, String>(0),
    )?
    .map(GenerationId::from_string)
    .ok_or_else(|| {
        Error::invariant("a security acquisition block requires a GenerationTaskBinding")
    })
}

fn count_to_i64(value: usize) -> Result<i64, Error> {
    i64::try_from(value)
        .map_err(|_| Error::invariant("a security acquisition count exceeds the durable range"))
}

fn count_from_i64(value: i64, what: &str) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| {
        Error::invariant(format!(
            "a durable security acquisition block has a negative {what}"
        ))
    })
}

struct RawBlock {
    task_id: String,
    generation_id: String,
    candidate_domain_digest: String,
    candidate_count: i64,
    binding_profile_digest: String,
    cost_candidates: i64,
    cost_compiles: i64,
    budget_max_candidates: i64,
    budget_max_compiles: i64,
    budget_limit: String,
    status: String,
    created_at: f64,
    released_at: Option<f64>,
    release_reason: Option<String>,
}

impl RawBlock {
    fn into_block(self) -> Result<SecurityAcquisitionBlock, Error> {
        let limit = BudgetLimit::parse(&self.budget_limit).ok_or_else(|| {
            Error::invariant(format!(
                "a durable security acquisition block has an unknown budget limit {:?}",
                self.budget_limit
            ))
        })?;
        let status = SecurityAcquisitionBlockStatus::parse(&self.status).ok_or_else(|| {
            Error::invariant(format!(
                "a durable security acquisition block has an unknown status {:?}",
                self.status
            ))
        })?;
        let release_reason = match self.release_reason {
            None => None,
            Some(reason) => Some(SecurityAcquisitionRelease::parse(&reason).ok_or_else(|| {
                Error::invariant(format!(
                    "a durable security acquisition block has an unknown release reason {reason:?}"
                ))
            })?),
        };
        let budget = AcquisitionBudget::new(
            count_from_i64(self.budget_max_candidates, "budget candidate ceiling")?,
            count_from_i64(self.budget_max_compiles, "budget compile ceiling")?,
        )
        .map_err(|error| Error::invariant(error.to_string()))?;
        SecurityAcquisitionBlock::from_durable_parts(
            TaskId::from_string(self.task_id),
            GenerationId::from_string(self.generation_id),
            self.candidate_domain_digest,
            count_from_i64(self.candidate_count, "candidate count")?,
            self.binding_profile_digest,
            AcquisitionCost::new(
                count_from_i64(self.cost_candidates, "candidate cost")?,
                count_from_i64(self.cost_compiles, "compile cost")?,
            ),
            budget,
            limit,
            status,
            self.created_at,
            self.released_at,
            release_reason,
        )
        .map_err(|error| Error::invariant(error.to_string()))
    }
}
