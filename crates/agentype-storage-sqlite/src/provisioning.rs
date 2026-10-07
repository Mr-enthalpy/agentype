//! M6-B.4 persistence for provisioning bindings and binding snapshots
//! (schema v8).
//!
//! A `ProvisioningBinding` is immutable and write-once, scoped to one
//! Incarnation. A `BindingSnapshot` is immutable, one per Execution, and is
//! created in the same transaction as the Execution. Both store the canonical
//! M6-B.4 document plus its Core-computed digest; the parent presence markers
//! (`incarnations.provisioning_mode`, `executions.binding_snapshot_mode`) are
//! cross-checked so a missing child row is corruption, never a silent downgrade.
//!
//! This module performs no external I/O and never mints enforcement evidence.

use crate::store::{map_sqlite, query_opt};
#[cfg(any(test, feature = "runtime-internal"))]
use crate::txutil::{birth_agent, required_agent, required_partition, required_task};
use agentype_agent_contract::{
    binding_snapshot_from_canonical_json, canonical_binding_snapshot_bytes,
    canonical_provisioning_binding_bytes, content_digest, credential_refs_digest,
    provisioning_binding_from_canonical_json, AdapterPolicyRef, BindingSnapshot, ConfigStatus,
    MaterializationDigest, ProvisioningBinding, ResolvedProvisioningEvidence, SourceConfigRef,
    SourceStatus, SpawnSourceRef,
};
use agentype_core::{Claim, Error, ExecutionId, IncarnationId, TaskId, UnixTime};
#[cfg(any(test, feature = "runtime-internal"))]
use agentype_core::{ContinuityPreference, LogicalAgentId};
use rusqlite::{params, Transaction};
#[cfg(any(test, feature = "runtime-internal"))]
use std::collections::BTreeSet;
#[cfg(any(test, feature = "runtime-internal"))]
use uuid::Uuid;

fn canonical_utf8(bytes: Vec<u8>, what: &str) -> Result<String, Error> {
    String::from_utf8(bytes)
        .map_err(|_| Error::invariant(format!("{what} canonical bytes are not UTF-8")))
}

fn contract_fault(error: agentype_agent_contract::ContractError) -> Error {
    Error::invariant(error.to_string())
}

/// Mark an Incarnation as carrying a provisioning binding before the binding
/// row is inserted. The schema's `provisioning_bindings_require_provisioned_parent`
/// trigger rejects a binding whose parent marker is not yet `PROVISIONED`.
pub fn mark_incarnation_provisioned(
    tx: &Transaction<'_>,
    incarnation_id: &IncarnationId,
) -> Result<(), Error> {
    let changed = tx
        .execute(
            "UPDATE incarnations SET provisioning_mode='PROVISIONED' WHERE id=?1",
            params![incarnation_id.as_str()],
        )
        .map_err(map_sqlite)?;
    if changed == 0 {
        return Err(Error::not_found(format!(
            "incarnation {incarnation_id} not found"
        )));
    }
    Ok(())
}

pub fn incarnation_provisioning_mode(
    tx: &Transaction<'_>,
    incarnation_id: &IncarnationId,
) -> Result<String, Error> {
    query_opt(
        tx,
        "SELECT provisioning_mode FROM incarnations WHERE id=?1",
        params![incarnation_id.as_str()],
        |row| row.get(0),
    )?
    .ok_or_else(|| Error::not_found(format!("incarnation {incarnation_id} not found")))
}

