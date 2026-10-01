//! M6-B contract records. Pure value types; persistence is a later milestone.

use crate::capability::{CapabilityClaim, CapabilitySpec, CapabilityValue};
use crate::ids::{AdapterPolicyId, AgentTypeId, CapabilityId, SourceConfigId, SpawnSourceId};
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

/// Exact, immutable `(type_id, revision)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentTypeRef {
    pub type_id: AgentTypeId,
    pub revision: u64,
}

impl AgentTypeRef {
    pub fn new(type_id: impl Into<String>, revision: u64) -> Self {
        Self {
            type_id: AgentTypeId::from_string(type_id),
            revision,
        }
    }
}

/// Exact, immutable `(source_id, revision)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpawnSourceRef {
    pub source_id: SpawnSourceId,
    pub revision: u64,
}

/// Exact, immutable `(source, config_id, revision)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceConfigRef {
    pub source: SpawnSourceRef,
    pub config_id: SourceConfigId,
    pub revision: u64,
}

/// Exact adapter policy revision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AdapterPolicyRef {
    pub policy_id: AdapterPolicyId,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentTypeStatus {
    Published,
    Deprecated,
}

/// How strong a continuity guarantee an agent contract requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContinuityMode {
    None,
    Logical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LifecycleMode {
    Ephemeral,
    Resident,
    Revivable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NetworkPolicy {
    Disabled,
    Restricted,
    Enabled,
}

pub fn workspace_rank(mode: WorkspaceMode) -> u8 {
    match mode {
        WorkspaceMode::ReadOnly => 0,
        WorkspaceMode::Write => 1,
    }
}

pub fn network_rank(policy: NetworkPolicy) -> u8 {
    match policy {
        NetworkPolicy::Disabled => 0,
        NetworkPolicy::Restricted => 1,
        NetworkPolicy::Enabled => 2,
    }
}

/// Security envelope of an AgentType contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityContract {
    pub workspace: WorkspaceMode,
    pub network: NetworkPolicy,
    pub tool_roots: BTreeSet<String>,
    pub requires_attempt_isolation: bool,
}

/// The AgentType contract: semantic/security/lifecycle/continuity envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentTypeContract {
    /// `InformationFunction` is a closed enum without `Ord`, so this stays a
    /// sorted-by-construction `Vec` rather than a `BTreeSet`.
    pub allowed_information_functions: Vec<InformationFunction>,
    /// Capability ids and values the type requires an execution source to provide.
    pub required_capabilities: BTreeMap<CapabilityId, CapabilityValue>,
    /// Capability specs (matcher kind + security class) for each capability id.
    pub capability_specs: BTreeMap<CapabilityId, CapabilitySpec>,
    pub permission_ceiling: BTreeSet<String>,
    pub visibility: BTreeSet<String>,
    pub tools: BTreeSet<String>,
    pub roots: BTreeSet<String>,
    pub budget_ceiling: f64,
    pub security: SecurityContract,
    pub lifecycle: BTreeSet<LifecycleMode>,
    pub continuity: ContinuityMode,
    pub anchor_constraint: Option<String>,
}

/// A published, immutable AgentType revision.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentType {
    pub type_ref: AgentTypeRef,
    pub based_on: Option<AgentTypeRef>,
    pub contract: AgentTypeContract,
    pub status: AgentTypeStatus,
}

/// A Task's hard agent requirement (derived from admission).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRequirement {
    pub information_function: InformationFunction,
    pub required_capabilities: BTreeMap<CapabilityId, CapabilityValue>,
    pub required_permissions: BTreeSet<String>,
    pub required_tools: BTreeSet<String>,
    pub required_workspace: WorkspaceMode,
    pub required_network: NetworkPolicy,
    pub required_continuity: ContinuityMode,
    pub required_anchor: Option<String>,
    pub budget: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceStatus {
    Active,
    Draining,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfigStatus {
    Active,
    Draining,
    Disabled,
}

/// A physical provisioning source: advertised envelopes plus capability claims.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnSource {
    pub source_ref: SpawnSourceRef,
    pub adapter_policy: AdapterPolicyRef,
    pub lifecycle_modes: BTreeSet<LifecycleMode>,
    pub continuity_modes: BTreeSet<ContinuityMode>,
    pub functional_envelope: BTreeMap<CapabilityId, CapabilityValue>,
    pub claims: Vec<CapabilityClaim>,
    pub status: SourceStatus,
}

/// A source-specific operator configuration. The payload is opaque to Core;
/// only its identity, digest, and credential references are modeled here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceConfig {
    pub config_ref: SourceConfigRef,
    pub config_digest: String,
    pub credential_refs: Vec<String>,
    pub status: ConfigStatus,
}

/// Physical safety facts a source/config can actually realize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalSafety {
    pub attempt_isolation: bool,
    pub workspace: WorkspaceMode,
    pub network: NetworkPolicy,
}

/// Persisted operator intent connecting a stable alias to a physical binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterBindingPolicy {
    pub policy_ref: AdapterPolicyRef,
    pub adapter_kind: String,
    pub binding_ref: String,
    pub required_safety: PhysicalSafety,
    pub status: ConfigStatus,
}

/// Read-only catalog view used by selector resolution.
pub trait AgentTypeLookup {
    fn is_published(&self, reference: &AgentTypeRef) -> bool;
    fn latest_revision(&self, type_id: &AgentTypeId) -> Option<u64>;
}

/// A tiny in-memory published catalog for selector tests and early wiring.
#[derive(Clone, Debug, Default)]
pub struct PublishedCatalog {
    published: BTreeSet<AgentTypeRef>,
}

impl PublishedCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&mut self, reference: AgentTypeRef) -> bool {
        self.published.insert(reference)
    }

    pub fn deprecate(&mut self, reference: &AgentTypeRef) -> bool {
        self.published.remove(reference)
    }
}

impl AgentTypeLookup for PublishedCatalog {
    fn is_published(&self, reference: &AgentTypeRef) -> bool {
        self.published.contains(reference)
    }

    fn latest_revision(&self, type_id: &AgentTypeId) -> Option<u64> {
        self.published
            .iter()
            .filter(|r| &r.type_id == type_id)
            .map(|r| r.revision)
            .max()
    }
}
