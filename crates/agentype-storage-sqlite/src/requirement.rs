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
    can_execute, can_execute_security_v1, canonical_generation_policy_bytes,
    canonical_sandbox_policy_bytes, canonical_security_derivation_bytes,
    canonical_task_agent_requirement_bytes, canonicalize_generation_policy,
    canonicalize_task_agent_requirement, check_required_access, content_digest,
    fold_generation_policy, fold_generation_policy_security_v1, generation_policy_content_digest,
    generation_policy_from_canonical_json, match_existing_agents, meet_optional_layer,
    project_capability_access, required_access_content_digest, resolve_selector,
    sandbox_policy_content_digest, sandbox_policy_from_canonical_json,
    security_derivation_content_digest, security_derivation_from_canonical_json,
    task_agent_requirement_content_digest, task_agent_requirement_from_canonical_json,
    validate_effective_task_execution_modes, validate_generation_coarse_fine,
    validate_task_local_coarse_fine, AgentRequirementDraft, AgentTypeRef, AgentTypeSelector,
    CapabilityCatalog, ContinuityMode, ExistingAgentCandidate, GenerationPolicy,
    NormalizedSandboxPolicy, PendingObligation, PolicyAccess, PolicyInput, RequiredAccess,
    SandboxPolicyRef, SecurityContractMode, SecurityDerivation, TaskAgentRequirement,
    TaskPlacement, TaskSecurityBinding,
};
use agentype_core::{
    ContinuityPreference, Error, GenerationId, InformationFunction, LogicalAgentId, PartitionId,
    TaskId, TaskSpec, UnixTime, WorkspaceMode, WorkstreamId,
};
use rusqlite::{params, Transaction};
use std::collections::BTreeSet;

fn contract_fault(error: agentype_agent_contract::ContractError) -> Error {
    Error::invariant(error.to_string())
}