/// Insert an immutable provisioning binding. The parent Incarnation MUST already
/// be marked `PROVISIONED` in this transaction; the schema trigger enforces it.
/// Returns the record content digest.
pub fn insert_provisioning_binding(
    tx: &Transaction<'_>,
    now: UnixTime,
    binding: &ProvisioningBinding,
) -> Result<String, Error> {
    binding.validate().map_err(contract_fault)?;
    let json = canonical_utf8(
        canonical_provisioning_binding_bytes(binding),
        "provisioning binding",
    )?;
    let digest = content_digest(json.as_bytes());
    tx.execute(
        "INSERT INTO provisioning_bindings(
            provisioning_binding_id,logical_agent_id,incarnation_id,
            agent_type_id,agent_type_revision,record_json,record_digest,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            binding.provisioning_binding_id,
            binding.logical_agent_id.as_str(),
            binding.incarnation_id.as_str(),
            binding.agent_type.id().as_str(),
            binding.agent_type.revision(),
            json,
            digest,
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

pub fn get_provisioning_binding(
    tx: &Transaction<'_>,
    incarnation_id: &IncarnationId,
) -> Result<Option<ProvisioningBinding>, Error> {
    // Parent presence marker and child row are one invariant. Read the marker
    // first, then classify the four combinations: a PROVISIONED incarnation
    // without its row is corruption (never `None`), and a LEGACY incarnation
    // with a row is corruption (never a silent downgrade), mirroring the B.3
    // requirement/policy/binding reads.
    let mode = incarnation_provisioning_mode(tx, incarnation_id)?;
    let row = query_opt(
        tx,
        "SELECT record_json, record_digest, provisioning_binding_id, logical_agent_id,
                incarnation_id, agent_type_id, agent_type_revision
         FROM provisioning_bindings WHERE incarnation_id=?1",
        params![incarnation_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        },
    )?;
    let (json, digest, id, logical_agent, incarnation, agent_type_id, agent_type_revision) =
        match (mode.as_str(), row) {
            ("LEGACY", None) => return Ok(None),
            ("LEGACY", Some(_)) => {
                return Err(Error::invariant(
                    "provisioning binding exists under a non-PROVISIONED incarnation",
                ))
            }
            ("PROVISIONED", None) => {
                return Err(Error::invariant(
                    "a PROVISIONED incarnation is missing its provisioning binding",
                ))
            }
            ("PROVISIONED", Some(row)) => row,
            (other, _) => {
                return Err(Error::invariant(format!(
                    "unknown incarnation provisioning mode {other}"
                )))
            }
        };
    verify_binding_document(&json, &digest, "provisioning binding")?;
    let binding = provisioning_binding_from_canonical_json(&json).map_err(contract_fault)?;
    let reencoded = canonical_utf8(
        canonical_provisioning_binding_bytes(&binding),
        "provisioning binding",
    )?;
    if reencoded != json {
        return Err(Error::invariant(
            "provisioning binding row is not the canonical encoding of its record",
        ));
    }
    if binding.provisioning_binding_id != id
        || binding.logical_agent_id.as_str() != logical_agent
        || binding.incarnation_id.as_str() != incarnation
        || binding.agent_type.id().as_str() != agent_type_id
        || binding.agent_type.revision() != agent_type_revision as u64
    {
        return Err(Error::invariant(
            "provisioning binding relational columns disagree with its record",
        ));
    }
    // Cross-record authority proof: the same check the acquisition path uses, so
    // a self-consistent but cross-record-incoherent row fails the read closed.
    validate_provisioning_binding_authority(tx, &binding)?;
    Ok(Some(binding))
}

/// Insert an immutable binding snapshot. The parent Execution MUST already carry
/// `binding_snapshot_mode='SNAPSHOT'` in this transaction; the schema trigger
/// enforces it. Returns the record content digest.
pub fn insert_binding_snapshot(
    tx: &Transaction<'_>,
    now: UnixTime,
    snapshot: &BindingSnapshot,
) -> Result<String, Error> {
    snapshot.validate().map_err(contract_fault)?;
    let json = canonical_utf8(
        canonical_binding_snapshot_bytes(snapshot),
        "binding snapshot",
    )?;
    let digest = content_digest(json.as_bytes());
    tx.execute(
        "INSERT INTO binding_snapshots(
            snapshot_id,execution_id,provisioning_binding_id,adapter_kind,adapter_binding_key,
            execution_target,execution_profile,record_json,record_digest,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            snapshot.snapshot_id,
            snapshot.execution_id.as_str(),
            snapshot.provisioning_binding_id,
            snapshot.adapter_kind,
            snapshot.adapter_binding_key,
            snapshot.execution_target,
            snapshot.execution_profile,
            json,
            digest,
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

pub fn get_binding_snapshot(
    tx: &Transaction<'_>,
    execution_id: &ExecutionId,
) -> Result<Option<BindingSnapshot>, Error> {
    let row = query_opt(
        tx,
        "SELECT record_json, record_digest, snapshot_id, execution_id, provisioning_binding_id,
                adapter_kind, adapter_binding_key, execution_target, execution_profile
         FROM binding_snapshots WHERE execution_id=?1",
        params![execution_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        },
    )?;
    // Parent marker and child row are one invariant: a SNAPSHOT execution
    // without its row is corruption (never `None`), and a NONE execution with a
    // row is corruption (never a silent downgrade).
    let mode = execution_binding_snapshot_mode(tx, execution_id)?;
    let (
        json,
        digest,
        snapshot_id,
        execution,
        provisioning_binding,
        adapter_kind,
        adapter_binding_key,
        execution_target,
        execution_profile,
    ) = match (mode.as_str(), row) {
        ("NONE", None) => return Ok(None),
        ("NONE", Some(_)) => {
            return Err(Error::invariant(
                "binding snapshot exists under a non-SNAPSHOT execution",
            ))
        }
        ("SNAPSHOT", None) => {
            return Err(Error::invariant(
                "a SNAPSHOT execution is missing its binding snapshot",
            ))
        }
        ("SNAPSHOT", Some(row)) => row,
        (other, _) => {
            return Err(Error::invariant(format!(
                "unknown execution binding snapshot mode {other}"
            )))
        }
    };
    verify_binding_document(&json, &digest, "binding snapshot")?;
    let snapshot = binding_snapshot_from_canonical_json(&json).map_err(contract_fault)?;
    let reencoded = canonical_utf8(
        canonical_binding_snapshot_bytes(&snapshot),
        "binding snapshot",
    )?;
    if reencoded != json {
        return Err(Error::invariant(
            "binding snapshot row is not the canonical encoding of its record",
        ));
    }
    if snapshot.snapshot_id != snapshot_id
        || snapshot.execution_id.as_str() != execution
        || snapshot.provisioning_binding_id != provisioning_binding
        || snapshot.adapter_kind != adapter_kind
        || snapshot.adapter_binding_key != adapter_binding_key
        || snapshot.execution_target != execution_target
        || snapshot.execution_profile != execution_profile
    {
        return Err(Error::invariant(
            "binding snapshot relational columns disagree with its record",
        ));
    }
    // Full cross-record authority proof (shared with the commitment path).
    validate_binding_snapshot_authority(tx, &snapshot)?;
    Ok(Some(snapshot))
}

pub fn execution_binding_snapshot_mode(
    tx: &Transaction<'_>,
    execution_id: &ExecutionId,
) -> Result<String, Error> {
    query_opt(
        tx,
        "SELECT binding_snapshot_mode FROM executions WHERE id=?1",
        params![execution_id.as_str()],
        |row| row.get(0),
    )?
    .ok_or_else(|| Error::not_found(format!("execution {execution_id} not found")))
}

fn verify_binding_document(json: &str, digest: &str, what: &str) -> Result<(), Error> {
    if content_digest(json.as_bytes()) != digest {
        return Err(Error::invariant(format!("{what} content digest mismatch")));
    }
    Ok(())
}

/// Cross-record authority proof for a `ProvisioningBinding`.
///
/// Used by BOTH the acquisition/creation path and the authoritative read, so a
/// self-consistent but cross-record-incoherent durable row cannot be accepted.
/// The binding's agent must be its Incarnation's agent; the frozen catalogue
/// provenance must resolve through the validated reads and agree with the
/// binding (source→policy, config→source, policy kind).
pub(crate) fn validate_provisioning_binding_authority(
    tx: &Transaction<'_>,
    binding: &ProvisioningBinding,
) -> Result<(), Error> {
    let incarnation_agent: Option<String> = query_opt(
        tx,
        "SELECT logical_agent_id FROM incarnations WHERE id=?1",
        params![binding.incarnation_id.as_str()],
        |row| row.get(0),
    )?;
    if incarnation_agent.as_deref() != Some(binding.logical_agent_id.as_str()) {
        return Err(Error::invariant(
            "provisioning binding logical agent disagrees with its incarnation",
        ));
    }
    let source =
        crate::catalog::load_spawn_source(tx, &binding.spawn_source)?.ok_or_else(|| {
            Error::invariant("provisioning binding references a missing spawn source")
        })?;
    if source.adapter_policy != binding.adapter_policy {
        return Err(Error::invariant(
            "provisioning binding adapter policy disagrees with its spawn source",
        ));
    }
    let config_revision = crate::catalog::load_source_config_revision(tx, &binding.source_config)?
        .ok_or_else(|| {
            Error::invariant("provisioning binding references a missing source config")
        })?;
    if config_revision.config().config_ref.source() != &binding.spawn_source {
        return Err(Error::invariant(
            "provisioning binding source config does not belong to its spawn source",
        ));
    }
    let policy = crate::catalog::get_adapter_binding_policy(tx, &binding.adapter_policy)?
        .ok_or_else(|| {
            Error::invariant("provisioning binding references a missing adapter policy")
        })?;
    if policy.adapter_kind != binding.adapter_kind {
        return Err(Error::invariant(
            "provisioning binding adapter kind disagrees with its adapter policy",
        ));
    }
    // The frozen enforceability must satisfy the policy's required_safety.
    if !safety_satisfies(&policy.required_safety, &binding.effective_security) {
        return Err(Error::invariant(
            "provisioning binding enforceability does not satisfy its adapter policy required_safety",
        ));
    }
    // The binding's agent type MUST be the LogicalAgent's frozen semantic type
    // binding, not merely an existing revision.
    let logical_type =
        crate::requirement::get_logical_agent_type_binding(tx, &binding.logical_agent_id)?
            .ok_or_else(|| Error::invariant("provisioning binding agent has no type binding"))?;
    if logical_type != binding.agent_type {
        return Err(Error::invariant(
            "provisioning binding agent type disagrees with the LogicalAgent type binding",
        ));
    }
    crate::catalog::get_agent_type(tx, &binding.agent_type)?
        .ok_or_else(|| Error::invariant("provisioning binding references a missing agent type"))?;
    Ok(())
}

/// Cross-record authority proof for a `BindingSnapshot`.
///
/// Used by BOTH the execution-commitment path and the authoritative read: the
/// snapshot must agree with its Execution's frozen physical choice, own that
/// Execution's Incarnation `ProvisioningBinding`, and carry semantic/security
/// fields reconstructed from durable authority (the admitted requirement, the
/// binding, and the validated config credential digest).
pub(crate) fn validate_binding_snapshot_authority(
    tx: &Transaction<'_>,
    snapshot: &BindingSnapshot,
) -> Result<(), Error> {
    let execution = query_opt(
        tx,
        "SELECT incarnation_id, task_id, adapter_kind, adapter_binding_key, execution_target,
                execution_profile, attempt_isolation
         FROM executions WHERE id=?1",
        params![snapshot.execution_id.as_str()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("execution {} not found", snapshot.execution_id)))?;
    if execution.2 != snapshot.adapter_kind
        || execution.3 != snapshot.adapter_binding_key
        || execution.4 != snapshot.execution_target
        || execution.5 != snapshot.execution_profile
    {
        return Err(Error::invariant(
            "binding snapshot disagrees with its execution's frozen physical choice",
        ));
    }
    // The snapshot's effective isolation MUST equal the authoritative launch
    // isolation the Execution actually froze, so B.4 can never claim a stricter
    // (or looser) isolation than the physical launch provides.
    if snapshot.effective_isolation != (execution.6 != 0) {
        return Err(Error::invariant(
            "binding snapshot effective isolation disagrees with the execution",
        ));
    }
    let incarnation = IncarnationId::from_string(execution.0);
    let binding = get_provisioning_binding(tx, &incarnation)?.ok_or_else(|| {
        Error::invariant("binding snapshot's incarnation has no provisioning binding")
    })?;
    if binding.provisioning_binding_id != snapshot.provisioning_binding_id
        || binding.spawn_source != snapshot.spawn_source
        || binding.source_config != snapshot.source_config
        || binding.adapter_kind != snapshot.adapter_kind
        || binding.adapter_binding_key != snapshot.adapter_binding_key
        || binding.attested_materialization_digest != snapshot.attested_materialization_digest
        // The snapshot's enforceability capability is the binding's imported
        // capability; the per-execution effective policy is separate.
        || binding.effective_security != snapshot.enforceable_security
    {
        return Err(Error::invariant(
            "binding snapshot disagrees with its provisioning binding",
        ));
    }
    // The effective workspace/network the execution requests must be
    // enforceable by the binding's frozen capability.
    if !binding
        .effective_security
        .enforces_workspace(snapshot.effective_workspace)
        || !binding
            .effective_security
            .enforces_network(snapshot.effective_network)
    {
        return Err(Error::invariant(
            "binding snapshot effective policy is not enforceable by the binding capability",
        ));
    }
    let revision = crate::catalog::load_source_config_revision(tx, &snapshot.source_config)?
        .ok_or_else(|| Error::invariant("binding snapshot references a missing source config"))?;
    if revision.config().config_digest != snapshot.source_config_digest {
        return Err(Error::invariant(
            "binding snapshot source config digest disagrees with the validated revision",
        ));
    }
    let requirement =
        crate::requirement::get_task_agent_requirement(tx, &TaskId::from_string(execution.1))?
            .ok_or_else(|| Error::invariant("typed execution without its agent requirement"))?;
    if snapshot.required_capabilities != requirement.hard.required_capabilities {
        return Err(Error::invariant(
            "binding snapshot required_capabilities disagree with the authoritative requirement",
        ));
    }
    if snapshot.effective_workspace != requirement.hard.required_workspace
        || snapshot.effective_network != requirement.hard.required_network
    {
        return Err(Error::invariant(
            "binding snapshot effective workspace/network disagree with the authoritative requirement",
        ));
    }
    // Re-prove the effective isolation requirement at the second authority
    // boundary: the frozen effective isolation is the OR of the bound AgentType
    // and the Task requirement, and it must actually be enforced by the frozen
    // Execution. The snapshot's self-consistency with the Execution is not
    // enough; a `unisolated` safety that lowers an admitted hard security
    // requirement must fail here.
    let (agent_type, _status) = crate::catalog::get_agent_type(tx, &binding.agent_type)?
        .ok_or_else(|| Error::invariant("binding snapshot's agent type is missing"))?;
    let required_isolation = agent_type.contract.security.requires_attempt_isolation
        || requirement.hard.required_attempt_isolation;
    if required_isolation && execution.6 == 0 {
        return Err(Error::invariant(
            "the frozen execution does not enforce the effective attempt isolation requirement",
        ));
    }
    if snapshot.credential_refs_digest != credential_refs_digest(&revision.config().credential_refs)
    {
        return Err(Error::invariant(
            "binding snapshot credential_refs_digest disagrees with the validated config",
        ));
    }
    // The resolver version is audit metadata describing which resolver
    // semantics produced this immutable historical commitment, not an equality
    // with the current binary's producer constant. All versions in the
    // `agentype-resolver/` family are readable; a version outside the family is
    // unsupported durable state.
    if !snapshot
        .resolver_version
        .starts_with(agentype_agent_contract::RESOLVER_VERSION_FAMILY)
    {
        return Err(Error::invariant(
            "binding snapshot resolver version is not a supported resolver family",
        ));
    }
    Ok(())
}

/// Active `SpawnSource` revision refs, for M6-B.4 candidate enumeration.
///
/// Every revision row is loaded through the validated catalog read before the
/// disposition is consulted, so a revision whose overlay is missing fails the
/// whole enumeration closed as corruption instead of silently disappearing (the
/// B.2 "missing overlay is corruption, never not-found" rule). A non-active
/// revision is simply skipped.
pub fn list_active_spawn_source_refs(tx: &Transaction<'_>) -> Result<Vec<SpawnSourceRef>, Error> {
    let mut stmt = tx
        .prepare("SELECT source_id, revision FROM spawn_sources ORDER BY source_id, revision")
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64))
        })
        .map_err(map_sqlite)?;
    let mut refs = Vec::new();
    for row in rows {
        let (source_id, revision) = row.map_err(map_sqlite)?;
        refs.push(SpawnSourceRef::new(source_id, revision).map_err(contract_fault)?);
    }
    let mut out = Vec::new();
    for reference in refs {
        // Validated read: a missing disposition overlay fails closed.
        let source = crate::catalog::load_spawn_source(tx, &reference)?
            .ok_or_else(|| Error::invariant("spawn source revision vanished during enumeration"))?;
        if source.status == SourceStatus::Active {
            out.push(reference);
        }
    }
    Ok(out)
}

