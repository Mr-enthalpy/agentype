//! M6-B Agent Contract ontology and the four matching predicates.
//!
//! This crate is pure: it MUST NOT depend on SQLite, Tokio, the Runtime, or the
//! ExecutionAdapter. It answers only "what contract must execute this work, and
//! which provisioning source can realize it", never "what work becomes real"
//! (M6-A) and never "how a physical environment is created" (M5).
//!
//! Frozen directions:
//! - AgentType expresses semantic/security/lifecycle/continuity contract and
//!   MUST NOT be defined by model, provider, terminal, price tier, prompt
//!   alias, SourceConfig, or AdapterBinding.
//! - The four relations stay four; they MUST NOT collapse into one
//!   subtype/inheritance operator.
//! - Security/authority requirements MUST be `ENFORCED`; `DECLARED` never
//!   produces an isolation proof.
//! - Hard constraints are filtered before ranking; cost MUST NOT override
//!   correctness or security.

mod capability;
mod error;
mod evidence;
mod ids;
mod predicates;
mod records;
mod selector;

pub use capability::{
    value_satisfies, value_within, Assurance, CapabilityCatalog, CapabilityClaim,
    CapabilityDefinition, CapabilityRef, CapabilityValue, MatcherKind, Quantity, SecurityClass,
};
pub use error::ContractError;
pub use evidence::ResolvedProvisioningEvidence;
pub use ids::{
    AdapterPolicyId, AgentTypeId, CapabilityId, SandboxPolicyId, SourceConfigId, SpawnSourceId,
};
pub use predicates::{
    can_execute, can_provision, can_provision_task, is_valid_refinement, more_specific_for,
    validate_source_config, validate_spawn_source,
};
pub use records::{
    network_rank, workspace_rank, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentType, AgentTypeContract, AgentTypeLookup, AgentTypeRef, Budget, ConfigDigest,
    ConfigStatus, ContinuityMode, CredentialRef, LifecycleMode, NetworkPolicy, PhysicalSafety,
    PublishedCatalog, SandboxPolicyRef, SecurityContract, SourceConfig, SourceConfigRef,
    SourceStatus, SpawnSource, SpawnSourceRef, TaskRequirement,
};
pub use selector::{resolve_selector, AgentTypeSelector};
