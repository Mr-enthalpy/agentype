//! M6-B.5 B5.2 single-transaction security candidate observation.
//!
//! The B5.2-E scheduler hold used to enumerate the existing-owner routes and the
//! pinned new-agent route through several consecutive catalog reads, then union
//! the identities. A concurrent publish could therefore produce a round that
//! never existed at one instant. This module is the durable answer: one
//! transaction loads every input candidate enumeration needs, and the runtime
//! qualifies candidates from that owned snapshot instead of issuing further
//! reads.
//!
//! The observation is not authority. It creates no Attempt, Lease or
//! `ProvisioningBinding`; it only makes "these are the catalog facts at one
//! instant" a checkable value. Its digest is re-read after enumeration so a
//! publish that lands while external attestation runs is reported as catalog
//! drift, never silently folded into the earlier round.

use crate::catalog::{
    agent_type_security_mode, get_adapter_binding_policy, get_agent_type, get_spawn_source,
    load_capability_catalog, load_source_config_revision, spawn_source_security_mode,
    SourceConfigRevision,
};
use crate::provisioning::{
    active_catalog_frontier_digest, list_active_source_config_refs, list_active_spawn_source_refs,
};
use crate::txutil::{required_partition, required_task};
use agentype_agent_contract::{
    canonical_json_body_digest, AdapterBindingPolicy, AgentType, CapabilityCatalog,
    SecurityContractMode, SpawnSource, SpawnSourceRef, TaskAgentRequirement,
};
use agentype_core::{Error, LogicalAgentId, PartitionRecord, TaskId};
use rusqlite::Transaction;
use serde_json::json;

/// One matched existing owner and the exact bound `AgentType` revision it
/// evaluates against.
#[derive(Clone, Debug)]
pub struct ObservedRosterEntry {
    logical_agent_id: LogicalAgentId,
    agent_type: AgentType,
    security_mode: SecurityContractMode,
}

impl ObservedRosterEntry {
    pub fn logical_agent_id(&self) -> &LogicalAgentId {
        &self.logical_agent_id
    }

    pub fn agent_type(&self) -> &AgentType {
        &self.agent_type
    }

    pub fn security_mode(&self) -> SecurityContractMode {
        self.security_mode
    }
}

/// One active source revision with its policy and every active config revision.
///
/// The source/config set is owner-independent: the same physical sources are
/// evaluated against each matched agent's bound type and against the Task pin.
#[derive(Clone, Debug)]
pub struct ObservedSpawnSource {
    source_ref: SpawnSourceRef,
    source: SpawnSource,
    security_mode: SecurityContractMode,
    policy: AdapterBindingPolicy,
    configs: Vec<SourceConfigRevision>,
}

impl ObservedSpawnSource {
    pub fn source_ref(&self) -> &SpawnSourceRef {
        &self.source_ref
    }

    pub fn source(&self) -> &SpawnSource {
        &self.source
    }

    pub fn security_mode(&self) -> SecurityContractMode {
        self.security_mode
    }

    pub fn policy(&self) -> &AdapterBindingPolicy {
        &self.policy
    }

    pub fn configs(&self) -> &[SourceConfigRevision] {
        &self.configs
    }
}

/// Every durable input to one `SECURITY_V1` candidate round, loaded in a single
/// storage transaction.
#[derive(Clone, Debug)]
pub struct SecurityCandidateObservation {
    requirement: TaskAgentRequirement,
    pinned_agent_type: AgentType,
    pinned_security_mode: SecurityContractMode,
    partition: PartitionRecord,
    catalog: CapabilityCatalog,
    roster: Vec<ObservedRosterEntry>,
    sources: Vec<ObservedSpawnSource>,
    frontier_digest: String,
    digest: String,
}

impl SecurityCandidateObservation {
    pub fn requirement(&self) -> &TaskAgentRequirement {
        &self.requirement
    }

    pub fn pinned_agent_type(&self) -> &AgentType {
        &self.pinned_agent_type
    }

    pub fn pinned_security_mode(&self) -> SecurityContractMode {
        self.pinned_security_mode
    }

    pub fn partition(&self) -> &PartitionRecord {
        &self.partition
    }

    pub fn catalog(&self) -> &CapabilityCatalog {
        &self.catalog
    }

    pub fn roster(&self) -> &[ObservedRosterEntry] {
        &self.roster
    }

    pub fn sources(&self) -> &[ObservedSpawnSource] {
        &self.sources
    }

    pub fn frontier_digest(&self) -> &str {
        &self.frontier_digest
    }

