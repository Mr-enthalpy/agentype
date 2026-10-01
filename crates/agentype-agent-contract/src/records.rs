//! M6-B contract records. Pure value types; persistence is a later milestone.
//!
//! Every identity is a private-field newtype with a validated constructor and
//! every numeric contract value is a validated finite non-negative newtype, so
//! a durable digest can never be formed from an invalid or non-canonical value.

use crate::capability::{CapabilityClaim, CapabilityRef, CapabilitySpec, CapabilityValue};
use crate::error::ContractError;
use crate::ids::{AdapterPolicyId, AgentTypeId, SourceConfigId, SpawnSourceId};
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

/// A validated non-negative, finite budget.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Budget(f64);

impl Budget {
    pub fn new(value: f64) -> Result<Self, ContractError> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(ContractError::InvalidNumber {
                field: "budget".into(),
            })
        }
    }

    pub fn zero() -> Self {
        Self(0.0)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// An opaque credential reference. Never a secret value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CredentialRef(String);

impl CredentialRef {
    pub fn new(reference: impl Into<String>) -> Result<Self, ContractError> {
        let reference = reference.into();
        if reference.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "credential reference cannot be empty".into(),
            });
        }
        Ok(Self(reference))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An opaque concrete physical execution domain (adapter-owned).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AdapterBindingKey(String);

impl AdapterBindingKey {
    pub fn new(key: impl Into<String>) -> Result<Self, ContractError> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "adapter binding key cannot be empty".into(),
            });
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_identity(id: &str, revision: u64, what: &str) -> Result<(), ContractError> {
    if id.trim().is_empty() {
        return Err(ContractError::InvalidRef {
            reason: format!("{what} id cannot be empty"),
        });
    }
    if revision == 0 {
        return Err(ContractError::InvalidRef {
            reason: format!("{what} revision must be >= 1"),
        });
    }
    Ok(())
}

/// Exact, immutable `(type_id, revision)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentTypeRef {
    type_id: AgentTypeId,
    revision: u64,
}

impl AgentTypeRef {
    pub fn new(type_id: impl Into<String>, revision: u64) -> Result<Self, ContractError> {
        let type_id = AgentTypeId::from_string(type_id);
        validate_identity(type_id.as_str(), revision, "agent type")?;
        Ok(Self { type_id, revision })
    }