/// Active `SourceConfig` revision refs for one exact source revision, each
/// loaded through the validated read so a missing overlay fails closed.
pub fn list_active_source_config_refs(
    tx: &Transaction<'_>,
    source: &SpawnSourceRef,
) -> Result<Vec<SourceConfigRef>, Error> {
    let mut stmt = tx
        .prepare(
            "SELECT config_id, config_revision FROM source_configs
             WHERE source_id=?1 AND source_revision=?2
             ORDER BY config_id, config_revision",
        )
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map(params![source.id().as_str(), source.revision()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64))
        })
        .map_err(map_sqlite)?;
    let mut refs = Vec::new();
    for row in rows {
        let (config_id, revision) = row.map_err(map_sqlite)?;
        refs.push(
            SourceConfigRef::new(source.clone(), config_id, revision).map_err(contract_fault)?,
        );
    }
    let mut out = Vec::new();
    for reference in refs {
        let revision =
            crate::catalog::load_source_config_revision(tx, &reference)?.ok_or_else(|| {
                Error::invariant("source config revision vanished during enumeration")
            })?;
        if revision.config().status == ConfigStatus::Active {
            out.push(reference);
        }
    }
    Ok(out)
}

/// A deterministic fingerprint of the active source/config candidate frontier.
///
/// The resolver captures it before selecting a winner; the authority transaction
/// recomputes it and rejects a mismatch, so a concurrent publish of a new active
/// source/config cannot slip past the deterministic selection (the linearization
/// fence for spec 07 selection).
pub fn active_catalog_frontier_digest(tx: &Transaction<'_>) -> Result<String, Error> {
    use serde_json::json;
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for source in list_active_spawn_source_refs(tx)? {
        let loaded = crate::catalog::load_spawn_source(tx, &source)?
            .ok_or_else(|| Error::invariant("active spawn source vanished"))?;
        let policy = crate::catalog::get_adapter_binding_policy(tx, &loaded.adapter_policy)?
            .ok_or_else(|| Error::invariant("active source policy vanished"))?;
        entries.push(json!({
            "kind": "SOURCE",
            "source_id": source.id().as_str(),
            "revision": source.revision(),
            "policy_id": loaded.adapter_policy.id().as_str(),
            "policy_revision": loaded.adapter_policy.revision(),
            // Policy disposition affects durable eligibility (ACTIVE required).
            "policy_status": config_status_sql(policy.status),
        }));
        for config in list_active_source_config_refs(tx, &source)? {
            entries.push(json!({
                "kind": "CONFIG",
                "source_id": config.source().id().as_str(),
                "source_revision": config.source().revision(),
                "config_id": config.config_id().as_str(),
                "revision": config.revision(),
            }));
        }
    }
    let document = json!({"kind": "CATALOG_FRONTIER", "entries": entries});
    Ok(agentype_agent_contract::canonical_json_body_digest(
        &document,
    ))
}