    /// The canonical digest of every identity fact this observation carries.
    /// Re-reading the observation after enumeration and comparing this value is
    /// the drift fence; it is not a transaction and proves nothing about
    /// physical truth.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Load one coherent observation of a Task's security candidate inputs.
pub fn load_security_candidate_observation(
    tx: &Transaction<'_>,
    task_id: &TaskId,
) -> Result<SecurityCandidateObservation, Error> {
    let requirement = crate::requirement::get_task_agent_requirement(tx, task_id)?
        .ok_or_else(|| Error::not_found(format!("task {task_id} has no agent requirement")))?;
    let (pinned_agent_type, _pinned_agent_type_status) =
        get_agent_type(tx, &requirement.required_type)?.ok_or_else(|| {
            Error::invariant("a task agent requirement references a missing agent type")
        })?;
    let pinned_security_mode = agent_type_security_mode(tx, &requirement.required_type)?;

    let task = required_task(tx, task_id.as_str())?;
    let partition_row = required_partition(tx, &task.partition, false)?;
    let partition = PartitionRecord {
        name: agentype_core::PartitionId::new(&partition_row.name),
        desired_capacity: partition_row.desired_capacity,
        retention: agentype_core::Retention::parse_sql(&partition_row.retention)?,
        execution_target: partition_row.execution_target,
        execution_profile: partition_row.execution_profile,
        active: partition_row.active,
        merged_into: partition_row
            .merged_into
            .map(agentype_core::PartitionId::new),
        topology_revision: partition_row.topology_revision,
    };
    let catalog = load_capability_catalog(tx)?;

    let mut roster = Vec::new();
    for candidate in crate::requirement::match_existing_agents_for_task(tx, task_id)? {
        let mode = agent_type_security_mode(tx, &candidate.agent_type.type_ref)?;
        roster.push(ObservedRosterEntry {
            logical_agent_id: candidate.logical_agent_id,
            agent_type: candidate.agent_type,
            security_mode: mode,
        });
    }
    roster.sort_by(|a, b| {
        a.logical_agent_id
            .as_str()
            .cmp(b.logical_agent_id.as_str())
            .then_with(|| {
                a.agent_type
                    .type_ref
                    .id()
                    .as_str()
                    .cmp(b.agent_type.type_ref.id().as_str())
            })
            .then_with(|| {
                a.agent_type
                    .type_ref
                    .revision()
                    .cmp(&b.agent_type.type_ref.revision())
            })
    });

    let mut sources = Vec::new();
    for source_ref in list_active_spawn_source_refs(tx)? {
        let source = get_spawn_source(tx, &source_ref)?
            .ok_or_else(|| Error::invariant("active spawn source vanished during observation"))?;
        let security_mode = spawn_source_security_mode(tx, &source_ref)?;
        let policy = get_adapter_binding_policy(tx, &source.adapter_policy)?
            .ok_or_else(|| Error::invariant("active source policy vanished during observation"))?;
        let mut configs = Vec::new();
        for config_ref in list_active_source_config_refs(tx, &source_ref)? {
            let revision = load_source_config_revision(tx, &config_ref)?.ok_or_else(|| {
                Error::invariant("active source config vanished during observation")
            })?;
            configs.push(revision);
        }
        sources.push(ObservedSpawnSource {
            source_ref,
            source,
            security_mode,
            policy,
            configs,
        });
    }
    sources.sort_by(|a, b| {
        a.source_ref
            .id()
            .as_str()
            .cmp(b.source_ref.id().as_str())
            .then_with(|| a.source_ref.revision().cmp(&b.source_ref.revision()))
    });

    let frontier_digest = active_catalog_frontier_digest(tx)?;
    let digest = observation_digest(
        &requirement,
        &pinned_agent_type,
        pinned_security_mode,
        &partition,
        &roster,
        &sources,
        &frontier_digest,
    );
    Ok(SecurityCandidateObservation {
        requirement,
        pinned_agent_type,
        pinned_security_mode,
        partition,
        catalog,
        roster,
        sources,
        frontier_digest,
        digest,
    })
}

fn observation_digest(
    requirement: &TaskAgentRequirement,
    pinned_agent_type: &AgentType,
    pinned_security_mode: SecurityContractMode,
    partition: &PartitionRecord,
    roster: &[ObservedRosterEntry],
    sources: &[ObservedSpawnSource],
    frontier_digest: &str,
) -> String {
    let roster_facts: Vec<_> = roster
        .iter()
        .map(|entry| {
            json!({
                "agent": entry.logical_agent_id.as_str(),
                "type": entry.agent_type.type_ref.id().as_str(),
                "revision": entry.agent_type.type_ref.revision(),
                "mode": entry.security_mode.as_str(),
            })
        })
        .collect();
    let source_facts: Vec<_> = sources
        .iter()
        .map(|entry| {
            let configs: Vec<_> = entry
                .configs
                .iter()
                .map(|config| {
                    json!({
                        "config": config.config().config_ref.config_id().as_str(),
                        "revision": config.config().config_ref.revision(),
                        "digest": config.config().config_digest.as_str(),
                    })
                })
                .collect();
            json!({
                "source": entry.source_ref.id().as_str(),
                "revision": entry.source_ref.revision(),
                "mode": entry.security_mode.as_str(),
                "configs": configs,
            })
        })
        .collect();
    canonical_json_body_digest(&json!({
        "kind": "SECURITY_CANDIDATE_OBSERVATION",
        "frontier": frontier_digest,
        "required_type": {
            "id": requirement.required_type.id().as_str(),
            "revision": requirement.required_type.revision(),
        },
        "pinned_mode": pinned_security_mode.as_str(),
        "pinned_type": {
            "id": pinned_agent_type.type_ref.id().as_str(),
            "revision": pinned_agent_type.type_ref.revision(),
        },
        "partition": {
            "name": partition.name.as_str(),
            "target": partition.execution_target,
            "profile": partition.execution_profile,
            "active": partition.active,
        },
        "roster": roster_facts,
        "sources": source_facts,
    }))
}
