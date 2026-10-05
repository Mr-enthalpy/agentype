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

use crate::catalog::{get_agent_type, load_capability_catalog, AgentTypeStatus};
use crate::store::{map_sqlite, query_opt};
use agentype_agent_contract::{
    can_execute, canonical_generation_policy_bytes, canonical_task_agent_requirement_bytes,
    canonicalize_generation_policy, canonicalize_task_agent_requirement, content_digest,
    fold_generation_policy, generation_policy_content_digest,
    generation_policy_from_canonical_json, match_existing_agents, resolve_selector,
    task_agent_requirement_content_digest, task_agent_requirement_from_canonical_json,
    AgentRequirementDraft, AgentTypeRef, AgentTypeSelector, ContinuityMode, ExistingAgentCandidate,
    GenerationPolicy, TaskAgentRequirement, TaskPlacement,
};
use agentype_core::{
    ContinuityPreference, Error, GenerationId, InformationFunction, LogicalAgentId, PartitionId,
    TaskId, TaskSpec, UnixTime, WorkstreamId,
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
    let reference = &canonical.required_type;
    tx.execute(
        "INSERT INTO task_agent_requirements(
             task_id, required_type_id, required_type_revision,
             requirement_json, requirement_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            task_id.as_str(),
            reference.id().as_str(),
            reference.revision() as i64,
            content_json,
            digest,
            now
        ],
    )
    .map_err(map_sqlite)?;
    // The durable parent marker makes "typed" a positive fact, so a lost
    // requirement row is corruption rather than a silent legacy Task.
    tx.execute(
        "UPDATE tasks SET agent_requirement_mode='TYPED', updated_at=?1 WHERE id=?2",
        params![now, task_id.as_str()],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

/// The single authoritative read of a Task's agent requirement.
///
/// The parent Task's `agent_requirement_mode` and the child row are one
/// invariant. A `TYPED` Task without its requirement row, or a `LEGACY` Task that
/// unexpectedly has one, is corruption and fails closed, never a silent `None`.
pub fn get_task_agent_requirement(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<Option<TaskAgentRequirement>, Error> {
    let mode = task_agent_requirement_mode(tx, task_id)?;
    let row = query_opt(
        tx,
        "SELECT requirement_json, requirement_digest, required_type_id, required_type_revision
         FROM task_agent_requirements WHERE task_id=?1",
        params![task_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )?;
    let (json, digest, type_id, type_revision) = match (mode.as_str(), row) {
        ("LEGACY", None) => return Ok(None),
        ("TYPED", None) => {
            return Err(Error::invariant(format!(
                "typed task {} is missing its agent requirement row",
                task_id.as_str()
            )))
        }
        ("LEGACY", Some(_)) => {
            return Err(Error::invariant(format!(
                "legacy task {} unexpectedly has an agent requirement row",
                task_id.as_str()
            )))
        }
        ("TYPED", Some(row)) => row,
        (other, _) => {
            return Err(Error::invariant(format!(
                "unknown agent_requirement_mode {other} for task {}",
                task_id.as_str()
            )))
        }
    };
    let recomputed = content_digest(json.as_bytes());
    if recomputed != digest {
        return Err(Error::invariant(format!(
            "task agent requirement digest {digest} does not match recomputed {recomputed}"
        )));
    }
    let mut requirement =
        task_agent_requirement_from_canonical_json(&json).map_err(contract_fault)?;
    let reference = &requirement.required_type;
    let mirror =
        reference.id().as_str() == type_id.as_str() && type_revision == reference.revision() as i64;
    if !mirror {
        return Err(Error::invariant(
            "task agent requirement required_type mirror does not match its canonical document",
        ));
    }
    // The exact pin MUST resolve through the B.2 validated catalog read, not
    // merely its FK row: a corrupt revision (missing overlay, non-canonical
    // content) must fail this authoritative read closed. Deprecation is NOT
    // corruption, so a `DEPRECATED` revision remains a valid committed pin.
    get_agent_type(tx, reference)?.ok_or_else(|| {
        Error::invariant(format!(
            "task agent requirement pins a missing agent type {}@{}",
            reference.id().as_str(),
            reference.revision()
        ))
    })?;
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

fn task_agent_requirement_mode(tx: &Transaction<'_>, task_id: &TaskId) -> Result<String, Error> {
    query_opt(
        tx,
        "SELECT agent_requirement_mode FROM tasks WHERE id=?1",
        params![task_id.as_str()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::not_found(format!("task {}", task_id.as_str())))
}

/// Resolve a pre-commit `AgentRequirementDraft` into the canonical durable
/// requirement for one admitted Task, folding its Generation's policy.
///
/// Initial admission only: an exact selector MUST currently be `PUBLISHED`.
/// Replay uses [`build_task_agent_requirement_replay`], which additionally
/// accepts an already-committed exact revision even if it was later deprecated.
///
/// The dimensions a `TaskSpec` owns are derived from it (single authority), and
/// a `Required` continuity preference becomes a hard `Logical` requirement. The
/// result is canonicalized but NOT persisted.
pub fn build_task_agent_requirement(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    draft: AgentRequirementDraft,
    information_function: InformationFunction,
    task_spec: &TaskSpec,
) -> Result<TaskAgentRequirement, Error> {
    let required_type = resolve_draft_selector(tx, &draft.required_type, None)?;
    let requirement = derive_task_agent_requirement(
        tx,
        generation_id,
        draft,
        information_function,
        task_spec,
        required_type,
    )?;
    validate_pinned_type_can_execute(tx, &requirement)?;
    Ok(requirement)
}

/// Replay of an already-admitted typed proposal. An exact selector that equals
/// the committed pin is accepted against the exact immutable catalog revision
/// (validated content, `PUBLISHED` or `DEPRECATED`), because deprecation is
/// operational disposition drift, not corruption. Any other selector resolves
/// against the current published catalog, so `Latest` drift still fails closed.
pub fn build_task_agent_requirement_replay(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    draft: AgentRequirementDraft,
    information_function: InformationFunction,
    task_spec: &TaskSpec,
    committed_pin: &AgentTypeRef,
) -> Result<TaskAgentRequirement, Error> {
    let required_type = resolve_draft_selector(tx, &draft.required_type, Some(committed_pin))?;
    derive_task_agent_requirement(
        tx,
        generation_id,
        draft,
        information_function,
        task_spec,
        required_type,
    )
}

/// Resolve a draft's selector to an exact revision. `committed` is the pin of an
/// already-admitted requirement being replayed; an exact selector equal to it is
/// accepted even when deprecated.
fn resolve_draft_selector(
    tx: &Transaction<'_>,
    selector: &AgentTypeSelector,
    committed: Option<&AgentTypeRef>,
) -> Result<AgentTypeRef, Error> {
    if let (AgentTypeSelector::Exact(reference), Some(committed)) = (selector, committed) {
        if reference == committed {
            get_agent_type(tx, reference)?.ok_or_else(|| {
                Error::not_found(format!(
                    "agent type {}@{} does not exist",
                    reference.id().as_str(),
                    reference.revision()
                ))
            })?;
            return Ok(reference.clone());
        }
    }
    let lookup = crate::catalog::load_agent_type_lookup(tx)?;
    let resolved = resolve_selector(selector, &lookup).map_err(contract_fault)?;
    get_agent_type(tx, &resolved)?.ok_or_else(|| {
        Error::not_found(format!(
            "agent type {}@{} does not exist",
            resolved.id().as_str(),
            resolved.revision()
        ))
    })?;
    Ok(resolved)
}

fn derive_task_agent_requirement(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    draft: AgentRequirementDraft,
    information_function: InformationFunction,
    task_spec: &TaskSpec,
    required_type: AgentTypeRef,
) -> Result<TaskAgentRequirement, Error> {
    let required_affinity: BTreeSet<String> = task_spec.affinity_tags.iter().cloned().collect();
    // Only a `Required` continuity preference becomes a hard `Logical`
    // requirement. `Preferred`/`None` stay out of the hard requirement; the
    // `Preferred` placement preference is applied by the matcher from the Task's
    // M5 row.
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
    // A policy-bearing Generation MUST have its policy row; a missing row is
    // corruption, never an unconstrained Generation.
    if generation_policy_mode(tx, generation_id)? == "POLICY" {
        let policy = get_generation_policy(tx, generation_id)?.ok_or_else(|| {
            Error::invariant("a policy-bearing generation is missing its policy row")
        })?;
        requirement.hard =
            fold_generation_policy(&policy, &requirement.hard).map_err(contract_fault)?;
    }
    requirement.normalize(&catalog).map_err(contract_fault)?;

    Ok(requirement)
}

/// The pinned contract MUST be able to execute its own effective requirement;
/// otherwise the Task is unsatisfiable and initial admission fails closed. This
/// is an admission-time check only: a replay compares the incoming requirement
/// against the committed one and reports a Conflict on any difference.
fn validate_pinned_type_can_execute(
    tx: &Transaction<'_>,
    requirement: &TaskAgentRequirement,
) -> Result<(), Error> {
    let reference = &requirement.required_type;
    let (agent_type, _status) = get_agent_type(tx, reference)?.ok_or_else(|| {
        Error::not_found(format!(
            "agent type {}@{} does not exist",
            reference.id().as_str(),
            reference.revision()
        ))
    })?;
    let catalog = load_capability_catalog(tx)?;
    can_execute(&agent_type, &requirement.hard, &catalog).map_err(contract_fault)
}

/// The durable generation policy presence marker. A missing row for a
/// `POLICY` marker is corruption, never an unconstrained Generation.
pub fn generation_policy_mode(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
) -> Result<String, Error> {
    query_opt(
        tx,
        "SELECT policy_mode FROM generations WHERE generation_id=?1",
        params![generation_id.as_str()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::not_found(format!("generation {}", generation_id.as_str())))
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

/// The single authoritative read of a Generation's policy.
///
/// The parent Generation's `policy_mode` and the child row are one invariant. A
/// `POLICY` Generation without its row, or a `NONE` Generation that unexpectedly
/// has one, is corruption and fails closed, never a silent `None`.
pub fn get_generation_policy(
    tx: &Transaction<'_>,
    generation_id: &agentype_core::GenerationId,
) -> Result<Option<GenerationPolicy>, Error> {
    let mode = generation_policy_mode(tx, generation_id)?;
    let row = query_opt(
        tx,
        "SELECT policy_json, policy_digest FROM generation_policies WHERE generation_id=?1",
        params![generation_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let (json, digest) = match (mode.as_str(), row) {
        ("NONE", None) => return Ok(None),
        ("POLICY", None) => {
            return Err(Error::invariant(format!(
                "policy-bearing generation {} is missing its policy row",
                generation_id.as_str()
            )))
        }
        ("NONE", Some(_)) => {
            return Err(Error::invariant(format!(
                "generation {} is not policy-bearing but has a policy row",
                generation_id.as_str()
            )))
        }
        ("POLICY", Some(row)) => row,
        (other, _) => {
            return Err(Error::invariant(format!(
                "unknown policy_mode {other} for generation {}",
                generation_id.as_str()
            )))
        }
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
    // row existence: a corrupt dependency must not become a new binding. A fresh
    // binding requires a currently `PUBLISHED` revision, matching pre-commit
    // selector semantics; an existing committed pin survives deprecation, but an
    // operator cannot mint a new commitment to a deprecated identity.
    let (_, status) = get_agent_type(tx, type_ref)?.ok_or_else(|| {
        Error::not_found(format!(
            "agent type {}@{} does not exist",
            type_ref.id().as_str(),
            type_ref.revision()
        ))
    })?;
    if status != AgentTypeStatus::Published {
        return Err(Error::invalid_authority(format!(
            "a fresh LogicalAgent type binding requires a PUBLISHED agent type; {}@{} is {}",
            type_ref.id().as_str(),
            type_ref.revision(),
            if status == AgentTypeStatus::Deprecated {
                "DEPRECATED"
            } else {
                "not published"
            }
        )));
    }
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

fn parse_tags(json: &str) -> Result<BTreeSet<String>, Error> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::invariant(format!("tags json: {e}")))?;
    let array = value
        .as_array()
        .ok_or_else(|| Error::invariant("tags json must be an array"))?;
    let mut tags = BTreeSet::new();
    for item in array {
        let tag = item
            .as_str()
            .ok_or_else(|| Error::invariant("tags json element must be a string"))?;
        tags.insert(tag.to_string());
    }
    Ok(tags)
}

/// Rank existing, bound, immediately usable LogicalAgents for one Task's
/// requirement. Pure: it reads durable bindings, the Task's M5 placement row, and
/// the validated catalog, writes nothing, and never touches a `SpawnSource`.
///
/// Eligibility composes the frozen M5 placement rules (exact partition, tag
/// superset, `Required` workstream gate) with the AgentType contract
/// (`can_execute`). Only M5 `READY` and unassigned agents are candidates;
/// non-READY states are not approximated as "cold/revivable".
pub fn match_existing_agents_for_task(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<Vec<ExistingAgentCandidate>, Error> {
    let Some(requirement) = get_task_agent_requirement(tx, task_id)? else {
        return Ok(Vec::new());
    };
    let required_ref = requirement.required_type.clone();
    let (required, _status) = get_agent_type(tx, &required_ref)?.ok_or_else(|| {
        Error::invariant("a task agent requirement references a missing agent type")
    })?;

    let task_row = query_opt(
        tx,
        "SELECT partition_name, affinity_tags_json, workstream_id, continuity, agent_requirement_mode
         FROM tasks WHERE id=?1",
        params![task_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("task {}", task_id.as_str())))?;
    let (partition, tags_json, workstream, continuity, mode) = task_row;
    if mode != "TYPED" {
        return Err(Error::invariant(
            "a task agent requirement exists but the task is not marked TYPED",
        ));
    }
    let placement = TaskPlacement {
        partition: PartitionId::new(partition),
        required_tags: parse_tags(&tags_json)?,
        workstream_id: workstream.map(WorkstreamId::from_string),
        continuity: ContinuityPreference::parse_sql(&continuity)?,
    };

    let rows: Vec<(String, String, String, Option<String>, Option<f64>, f64)> = {
        let mut statement = tx
            .prepare(
                "SELECT la.id, la.partition_name, la.tags_json, la.workstream_id,
                        la.available_since, la.created_at
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
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, f64>(5)?,
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
    for (agent_id, partition, tags_json, workstream, available_since, created_at) in rows {
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
            partition: PartitionId::new(partition),
            tags: parse_tags(&tags_json)?,
            workstream_id: workstream.map(WorkstreamId::from_string),
            available_since,
            created_at,
        });
    }

    let catalog = load_capability_catalog(tx)?;
    match_existing_agents(&required, &requirement, &placement, &candidates, &catalog)
        .map_err(contract_fault)
}