    pub fn type_id(&self) -> &AgentTypeId {
        &self.type_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Exact, immutable `(source_id, revision)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpawnSourceRef {
    source_id: SpawnSourceId,
    revision: u64,
}

impl SpawnSourceRef {
    pub fn new(source_id: impl Into<String>, revision: u64) -> Result<Self, ContractError> {
        let source_id = SpawnSourceId::from_string(source_id);
        validate_identity(source_id.as_str(), revision, "spawn source")?;
        Ok(Self {
            source_id,
            revision,
        })
    }

    pub fn source_id(&self) -> &SpawnSourceId {
        &self.source_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Exact, immutable `(source, config_id, revision)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceConfigRef {
    source: SpawnSourceRef,
    config_id: SourceConfigId,
    revision: u64,
}

impl SourceConfigRef {
    pub fn new(
        source: SpawnSourceRef,
        config_id: impl Into<String>,
        revision: u64,
    ) -> Result<Self, ContractError> {
        let config_id = SourceConfigId::from_string(config_id);
        validate_identity(config_id.as_str(), revision, "source config")?;
        Ok(Self {
            source,
            config_id,
            revision,
        })
    }

    pub fn source(&self) -> &SpawnSourceRef {
        &self.source
    }

    pub fn config_id(&self) -> &SourceConfigId {
        &self.config_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Exact adapter policy revision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AdapterPolicyRef {
    policy_id: AdapterPolicyId,
    revision: u64,
}

impl AdapterPolicyRef {
    pub fn new(policy_id: impl Into<String>, revision: u64) -> Result<Self, ContractError> {
        let policy_id = AdapterPolicyId::from_string(policy_id);
        validate_identity(policy_id.as_str(), revision, "adapter policy")?;
        Ok(Self {
            policy_id,
            revision,
        })
    }

    pub fn policy_id(&self) -> &AdapterPolicyId {
        &self.policy_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// How strong a continuity guarantee an agent contract requires. A higher mode
/// is a *stronger requirement* that narrows the eligible provisioning set.
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

/// The AgentType contract: semantic/security/lifecycle/continuity/affinity
/// envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentTypeContract {
    /// `InformationFunction` is a closed enum without `Ord`, so this stays a
    /// `Vec`; call [`AgentTypeContract::normalize`] before digesting.
    pub allowed_information_functions: Vec<InformationFunction>,
    /// Capabilities the type requires an execution source to provide, keyed by
    /// exact revision.
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    /// Exact capability specs (matcher kind + security class) per revision.
    pub capability_specs: BTreeMap<CapabilityRef, CapabilitySpec>,
    pub permission_ceiling: BTreeSet<String>,
    pub visibility: BTreeSet<String>,
    pub tools: BTreeSet<String>,
    pub roots: BTreeSet<String>,
    /// Semantic affinity tags. Narrowing is allowed; broadening authority is not.
    pub affinity: BTreeSet<String>,
    pub budget_ceiling: Budget,
    pub security: SecurityContract,
    pub lifecycle: BTreeSet<LifecycleMode>,
    pub continuity: ContinuityMode,
    pub anchor_constraint: Option<String>,
}

impl AgentTypeContract {
    /// Canonicalize the information-function list so equal contracts cannot
    /// encode differently (and therefore cannot digest differently).
    pub fn normalize(&mut self) {
        self.allowed_information_functions
            .sort_by_key(|f| f.as_sql());
        self.allowed_information_functions.dedup();
    }

    /// Whole-record invariant: every required capability must have an exact spec.
    pub fn validate(&self) -> Result<(), ContractError> {
        for reference in self.required_capabilities.keys() {
            if !self.capability_specs.contains_key(reference) {
                return Err(ContractError::InvariantViolation(format!(
                    "missing capability spec for {}@{}",
                    reference.capability_id().as_str(),
                    reference.revision()
                )));
            }
        }
        Ok(())
    }
}

/// A published, immutable AgentType revision. Publication/deprecation status is
/// owned by the catalog, not duplicated here.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentType {
    pub type_ref: AgentTypeRef,
    pub based_on: Option<AgentTypeRef>,
    pub contract: AgentTypeContract,
}

/// A Task's hard agent requirement (derived from admission).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRequirement {
    pub information_function: InformationFunction,
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    pub required_permissions: BTreeSet<String>,
    pub required_tools: BTreeSet<String>,
    /// Semantic affinity tags this Task needs the agent to carry.
    pub required_affinity: BTreeSet<String>,
    pub required_workspace: WorkspaceMode,
    pub required_network: NetworkPolicy,
    pub required_continuity: ContinuityMode,
    pub required_anchor: Option<String>,
    pub budget: Budget,
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

/// A physical provisioning source: advertised envelopes plus declarations.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnSource {
    pub source_ref: SpawnSourceRef,
    pub adapter_policy: AdapterPolicyRef,
    pub lifecycle_modes: BTreeSet<LifecycleMode>,
    pub continuity_modes: BTreeSet<ContinuityMode>,
    pub functional_envelope: BTreeMap<CapabilityRef, CapabilityValue>,
    pub claims: Vec<CapabilityClaim>,
    pub status: SourceStatus,
}

/// A source-specific operator configuration. The payload is opaque to Core;
/// only its identity, digest, credential references, and any config-specific
/// declarations are modeled here.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceConfig {
    pub config_ref: SourceConfigRef,
    pub config_digest: String,
    pub credential_refs: Vec<CredentialRef>,
    pub claims: Vec<CapabilityClaim>,
    pub status: ConfigStatus,
}

/// Physical facts a source/adapter can actually *enforce*. Private fields and a
/// validated, canonicalizing constructor so a caller cannot assemble an
/// arbitrary enforcement claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalSafety {
    attempt_isolation: bool,
    enforceable_workspace_modes: Vec<WorkspaceMode>,
    enforceable_network_modes: BTreeSet<NetworkPolicy>,
}

impl PhysicalSafety {
    pub fn new(
        attempt_isolation: bool,
        mut workspace_modes: Vec<WorkspaceMode>,
        network_modes: BTreeSet<NetworkPolicy>,
    ) -> Result<Self, ContractError> {
        workspace_modes.sort_by_key(|m| workspace_rank(*m));
        workspace_modes.dedup();
        Ok(Self {
            attempt_isolation,
            enforceable_workspace_modes: workspace_modes,
            enforceable_network_modes: network_modes,
        })
    }

    pub fn attempt_isolation(&self) -> bool {
        self.attempt_isolation
    }

    pub fn enforces_workspace(&self, mode: WorkspaceMode) -> bool {
        self.enforceable_workspace_modes.contains(&mode)
    }

    pub fn enforces_network(&self, policy: NetworkPolicy) -> bool {
        self.enforceable_network_modes.contains(&policy)
    }
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
///
/// The catalog is the single publication-status authority. Deprecation is
/// monotonic: a deprecated revision cannot be resurrected, only superseded by a
/// new revision.
#[derive(Clone, Debug, Default)]
pub struct PublishedCatalog {
    published: BTreeSet<AgentTypeRef>,
    deprecated: BTreeSet<AgentTypeRef>,
}

impl PublishedCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&mut self, reference: AgentTypeRef) -> bool {
        if self.deprecated.contains(&reference) {
            return false;
        }
        self.published.insert(reference)
    }

    pub fn deprecate(&mut self, reference: &AgentTypeRef) -> bool {
        let was_published = self.published.remove(reference);
        let recorded = self.deprecated.insert(reference.clone());
        was_published || recorded
    }

    pub fn is_deprecated(&self, reference: &AgentTypeRef) -> bool {
        self.deprecated.contains(reference)
    }
}

impl AgentTypeLookup for PublishedCatalog {
    fn is_published(&self, reference: &AgentTypeRef) -> bool {
        self.published.contains(reference)
    }

    fn latest_revision(&self, type_id: &AgentTypeId) -> Option<u64> {
        self.published
            .iter()
            .filter(|r| r.type_id() == type_id)
            .map(|r| r.revision())
            .max()
    }
}