fn config_status_sql(status: ConfigStatus) -> &'static str {
    match status {
        ConfigStatus::Active => "ACTIVE",
        ConfigStatus::Draining => "DRAINING",
        ConfigStatus::Disabled => "DISABLED",
    }
}

// =============================================================================
// Typed acquisition
// =============================================================================

/// A resolver-produced provisioning selection.
///
/// This is a plain durable-record input, not an authority token: the authority
/// transaction re-proves every mandatory conjunct over the current durable
/// catalog and the agent's **actual** bound AgentType, and binds the imported
/// evidence to the exact validated source/config revision via
/// `can_provision_task`. A caller-assembled selection that does not satisfy
/// `can_execute` / `can_provision_task` is rejected at the transaction, not
/// trusted. The exact imported binding identity the resolver proved is carried
/// here so the eventual Execution and BindingSnapshot freeze the same key.
#[derive(Clone, Debug)]
pub struct ResolvedProvisioningSelection {
    pub spawn_source: SpawnSourceRef,
    pub source_config: SourceConfigRef,
    pub adapter_policy: AdapterPolicyRef,
    pub evidence: ResolvedProvisioningEvidence,
    pub adapter_kind: String,
    pub adapter_binding_key: String,
    /// Exact source-integration protocol identity the winner was qualified
    /// against; the authority transaction stores it in the `ProvisioningBinding`.
    pub provisioning_protocol: String,
    /// Secret-free canonical **attested** digest the pure integration resolved
    /// for the exact `SourceConfig` revision and physical domain (an expected
    /// identity, not proof of materialization).
    pub attested_materialization_digest: MaterializationDigest,
    /// The active source/config frontier the resolver selected against. The
    /// authority transaction recomputes it and rejects a mismatch, so a
    /// concurrent publish cannot bypass the deterministic selection.
    pub catalog_frontier_digest: String,
}

