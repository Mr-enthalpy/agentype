//! M6-B.4 durable provisioning value types.
//!
//! A `ProvisioningBinding` freezes one Incarnation's chosen provisioning: which
//! exact `SpawnSource` revision, `SourceConfig` revision, and
//! `AdapterBindingPolicy` revision were selected for an exact `AgentType`
//! revision. It is the durable audit/recovery glue between the semantic
//! requirement and the physical `Execution`.
//!
//! A `BindingSnapshot` freezes one Execution's exact physical choice: the
//! resolved M5 `execution_target`/`execution_profile`, the exact
//! `adapter_kind` + opaque `adapter_binding_key`, the source/config provenance,
//! the effective capabilities/security, and a secret-free credential digest.
//! It is created atomically with the Execution row and never rewritten.
//!
//! Both are immutable: a changed source/config is a new Incarnation, and a
//! changed physical choice is a new Execution. Neither type stores a secret
//! value; only opaque references and a credential-reference digest.

use crate::capability::{CapabilityRef, CapabilityValue};
use crate::error::ContractError;
use crate::records::{
    AdapterPolicyRef, AgentTypeRef, ConfigDigest, NetworkPolicy, PhysicalSafety, SourceConfigRef,
    SpawnSourceRef,
};
use agentype_core::{ExecutionId, IncarnationId, LogicalAgentId, WorkspaceMode};
use std::collections::BTreeMap;

/// A source materialization digest with the frozen canonical grammar:
/// `sha256:` followed by exactly 64 lowercase hexadecimal characters.
///
/// Unlike [`ConfigDigest`], construction always enforces the canonical grammar:
/// a materialization digest only ever enters durable provenance (a
/// `ProvisioningBinding` / `BindingSnapshot`), so there is no pre-commit draft
/// phase. Core never interprets the materialized content; it only records that
/// the trusted source integration attested it under the exact physical domain.
/// This keeps "digest" a constrained value rather than an arbitrary string and
/// preserves the B.2 source-private-to-durable-Core seam.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterializationDigest(String);

impl MaterializationDigest {
    pub fn new(digest: impl Into<String>) -> Result<Self, ContractError> {
        let digest = digest.into();
        let canonical = digest.strip_prefix("sha256:").is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        if !canonical {
            return Err(ContractError::InvalidRef {
                reason: "materialization digest must have the form sha256:<64 lowercase hex>"
                    .into(),
            });
        }
        Ok(Self(digest))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Resolver version family. Every committed snapshot records a member of this
/// family; the authoritative read accepts any member so an immutable historical
/// snapshot does not become "corruption" when the current producer version is
/// bumped. Bump the numeric suffix when the resolution input shape changes.
pub const RESOLVER_VERSION_FAMILY: &str = "agentype-resolver/";

/// Current resolver encoding version recorded in new snapshots.
pub const RESOLVER_VERSION: &str = "agentype-resolver/1";

/// One Incarnation's immutable provisioning choice.
#[derive(Clone, Debug, PartialEq)]
pub struct ProvisioningBinding {
    pub provisioning_binding_id: String,
    pub logical_agent_id: LogicalAgentId,
    pub incarnation_id: IncarnationId,
    pub agent_type: AgentTypeRef,
    pub spawn_source: SpawnSourceRef,
    pub source_config: SourceConfigRef,
    pub adapter_policy: AdapterPolicyRef,
    /// Exact imported adapter kind the Incarnation was qualified against.
    pub adapter_kind: String,
    /// Opaque exact physical execution domain the Incarnation was qualified
    /// against. Core MUST NOT interpret it; it is the durable proof that the
    /// physical binding chosen before authority survives the acquisition
    /// transaction, so restart/recovery never has to re-derive it from a
    /// transient resolver candidate.
    pub adapter_binding_key: String,
    /// Secret-free **attested** content digest the pure `SourceConfigIntegration`
    /// resolved for this exact `SourceConfig` revision and physical domain. It is
    /// an expected/attested identity committed BEFORE any physical work, not
    /// proof that materialization happened (that is the adapter's start
    /// operation under M5). Core never interprets the config.
    pub attested_materialization_digest: MaterializationDigest,
    /// Exact source-integration **protocol identity** the Incarnation was
    /// qualified against. It binds the resolved config descriptor to the exact
    /// adapter binding that must consume it, so a descriptor produced under one
    /// protocol can never be launched against an adapter that does not accept it.
    pub provisioning_protocol: String,
    /// The resolved hard security facts the chosen environment can enforce.
    /// This is Incarnation-scoped provisioning provenance; the per-Task
    /// requirement commitment lives on the Execution's `BindingSnapshot`.
    pub effective_security: PhysicalSafety,
}

impl ProvisioningBinding {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.provisioning_binding_id.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "provisioning binding id cannot be empty".into(),
            });
        }
        if self.adapter_kind.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "provisioning binding adapter kind cannot be empty".into(),
            });
        }
        if self.adapter_binding_key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "provisioning binding adapter binding key cannot be empty".into(),
            });
        }
        if self.provisioning_protocol.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "provisioning binding protocol cannot be empty".into(),
            });
        }
        Ok(())
    }

    /// Whether this frozen Incarnation provenance already qualifies the exact
    /// physical domain, source/config/policy, and resolved enforceability the
    /// caller wants to use. Any difference requires a new Incarnation, never an
    /// in-place rebind.
    #[allow(clippy::too_many_arguments)]
    pub fn qualifies(
        &self,
        agent_type: &AgentTypeRef,
        spawn_source: &SpawnSourceRef,
        source_config: &SourceConfigRef,
        adapter_policy: &AdapterPolicyRef,
        adapter_kind: &str,
        adapter_binding_key: &str,
        attested_materialization_digest: &MaterializationDigest,
        provisioning_protocol: &str,
        effective_security: &PhysicalSafety,
    ) -> bool {
        &self.agent_type == agent_type
            && &self.spawn_source == spawn_source
            && &self.source_config == source_config
            && &self.adapter_policy == adapter_policy
            && self.adapter_kind == adapter_kind
            && self.adapter_binding_key == adapter_binding_key
            && &self.attested_materialization_digest == attested_materialization_digest
            && self.provisioning_protocol == provisioning_protocol
            && &self.effective_security == effective_security
    }
}

