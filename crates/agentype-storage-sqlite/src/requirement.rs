//! M6-B.3 persistence for typed agent requirements, LogicalAgent type bindings,
//! and immutable Generation policies (schema v7).
//!
//! All three are immutable. A read re-canonicalizes the stored document against
//! the catalog and requires byte equality, so a self-consistent but non-canonical
//! row fails closed. A `TaskAgentRequirement` is created only inside the typed
//! admission transaction that also creates its Task and GenerationTaskBinding.
//!
//! This module performs no external I/O and never mints enforcement evidence or
//! selects a `SpawnSource`; the pure matching engine only orders already-bound
//! LogicalAgents.

use crate::catalog::{get_agent_type, load_capability_catalog};
use crate::store::{map_sqlite, query_opt};
use agentype_agent_contract::{
    canonical_generation_policy_bytes, canonical_task_agent_requirement_bytes,
    canonicalize_generation_policy, canonicalize_task_agent_requirement, content_digest,
    fold_generation_policy, generation_policy_content_digest,
    generation_policy_from_canonical_json, match_existing_agents, resolve_selector,
    task_agent_requirement_content_digest, task_agent_requirement_from_canonical_json,
    AgentRequirementDraft, AgentTypeRef, ContinuityMode, ExistingAgentCandidate, GenerationPolicy,
    LifecycleMode, TaskAgentRequirement,
};
use agentype_core::{
    ContinuityPreference, Error, GenerationId, InformationFunction, LogicalAgentId, TaskId,
    TaskSpec, UnixTime,
};
use rusqlite::{params, Transaction};
use std::collections::BTreeSet;

fn contract_fault(error: agentype_agent_contract::ContractError) -> Error {
    Error::invariant(error.to_string())
}

fn canonical_utf8(bytes: Vec<u8>, what: &str) -> Result<String, Error> {
    String::from_utf8(bytes)
        .map_err(|_| Error::invariant(format!("{what} canonical bytes are not UTF-8")))
}

// =============================================================================
// TaskAgentRequirement
// =============================================================================