/// The outcome of a typed acquisition: the mechanical Claim (Attempt + Lease +
/// agent assignment) plus the immutable `ProvisioningBinding` for its
/// Incarnation.
#[derive(Clone, Debug)]
pub struct TypedAcquisition {
    pub claim: Claim,
    pub provisioning_binding: ProvisioningBinding,
}

#[cfg(any(test, feature = "runtime-internal"))]
fn acquisition_rejection(error: agentype_agent_contract::ContractError) -> Error {
    Error::invalid_authority(error.to_string())
}

#[cfg(any(test, feature = "runtime-internal"))]
/// Re-check the frozen M5 placement gates (exact partition, concrete tag
/// superset, `Required` continuity workstream) over the durable agent and task.
fn placement_matches(
    task: &crate::txutil::TaskRow,
    agent: &crate::txutil::AgentRow,
) -> Result<bool, Error> {
    if agent.partition != task.partition {
        return Ok(false);
    }
    let required: BTreeSet<String> = serde_json::from_str(&task.affinity_tags_json)
        .map_err(|e| Error::invariant(format!("task affinity tags: {e}")))?;
    let actual: BTreeSet<String> = serde_json::from_str(&agent.tags_json)
        .map_err(|e| Error::invariant(format!("agent tags: {e}")))?;
    if !required.is_subset(&actual) {
        return Ok(false);
    }
    if ContinuityPreference::parse_sql(&task.continuity)? == ContinuityPreference::Required
        && agent.workstream_id != task.workstream_id
    {
        return Ok(false);
    }
    Ok(true)
}

fn safety_satisfies(
    required: &agentype_agent_contract::PhysicalSafety,
    provided: &agentype_agent_contract::PhysicalSafety,
) -> bool {
    (!required.attempt_isolation() || provided.attempt_isolation())
        && required
            .enforceable_workspace_modes()
            .iter()
            .all(|mode| provided.enforces_workspace(*mode))
        && required
            .enforceable_network_modes()
            .iter()
            .all(|policy| provided.enforces_network(*policy))
}