/// One Execution's immutable exact physical choice.
#[derive(Clone, Debug, PartialEq)]
pub struct BindingSnapshot {
    pub snapshot_id: String,
    pub execution_id: ExecutionId,
    pub provisioning_binding_id: String,
    pub adapter_kind: String,
    /// Opaque concrete physical execution domain; Core MUST NOT interpret it.
    pub adapter_binding_key: String,
    pub spawn_source: SpawnSourceRef,
    pub source_config: SourceConfigRef,
    pub source_config_digest: ConfigDigest,
    /// Secret-free **attested** content digest (see `ProvisioningBinding`); an
    /// expected identity, not proof that materialization happened.
    pub attested_materialization_digest: MaterializationDigest,
    /// Opaque, secret-free **launch descriptor** the pure source integration
    /// resolved for this exact physical domain. Core never interprets it; it is
    /// carried to the physical start request, and the exact adapter performs the
    /// physical materialization of the described environment as part of
    /// `start_execution` (M5 owns the physical lifecycle).
    pub launch_descriptor: String,
    /// M5 resolved execution target name (from the partition/anchor).
    pub execution_target: String,
    /// M5 resolved execution profile name (from the partition/anchor).
    pub execution_profile: String,
    /// The authoritative admitted requirement's capability values (diagnostic;
    /// the full AgentType/Task capability join is B.5).
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    /// The imported **enforceability capability** the selection was proven
    /// against (the source/adapter's `PhysicalSafety`). This is a capability
    /// set, not the per-execution policy.
    pub enforceable_security: PhysicalSafety,
    /// The **per-execution effective** isolation the authoritative M4
    /// `ExecutionRegistry` target actually provides.
    pub effective_isolation: bool,
    /// The effective workspace mode this execution requests.
    pub effective_workspace: WorkspaceMode,
    /// The effective network policy this execution requests.
    pub effective_network: NetworkPolicy,
    /// Secret-free digest of the resolved credential references, when any.
    pub credential_refs_digest: Option<String>,
    pub resolver_version: String,
}

impl BindingSnapshot {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.snapshot_id.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot id cannot be empty".into(),
            });
        }
        if self.adapter_kind.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot adapter kind cannot be empty".into(),
            });
        }
        if self.adapter_binding_key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot adapter binding key cannot be empty".into(),
            });
        }
        if self.provisioning_binding_id.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot provisioning binding id cannot be empty".into(),
            });
        }
        if self.execution_target.trim().is_empty() || self.execution_profile.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot target/profile cannot be empty".into(),
            });
        }
        if self.launch_descriptor.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot launch descriptor cannot be empty".into(),
            });
        }
        if self.resolver_version.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "binding snapshot resolver version cannot be empty".into(),
            });
        }
        Ok(())
    }
}
