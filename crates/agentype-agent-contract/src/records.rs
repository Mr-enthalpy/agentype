//! M6-B contract records. Pure value types; persistence is a later milestone.
//!
//! Every identity is a private-field newtype with a validated constructor and
//! every numeric contract value is a validated finite non-negative newtype, so
//! a durable digest can never be formed from an invalid or non-canonical value.

use crate::capability::{CapabilityCatalog, CapabilityClaim, CapabilityRef, CapabilityValue};
use crate::error::ContractError;
use crate::ids::{AdapterPolicyId, AgentTypeId, SandboxPolicyId, SourceConfigId, SpawnSourceId};
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

/// A validated non-negative, finite budget.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Budget(f64);

impl Budget {
    pub fn new(value: f64) -> Result<Self, ContractError> {
        if value.is_finite() && value >= 0.0 {
            // Canonicalize negative zero so equal values cannot digest differently.
            Ok(Self(if value == 0.0 { 0.0 } else { value }))
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

/// A non-empty config content digest.
///
/// Only non-emptiness is enforced here; the canonical digest representation is a
/// B.2 publication obligation and MUST NOT be assumed from this type alone.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConfigDigest(String);

impl ConfigDigest {
    pub fn new(digest: impl Into<String>) -> Result<Self, ContractError> {
        let digest = digest.into();
        if digest.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "config digest cannot be empty".into(),
            });
        }
        Ok(Self(digest))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

macro_rules! revision_ref {
    ($name:ident, $id_ty:ty, $label:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            id: $id_ty,
            revision: u64,
        }

        impl $name {
            pub fn new(id: impl Into<String>, revision: u64) -> Result<Self, ContractError> {
                let id = <$id_ty>::from_string(id);
                validate_identity(id.as_str(), revision, $label)?;
                Ok(Self { id, revision })
            }

            pub fn from_id(id: $id_ty, revision: u64) -> Result<Self, ContractError> {
                validate_identity(id.as_str(), revision, $label)?;
                Ok(Self { id, revision })
            }

            pub fn id(&self) -> &$id_ty {
                &self.id
            }

            pub fn revision(&self) -> u64 {
                self.revision
            }
        }
    };
}

revision_ref!(AgentTypeRef, AgentTypeId, "agent type");
revision_ref!(SpawnSourceRef, SpawnSourceId, "spawn source");
revision_ref!(SandboxPolicyRef, SandboxPolicyId, "sandbox policy");
revision_ref!(AdapterPolicyRef, AdapterPolicyId, "adapter policy");

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

/// Coarse security envelope of an AgentType contract: the mechanically
/// enforceable facts `can_provision` proves against imported evidence.
///
/// Filesystem/tool/visibility restrictions are NOT represented here: they
/// belong to the full [`SandboxPolicyRef`], which must also carry imported
/// enforcement evidence. This keeps every field in this struct backed by a
/// proof path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityContract {
    pub workspace: WorkspaceMode,
    pub network: NetworkPolicy,
    pub requires_attempt_isolation: bool,
}

/// Semantic affinity constraint. `Any` is the explicit top element (a truly
/// general agent that accepts every Task affinity); `Only(S)` accepts only
/// Tasks whose required tags are a subset of `S`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AffinityConstraint {
    Any,
    Only(BTreeSet<String>),
}