#[cfg(any(test, feature = "runtime-internal"))]
/// The active Incarnation of an agent on the given target, mirroring the frozen
/// M5 `ensure_incarnation` target lock. An agent fixed to a different target
/// fails closed.
fn active_incarnation_for_agent(
    tx: &Transaction<'_>,
    agent_id: &str,
    target: &str,
) -> Result<Option<String>, Error> {
    let found = query_opt(
        tx,
        "SELECT id, execution_target FROM incarnations WHERE logical_agent_id=?1
         AND state IN ('STARTING','WARM','COLD') ORDER BY generation DESC LIMIT 1",
        params![agent_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    match found {
        None => Ok(None),
        Some((id, exec_target)) if exec_target == target => Ok(Some(id)),
        Some((id, exec_target)) => Err(Error::invalid_transition(format!(
            "logical agent {agent_id} already has active incarnation {id} on target {exec_target}"
        ))),
    }
}

#[cfg(any(test, feature = "runtime-internal"))]
/// Whether a legacy Incarnation is fresh enough to be adopted in place.
///
/// M5 Executions freeze only `adapter_kind`/`adapter_binding_key`; they never
/// freeze the `SpawnSource`/`SourceConfig` a legacy Incarnation was created
/// from. Two different `SourceConfig` bodies can share one physical domain, and
/// SourceConfig is Core-opaque, so an existing physical history can NOT be
/// promoted into a specific SourceConfig provenance. Only an Incarnation with no
/// execution history at all may be adopted; one with any physical history rolls
/// over to a fresh Incarnation.
fn incarnation_is_fresh(tx: &Transaction<'_>, incarnation_id: &str) -> Result<bool, Error> {
    let any_execution: Option<i64> = query_opt(
        tx,
        "SELECT 1 FROM executions WHERE incarnation_id=?1 LIMIT 1",
        params![incarnation_id],
        |row| row.get(0),
    )?;
    Ok(any_execution.is_none())
}

#[cfg(any(test, feature = "runtime-internal"))]
/// Fence an idle Incarnation so a new provenance can be rolled over onto a fresh
/// one. An Incarnation with an active execution may not be rolled over.
fn fence_idle_incarnation(
    tx: &Transaction<'_>,
    incarnation_id: &str,
    now: UnixTime,
) -> Result<(), Error> {
    let busy: Option<i64> = query_opt(
        tx,
        "SELECT 1 FROM executions WHERE incarnation_id=?1
         AND state IN ('STARTING','RUNNING','UNKNOWN') LIMIT 1",
        params![incarnation_id],
        |row| row.get(0),
    )?;
    if busy.is_some() {
        return Err(Error::invalid_transition(
            "cannot roll over an incarnation with an active execution",
        ));
    }
    let state: String = query_opt(
        tx,
        "SELECT state FROM incarnations WHERE id=?1",
        params![incarnation_id],
        |row| row.get(0),
    )?
    .ok_or_else(|| Error::not_found(format!("incarnation {incarnation_id} not found")))?;
    if state == "STARTING" {
        return Err(Error::invalid_transition(
            "cannot roll over a starting incarnation",
        ));
    }
    tx.execute(
        "UPDATE incarnations SET state='LOST', ended_at=COALESCE(ended_at,?1) WHERE id=?2",
        params![now, incarnation_id],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

#[cfg(any(test, feature = "runtime-internal"))]
fn create_new_incarnation(
    tx: &Transaction<'_>,
    agent_id: &str,
    target: &str,
    now: UnixTime,
) -> Result<String, Error> {
    let generation: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(generation),0)+1 FROM incarnations WHERE logical_agent_id=?1",
            params![agent_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    let incarnation_id = IncarnationId::new().to_string();
    tx.execute(
        "INSERT INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
         VALUES(?1,?2,?3,?4,'STARTING',?5)",
        params![incarnation_id, agent_id, generation, target, now],
    )
    .map_err(map_sqlite)?;
    Ok(incarnation_id)
}

/// Pick the Incarnation the acquisition commits to, applying the frozen
/// lifecycle rules: reuse the active Incarnation and its `ProvisioningBinding`
/// when the provenance and exact physical domain still match; roll over to a
/// fresh Incarnation when they differ; adopt a fresh/legacy Incarnation only
/// when its prior physical hosting matches the selection.
#[cfg(any(test, feature = "runtime-internal"))]
fn resolve_incarnation(
    tx: &Transaction<'_>,
    agent: &crate::txutil::AgentRow,
    target: &str,
    agent_type: &agentype_agent_contract::AgentTypeRef,
    selection: &ResolvedProvisioningSelection,
    now: UnixTime,
) -> Result<(IncarnationId, Option<ProvisioningBinding>), Error> {
    let Some(inc_id) = active_incarnation_for_agent(tx, &agent.id, target)? else {
        let new_id = create_new_incarnation(tx, &agent.id, target, now)?;
        return Ok((IncarnationId::from_string(new_id), None));
    };
    let incarnation = IncarnationId::from_string(inc_id.clone());
    if let Some(existing) = get_provisioning_binding(tx, &incarnation)? {
        // Already-provisioned Incarnation: reuse only if the frozen provenance
        // and exact physical domain still qualify.
        if existing.qualifies(
            agent_type,
            &selection.spawn_source,
            &selection.source_config,
            &selection.adapter_policy,
            &selection.adapter_kind,
            &selection.adapter_binding_key,
            &selection.attested_materialization_digest,
            &selection.provisioning_protocol,
            selection.evidence.enforceable_safety(),
        ) {
            return Ok((incarnation, Some(existing)));
        }
        fence_idle_incarnation(tx, &inc_id, now)?;
        let new_id = create_new_incarnation(tx, &agent.id, target, now)?;
        return Ok((IncarnationId::from_string(new_id), None));
    }
    // Legacy Incarnation: a real execution history proves only the physical
    // domain, never the SourceConfig provenance, so it may be adopted only when
    // it is fresh; otherwise it rolls over to a new Incarnation.
    if incarnation_is_fresh(tx, &inc_id)? {
        Ok((incarnation, None))
    } else {
        fence_idle_incarnation(tx, &inc_id, now)?;
        let new_id = create_new_incarnation(tx, &agent.id, target, now)?;
        Ok((IncarnationId::from_string(new_id), None))
    }
}

#[cfg(any(test, feature = "runtime-internal"))]
fn finish_acquisition(
    tx: &Transaction<'_>,
    now: UnixTime,
    lease_seconds: f64,
    task_id: &TaskId,
    agent_id: &LogicalAgentId,
    selection: &ResolvedProvisioningSelection,
    execution_registry: &agentype_execution_config::ExecutionRegistry,
) -> Result<TypedAcquisition, Error> {
    let task = required_task(tx, task_id.as_str())?;
    if task.state != "QUEUED" {
        return Err(Error::invalid_transition(
            "typed acquisition requires a QUEUED task",
        ));
    }
    let mode: String = query_opt(
        tx,
        "SELECT agent_requirement_mode FROM tasks WHERE id=?1",
        params![task_id.as_str()],
        |row| row.get(0),
    )?
    .ok_or_else(|| Error::not_found(format!("task {task_id} not found")))?;
    if mode != "TYPED" {
        return Err(Error::invalid_transition(
            "typed acquisition requires a TYPED task",
        ));
    }
    let requirement = crate::requirement::get_task_agent_requirement(tx, task_id)?
        .ok_or_else(|| Error::invariant("a TYPED task is missing its agent requirement"))?;

    let agent = required_agent(tx, agent_id.as_str())?;
    if agent.state != "READY" || agent.current_task_id.is_some() {
        return Err(Error::invalid_transition(
            "typed acquisition requires a READY, unassigned agent",
        ));
    }
    // The agent's actual semantic identity, not the Task pin, is the contract
    // that must be executable: B.3 preselection deliberately admits a
    // refinement, broader, or otherwise compatible bound type. A different
    // revision of the same type_id stays excluded (D-TYPE-REV-COMPAT deferred).
    let bound =
        crate::requirement::get_logical_agent_type_binding(tx, agent_id)?.ok_or_else(|| {
            Error::invalid_transition("typed acquisition requires a type-bound agent")
        })?;
    if bound.id() == requirement.required_type.id()
        && bound.revision() != requirement.required_type.revision()
    {
        return Err(Error::invalid_authority(
            "a different revision of the pinned type is not a substitute",
        ));
    }
    if !placement_matches(&task, &agent)? {
        return Err(Error::invalid_authority(
            "agent is not M5-placeable for this task",
        ));
    }
    let (actual, _status) = crate::catalog::get_agent_type(tx, &bound)?
        .ok_or_else(|| Error::invariant("agent type binding references a missing revision"))?;
    let catalog = crate::catalog::load_capability_catalog(tx)?;
    agentype_agent_contract::can_execute(&actual, &requirement.hard, &catalog)
        .map_err(acquisition_rejection)?;

    // Selection linearization fence: the active candidate frontier must be
    // unchanged since the resolver picked the winner, so a concurrent publish
    // of a new active source/config cannot bypass the deterministic selection.
    if active_catalog_frontier_digest(tx)? != selection.catalog_frontier_digest {
        // A global authority-snapshot invalidation, not a per-candidate
        // ineligibility: the whole acquisition must be re-resolved from the
        // top of the B.3 ranking, never fall through to a lower-ranked agent.
        return Err(Error::stale(
            "the active source/config frontier changed during selection; re-resolve the whole acquisition",
        ));
    }

    // Re-load the exact validated source/config/policy revisions and re-prove
    // every mandatory conjunct, so a caller-assembled selection or a catalog
    // mutation between resolution and commitment cannot bypass them.
    let source = crate::catalog::load_spawn_source(tx, &selection.spawn_source)?
        .ok_or_else(|| Error::invariant("selected spawn source revision is missing"))?;
    if source.adapter_policy != selection.adapter_policy {
        return Err(Error::invalid_authority(
            "selected adapter policy is not the source's policy",
        ));
    }
    let policy = crate::catalog::get_adapter_binding_policy(tx, &source.adapter_policy)?
        .ok_or_else(|| Error::invariant("source adapter policy is missing"))?;
    if policy.status != ConfigStatus::Active {
        return Err(Error::invalid_authority(
            "the adapter binding policy is not active",
        ));
    }
    if policy.adapter_kind != selection.adapter_kind {
        return Err(Error::invalid_authority(
            "selection adapter kind does not match the active policy",
        ));
    }
    // The imported evidence subject must bind the same exact physical domain the
    // selection commits to, so an enforceability proof from another installed
    // domain (K1) can never authorize the selected domain (K2).
    if selection.evidence.adapter_kind() != policy.adapter_kind
        || selection.evidence.adapter_kind() != selection.adapter_kind
        || selection.evidence.adapter_binding_key() != selection.adapter_binding_key
    {
        return Err(Error::invalid_authority(
            "imported evidence exact physical domain does not match the selected binding",
        ));
    }
    if !safety_satisfies(
        &policy.required_safety,
        selection.evidence.enforceable_safety(),
    ) {
        return Err(Error::invalid_authority(
            "imported enforceability does not satisfy the policy required_safety",
        ));
    }
    let config_revision =
        crate::catalog::load_source_config_revision(tx, &selection.source_config)?
            .ok_or_else(|| Error::invariant("selected source config revision is missing"))?;
    let config = config_revision.config();
    // B.3 entry boundary: credential availability is a pre-authority conjunct.
    // M6-B.4 has no trusted, source-bound availability authority, and a caller
    // must never mint that authority, so any config that declares credential
    // refs fails closed here. Trusted resolution/availability/brokering is B.5.
    if !config.credential_refs.is_empty() {
        return Err(Error::invalid_authority(
            "credential availability is B.5; B.4 fails closed for credential-bearing configs",
        ));
    }
    // The selected SourceConfig carries a source-integration materialization
    // attestation for the exact physical domain (`MaterializationDigest` already
    // enforces the canonical `sha256:<64 hex>` grammar at construction); a config
    // identity/digest alone is not a materialized environment.
    agentype_agent_contract::can_provision_task(
        &actual,
        &source,
        config,
        &selection.evidence,
        &catalog,
        &requirement.hard,
    )
    .map_err(acquisition_rejection)?;

    // The partition target/profile must be a valid authoritative environment and
    // the target must agree with the policy on the adapter kind before any
    // Attempt/Lease is created; discovering a static target/profile
    // misconfiguration at Execution creation (after the side-effectful
    // materialize) is too late and would promote a config error into a
    // physical-quiescence incident.
    let partition = required_partition(tx, &task.partition, true)?;
    // Realized-lifecycle conjunct (B.3 deferred this to B.4): the source/config
    // must be able to realize the member's actual M4 retention before any
    // authority is granted.
    let retention = agentype_core::Retention::parse_sql(&partition.retention)?;
    let required_lifecycle = agentype_agent_contract::required_lifecycle(retention);
    if !config
        .effective_lifecycle(&source)
        .contains(&required_lifecycle)
    {
        return Err(Error::invalid_authority(
            "the source/config lifecycle cannot realize the partition retention",
        ));
    }
    let (target, _profile) = agentype_execution_config::resolve_target_profile(
        execution_registry,
        &partition.execution_target,
        &partition.execution_profile,
    )
    .map_err(|err| Error::invalid_authority(err.to_string()))?;
    if target.adapter_kind != policy.adapter_kind {
        return Err(Error::invalid_authority(
            "partition execution target adapter kind does not match the policy",
        ));
    }
    // M4 owns the authoritative attempt isolation: it MUST come from the
    // ExecutionRegistry target, never from adapter self-assertion. The frozen
    // effective isolation is the OR of the AgentType and Task requirements, so
    // both sides are checked before authority is granted.
    let effective_isolation = actual.contract.security.requires_attempt_isolation
        || requirement.hard.required_attempt_isolation;
    if effective_isolation && !target.attempt_isolation {
        return Err(Error::invalid_authority(
            "partition execution target cannot enforce the effective attempt isolation",
        ));
    }

    let (incarnation, reused) = resolve_incarnation(
        tx,
        &agent,
        &partition.execution_target,
        &bound,
        selection,
        now,
    )?;
    if reused.is_none() {
        mark_incarnation_provisioned(tx, &incarnation)?;
    }

    let provisioning_binding = match reused {
        Some(existing) => existing,
        None => {
            let binding = ProvisioningBinding {
                provisioning_binding_id: format!("pb_{}", Uuid::new_v4().simple()),
                logical_agent_id: LogicalAgentId::from_string(agent.id.clone()),
                incarnation_id: incarnation.clone(),
                agent_type: actual.type_ref,
                spawn_source: selection.spawn_source.clone(),
                source_config: selection.source_config.clone(),
                adapter_policy: selection.adapter_policy.clone(),
                adapter_kind: selection.adapter_kind.clone(),
                adapter_binding_key: selection.adapter_binding_key.clone(),
                provisioning_protocol: selection.provisioning_protocol.clone(),
                attested_materialization_digest: selection.attested_materialization_digest.clone(),
                effective_security: selection.evidence.enforceable_safety().clone(),
            };
            insert_provisioning_binding(tx, now, &binding)?;
            binding
        }
    };

    let claim = crate::kernel::claim_selected(tx, &agent, &partition, &task, now, lease_seconds)?;
    tx.execute(
        "UPDATE attempts SET incarnation_id=?1 WHERE id=?2 AND incarnation_id IS NULL",
        params![incarnation.as_str(), claim.attempt_id.as_str()],
    )
    .map_err(map_sqlite)?;

    Ok(TypedAcquisition {
        claim,
        provisioning_binding,
    })
}

/// Acquire a typed Task for an existing, READY, unassigned, type-bound
/// LogicalAgent. This is the authority-bearing transition: it re-loads and
/// re-proves every mandatory conjunct (including the active
/// `AdapterBindingPolicy` and the partition target's adapter kind) over the
/// current durable catalog and the agent's actual bound type, then creates the
/// Attempt/Lease and freezes the `ProvisioningBinding` in one transaction. It
/// performs no external I/O.
#[cfg(any(test, feature = "runtime-internal"))]
#[allow(clippy::too_many_arguments)]
pub fn acquire_typed_task_existing(
    tx: &Transaction<'_>,
    now: UnixTime,
    lease_seconds: f64,
    task_id: &TaskId,
    agent_id: &LogicalAgentId,
    selection: &ResolvedProvisioningSelection,
    execution_registry: &agentype_execution_config::ExecutionRegistry,
) -> Result<TypedAcquisition, Error> {
    finish_acquisition(
        tx,
        now,
        lease_seconds,
        task_id,
        agent_id,
        selection,
        execution_registry,
    )
}

/// Acquire a typed Task by provisioning a new LogicalAgent: birth a fresh agent
/// in the Task's partition with the Task's tags/workstream, bind it to the exact
/// pinned AgentType, then perform the same acquisition. One transaction; no
/// external I/O.
#[cfg(any(test, feature = "runtime-internal"))]
pub fn acquire_typed_task_new_agent(
    tx: &Transaction<'_>,
    now: UnixTime,
    lease_seconds: f64,
    task_id: &TaskId,
    selection: &ResolvedProvisioningSelection,
    execution_registry: &agentype_execution_config::ExecutionRegistry,
) -> Result<TypedAcquisition, Error> {
    let task = required_task(tx, task_id.as_str())?;
    let requirement = crate::requirement::get_task_agent_requirement(tx, task_id)?
        .ok_or_else(|| Error::invariant("a TYPED task is missing its agent requirement"))?;
    let tags: Vec<String> = serde_json::from_str(&task.affinity_tags_json)
        .map_err(|e| Error::invariant(format!("task affinity tags: {e}")))?;
    let agent_id = birth_agent(
        tx,
        &task.partition,
        task.workstream_id.as_deref(),
        Some(&tags),
        now,
    )?;
    let agent_id = LogicalAgentId::from_string(agent_id);
    crate::requirement::bind_logical_agent_type(tx, now, &agent_id, &requirement.required_type)?;
    finish_acquisition(
        tx,
        now,
        lease_seconds,
        task_id,
        &agent_id,
        selection,
        execution_registry,
    )
}