pub fn insert_task_agent_requirement(
    tx: &Transaction<'_>,
    now: UnixTime,
    task_id: &TaskId,
    requirement: &TaskAgentRequirement,
) -> Result<String, Error> {
    let catalog = load_capability_catalog(tx)?;
    let mut canonical = requirement.clone();
    canonicalize_task_agent_requirement(&mut canonical, &catalog).map_err(contract_fault)?;

    let exists: bool = query_opt(
        tx,
        "SELECT 1 FROM task_agent_requirements WHERE task_id=?1",
        params![task_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?
    .is_some();
    if exists {
        return Err(Error::conflict(format!(
            "task {} already has an agent requirement",
            task_id.as_str()
        )));
    }

    let digest = task_agent_requirement_content_digest(&canonical);
    let content_json = canonical_utf8(
        canonical_task_agent_requirement_bytes(&canonical),
        "task agent requirement",
    )?;
    let (type_id, type_revision) = match &canonical.required_type {
        Some(reference) => (
            Some(reference.id().as_str().to_string()),
            Some(reference.revision() as i64),
        ),
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO task_agent_requirements(
             task_id, required_type_id, required_type_revision,
             requirement_json, requirement_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            task_id.as_str(),
            type_id,
            type_revision,
            content_json,
            digest,
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

pub fn get_task_agent_requirement(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<Option<TaskAgentRequirement>, Error> {
    let row = query_opt(
        tx,
        "SELECT requirement_json, requirement_digest, required_type_id, required_type_revision
         FROM task_agent_requirements WHERE task_id=?1",
        params![task_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    )?;
    let Some((json, digest, type_id, type_revision)) = row else {
        return Ok(None);
    };
    let recomputed = content_digest(json.as_bytes());
    if recomputed != digest {
        return Err(Error::invariant(format!(
            "task agent requirement digest {digest} does not match recomputed {recomputed}"
        )));
    }
    let mut requirement =
        task_agent_requirement_from_canonical_json(&json).map_err(contract_fault)?;
    let mirror = match (&requirement.required_type, type_id, type_revision) {
        (Some(reference), Some(id), Some(revision)) => {
            reference.id().as_str() == id && reference.revision() as i64 == revision
        }
        (None, None, None) => true,
        _ => false,
    };
    if !mirror {
        return Err(Error::invariant(
            "task agent requirement required_type mirror does not match its canonical document",
        ));
    }
    let catalog = load_capability_catalog(tx)?;
    canonicalize_task_agent_requirement(&mut requirement, &catalog).map_err(contract_fault)?;
    let reencoded = canonical_utf8(
        canonical_task_agent_requirement_bytes(&requirement),
        "task agent requirement",
    )?;
    if reencoded != json {
        return Err(Error::invariant(
            "task agent requirement is not the canonical encoding; refusing to read",
        ));
    }
    Ok(Some(requirement))
}

/// Resolve a pre-commit `AgentRequirementDraft` into the canonical durable
/// requirement for one admitted Task, folding its Generation's policy.
///
/// The exact `required_type` is resolved through the validated catalog lookup.
/// The dimensions a `TaskSpec` owns are derived from it (single authority), and
/// a `Required` continuity preference becomes a hard `Logical` requirement. The
/// result is canonicalized but NOT persisted; the typed admission transaction
/// inserts it atomically with its Task and binding.
pub fn build_task_agent_requirement(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    draft: AgentRequirementDraft,
    information_function: InformationFunction,
    task_spec: &TaskSpec,
) -> Result<TaskAgentRequirement, Error> {
    let required_type = match &draft.required_type {
        None => None,
        Some(selector) => {
            let lookup = crate::catalog::load_agent_type_lookup(tx)?;
            let resolved = resolve_selector(selector, &lookup).map_err(contract_fault)?;
            get_agent_type(tx, &resolved)?.ok_or_else(|| {
                Error::not_found(format!(
                    "agent type {}@{} does not exist",
                    resolved.id().as_str(),
                    resolved.revision()
                ))
            })?;
            Some(resolved)
        }
    };

    let required_affinity: BTreeSet<String> = task_spec.affinity_tags.iter().cloned().collect();
    // A `Required` continuity preference becomes a hard `Logical` requirement.
    // `Preferred` is a soft dimension that M6-B.3 does not rank on (it is not
    // part of the frozen matching order), so it stays out of the requirement.
    let required_continuity = match task_spec.continuity {
        ContinuityPreference::Required => ContinuityMode::Logical,
        ContinuityPreference::Preferred | ContinuityPreference::None => ContinuityMode::None,
    };

    let mut requirement = draft.into_requirement(
        required_type,
        information_function,
        required_affinity,
        task_spec.workspace_mode,
        required_continuity,
    );

    let catalog = load_capability_catalog(tx)?;
    requirement.normalize(&catalog).map_err(contract_fault)?;
    if let Some(policy) = get_generation_policy(tx, generation_id)? {
        requirement.hard =
            fold_generation_policy(&policy, &requirement.hard).map_err(contract_fault)?;
    }
    requirement.normalize(&catalog).map_err(contract_fault)?;
    Ok(requirement)
}

// =============================================================================
// GenerationPolicy
// =============================================================================
pub fn insert_generation_policy(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &agentype_core::GenerationId,
    policy: &GenerationPolicy,
) -> Result<String, Error> {
    let catalog = load_capability_catalog(tx)?;
    let mut canonical = policy.clone();
    canonicalize_generation_policy(&mut canonical, &catalog).map_err(contract_fault)?;

    let exists: bool = query_opt(
        tx,
        "SELECT 1 FROM generation_policies WHERE generation_id=?1",
        params![generation_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?
    .is_some();
    if exists {
        return Err(Error::conflict(format!(
            "generation {} already has a policy",
            generation_id.as_str()
        )));
    }
    let digest = generation_policy_content_digest(&canonical);
    let content_json = canonical_utf8(
        canonical_generation_policy_bytes(&canonical),
        "generation policy",
    )?;
    tx.execute(
        "INSERT INTO generation_policies(generation_id, policy_json, policy_digest, created_at)
         VALUES(?1,?2,?3,?4)",
        params![generation_id.as_str(), content_json, digest, now],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

pub fn get_generation_policy(
    tx: &Transaction<'_>,
    generation_id: &agentype_core::GenerationId,
) -> Result<Option<GenerationPolicy>, Error> {
    let row = query_opt(
        tx,
        "SELECT policy_json, policy_digest FROM generation_policies WHERE generation_id=?1",
        params![generation_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let Some((json, digest)) = row else {
        return Ok(None);
    };
    let recomputed = content_digest(json.as_bytes());
    if recomputed != digest {
        return Err(Error::invariant(format!(
            "generation policy digest {digest} does not match recomputed {recomputed}"
        )));
    }
    let mut policy = generation_policy_from_canonical_json(&json).map_err(contract_fault)?;
    let catalog = load_capability_catalog(tx)?;
    canonicalize_generation_policy(&mut policy, &catalog).map_err(contract_fault)?;
    let reencoded = canonical_utf8(
        canonical_generation_policy_bytes(&policy),
        "generation policy",
    )?;
    if reencoded != json {
        return Err(Error::invariant(
            "generation policy is not the canonical encoding; refusing to read",
        ));
    }
    Ok(Some(policy))
}

// =============================================================================
// LogicalAgent type binding
// =============================================================================

/// Bind one already-materialized LogicalAgent to an exact AgentType revision.
///
/// Write-once: an agent may be bound at most once, and the bound revision is
/// immutable. The type is resolved through the validated catalog read, so a
/// corrupt revision can never become an agent's identity. This does not create a
/// PhysicalAgent, does not select a source, and grants no scheduling authority.
pub fn bind_logical_agent_type(
    tx: &Transaction<'_>,
    now: UnixTime,
    agent_id: &LogicalAgentId,
    type_ref: &AgentTypeRef,
) -> Result<(), Error> {
    let agent_exists: bool = query_opt(
        tx,
        "SELECT 1 FROM logical_agents WHERE id=?1",
        params![agent_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?
    .is_some();
    if !agent_exists {
        return Err(Error::not_found(format!(
            "logical agent {} does not exist",
            agent_id.as_str()
        )));
    }
    // Validate the exact revision through the canonical read, not merely its FK
    // row existence: a corrupt dependency must not become a new binding.
    get_agent_type(tx, type_ref)?.ok_or_else(|| {
        Error::not_found(format!(
            "agent type {}@{} does not exist",
            type_ref.id().as_str(),
            type_ref.revision()
        ))
    })?;
    let already: bool = query_opt(
        tx,
        "SELECT 1 FROM logical_agent_type_bindings WHERE logical_agent_id=?1",
        params![agent_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?
    .is_some();
    if already {
        return Err(Error::conflict(format!(
            "logical agent {} already has a type binding",
            agent_id.as_str()
        )));
    }
    tx.execute(
        "INSERT INTO logical_agent_type_bindings(logical_agent_id, type_id, type_revision, bound_at)
         VALUES(?1,?2,?3,?4)",
        params![
            agent_id.as_str(),
            type_ref.id().as_str(),
            type_ref.revision(),
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

pub fn get_logical_agent_type_binding(
    tx: &Transaction<'_>,
    agent_id: &LogicalAgentId,
) -> Result<Option<AgentTypeRef>, Error> {
    let row = query_opt(
        tx,
        "SELECT type_id, type_revision FROM logical_agent_type_bindings WHERE logical_agent_id=?1",
        params![agent_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let Some((type_id, revision)) = row else {
        return Ok(None);
    };
    let reference = AgentTypeRef::new(type_id, revision as u64).map_err(contract_fault)?;
    if get_agent_type(tx, &reference)?.is_none() {
        return Err(Error::invariant(
            "a LogicalAgent type binding references a missing agent type revision",
        ));
    }
    Ok(Some(reference))
}

// =============================================================================
// Matching (pure)
// =============================================================================

fn lifecycle_from_retention(retention: &str) -> Result<LifecycleMode, Error> {
    match retention {
        "resident" => Ok(LifecycleMode::Resident),
        "ephemeral" => Ok(LifecycleMode::Ephemeral),
        other => Err(Error::invariant(format!("unknown retention {other}"))),
    }
}

/// Rank existing, bound, immediately usable LogicalAgents for one Task's
/// requirement. Pure: it reads only durable bindings and the validated catalog,
/// writes nothing, and never touches a `SpawnSource`.
///
/// Only M5 `READY` and unassigned agents are candidates. Non-READY states
/// (`ASSIGNED`, `DRAINING`, `SUSPENDED`, `RETIRED`, ...) are NOT approximated as
/// "cold/revivable": M6 revival is a later seam, and returning a retired or busy
/// agent as an eligible candidate would freeze a false abstraction.
pub fn match_existing_agents_for_task(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<Vec<ExistingAgentCandidate>, Error> {
    let Some(requirement) = get_task_agent_requirement(tx, task_id)? else {
        return Ok(Vec::new());
    };
    let Some(required_ref) = requirement.required_type.clone() else {
        return Ok(Vec::new());
    };
    let (required, _status) = get_agent_type(tx, &required_ref)?.ok_or_else(|| {
        Error::invariant("a task agent requirement references a missing agent type")
    })?;

    let rows: Vec<(String, String, Option<f64>, f64)> = {
        let mut statement = tx
            .prepare(
                "SELECT la.id, la.retention, la.available_since, la.created_at
                 FROM logical_agents la
                 JOIN logical_agent_type_bindings b ON b.logical_agent_id = la.id
                 WHERE la.state='READY' AND la.current_task_id IS NULL
                 ORDER BY la.id",
            )
            .map_err(map_sqlite)?;
        let mapped = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row.map_err(map_sqlite)?);
        }
        out
    };

    let mut candidates = Vec::with_capacity(rows.len());
    for (agent_id, retention, available_since, created_at) in rows {
        let agent = LogicalAgentId::from_string(agent_id);
        let Some(bound_ref) = get_logical_agent_type_binding(tx, &agent)? else {
            continue;
        };
        let (agent_type, _status) = get_agent_type(tx, &bound_ref)?.ok_or_else(|| {
            Error::invariant("a LogicalAgent type binding references a missing agent type")
        })?;
        candidates.push(ExistingAgentCandidate {
            logical_agent_id: agent,
            agent_type,
            lifecycle: lifecycle_from_retention(&retention)?,
            available_since,
            created_at,
        });
    }

    let catalog = load_capability_catalog(tx)?;
    match_existing_agents(&required, &requirement, &candidates, &catalog).map_err(contract_fault)
}