/// The AgentType contract: semantic/security/lifecycle/continuity/affinity
/// envelope plus an optional full sandbox policy reference.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentTypeContract {
    /// `InformationFunction` is a closed enum without `Ord`, so this stays a
    /// `Vec`; call [`AgentTypeContract::normalize`] before digesting.
    pub allowed_information_functions: Vec<InformationFunction>,
    /// Capabilities the type requires an execution source to provide, keyed by
    /// exact revision. Semantics (matcher kind, security class, polarity) come
    /// from the [`CapabilityCatalog`]. Authority, sandbox, and continuity
    /// guarantees are carried here (and by the structured security fields
    /// below), never as free-form tools/roots/visibility lists.
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    /// Semantic affinity constraint. Narrowing is allowed; broadening is not.
    pub affinity: AffinityConstraint,
    pub budget_ceiling: Budget,
    pub security: SecurityContract,
    pub lifecycle: BTreeSet<LifecycleMode>,
    pub continuity: ContinuityMode,
    /// Reference to the full sandbox policy (spec 10 vocabulary) when one applies.
    pub sandbox_policy: Option<SandboxPolicyRef>,
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

    /// Whole-record invariant: a non-empty lifecycle, every required capability
    /// has a catalog definition, and the declared value shape matches it.
    pub fn validate(&self, catalog: &CapabilityCatalog) -> Result<(), ContractError> {
        if self.lifecycle.is_empty() {
            return Err(ContractError::InvariantViolation(
                "agent lifecycle mode set must not be empty (uninhabited type)".into(),
            ));
        }
        for (reference, value) in &self.required_capabilities {
            // Bool(false) is absence: normalize it away so a contract that
            // omits a capability and one that spells Bool(false) validate and
            // behave identically.
            if !value.is_present() {
                continue;
            }
            let definition =
                catalog
                    .get(reference)
                    .ok_or_else(|| ContractError::CapabilityMismatch {
                        capability: reference.capability_id().as_str().to_string(),
                    })?;
            if definition.matcher_kind != value.matcher_kind() {
                return Err(ContractError::InvariantViolation(format!(
                    "capability value shape does not match the definition for {}@{}",
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
    pub required_affinity: BTreeSet<String>,
    pub required_workspace: WorkspaceMode,
    pub required_network: NetworkPolicy,
    /// A Task may tighten attempt isolation beyond the AgentType default; the
    /// effective requirement is the OR with the AgentType's.
    pub required_attempt_isolation: bool,
    pub required_continuity: ContinuityMode,
    pub sandbox_policy: Option<SandboxPolicyRef>,
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

/// A physical provisioning source: immutable revision content composed with the
/// current operational disposition.
///
/// `source_ref` identifies immutable revision content. `status` is a mutable
/// operational disposition overlay, not part of the revision identity or any
/// content digest (see spec 07 "Revision and disposition ownership").
///
/// This is a resolved *view*, not the immutable revision authority: `PartialEq`
/// compares content only, so the mutable disposition can never leak into a
/// content digest, cache key, or snapshot identity. B.2 persists the revision
/// and the disposition separately.
#[derive(Clone, Debug)]
pub struct SpawnSource {
    pub source_ref: SpawnSourceRef,
    pub adapter_policy: AdapterPolicyRef,
    pub lifecycle_modes: BTreeSet<LifecycleMode>,
    pub continuity_modes: BTreeSet<ContinuityMode>,
    /// The provisionable capability ceiling for this source.
    pub functional_envelope: BTreeMap<CapabilityRef, CapabilityValue>,
    pub claims: Vec<CapabilityClaim>,
    pub status: SourceStatus,
}

impl PartialEq for SpawnSource {
    fn eq(&self, other: &Self) -> bool {
        self.source_ref == other.source_ref
            && self.adapter_policy == other.adapter_policy
            && self.lifecycle_modes == other.lifecycle_modes
            && self.continuity_modes == other.continuity_modes
            && self.functional_envelope == other.functional_envelope
            && self.claims == other.claims
    }
}

/// A source-specific operator configuration. The payload is opaque to Core;
/// only its identity, digest, credential references, and any config-specific
/// declarations (which MUST stay within the source envelope) are modeled here.
///
/// Like [`SpawnSource`], this is a resolved view: `PartialEq` compares
/// immutable content only, so the mutable `status` disposition never enters a
/// content digest or identity comparison.
#[derive(Clone, Debug)]
pub struct SourceConfig {
    pub config_ref: SourceConfigRef,
    pub config_digest: ConfigDigest,
    /// Config-specific lifecycle narrowing of the source envelope (duplicate of
    /// a source mode is allowed; a mode the source lacks is rejected).
    pub lifecycle_modes: Option<BTreeSet<LifecycleMode>>,
    /// Config-specific continuity narrowing of the source envelope.
    pub continuity_modes: Option<BTreeSet<ContinuityMode>>,
    pub credential_refs: Vec<CredentialRef>,
    pub claims: Vec<CapabilityClaim>,
    /// Mutable operational disposition, excluded from `config_digest` and from
    /// the immutable config revision identity.
    pub status: ConfigStatus,
}

impl PartialEq for SourceConfig {
    fn eq(&self, other: &Self) -> bool {
        self.config_ref == other.config_ref
            && self.config_digest == other.config_digest
            && self.lifecycle_modes == other.lifecycle_modes
            && self.continuity_modes == other.continuity_modes
            && self.credential_refs == other.credential_refs
            && self.claims == other.claims
    }
}

impl SourceConfig {
    /// Effective lifecycle modes: the config override when present, else the
    /// source envelope. The override is validated to be a subset.
    pub fn effective_lifecycle<'a>(
        &'a self,
        source: &'a SpawnSource,
    ) -> &'a BTreeSet<LifecycleMode> {
        self.lifecycle_modes
            .as_ref()
            .unwrap_or(&source.lifecycle_modes)
    }

    /// Effective continuity modes: the config override when present, else the
    /// source envelope.
    pub fn effective_continuity<'a>(
        &'a self,
        source: &'a SpawnSource,
    ) -> &'a BTreeSet<ContinuityMode> {
        self.continuity_modes
            .as_ref()
            .unwrap_or(&source.continuity_modes)
    }
}

/// Physical facts a source/adapter can actually *enforce*. Private fields and a
/// validated, canonicalizing constructor so a caller cannot assemble an
/// arbitrary enforcement claim; the enforced facts live in imported evidence.
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
///
/// B.1 defines only the value shape; B.2/B.4 own catalogue validation and the
/// physical binding key. Use [`AdapterBindingPolicy::new`] so the non-empty
/// invariant holds at construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterBindingPolicy {
    pub policy_ref: AdapterPolicyRef,
    pub adapter_kind: String,
    pub binding_ref: String,
    pub required_safety: PhysicalSafety,
    pub status: ConfigStatus,
}

impl AdapterBindingPolicy {
    pub fn new(
        policy_ref: AdapterPolicyRef,
        adapter_kind: impl Into<String>,
        binding_ref: impl Into<String>,
        required_safety: PhysicalSafety,
        status: ConfigStatus,
    ) -> Result<Self, ContractError> {
        let adapter_kind = adapter_kind.into();
        let binding_ref = binding_ref.into();
        if adapter_kind.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "adapter binding policy kind cannot be empty".into(),
            });
        }
        if binding_ref.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "adapter binding policy binding_ref cannot be empty".into(),
            });
        }
        Ok(Self {
            policy_ref,
            adapter_kind,
            binding_ref,
            required_safety,
            status,
        })
    }
}