/// Classify a contract error raised while building an admission command, so an
/// ordinary command/config rejection is not reported as durable corruption.
/// Read-side integrity failures keep `contract_fault` (`InvariantViolation`).
fn contract_rejection(error: agentype_agent_contract::ContractError) -> Error {
    use agentype_agent_contract::ContractError as C;
    match &error {
        C::AgentTypeNotFound { .. } => Error::not_found(error.to_string()),
        C::CapabilityMismatch { .. }
        | C::SecurityUnenforceable { .. }
        | C::GenerationPolicyConflict { .. }
        | C::RequirementConflict { .. }
        | C::InvalidRefinement { .. }
        | C::SourceConfigInvalid { .. } => Error::invalid_authority(error.to_string()),
        C::EvidencePolicyMismatch { .. }
        | C::EvidenceSubjectMismatch { .. }
        | C::InvalidNumber { .. }
        | C::InvalidRef { .. }
        | C::CapabilityDefinitionConflict { .. }
        | C::InvalidPolicyProjection { .. }
        | C::InvalidSandboxPolicy { .. }
        | C::UnsupportedPolicyVersion { .. }
        | C::UnsupportedPolicyDerivationVersion { .. }
        | C::InvariantViolation(_) => Error::invariant(error.to_string()),
    }
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
    security_mode: SecurityContractMode,
) -> Result<String, Error> {
    let catalog = load_capability_catalog(tx)?;
    let mut canonical = requirement.clone();
    canonicalize_task_agent_requirement(&mut canonical, &catalog).map_err(contract_rejection)?;

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
             requirement_json, requirement_digest, security_contract_mode, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            task_id.as_str(),
            reference.id().as_str(),
            reference.revision() as i64,
            content_json,
            digest,
            security_mode.as_str(),
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

/// The durable `SecurityContractMode` owned by `task_agent_requirements`.
///
/// `COARSE_V1` is the frozen absence default for a typed requirement. The mode
/// is never inferred from `sandbox_policy.is_some()`.
pub fn task_security_mode(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<SecurityContractMode, Error> {
    let stored = query_opt(
        tx,
        "SELECT security_contract_mode FROM task_agent_requirements WHERE task_id=?1",
        params![task_id.as_str()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| {
        Error::invariant(format!(
            "task {} is missing its agent requirement mode",
            task_id.as_str()
        ))
    })?;
    SecurityContractMode::parse(&stored).ok_or_else(|| {
        Error::invariant(format!(
            "task {} records unsupported security_contract_mode {stored:?}",
            task_id.as_str()
        ))
    })
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
    let mode = task_security_mode(tx, task_id)?;
    verify_requirement_mirrors(tx, task_id, &requirement, mode)?;
    verify_task_security_binding_presence(tx, task_id, mode)?;
    Ok(Some(requirement))
}

fn task_security_binding_exists(tx: &Transaction<'_>, task_id: &TaskId) -> Result<bool, Error> {
    Ok(query_opt(
        tx,
        "SELECT 1 FROM task_security_bindings WHERE task_id=?1",
        params![task_id.as_str()],
        |row| row.get::<_, i64>(0),
    )?
    .is_some())
}

/// The conditional biconditional between the requirement's mode and the
/// binding's existence (ADR-0013 §11).
///
/// A `SECURITY_V1` requirement without its binding, or a binding under a
/// `COARSE_V1` requirement, is corruption and MUST NOT degrade to `COARSE_V1`.
fn verify_task_security_binding_presence(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    mode: SecurityContractMode,
) -> Result<(), Error> {
    let present = task_security_binding_exists(tx, task_id)?;
    match (mode, present) {
        (SecurityContractMode::SecurityV1, true) | (SecurityContractMode::CoarseV1, false) => {
            Ok(())
        }
        (SecurityContractMode::SecurityV1, false) => Err(Error::invariant(format!(
            "SECURITY_V1 task {} is missing its security binding",
            task_id.as_str()
        ))),
        (SecurityContractMode::CoarseV1, true) => Err(Error::invariant(format!(
            "COARSE_V1 task {} unexpectedly has a security binding",
            task_id.as_str()
        ))),
    }
}

/// Cross-check the duplicated TaskSpec-owned dimensions against their
/// authoritative sources. The Task, the GenerationTaskBinding, and the folded
/// Generation policy remain the single authority; a self-consistent corruption of
/// the requirement copy that disagrees with them MUST fail closed.
fn verify_requirement_mirrors(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    requirement: &TaskAgentRequirement,
    security_mode: SecurityContractMode,
) -> Result<(), Error> {
    let task = query_opt(
        tx,
        "SELECT affinity_tags_json, workspace_mode, continuity FROM tasks WHERE id=?1",
        params![task_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("task {}", task_id.as_str())))?;
    let (tags_json, workspace, continuity) = task;
    if parse_tags(&tags_json)? != requirement.hard.required_affinity {
        return Err(Error::invariant(
            "task agent requirement required_affinity does not match the Task affinity tags",
        ));
    }
    if WorkspaceMode::parse_sql(&workspace)? != requirement.hard.required_workspace {
        return Err(Error::invariant(
            "task agent requirement required_workspace does not match the Task workspace mode",
        ));
    }

    let binding = query_opt(
        tx,
        "SELECT generation_id, information_function FROM generation_task_bindings WHERE task_id=?1",
        params![task_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?
    .ok_or_else(|| Error::invariant("typed task is missing its GenerationTaskBinding"))?;
    let (generation_id, information_function) = binding;
    if InformationFunction::parse_sql(&information_function)?
        != requirement.hard.information_function
    {
        return Err(Error::invariant(
            "task agent requirement information_function does not match the GenerationTaskBinding",
        ));
    }

    let base = match ContinuityPreference::parse_sql(&continuity)? {
        ContinuityPreference::Required => ContinuityMode::Logical,
        ContinuityPreference::Preferred | ContinuityPreference::None => ContinuityMode::None,
    };
    // Re-prove the §3 Task-local consistency on read. A durable requirement that
    // contradicts its own authored Task policy is corruption, never a valid past
    // commitment, even when a stricter ceiling would hide it in `P_eff`.
    if security_mode == SecurityContractMode::SecurityV1 {
        let authored = load_authored_policy(tx, requirement.hard.sandbox_policy.as_ref())?;
        validate_task_local_coarse_fine(
            requirement.hard.required_workspace,
            requirement.hard.required_network,
            authored.as_ref(),
        )
        .map_err(|error| {
            Error::invariant(format!(
                "task agent requirement contradicts its authored Task policy: {error}"
            ))
        })?;
    }
    let generation_id = GenerationId::from_string(generation_id);
    match get_generation_policy(tx, &generation_id)? {
        Some(policy) => {
            let effective = policy.min_continuity.max(base);
            if effective != requirement.hard.required_continuity {
                return Err(Error::invariant(
                    "task agent requirement required_continuity does not match the folded Task/Generation continuity",
                ));
            }
            // Re-fold the whole policy and require the stored requirement to be
            // the exact fixed point. This revalidates every policy-owned
            // dimension (workspace, network, isolation, sandbox, budget, anchor,
            // information function, affinity, continuity) rather than a
            // hand-picked subset, and keeps future policy fields covered.
            let refolded = match security_mode {
                SecurityContractMode::CoarseV1 => {
                    fold_generation_policy(&policy, &requirement.hard)
                }
                SecurityContractMode::SecurityV1 => {
                    fold_generation_policy_security_v1(&policy, &requirement.hard)
                }
            }
            .map_err(|error| {
                Error::invariant(format!(
                    "task agent requirement violates its Generation policy: {error}"
                ))
            })?;
            if refolded != requirement.hard {
                return Err(Error::invariant(
                    "task agent requirement is not the fixed point of its Generation policy",
                ));
            }
        }
        None => {
            if security_mode == SecurityContractMode::SecurityV1 {
                return Err(Error::invariant(
                    "SECURITY_V1 task requirement has a generation without a security policy",
                ));
            }
            if base != requirement.hard.required_continuity {
                return Err(Error::invariant(
                    "task agent requirement required_continuity does not match the Task continuity",
                ));
            }
        }
    }
    Ok(())
}

pub fn task_agent_requirement_mode(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<String, Error> {
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
    security_mode: SecurityContractMode,
    draft: AgentRequirementDraft,
    information_function: InformationFunction,
    task_spec: &TaskSpec,
) -> Result<TaskAgentRequirement, Error> {
    let required_type = resolve_draft_selector(tx, &draft.required_type, None)?;
    let requirement = derive_task_agent_requirement(
        tx,
        generation_id,
        security_mode,
        draft,
        information_function,
        task_spec,
        required_type,
    )?;
    validate_pinned_type_can_execute(tx, generation_id, security_mode, &requirement)?;
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
    security_mode: SecurityContractMode,
    draft: AgentRequirementDraft,
    information_function: InformationFunction,
    task_spec: &TaskSpec,
    committed_pin: &AgentTypeRef,
) -> Result<TaskAgentRequirement, Error> {
    let required_type = resolve_draft_selector(tx, &draft.required_type, Some(committed_pin))?;
    derive_task_agent_requirement(
        tx,
        generation_id,
        security_mode,
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
    // A replay must not let a mutable catalog view change a past commitment: an
    // exact selector equal to the committed pin, or a `Latest` selector for the
    // committed pin's `type_id`, is answered from the committed pin (validated
    // content, `PUBLISHED` or `DEPRECATED`). Any other selector resolves against
    // the current published catalog, so a genuinely new commitment picks up new
    // revisions.
    let replay_pin = match (selector, committed) {
        (AgentTypeSelector::Exact(reference), Some(committed)) if reference == committed => {
            Some(committed)
        }
        (AgentTypeSelector::Latest(type_id), Some(committed)) if type_id == committed.id() => {
            Some(committed)
        }
        _ => None,
    };
    if let Some(committed) = replay_pin {
        get_agent_type(tx, committed)?.ok_or_else(|| {
            Error::not_found(format!(
                "agent type {}@{} does not exist",
                committed.id().as_str(),
                committed.revision()
            ))
        })?;
        return Ok(committed.clone());
    }
    let lookup = crate::catalog::load_agent_type_lookup(tx)?;
    let resolved = resolve_selector(selector, &lookup).map_err(contract_rejection)?;
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
    security_mode: SecurityContractMode,
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

    // ADR-0013 §3: the Task-local coarse/fine check runs on the un-normalized
    // authored inputs, before any cross-object meet and before normalization. It
    // is part of the `SECURITY_V1` contract; `COARSE_V1` keeps its frozen replay
    // exactly, where the optional policy is not admitted as a second grant.
    if security_mode == SecurityContractMode::SecurityV1 {
        let authored_policy = load_authored_policy(tx, draft.sandbox_policy.as_ref())?;
        validate_task_local_coarse_fine(
            task_spec.workspace_mode,
            draft.required_network,
            authored_policy.as_ref(),
        )
        .map_err(|error| {
            Error::invalid_authority(format!(
                "task coarse fields contradict its authored sandbox policy: {error}"
            ))
        })?;
    }

    let mut requirement = draft.into_requirement(
        required_type,
        information_function,
        required_affinity,
        task_spec.workspace_mode,
        required_continuity,
    );

    let catalog = load_capability_catalog(tx)?;
    requirement
        .normalize(&catalog)
        .map_err(contract_rejection)?;
    // The authoritative policy read is the single presence authority: a `NONE`
    // generation with an unexpected row, or a `POLICY` generation with a missing
    // row, fails closed rather than being treated as unconstrained.
    let generation_policy = get_generation_policy(tx, generation_id)?;
    if security_mode == SecurityContractMode::SecurityV1 && generation_policy.is_none() {
        return Err(Error::invalid_authority(
            "a SECURITY_V1 task requires a policy-bearing generation",
        ));
    }
    if let Some(policy) = generation_policy {
        // `SECURITY_V1` keeps the Task's authored policy reference as an
        // independent input; the effective policy is a real meet at binding
        // time. `COARSE_V1` keeps the frozen exact-reference fold.
        let folded = match security_mode {
            SecurityContractMode::CoarseV1 => fold_generation_policy(&policy, &requirement.hard),
            SecurityContractMode::SecurityV1 => {
                fold_generation_policy_security_v1(&policy, &requirement.hard)
            }
        };
        requirement.hard = folded.map_err(contract_rejection)?;
    }
    requirement
        .normalize(&catalog)
        .map_err(contract_rejection)?;

    Ok(requirement)
}

/// Load the canonical authored policy a Task or Generation names, if any.
fn load_authored_policy(
    tx: &Transaction<'_>,
    reference: Option<&SandboxPolicyRef>,
) -> Result<Option<NormalizedSandboxPolicy>, Error> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    crate::catalog::get_sandbox_policy(tx, reference)?
        .map(Some)
        .ok_or_else(|| {
            Error::not_found(format!(
                "sandbox policy {}@{} does not exist",
                reference.id().as_str(),
                reference.revision()
            ))
        })
}

/// The pinned contract MUST be able to execute its own effective requirement;
/// otherwise the Task is unsatisfiable and initial admission fails closed. This
/// is an admission-time check only: a replay compares the incoming requirement
/// against the committed one and reports a Conflict on any difference.
///
/// A `SECURITY_V1` requirement is checked with the real policy order: the
/// effective Task policy `meet(P_generation, P_task)` MUST stay within the
/// pinned AgentType's authored policy even when the Task names a different
/// catalog revision. `COARSE_V1` keeps the frozen exact-reference predicate.
fn validate_pinned_type_can_execute(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    security_mode: SecurityContractMode,
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
    match security_mode {
        SecurityContractMode::CoarseV1 => {
            can_execute(&agent_type, &requirement.hard, &catalog).map_err(contract_rejection)
        }
        SecurityContractMode::SecurityV1 => {
            // A `SECURITY_V1` Task may pin a `COARSE_V1` AgentType; that relation
            // is a candidate-stage execution-mode decision, not an admission
            // rejection. The coarse revision's policy field keeps its frozen
            // exact-reference meaning here. Only a `SECURITY_V1` pinned revision
            // carries a fine authored ceiling that is ordered by the algebra.
            if crate::catalog::agent_type_security_mode(tx, reference)?
                != SecurityContractMode::SecurityV1
            {
                return can_execute(&agent_type, &requirement.hard, &catalog)
                    .map_err(contract_rejection);
            }
            let resolved = resolve_effective_task_policy(tx, generation_id, requirement)?;
            let agent_reference = agent_type.contract.sandbox_policy.as_ref().ok_or_else(|| {
                Error::invariant("a SECURITY_V1 agent type is missing its authored sandbox policy")
            })?;
            let agent_policy = crate::catalog::get_sandbox_policy(tx, agent_reference)?
                .ok_or_else(|| {
                    Error::invariant(format!(
                        "SECURITY_V1 agent type pins a missing sandbox policy {}@{}",
                        agent_reference.id().as_str(),
                        agent_reference.revision()
                    ))
                })?;
            can_execute_security_v1(
                &agent_type,
                &requirement.hard,
                &resolved.effective,
                &agent_policy,
                &catalog,
            )
            .map_err(contract_rejection)
        }
    }
}

/// The authored inputs and effective result of the Task-stage policy meet.
struct EffectiveTaskPolicy {
    generation_reference: SandboxPolicyRef,
    generation_authored: NormalizedSandboxPolicy,
    task_authored: Option<NormalizedSandboxPolicy>,
    effective: NormalizedSandboxPolicy,
}

/// Resolve `meet(P_generation, P_task)` from the authored catalog revisions.
///
/// A missing Generation policy, missing Generation policy reference, or missing
/// authored policy is durable corruption. A structurally conflicting Task layer
/// is an admission rejection, reported against the same reason the binding uses.
fn resolve_effective_task_policy(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    requirement: &TaskAgentRequirement,
) -> Result<EffectiveTaskPolicy, Error> {
    let generation_policy = get_generation_policy(tx, generation_id)?.ok_or_else(|| {
        Error::invalid_authority(
            "a SECURITY_V1 task requires its generation to carry a security policy",
        )
    })?;
    let generation_reference = generation_policy.sandbox_policy.as_ref().ok_or_else(|| {
        Error::invalid_authority("a SECURITY_V1 generation policy must author a sandbox policy")
    })?;
    let generation_authored = crate::catalog::get_sandbox_policy(tx, generation_reference)?
        .ok_or_else(|| {
            Error::invariant(format!(
                "generation pins a missing sandbox policy {}@{}",
                generation_reference.id().as_str(),
                generation_reference.revision()
            ))
        })?;
    let task_authored = load_authored_policy(tx, requirement.hard.sandbox_policy.as_ref())?;
    let effective =
        meet_optional_layer(&generation_authored, task_authored.as_ref()).map_err(|error| {
            Error::invalid_authority(format!(
                "task sandbox policy conflicts with its generation policy: {error}"
            ))
        })?;
    Ok(EffectiveTaskPolicy {
        generation_reference: generation_reference.clone(),
        generation_authored,
        task_authored,
        effective,
    })
}

/// The durable generation policy presence marker. A missing row for a
/// `POLICY` marker is corruption, never an unconstrained Generation.
fn generation_policy_mode(
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
    security_mode: SecurityContractMode,
) -> Result<String, Error> {
    let catalog = load_capability_catalog(tx)?;
    let mut canonical = policy.clone();
    canonicalize_generation_policy(&mut canonical, &catalog).map_err(contract_rejection)?;

    if security_mode == SecurityContractMode::SecurityV1 {
        // A SECURITY_V1 Generation MUST carry an authored policy: an absent
        // policy would leave `project_gen` undefined (ADR-0013 §3). The
        // Generation's coarse ceiling must also agree with the authored policy
        // it projects.
        let reference = canonical.sandbox_policy.as_ref().ok_or_else(|| {
            Error::invalid_authority("a SECURITY_V1 generation policy must author a sandbox policy")
        })?;
        let authored = crate::catalog::get_sandbox_policy(tx, reference)?.ok_or_else(|| {
            Error::not_found(format!(
                "sandbox policy {}@{} does not exist",
                reference.id().as_str(),
                reference.revision()
            ))
        })?;
        validate_generation_coarse_fine(
            canonical.max_workspace,
            canonical.max_network,
            canonical.requires_attempt_isolation,
            &authored,
        )
        .map_err(|error| {
            Error::invalid_authority(format!(
                "SECURITY_V1 generation ceiling contradicts its authored policy: {error}"
            ))
        })?;
    }

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
        "INSERT INTO generation_policies(
             generation_id, policy_json, policy_digest, security_contract_mode, created_at)
         VALUES(?1,?2,?3,?4,?5)",
        params![
            generation_id.as_str(),
            content_json,
            digest,
            security_mode.as_str(),
            now
        ],
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
        "SELECT policy_json, policy_digest, security_contract_mode
         FROM generation_policies WHERE generation_id=?1",
        params![generation_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let (json, digest, stored_security_mode) = match (mode.as_str(), row) {
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
    // Re-prove the §3 Generation projection on read: a `SECURITY_V1` ceiling
    // whose authored policy does not project back to it is corruption, not a
    // valid past commitment.
    let security_mode = SecurityContractMode::parse(&stored_security_mode).ok_or_else(|| {
        Error::invariant(format!(
            "generation {} records unsupported security_contract_mode {stored_security_mode:?}",
            generation_id.as_str()
        ))
    })?;
    if security_mode == SecurityContractMode::SecurityV1 {
        let reference = policy.sandbox_policy.as_ref().ok_or_else(|| {
            Error::invariant("a SECURITY_V1 generation policy is missing its sandbox policy")
        })?;
        let authored = crate::catalog::get_sandbox_policy(tx, reference)?.ok_or_else(|| {
            Error::invariant(format!(
                "SECURITY_V1 generation pins a missing sandbox policy {}@{}",
                reference.id().as_str(),
                reference.revision()
            ))
        })?;
        validate_generation_coarse_fine(
            policy.max_workspace,
            policy.max_network,
            policy.requires_attempt_isolation,
            &authored,
        )
        .map_err(|error| {
            Error::invariant(format!(
                "SECURITY_V1 generation ceiling contradicts its authored policy: {error}"
            ))
        })?;
    }
    Ok(Some(policy))
}

/// The `SecurityContractMode` of an exact Generation.
///
/// A `NONE` Generation is the derived `COARSE_V1`; its mode is never stored as a
/// second marker and B5 MUST NOT backfill a policy row for it. A `POLICY`
/// Generation's mode is owned by its immutable policy row.
pub fn generation_security_mode(
    tx: &Transaction<'_>,
    generation_id: &agentype_core::GenerationId,
) -> Result<SecurityContractMode, Error> {
    // The validated policy read is the single presence authority: a `POLICY`
    // marker with a missing or corrupt row must fail closed rather than be
    // treated as an unconstrained Generation.
    get_generation_policy(tx, generation_id)?;
    let mode = generation_policy_mode(tx, generation_id)?;
    match mode.as_str() {
        "NONE" => Ok(SecurityContractMode::CoarseV1),
        "POLICY" => {
            let stored = query_opt(
                tx,
                "SELECT security_contract_mode FROM generation_policies WHERE generation_id=?1",
                params![generation_id.as_str()],
                |row| row.get::<_, String>(0),
            )?
            .ok_or_else(|| {
                Error::invariant(format!(
                    "policy-bearing generation {} is missing its policy row",
                    generation_id.as_str()
                ))
            })?;
            SecurityContractMode::parse(&stored).ok_or_else(|| {
                Error::invariant(format!(
                    "generation {} records unsupported security_contract_mode {stored:?}",
                    generation_id.as_str()
                ))
            })
        }
        other => Err(Error::invariant(format!(
            "unknown policy_mode {other} for generation {}",
            generation_id.as_str()
        ))),
    }
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
    let agent = query_opt(
        tx,
        "SELECT state, current_task_id, agent_type_binding_mode FROM logical_agents WHERE id=?1",
        params![agent_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let Some((state, current_task, binding_mode)) = agent else {
        return Err(Error::not_found(format!(
            "logical agent {} does not exist",
            agent_id.as_str()
        )));
    };
    // A type binding is a semantic identity commitment, so it may only be minted
    // at a safe assignment boundary: a READY, unassigned, currently-unbound agent.
    // Binding an agent that is already executing legacy work, or already bound,
    // would commit a contract that contradicts its current identity.
    if state != "READY" || current_task.is_some() {
        return Err(Error::invalid_authority(format!(
            "a LogicalAgent type binding requires a READY, unassigned agent; {} is {}",
            agent_id.as_str(),
            state
        )));
    }
    if binding_mode != "UNBOUND" {
        return Err(Error::conflict(format!(
            "logical agent {} is already type-bound",
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
    // Flip the positive parent marker first (the child insert requires a `BOUND`
    // parent), then write the immutable child row, all in one transaction.
    tx.execute(
        "UPDATE logical_agents SET agent_type_binding_mode='BOUND', updated_at=?1 WHERE id=?2",
        params![now, agent_id.as_str()],
    )
    .map_err(map_sqlite)?;
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
    let mode = query_opt(
        tx,
        "SELECT agent_type_binding_mode FROM logical_agents WHERE id=?1",
        params![agent_id.as_str()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::not_found(format!("logical agent {}", agent_id.as_str())))?;
    let row = query_opt(
        tx,
        "SELECT type_id, type_revision FROM logical_agent_type_bindings WHERE logical_agent_id=?1",
        params![agent_id.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let (type_id, revision) = match (mode.as_str(), row) {
        ("UNBOUND", None) => return Ok(None),
        ("BOUND", None) => {
            return Err(Error::invariant(format!(
                "bound LogicalAgent {} is missing its type binding row",
                agent_id.as_str()
            )))
        }
        ("UNBOUND", Some(_)) => {
            return Err(Error::invariant(format!(
                "unbound LogicalAgent {} unexpectedly has a type binding row",
                agent_id.as_str()
            )))
        }
        ("BOUND", Some(row)) => row,
        (other, _) => {
            return Err(Error::invariant(format!(
                "unknown agent_type_binding_mode {other} for LogicalAgent {}",
                agent_id.as_str()
            )))
        }
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

/// Rank existing, bound, `READY`, unassigned LogicalAgents for one Task's
/// requirement. Pure, non-authoritative **semantic candidate preselection**: it
/// reads durable bindings, the Task's M5 placement row, and the validated
/// catalog, writes nothing, and never touches a `SpawnSource`. A result proves
/// only semantic compatibility and M5 placement, never physical eligibility —
/// M6-B.4/M6-B.5 must still prove `can_provision_task` and the remaining physical
/// conjuncts before any authority-bearing acquisition.
///
/// Every usable agent is loaded and resolved through the authoritative binding
/// coherence read, so a `BOUND` agent whose row was lost fails the whole
/// discovery closed instead of being silently skipped. Only non-READY states are
/// excluded; they are not approximated as "cold/revivable".
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

    // Load every immediately usable agent and let the authoritative marker/row
    // coherence read decide: an inner join on the binding table would silently
    // drop a `BOUND` agent whose row is corrupt before the coherence check runs,
    // turning durable corruption into a weaker candidate-enumeration bypass.
    let rows: Vec<(String, String, String, Option<String>, Option<f64>, f64)> = {
        let mut statement = tx
            .prepare(
                "SELECT la.id, la.partition_name, la.tags_json, la.workstream_id,
                        la.available_since, la.created_at
                 FROM logical_agents la
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

// =============================================================================
// TaskSecurityBinding (M6-B.5, schema v9)
// =============================================================================

/// Recompute the positive access floor a Task requirement imposes.
///
/// The floor is derived only from the durable requirement through the validated
/// capability catalog, so the binding read can recompute it instead of trusting
/// the stored digest. B5.1 has no path-level obligation (ADR-0013 §2).
fn required_access_for(
    requirement: &TaskAgentRequirement,
    catalog: &CapabilityCatalog,
) -> Result<RequiredAccess, Error> {
    let mut operations = BTreeSet::new();
    for (reference, value) in &requirement.hard.required_capabilities {
        let definition = catalog.get(reference).ok_or_else(|| {
            Error::invariant(format!(
                "required capability {} is missing from the capability catalog",
                reference.capability_id().as_str()
            ))
        })?;
        operations.extend(project_capability_access(definition, value).map_err(contract_fault)?);
    }
    RequiredAccess::from_operations(operations).map_err(contract_fault)
}

/// The `R_min` components admission deliberately defers to candidate matching
/// (ADR-0013 §5).
///
/// The set is a pure function of the durable requirement so a read can recompute
/// it and reject a self-consistent but wrong stored obligation set.
fn pending_obligations_for(requirement: &TaskAgentRequirement) -> Vec<PendingObligation> {
    let mut pending = Vec::new();
    if !requirement.hard.required_capabilities.is_empty() {
        pending.push(PendingObligation::RequiredCapabilities);
    }
    if requirement.hard.required_attempt_isolation {
        pending.push(PendingObligation::AttemptIsolation);
    }
    let placement = !requirement.hard.required_affinity.is_empty()
        || requirement.hard.required_continuity != ContinuityMode::None
        || requirement.hard.required_anchor.is_some();
    if placement {
        pending.push(PendingObligation::PlacementConstraints);
    }
    pending
}

fn pending_obligations_json(pending: &BTreeSet<PendingObligation>) -> String {
    let encoded: Vec<&str> = pending.iter().map(|item| item.as_str()).collect();
    serde_json::to_string(&encoded).expect("a string array always serializes")
}

fn pending_obligations_from_json(json: &str) -> Result<BTreeSet<PendingObligation>, Error> {
    let stored: Vec<String> = serde_json::from_str(json)
        .map_err(|error| Error::invariant(format!("task security binding obligations: {error}")))?;
    let mut pending = BTreeSet::new();
    for item in stored {
        let obligation = PendingObligation::parse(&item).ok_or_else(|| {
            Error::invariant(format!(
                "task security binding records unknown obligation {item:?}"
            ))
        })?;
        pending.insert(obligation);
    }
    // The stored encoding MUST be the canonical one: a duplicate, reordered or
    // non-canonical spelling is corruption even if it decodes to the same set.
    if pending_obligations_json(&pending) != json {
        return Err(Error::invariant(
            "task security binding obligations are not the canonical encoding",
        ));
    }
    Ok(pending)
}

/// Derive the exact `TaskSecurityBinding` for a `SECURITY_V1` Task.
///
/// The effective policy is `meet(P_generation, P_task)` over the authored
/// catalog revisions (ADR-0013 §3, §4). Both authored inputs are recorded
/// independently, so a Task policy that differs from the Generation policy is a
/// legal narrowing rather than the frozen exact-reference conflict. A Task that
/// authored no policy contributes no additional fine layer (it is **not** an
/// explicit top, which would erase credential grants), and the binding remains
/// `P_generation`.
///
/// Admission then proves `RequiredAccess ⊆ ProjectAccess(P_eff)`; every
/// deferred component is recorded as pending rather than assumed satisfied.
pub fn prepare_task_security_binding(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    requirement: &TaskAgentRequirement,
) -> Result<TaskSecurityBinding, Error> {
    let resolved = resolve_effective_task_policy(tx, generation_id, requirement)?;

    // ADR-0013 §3: re-prove the Task-local consistency on the authored inputs,
    // not on a normalized meet where `None` and an explicit policy could become
    // indistinguishable. A conflict is rejected even when a stricter Generation
    // ceiling would hide it in `P_eff`.
    validate_task_local_coarse_fine(
        requirement.hard.required_workspace,
        requirement.hard.required_network,
        resolved.task_authored.as_ref(),
    )
    .map_err(|error| {
        Error::invalid_authority(format!(
            "task coarse fields contradict its authored sandbox policy: {error}"
        ))
    })?;

    let mut inputs = vec![PolicyInput::authored(
        resolved.generation_reference.clone(),
        &resolved.generation_authored,
    )];
    if let Some(reference) = requirement.hard.sandbox_policy.as_ref() {
        if reference != &resolved.generation_reference {
            let authored = resolved
                .task_authored
                .as_ref()
                .expect("an authored reference always has a loaded policy");
            inputs.push(PolicyInput::authored(reference.clone(), authored));
        }
    }
    let effective = resolved.effective;

    // A legal meet can still erase a positive coarse requirement that both
    // authored layers satisfied separately. Re-prove the Task's execution modes
    // on the admitted policy before the binding becomes durable.
    validate_effective_task_execution_modes(
        requirement.hard.required_workspace,
        requirement.hard.required_network,
        &effective,
    )
    .map_err(|error| {
        Error::invalid_authority(format!(
            "SECURITY_V1 task coarse requirement is not satisfied by its admitted policy: {error}"
        ))
    })?;

    let catalog = load_capability_catalog(tx)?;
    let required_access = required_access_for(requirement, &catalog)?;
    check_required_access(&required_access, &PolicyAccess::from_policy(&effective)).map_err(
        |deficit| {
            Error::invalid_authority(format!(
                "SECURITY_V1 task required access is not satisfied by its admitted policy: {deficit}"
            ))
        },
    )?;

    let derivation =
        SecurityDerivation::new(inputs, &effective, &required_access).map_err(contract_fault)?;
    TaskSecurityBinding::new(effective, derivation, pending_obligations_for(requirement))
        .map_err(contract_fault)
}

/// Persist the write-once security binding of a `SECURITY_V1` Task inside the
/// same admission transaction that writes the Task and its requirement.
pub fn insert_task_security_binding(
    tx: &Transaction<'_>,
    now: UnixTime,
    task_id: &TaskId,
    requirement: &TaskAgentRequirement,
    binding: &TaskSecurityBinding,
) -> Result<(), Error> {
    let policy = binding.policy();
    let derivation = binding.derivation();
    derivation.ensure_supported().map_err(contract_fault)?;

    let policy_json = canonical_utf8(
        canonical_sandbox_policy_bytes(policy),
        "task security binding policy",
    )?;
    let policy_digest = sandbox_policy_content_digest(policy);
    if derivation.output_digest() != policy_digest {
        return Err(Error::invariant(
            "task security derivation does not describe its admitted policy",
        ));
    }
    let derivation_json = canonical_utf8(
        canonical_security_derivation_bytes(derivation),
        "task security derivation",
    )?;
    let derivation_digest = security_derivation_content_digest(derivation);
    let pending_json = pending_obligations_json(binding.pending_obligations());

    // Recompute the floor from the durable requirement rather than trusting a
    // caller-supplied digest, so the mirrored digest and the derivation identity
    // can never disagree about what admission checked.
    let catalog = load_capability_catalog(tx)?;
    let required_access = required_access_for(requirement, &catalog)?;
    let required_access_digest = required_access_content_digest(&required_access);
    if required_access_digest != derivation.required_access_digest() {
        return Err(Error::invariant(
            "task security derivation required-access digest does not match the requirement",
        ));
    }

    tx.execute(
        "INSERT INTO task_security_bindings(
             task_id, policy_json, policy_digest, derivation_json, derivation_digest,
             normalization_version, meet_algorithm_version, required_access_digest,
             pending_obligations_json, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            task_id.as_str(),
            policy_json,
            policy_digest,
            derivation_json,
            derivation_digest,
            derivation.normalization_version() as i64,
            derivation.meet_algorithm_version() as i64,
            required_access_digest,
            pending_json,
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// The single authoritative read of a Task's security binding.
///
/// A `COARSE_V1` or legacy Task has none (its absence is already proven by
/// `get_task_agent_requirement`). A `SECURITY_V1` binding is re-derived from the
/// requirement, the Generation policy and the validated catalog, and the stored
/// rows MUST be byte-identical to that derivation.
pub fn get_task_security_binding(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<Option<TaskSecurityBinding>, Error> {
    let Some(requirement) = get_task_agent_requirement(tx, task_id)? else {
        return Ok(None);
    };
    if task_security_mode(tx, task_id)? == SecurityContractMode::CoarseV1 {
        return Ok(None);
    }

    let generation_id: String = query_opt(
        tx,
        "SELECT generation_id FROM generation_task_bindings WHERE task_id=?1",
        params![task_id.as_str()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::invariant("SECURITY_V1 task is missing its GenerationTaskBinding"))?;

    let row = query_opt(
        tx,
        "SELECT policy_json, policy_digest, derivation_json, derivation_digest,
                normalization_version, meet_algorithm_version, required_access_digest,
                pending_obligations_json
         FROM task_security_bindings WHERE task_id=?1",
        params![task_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        },
    )?
    .ok_or_else(|| {
        Error::invariant(format!(
            "SECURITY_V1 task {} is missing its security binding",
            task_id.as_str()
        ))
    })?;
    let (
        policy_json,
        policy_digest,
        derivation_json,
        derivation_digest,
        normalization_version,
        meet_algorithm_version,
        required_access_digest,
        pending_json,
    ) = row;

    if content_digest(policy_json.as_bytes()) != policy_digest {
        return Err(Error::invariant(
            "task security binding policy digest does not match its stored bytes",
        ));
    }
    if content_digest(derivation_json.as_bytes()) != derivation_digest {
        return Err(Error::invariant(
            "task security derivation digest does not match its stored bytes",
        ));
    }

    let policy = sandbox_policy_from_canonical_json(&policy_json).map_err(contract_fault)?;
    if canonical_utf8(
        canonical_sandbox_policy_bytes(&policy),
        "task security binding policy",
    )? != policy_json
    {
        return Err(Error::invariant(
            "task security binding policy is not the canonical encoding",
        ));
    }
    let derivation =
        security_derivation_from_canonical_json(&derivation_json).map_err(contract_fault)?;
    derivation.ensure_supported().map_err(contract_fault)?;
    if derivation.normalization_version() as i64 != normalization_version
        || derivation.meet_algorithm_version() as i64 != meet_algorithm_version
    {
        return Err(Error::invariant(
            "task security derivation version mirrors do not match its canonical document",
        ));
    }
    if canonical_utf8(
        canonical_security_derivation_bytes(&derivation),
        "task security derivation",
    )? != derivation_json
    {
        return Err(Error::invariant(
            "task security derivation is not the canonical encoding",
        ));
    }
    if derivation.required_access_digest() != required_access_digest {
        return Err(Error::invariant(
            "task security derivation required-access digest mirror does not match",
        ));
    }

    let pending = pending_obligations_from_json(&pending_json)?;

    // Re-derive the whole commitment from durable authority. A binding that is
    // internally consistent but describes a policy/floor the requirement does
    // not imply is corruption, not a valid past commitment.
    let expected =
        prepare_task_security_binding(tx, &GenerationId::from_string(generation_id), &requirement)?;
    let expected_policy_json = canonical_utf8(
        canonical_sandbox_policy_bytes(expected.policy()),
        "task security binding policy",
    )?;
    let expected_derivation_json = canonical_utf8(
        canonical_security_derivation_bytes(expected.derivation()),
        "task security derivation",
    )?;
    if policy_json != expected_policy_json
        || derivation_json != expected_derivation_json
        || pending_json != pending_obligations_json(expected.pending_obligations())
    {
        return Err(Error::invariant(
            "task security binding does not match its derived requirement",
        ));
    }

    TaskSecurityBinding::new(policy, derivation, pending)
        .map(Some)
        .map_err(contract_fault)
}
