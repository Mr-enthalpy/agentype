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
mod provisioning;
mod records;
mod requirement;
mod sandbox;
mod selector;

pub use canonical::{
    adapter_binding_policy_content_digest, agent_type_content_digest,
    binding_snapshot_content_digest, canonical_adapter_binding_policy_bytes,
    canonical_agent_type_bytes, canonical_binding_snapshot_bytes,
    canonical_capability_definition_bytes, canonical_capability_definition_bytes_v1,
    canonical_capability_definition_bytes_v2, canonical_generation_policy_bytes,
    canonical_json_body_digest, canonical_provisioning_binding_bytes,
    canonical_required_access_bytes, canonical_sandbox_policy_bytes,
    canonical_security_derivation_bytes, canonical_source_config_bytes,
    canonical_spawn_source_bytes, canonical_task_agent_requirement_bytes, canonicalize_agent_type,
    canonicalize_capability_map, canonicalize_claims, canonicalize_credential_refs,
    canonicalize_generation_policy, canonicalize_source_config, canonicalize_spawn_source,
    canonicalize_task_agent_requirement, capability_definition_content_digest, content_digest,
    credential_refs_digest, generation_policy_content_digest, provisioning_binding_content_digest,
    required_access_content_digest, sandbox_policy_content_digest,
    security_derivation_content_digest, source_config_content_digest, spawn_source_content_digest,
    task_agent_requirement_content_digest, CANONICAL_FORMAT_VERSION,
};
pub use capability::{
    value_satisfies, value_within, Assurance, CapabilityCatalog, CapabilityClaim,
    CapabilityDefinition, CapabilityPolarity, CapabilityRef, CapabilityValue, FileOperationClass,
    MatcherKind, NetworkEndpoint, NetworkTransport, PolicyAccessDimension, PolicyOperation,
    PolicyProjection, Quantity, SecurityClass, CAPABILITY_DEFINITION_CANONICAL_VERSION_V1,
    CAPABILITY_DEFINITION_CANONICAL_VERSION_V2,
};
pub use decode::{
    adapter_binding_policy_from_canonical_json, agent_type_from_canonical_json,
    binding_snapshot_from_canonical_json, capability_definition_from_canonical_json,
    generation_policy_from_canonical_json, provisioning_binding_from_canonical_json,
    sandbox_policy_from_canonical_json, security_derivation_from_canonical_json,
    source_config_revision_from_canonical_json, spawn_source_from_canonical_json,
    task_agent_requirement_from_canonical_json,
};
pub use error::ContractError;
pub use evidence::ResolvedProvisioningEvidence;
pub use ids::{
    AdapterPolicyId, AgentTypeId, CapabilityId, SandboxPolicyId, SourceConfigId, SpawnSourceId,
};
pub use matching::{match_existing_agents, ExistingAgentCandidate, TaskPlacement};
pub use predicates::{
    can_execute, can_execute_security_v1, can_provision, can_provision_task, is_valid_refinement,
    is_valid_refinement_with_policies, more_specific_for, validate_source_config,
    validate_spawn_source,
};
pub use provisioning::{
    BindingSnapshot, MaterializationDigest, ProvisioningBinding, RESOLVER_VERSION,
    RESOLVER_VERSION_FAMILY,
};
pub use records::{
    network_rank, required_lifecycle, workspace_rank, AdapterBindingPolicy, AdapterPolicyRef,
    AffinityConstraint, AgentType, AgentTypeContract, AgentTypeLookup, AgentTypeRef, Budget,
    ConfigDigest, ConfigStatus, ContinuityMode, CredentialRef, InMemorySelectorCatalog,
    LifecycleMode, NetworkPolicy, PhysicalSafety, SandboxPolicyRef, SecurityContract, SourceConfig,
    SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef, TaskRequirement,
};
pub use requirement::{
    fold_generation_policy, fold_generation_policy_security_v1, AgentRequirementDraft,
    GenerationPolicy, TaskAgentRequirement,
};
pub use sandbox::{
    check_required_access, is_within, meet, meet_optional_layer, project_capability_access,
    project_network, project_security_contract, project_workspace, prove_no_wider,
    validate_agent_type_coarse_fine, validate_capability_claims_against_policy,
    validate_generation_coarse_fine, validate_task_local_coarse_fine, AccessDeficit,
    CredentialAuthorizationId, CredentialDeliveryMode, CredentialGrant, EnforcementProfileRef,
    EnvironmentPolicy, FilePolicy, NameSet, NetworkEndpointSet, NormalizedSandboxPolicy, PathRoot,
    PathSelector, PathSet, PendingObligation, PolicyAccess, PolicyConflict, PolicyInput,
    RequiredAccess, ResourceCeiling, ResourceLimitName, ResourceScope, ResourceUnit,
    SandboxPolicyParts, SandboxPolicyVersion, SecurityContractMode, SecurityDerivation,
    SpawnSourceSecurityDeclaration, TaskSecurityBinding, ToolPolicy, WorkingDirectory,
    REQUIRED_ACCESS_SCHEMA_VERSION, SANDBOX_POLICY_MEET_ALGORITHM_VERSION,
    SANDBOX_POLICY_NORMALIZATION_VERSION, SANDBOX_POLICY_SCHEMA_VERSION,
};
pub use selector::{resolve_selector, AgentTypeSelector};