/// Read-only catalog view used by selector resolution.
pub trait AgentTypeLookup {
    fn is_published(&self, reference: &AgentTypeRef) -> bool;
    fn latest_revision(&self, type_id: &AgentTypeId) -> Option<u64>;
}

/// A tiny in-memory selector catalog for tests and early wiring.
///
/// This is **not** the production publication/content authority: it only holds
/// publication/deprecation status keyed by `AgentTypeRef` and cannot prove that
/// the contract content of an exact revision is immutable. B.2 owns the real
/// catalog (content digests, canonical representation, `based_on` provenance).
/// Deprecation here is monotonic: a deprecated revision cannot be resurrected,
/// only superseded by a new revision.
#[derive(Clone, Debug, Default)]
pub struct InMemorySelectorCatalog {
    published: BTreeSet<AgentTypeRef>,
    deprecated: BTreeSet<AgentTypeRef>,
}

impl InMemorySelectorCatalog {
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

impl AgentTypeLookup for InMemorySelectorCatalog {
    fn is_published(&self, reference: &AgentTypeRef) -> bool {
        self.published.contains(reference)
    }

    fn latest_revision(&self, type_id: &AgentTypeId) -> Option<u64> {
        self.published
            .iter()
            .filter(|r| r.id() == type_id)
            .map(|r| r.revision())
            .max()
    }
}
