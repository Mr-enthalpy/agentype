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

mod canonical;
mod capability;
mod decode;
mod error;
mod evidence;
mod ids;
mod matching;
mod predicates;
mod records;
mod requirement;
mod selector;

pub use canonical::{
    adapter_binding_policy_content_digest, agent_type_content_digest,
    canonical_adapter_binding_policy_bytes, canonical_agent_type_bytes,
    canonical_capability_definition_bytes, canonical_generation_policy_bytes,
    canonical_json_body_digest, canonical_source_config_bytes, canonical_spawn_source_bytes,
    canonical_task_agent_requirement_bytes, canonicalize_agent_type, canonicalize_capability_map,
    canonicalize_claims, canonicalize_credential_refs, canonicalize_generation_policy,
    canonicalize_source_config, canonicalize_spawn_source, canonicalize_task_agent_requirement,
    capability_definition_content_digest, content_digest, generation_policy_content_digest,
    source_config_content_digest, spawn_source_content_digest,
    task_agent_requirement_content_digest, CANONICAL_FORMAT_VERSION,
};
pub use capability::{
    join_requirement_values, value_satisfies, value_within, Assurance, CapabilityCatalog,
    CapabilityClaim, CapabilityDefinition, CapabilityPolarity, CapabilityRef, CapabilityValue,
    MatcherKind, Quantity, SecurityClass,
};
pub use decode::{
    adapter_binding_policy_from_canonical_json, agent_type_from_canonical_json,
    capability_definition_from_canonical_json, generation_policy_from_canonical_json,
    source_config_revision_from_canonical_json, spawn_source_from_canonical_json,
    task_agent_requirement_from_canonical_json,
};
pub use error::ContractError;
pub use evidence::ResolvedProvisioningEvidence;
pub use ids::{
    AdapterPolicyId, AgentTypeId, CapabilityId, SandboxPolicyId, SourceConfigId, SpawnSourceId,
};
pub use matching::{match_existing_agents, ExistingAgentCandidate};
pub use predicates::{
    can_execute, can_provision, can_provision_task, is_valid_refinement, more_specific_for,
    validate_source_config, validate_spawn_source,
};
pub use records::{
    network_rank, workspace_rank, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentType, AgentTypeContract, AgentTypeLookup, AgentTypeRef, Budget, ConfigDigest,
    ConfigStatus, ContinuityMode, CredentialRef, InMemorySelectorCatalog, LifecycleMode,
    NetworkPolicy, PhysicalSafety, SandboxPolicyRef, SecurityContract, SourceConfig,
    SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef, TaskRequirement,
};
pub use requirement::{
    fold_generation_policy, AgentRequirementDraft, AgentRequirementPreferences, GenerationPolicy,
    TaskAgentRequirement,
};
pub use selector::{resolve_selector, AgentTypeSelector};
