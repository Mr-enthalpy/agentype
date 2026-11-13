//! M6-B.5 B5.2 imported enforcement qualification and the pure root-relative
//! compiler (ADR-0014 §2–§5, §13–§14).
//!
//! Nothing in this module performs I/O or creates physical objects. The
//! compiler consumes an [`EffectiveTaskAuthority`], the exact
//! [`AdapterBindingPolicy`], and an [`ImportedEnforcementProfile`], and returns
//! a **root-relative** [`PolicyEnforcementPlan`] plus the evidence subject and
//! the memoization key. `prepare` and physical start are later phases with
//! different powers.
//!
//! `ImportedEnforcementProfile` models an *imported* external fact: which
//! enforcement dimensions an exact physical domain can actually enforce. Its
//! only constructors are gated behind the same `provisioning-producer` feature
//! as the imported provisioning evidence. A **contract-only** host therefore has
//! no way to mint enforcement facts. This is a supported-surface boundary, not
//! an unforgeable capability: any host that also embeds `agentype-runtime` gets
//! the constructors through Cargo feature unification, because the runtime is
//! the importer. The load-bearing boundary is that no production API accepts a
//! caller-supplied profile and the authority transaction re-proves imported
//! facts (ADR-0014 §13).

use crate::authority::EffectiveTaskAuthority;
use crate::canonical::{
    adapter_binding_policy_content_digest, canonical_json_body_digest,
    required_access_content_digest, sandbox_policy_content_digest,
};
use crate::error::ContractError;
use crate::records::{
    AdapterBindingPolicy, ConfigDigest, ConfigStatus, SourceConfig, SourceConfigRef,
};
use crate::sandbox::{
    check_required_access, required_access_from_claims, CredentialAuthorizationId,
    EnforcementProfileRef, EnvironmentPolicy, FilePolicy, NetworkEndpointSet,
    NormalizedSandboxPolicy, PolicyAccess, ResourceCeiling, ResourceLimitName, ToolPolicy,
    WorkingDirectory,
};
use agentype_core::{GenerationId, LogicalAgentId, TaskId, UnixTime};
use serde_json::json;
use std::collections::BTreeSet;

/// Version tag of the canonical imported-profile document.
pub const ENFORCEMENT_PROFILE_SCHEMA_VERSION: u32 = 1;
/// Version tag of the compiled plan document.
pub const ENFORCEMENT_PLAN_SCHEMA_VERSION: u32 = 1;
/// The compiler protocol this build implements. It is part of the plan digest
/// and the memoization key: a compiler semantic change cannot reuse an old
/// result even when the inputs are byte-identical.
pub const ENFORCEMENT_COMPILER_PROTOCOL: &str = "agentype-enforcement-compile/1";

/// The frozen `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` profile identity
/// (ADR-0014 §14). This is a positive minimum target, not a claim that a
/// particular build already enforces it.
pub const POLICY_ONLY_REFERENCE_PROTOCOL: &str = "agentype-enforcement";
pub const POLICY_ONLY_REFERENCE_PROFILE: &str = "SECURITY_V1_POLICY_ONLY_REFERENCE_V1";
pub const POLICY_ONLY_REFERENCE_VERSION: u32 = 1;

/// One enforceable policy dimension.
///
/// An obligation exists only where the effective policy or a guarantee imposes
/// an enforceable restriction or a positive guarantee (ADR-0014 §13). A
/// normalized top value contributes no obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnforcementFeature {
    /// The policy constrains workspace writes or deletes (including deny-all).
    WorkspaceWriteConfinement,
    /// The policy constrains readable regions (not claimed by the policy-only
    /// reference profile).
    FileReadConfinement,
    /// The policy constrains executable regions.
    FileExecuteConfinement,
    /// The policy constrains directory visibility.
    FileVisibilityConfinement,
    /// The policy constrains tool/command names.
    ToolCommandAllowlist,
    /// The policy constrains network egress (including deny-all).
    NetworkEgressRestriction,
    /// The policy constrains the worker environment.
    EnvironmentAllowlist,
    /// The policy declares resource ceilings.
    ResourceLimit,
    /// The Task or AgentType requires attempt isolation.
    AttemptIsolation,
    /// The policy authorizes Agentype-managed credential delivery.
    CredentialDelivery,
}

impl EnforcementFeature {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceWriteConfinement => "WORKSPACE_WRITE_CONFINEMENT",
            Self::FileReadConfinement => "FILE_READ_CONFINEMENT",
            Self::FileExecuteConfinement => "FILE_EXECUTE_CONFINEMENT",
            Self::FileVisibilityConfinement => "FILE_VISIBILITY_CONFINEMENT",
            Self::ToolCommandAllowlist => "TOOL_COMMAND_ALLOWLIST",
            Self::NetworkEgressRestriction => "NETWORK_EGRESS_RESTRICTION",
            Self::EnvironmentAllowlist => "ENVIRONMENT_ALLOWLIST",
            Self::ResourceLimit => "RESOURCE_LIMIT",
            Self::AttemptIsolation => "ATTEMPT_ISOLATION",
            Self::CredentialDelivery => "CREDENTIAL_DELIVERY",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "WORKSPACE_WRITE_CONFINEMENT" => Self::WorkspaceWriteConfinement,
            "FILE_READ_CONFINEMENT" => Self::FileReadConfinement,
            "FILE_EXECUTE_CONFINEMENT" => Self::FileExecuteConfinement,
            "FILE_VISIBILITY_CONFINEMENT" => Self::FileVisibilityConfinement,
            "TOOL_COMMAND_ALLOWLIST" => Self::ToolCommandAllowlist,
            "NETWORK_EGRESS_RESTRICTION" => Self::NetworkEgressRestriction,
            "ENVIRONMENT_ALLOWLIST" => Self::EnvironmentAllowlist,
            "RESOURCE_LIMIT" => Self::ResourceLimit,
            "ATTEMPT_ISOLATION" => Self::AttemptIsolation,
            "CREDENTIAL_DELIVERY" => Self::CredentialDelivery,
            _ => return None,
        })
    }
}

/// The explicit enforcement subject a claim is relative to (ADR-0014 §14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnforcementSubject {
    /// The exact process the secured start creates, holds and releases.
    ControlledPrimaryProcess,
    /// A descendant the OS proves remains inside the same confinement domain.
    OsRestrictedDescendant,
    /// Work delegated to a service/harness that runs outside the proven domain.
    ExternallyDelegatedExecution,
    /// Any process whose identity, parentage or domain is not proven.
    UnprovenExternalProcess,
}

impl EnforcementSubject {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ControlledPrimaryProcess => "CONTROLLED_PRIMARY_PROCESS",
            Self::OsRestrictedDescendant => "OS_RESTRICTED_DESCENDANT",
            Self::ExternallyDelegatedExecution => "EXTERNALLY_DELEGATED_EXECUTION",
            Self::UnprovenExternalProcess => "UNPROVEN_EXTERNAL_PROCESS",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "CONTROLLED_PRIMARY_PROCESS" => Self::ControlledPrimaryProcess,
            "OS_RESTRICTED_DESCENDANT" => Self::OsRestrictedDescendant,
            "EXTERNALLY_DELEGATED_EXECUTION" => Self::ExternallyDelegatedExecution,
            "UNPROVEN_EXTERNAL_PROCESS" => Self::UnprovenExternalProcess,
            _ => return None,
        })
    }

    /// Whether this subject is inside a policy-only profile's claimed subject.
    pub fn is_inside_proven_domain(self) -> bool {
        matches!(
            self,
            Self::ControlledPrimaryProcess | Self::OsRestrictedDescendant
        )
    }
}

/// The effective guarantees a Task imposes beyond its authored policy bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectiveExecutionGuarantees {
    requires_attempt_isolation: bool,
}

impl EffectiveExecutionGuarantees {
    pub fn new(requires_attempt_isolation: bool) -> Self {
        Self {
            requires_attempt_isolation,
        }
    }

    /// Derive the guarantees from the composite authority: the Task's durable
    /// requirement and the bound AgentType's contract both count.
    pub fn from_authority(authority: &EffectiveTaskAuthority) -> Self {
        Self::new(authority.requires_attempt_isolation())
    }

    pub fn requires_attempt_isolation(&self) -> bool {
        self.requires_attempt_isolation
    }
}

/// The obligations derived through the single normative relation
/// `RequiredEnforcementFeatures(NormalizedPolicy, EffectiveExecutionGuarantees)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequiredEnforcementFeatures {
    features: BTreeSet<EnforcementFeature>,
}

impl RequiredEnforcementFeatures {
    pub fn derive(
        policy: &NormalizedSandboxPolicy,
        guarantees: EffectiveExecutionGuarantees,
    ) -> Self {
        let mut features = BTreeSet::new();
        let files = policy.files();
        if !files.write().is_any() || !files.delete().is_any() {
            features.insert(EnforcementFeature::WorkspaceWriteConfinement);
        }
        if !files.read().is_any() {
            features.insert(EnforcementFeature::FileReadConfinement);
        }
        if !files.execute().is_any() {
            features.insert(EnforcementFeature::FileExecuteConfinement);
        }
        if !files.visible().is_any() {
            features.insert(EnforcementFeature::FileVisibilityConfinement);
        }
        if !policy.tools().tools().is_any() || !policy.tools().commands().is_any() {
            features.insert(EnforcementFeature::ToolCommandAllowlist);
        }
        if !policy.network().is_any() {
            features.insert(EnforcementFeature::NetworkEgressRestriction);
        }
        if !policy.environment().allowed_names().is_any()
            || !policy.environment().required_values().is_empty()
        {
            features.insert(EnforcementFeature::EnvironmentAllowlist);
        }
        if !policy.resources().is_empty() {
            features.insert(EnforcementFeature::ResourceLimit);
        }
        if policy.attempt_isolation() || guarantees.requires_attempt_isolation() {
            features.insert(EnforcementFeature::AttemptIsolation);
        }
        if !policy.credentials().is_empty() {
            features.insert(EnforcementFeature::CredentialDelivery);
        }
        Self { features }
    }

    pub fn features(&self) -> &BTreeSet<EnforcementFeature> {
        &self.features
    }

    pub fn contains(&self, feature: EnforcementFeature) -> bool {
        self.features.contains(&feature)
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }
}

/// The per-dimension coverage an exact imported physical domain claims.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnforcementSupport {
    supported: BTreeSet<EnforcementFeature>,
}

impl EnforcementSupport {
    pub fn new(features: impl IntoIterator<Item = EnforcementFeature>) -> Self {
        Self {
            supported: features.into_iter().collect(),
        }
    }

    /// The frozen `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` support matrix: it
    /// claims workspace content-write confinement and an explicit environment
    /// allowlist, and nothing else (ADR-0014 §14).
    pub fn policy_only_reference() -> Self {
        Self::new([
            EnforcementFeature::WorkspaceWriteConfinement,
            EnforcementFeature::EnvironmentAllowlist,
        ])
    }

    pub fn supports(&self, feature: EnforcementFeature) -> bool {
        self.supported.contains(&feature)
    }

    pub fn features(&self) -> &BTreeSet<EnforcementFeature> {
        &self.supported
    }

    pub fn missing(&self, required: &RequiredEnforcementFeatures) -> BTreeSet<EnforcementFeature> {
        required
            .features
            .iter()
            .copied()
            .filter(|feature| !self.supported.contains(feature))
            .collect()
    }

    /// `obligations ⊆ support`.
    pub fn covers(&self, required: &RequiredEnforcementFeatures) -> bool {
        self.missing(required).is_empty()
    }
}

/// An exact imported enforcement profile: the policy/receipt protocol identity,
/// the physical domain's support matrix, and the subjects its claims are
/// relative to.
///
/// The constructors are compiled only under `provisioning-producer` because
/// these are imported external facts. A **contract-only** host cannot name them;
/// a host that also embeds the runtime can, because the runtime is the importer.
/// Treat constructor visibility as a supported-surface convenience, never as the
/// security proof: no production API accepts a caller-supplied profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedEnforcementProfile {
    reference: EnforcementProfileRef,
    protocol_version: u32,
    support: EnforcementSupport,
    covered_subjects: BTreeSet<EnforcementSubject>,
    profile_digest: String,
}

impl ImportedEnforcementProfile {
    /// Import one exact profile. Only the internal provisioning producer may
    /// call this.
    #[cfg(any(test, feature = "provisioning-producer"))]
    pub fn import(
        reference: EnforcementProfileRef,
        protocol_version: u32,
        support: EnforcementSupport,
        covered_subjects: impl IntoIterator<Item = EnforcementSubject>,
    ) -> Result<Self, ContractError> {
        if protocol_version == 0 {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "enforcement protocol version must be at least 1".into(),
            });
        }
        let covered_subjects: BTreeSet<EnforcementSubject> = covered_subjects.into_iter().collect();
        if covered_subjects.is_empty() {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "an imported enforcement profile must name its subject".into(),
            });
        }
        let profile_digest =
            profile_digest(&reference, protocol_version, &support, &covered_subjects);
        Ok(Self {
            reference,
            protocol_version,
            support,
            covered_subjects,
            profile_digest,
        })
    }

    /// Reconstruct an already-imported profile from its durable parts. This is
    /// the read path; it recomputes the digest instead of trusting the stored
    /// value.
    #[cfg(any(test, feature = "provisioning-producer"))]
    pub fn from_parts(
        reference: EnforcementProfileRef,
        protocol_version: u32,
        support: EnforcementSupport,
        covered_subjects: impl IntoIterator<Item = EnforcementSubject>,
        expected_digest: &str,
    ) -> Result<Self, ContractError> {
        let profile = Self::import(reference, protocol_version, support, covered_subjects)?;
        if profile.profile_digest() != expected_digest {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "imported enforcement profile digest does not describe its parts".into(),
            });
        }
        Ok(profile)
    }

    pub fn reference(&self) -> &EnforcementProfileRef {
        &self.reference
    }

    pub fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    pub fn support(&self) -> &EnforcementSupport {
        &self.support
    }

    pub fn covered_subjects(&self) -> &BTreeSet<EnforcementSubject> {
        &self.covered_subjects
    }

    pub fn profile_digest(&self) -> &str {
        &self.profile_digest
    }

    pub fn covers_subject(&self, subject: EnforcementSubject) -> bool {
        self.covered_subjects.contains(&subject)
    }
}

#[cfg(any(test, feature = "provisioning-producer"))]
fn profile_digest(
    reference: &EnforcementProfileRef,
    protocol_version: u32,
    support: &EnforcementSupport,
    covered_subjects: &BTreeSet<EnforcementSubject>,
) -> String {
    canonical_json_body_digest(&json!({
        "kind": "IMPORTED_ENFORCEMENT_PROFILE",
        "schema": ENFORCEMENT_PROFILE_SCHEMA_VERSION,
        "protocol": reference.protocol(),
        "profile": reference.profile(),
        "profile_version": reference.version(),
        "protocol_version": protocol_version,
        "supported": support.features().iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "subjects": covered_subjects.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    }))
}

/// The seven-part compile memoization key (ADR-0014 §4).
///
/// The effective-policy digest commits to canonical bytes, not to the
/// normalization and meet algorithm versions that produced them, so both
/// versions are explicit components.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompileCacheKey {
    effective_policy_digest: String,
    normalization_version: u32,
    meet_algorithm_version: u32,
    adapter_kind: String,
    adapter_binding_key: String,
    enforcement_profile_digest: String,
    compiler_protocol_version: String,
}

impl CompileCacheKey {
    pub fn new(
        effective_policy_digest: impl Into<String>,
        normalization_version: u32,
        meet_algorithm_version: u32,
        adapter_kind: impl Into<String>,
        adapter_binding_key: impl Into<String>,
        enforcement_profile_digest: impl Into<String>,
    ) -> Result<Self, ContractError> {
        let key = Self {
            effective_policy_digest: effective_policy_digest.into(),
            normalization_version,
            meet_algorithm_version,
            adapter_kind: adapter_kind.into(),
            adapter_binding_key: adapter_binding_key.into(),
            enforcement_profile_digest: enforcement_profile_digest.into(),
            compiler_protocol_version: ENFORCEMENT_COMPILER_PROTOCOL.to_string(),
        };
        for (value, what) in [
            (&key.effective_policy_digest, "effective policy digest"),
            (&key.adapter_kind, "adapter kind"),
            (&key.adapter_binding_key, "adapter binding key"),
            (
                &key.enforcement_profile_digest,
                "enforcement profile digest",
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ContractError::InvalidRef {
                    reason: format!("compile cache key {what} cannot be empty"),
                });
            }
        }
        if normalization_version == 0 || meet_algorithm_version == 0 {
            return Err(ContractError::InvalidRef {
                reason: "compile cache key algorithm versions must be at least 1".into(),
            });
        }
        Ok(key)
    }

    pub fn effective_policy_digest(&self) -> &str {
        &self.effective_policy_digest
    }

    pub fn normalization_version(&self) -> u32 {
        self.normalization_version
    }

    pub fn meet_algorithm_version(&self) -> u32 {
        self.meet_algorithm_version
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
    }

    pub fn enforcement_profile_digest(&self) -> &str {
        &self.enforcement_profile_digest
    }

    pub fn compiler_protocol_version(&self) -> &str {
        &self.compiler_protocol_version
    }
}

/// A **root-relative** enforcement plan. Logical selectors only; no concrete
/// jail path, and no directory is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyEnforcementPlan {
    effective_policy: NormalizedSandboxPolicy,
    obligations: RequiredEnforcementFeatures,
    reference: EnforcementProfileRef,
    plan_digest: String,
}

impl PolicyEnforcementPlan {
    pub fn effective_policy(&self) -> &NormalizedSandboxPolicy {
        &self.effective_policy
    }

    pub fn files(&self) -> &FilePolicy {
        self.effective_policy.files()
    }

    pub fn tools(&self) -> &ToolPolicy {
        self.effective_policy.tools()
    }

    pub fn network(&self) -> &NetworkEndpointSet {
        self.effective_policy.network()
    }

    pub fn environment(&self) -> &EnvironmentPolicy {
        self.effective_policy.environment()
    }

    pub fn resources(&self) -> &std::collections::BTreeMap<ResourceLimitName, ResourceCeiling> {
        self.effective_policy.resources()
    }

    /// Credential authorization identities only: the policy-only compiler never
    /// copies a grant, and a policy-only plan has none.
    pub fn credential_authorizations(&self) -> Vec<&CredentialAuthorizationId> {
        self.effective_policy.credentials().keys().collect()
    }

    pub fn working_directory(&self) -> &WorkingDirectory {
        self.effective_policy.working_directory()
    }

    pub fn attempt_isolation(&self) -> bool {
        self.effective_policy.attempt_isolation()
    }

    pub fn obligations(&self) -> &RequiredEnforcementFeatures {
        &self.obligations
    }

    pub fn enforcement_profile(&self) -> &EnforcementProfileRef {
        &self.reference
    }

    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }
}

/// The candidate-qualification evidence subject (ADR-0014 §5).
///
/// Compilation happens before SourceConfig attestation, so the compiled subject
/// is deliberately partial: [`SecurityEvidenceSubject::with_source_config`]
/// completes it once the exact config revision is attested. The subject keeps
/// the Task requirement and effective policy out of the Incarnation-scoped
/// materialization identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityEvidenceSubject {
    agent_type: crate::records::AgentTypeRef,
    task_requirement_digest: String,
    spawn_source: crate::records::SpawnSourceRef,
    source_config: Option<SourceConfigRef>,
    source_config_digest: Option<ConfigDigest>,
    adapter_policy: crate::records::AdapterPolicyRef,
    adapter_kind: String,
    adapter_binding_key: String,
    effective_policy_digest: String,
    enforcement_profile: EnforcementProfileRef,
    enforcement_profile_digest: String,
    plan_digest: String,
}

impl SecurityEvidenceSubject {
    #[allow(clippy::too_many_arguments)]
    fn new(
        agent_type: crate::records::AgentTypeRef,
        task_requirement_digest: String,
        spawn_source: crate::records::SpawnSourceRef,
        adapter_policy: crate::records::AdapterPolicyRef,
        adapter_kind: String,
        adapter_binding_key: String,
        effective_policy_digest: String,
        enforcement_profile: EnforcementProfileRef,
        enforcement_profile_digest: String,
        plan_digest: String,
    ) -> Self {
        Self {
            agent_type,
            task_requirement_digest,
            spawn_source,
            source_config: None,
            source_config_digest: None,
            adapter_policy,
            adapter_kind,
            adapter_binding_key,
            effective_policy_digest,
            enforcement_profile,
            enforcement_profile_digest,
            plan_digest,
        }
    }

    /// Bind the exact attested SourceConfig revision. The config must belong to
    /// this subject's source; a config from another source is a subject
    /// mismatch, not a weaker match.
    pub fn with_source_config(mut self, config: &SourceConfig) -> Result<Self, ContractError> {
        if config.config_ref.source() != &self.spawn_source {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "source config does not belong to the subject's spawn source".into(),
            });
        }
        self.source_config = Some(config.config_ref.clone());
        self.source_config_digest = Some(config.config_digest.clone());
        Ok(self)
    }

    pub fn agent_type(&self) -> &crate::records::AgentTypeRef {
        &self.agent_type
    }

    pub fn task_requirement_digest(&self) -> &str {
        &self.task_requirement_digest
    }

    pub fn spawn_source(&self) -> &crate::records::SpawnSourceRef {
        &self.spawn_source
    }

    pub fn source_config(&self) -> Option<&SourceConfigRef> {
        self.source_config.as_ref()
    }

    pub fn source_config_digest(&self) -> Option<&ConfigDigest> {
        self.source_config_digest.as_ref()
    }

    pub fn adapter_policy(&self) -> &crate::records::AdapterPolicyRef {
        &self.adapter_policy
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
    }

    pub fn effective_policy_digest(&self) -> &str {
        &self.effective_policy_digest
    }

    pub fn enforcement_profile(&self) -> &EnforcementProfileRef {
        &self.enforcement_profile
    }

    pub fn enforcement_profile_digest(&self) -> &str {
        &self.enforcement_profile_digest
    }

    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    pub fn is_complete(&self) -> bool {
        self.source_config.is_some() && self.source_config_digest.is_some()
    }

    /// Canonical digest of the complete subject. An incomplete subject MUST NOT
    /// be used as acquisition evidence.
    pub fn digest(&self) -> Result<String, ContractError> {
        if !self.is_complete() {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "a security evidence subject is incomplete without its source config"
                    .into(),
            });
        }
        Ok(canonical_json_body_digest(&json!({
            "kind": "SECURITY_EVIDENCE_SUBJECT",
            "schema": ENFORCEMENT_PROFILE_SCHEMA_VERSION,
            "agent_type": {
                "type_id": self.agent_type.id().as_str(),
                "revision": self.agent_type.revision(),
            },
            "task_requirement_digest": self.task_requirement_digest,
            "spawn_source": {
                "source_id": self.spawn_source.id().as_str(),
                "revision": self.spawn_source.revision(),
            },
            "source_config": self.source_config.as_ref().map(|c| json!({
                "source_id": c.source().id().as_str(),
                "source_revision": c.source().revision(),
                "config_id": c.config_id().as_str(),
                "revision": c.revision(),
            })),
            "source_config_digest": self.source_config_digest.as_ref().map(|d| d.as_str()),
            "adapter_policy": {
                "policy_id": self.adapter_policy.id().as_str(),
                "revision": self.adapter_policy.revision(),
            },
            "adapter_kind": self.adapter_kind,
            "adapter_binding_key": self.adapter_binding_key,
            "effective_policy_digest": self.effective_policy_digest,
            "enforcement_profile": {
                "protocol": self.enforcement_profile.protocol(),
                "profile": self.enforcement_profile.profile(),
                "version": self.enforcement_profile.version(),
            },
            "enforcement_profile_digest": self.enforcement_profile_digest,
            "plan_digest": self.plan_digest,
        })))
    }
}

/// The pure result of one bounded compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledExecutionSecurity {
    plan: PolicyEnforcementPlan,
    cache_key: CompileCacheKey,
    evidence_subject: SecurityEvidenceSubject,
}

impl CompiledExecutionSecurity {
    pub fn plan(&self) -> &PolicyEnforcementPlan {
        &self.plan
    }

    pub fn plan_digest(&self) -> &str {
        self.plan.plan_digest()
    }

    pub fn cache_key(&self) -> &CompileCacheKey {
        &self.cache_key
    }

    pub fn evidence_subject(&self) -> &SecurityEvidenceSubject {
        &self.evidence_subject
    }

    /// Complete the evidence subject once the exact SourceConfig is attested.
    pub fn with_source_config(mut self, config: &SourceConfig) -> Result<Self, ContractError> {
        self.evidence_subject = self.evidence_subject.with_source_config(config)?;
        Ok(self)
    }
}

/// Compile the root-relative plan and the evidence subject.
///
/// Steps, in the ADR-0014 §3 order:
/// 1. the imported profile must be the one the SpawnSource declaration named;
/// 2. the exact adapter binding must be active and belong to the profile's
///    protocol family;
/// 3. the effective policy's obligations must be covered by the profile;
/// 4. the plan is the logical effective policy; its digest commits to the
///    policy, the profile, and the compiler protocol.
///
/// A missing feature is a deterministic candidate ineligibility
/// (`SecurityUnenforceable`). A profile/authority mismatch is a subject
/// mismatch. Neither is a Task failure.
pub fn compile_execution_security(
    authority: &EffectiveTaskAuthority,
    binding: &AdapterBindingPolicy,
    profile: &ImportedEnforcementProfile,
) -> Result<CompiledExecutionSecurity, ContractError> {
    if authority.enforcement_profile() != profile.reference() {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the imported enforcement profile is not the one the source declared".into(),
        });
    }
    binding.validate()?;
    if binding.status != ConfigStatus::Active {
        return Err(ContractError::SecurityUnenforceable {
            reason: "the exact adapter binding is not active".into(),
        });
    }
    if binding.adapter_kind.trim().is_empty() {
        return Err(ContractError::InvalidRef {
            reason: "adapter binding kind cannot be empty".into(),
        });
    }

    let obligations = RequiredEnforcementFeatures::derive(
        authority.policy(),
        EffectiveExecutionGuarantees::from_authority(authority),
    );
    let missing = profile.support().missing(&obligations);
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|feature| feature.as_str()).collect();
        return Err(ContractError::SecurityUnenforceable {
            reason: format!(
                "enforcement profile {} does not cover {}",
                profile.reference().profile(),
                names.join(", ")
            ),
        });
    }

    let effective_policy = authority.policy().clone();
    let plan_digest = canonical_json_body_digest(&json!({
        "kind": "POLICY_ENFORCEMENT_PLAN",
        "schema": ENFORCEMENT_PLAN_SCHEMA_VERSION,
        "compiler_protocol": ENFORCEMENT_COMPILER_PROTOCOL,
        "effective_policy_digest": authority.effective_policy_digest(),
        "enforcement_profile_digest": profile.profile_digest(),
        "obligations": obligations.features().iter().map(|f| f.as_str()).collect::<Vec<_>>(),
    }));
    let cache_key = CompileCacheKey::new(
        authority.effective_policy_digest(),
        authority.normalization_version(),
        authority.meet_algorithm_version(),
        &binding.adapter_kind,
        &binding.binding_ref,
        profile.profile_digest(),
    )?;
    let plan = PolicyEnforcementPlan {
        effective_policy,
        obligations,
        reference: profile.reference().clone(),
        plan_digest,
    };
    let evidence_subject = SecurityEvidenceSubject::new(
        authority.agent_type_ref().clone(),
        authority.requirement_digest().to_string(),
        authority.spawn_source_ref().clone(),
        binding.policy_ref.clone(),
        binding.adapter_kind.clone(),
        binding.binding_ref.clone(),
        authority.effective_policy_digest().to_string(),
        profile.reference().clone(),
        profile.profile_digest().to_string(),
        plan.plan_digest.clone(),
    );

    Ok(CompiledExecutionSecurity {
        plan,
        cache_key,
        evidence_subject,
    })
}

/// Qualify one candidate: prove the candidate's authored ceiling bounds the
/// effective Task policy with the real policy order, re-check the Task's coarse
/// modes and its positive access floor on the complete policy, then compile.
///
/// This is the acquisition-side predicate. It grants no Attempt/Lease
/// authority; the authority transaction still owns that.
#[allow(clippy::too_many_arguments)]
pub fn qualify_security_candidate(
    authority: &EffectiveTaskAuthority,
    requirement: &crate::requirement::TaskAgentRequirement,
    agent: &crate::authority::SecurityRevision<'_>,
    binding: &AdapterBindingPolicy,
    profile: &ImportedEnforcementProfile,
    catalog: &crate::capability::CapabilityCatalog,
) -> Result<CompiledExecutionSecurity, ContractError> {
    if authority.requirement_digest()
        != crate::canonical::task_agent_requirement_content_digest(requirement)
    {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the composite authority was derived from a different Task requirement".into(),
        });
    }
    if authority.agent_type_ref() != &agent.agent_type().type_ref {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the composite authority was derived from a different AgentType revision"
                .into(),
        });
    }
    if !agent.is_security_v1() {
        return Err(ContractError::SecurityUnenforceable {
            reason: "a COARSE_V1 AgentType revision cannot execute a SECURITY_V1 Task".into(),
        });
    }
    let agent_policy = agent.authored_policy().ok_or_else(|| {
        ContractError::InvariantViolation(
            "a SECURITY_V1 AgentType revision lost its authored policy".into(),
        )
    })?;
    if authority.agent_policy_digest() != sandbox_policy_content_digest(agent_policy) {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the composite authority was derived from different AgentType policy bytes"
                .into(),
        });
    }
    crate::predicates::can_execute_security_v1(
        agent.agent_type(),
        &requirement.hard,
        authority.policy(),
        agent_policy,
        catalog,
    )?;
    authority.validate_coarse_modes(requirement)?;
    // ADR-0013 §5 Candidate row: every positive Task requirement is
    // re-evaluated against the complete `P_eff`. Admission proved the floor
    // against `TaskAuthority(meet(P_generation, P_task))` only; the AgentType
    // and SpawnSource terms can delete a capability the Task requires, so the
    // floor MUST be rebuilt through the same catalog projection admission used
    // and checked again here. A deficit is candidate-local
    // (`SecurityUnenforceable`), never a Task failure.
    let required_access =
        required_access_from_claims(&requirement.hard.required_capabilities, catalog)?;
    if required_access_content_digest(&required_access) != authority.required_access_digest() {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the composite authority was derived from a different Task access floor".into(),
        });
    }
    check_required_access(
        &required_access,
        &PolicyAccess::from_policy(authority.policy()),
    )
    .map_err(|deficit| ContractError::SecurityUnenforceable {
        reason: format!(
            "the complete effective policy does not satisfy the Task's positive access floor: \
             {deficit}"
        ),
    })?;
    compile_execution_security(authority, binding, profile)
}

/// The ordered, deduplicated candidate identity set one acquisition round sees.
///
/// The digest is stable under input order, so two rounds that see the same
/// candidate identities produce the same diagnostic key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateDomain {
    digest: String,
    candidate_count: usize,
}

impl CandidateDomain {
    pub fn from_ids(ids: impl IntoIterator<Item = LogicalAgentId>) -> Self {
        let ids: Vec<String> = ids
            .into_iter()
            .map(|id| id.as_str().to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let candidate_count = ids.len();
        let digest = canonical_json_body_digest(&json!({
            "kind": "SECURITY_CANDIDATE_DOMAIN",
            "agents": ids,
        }));
        Self {
            digest,
            candidate_count,
        }
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn candidate_count(&self) -> usize {
        self.candidate_count
    }
}

/// The fixed per-round budget (ADR-0014 §4). Both bounds are required: a
/// candidate cap without a compile cap could still spend unbounded compile
/// work on one wide candidate set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcquisitionBudget {
    max_candidates: usize,
    max_compiles: usize,
}

impl AcquisitionBudget {
    pub fn new(max_candidates: usize, max_compiles: usize) -> Result<Self, ContractError> {
        if max_candidates == 0 || max_compiles == 0 {
            return Err(ContractError::InvalidNumber {
                field: "security acquisition budget".into(),
            });
        }
        Ok(Self {
            max_candidates,
            max_compiles,
        })
    }

    pub fn max_candidates(&self) -> usize {
        self.max_candidates
    }

    pub fn max_compiles(&self) -> usize {
        self.max_compiles
    }

    /// Require that the whole candidate set fits the budget. There is no
    /// partial-success path: callers MUST treat `Err` as a whole-round defer.
    pub fn require_whole_round(&self, cost: AcquisitionCost) -> Result<(), BudgetExhaustion> {
        let limit = if cost.candidates > self.max_candidates {
            Some(BudgetLimit::CandidateCount)
        } else if cost.compiles > self.max_compiles {
            Some(BudgetLimit::CompileCount)
        } else {
            None
        };
        match limit {
            None => Ok(()),
            Some(limit) => Err(BudgetExhaustion {
                limit,
                cost,
                budget: *self,
            }),
        }
    }
}

/// The observed cost of one attempted complete round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcquisitionCost {
    candidates: usize,
    compiles: usize,
}

impl AcquisitionCost {
    pub fn new(candidates: usize, compiles: usize) -> Self {
        Self {
            candidates,
            compiles,
        }
    }

    pub fn candidates(&self) -> usize {
        self.candidates
    }

    pub fn compiles(&self) -> usize {
        self.compiles
    }
}

/// Which budget bound the complete round exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BudgetLimit {
    CandidateCount,
    CompileCount,
}

impl BudgetLimit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CandidateCount => "CANDIDATE_COUNT",
            Self::CompileCount => "COMPILE_COUNT",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "CANDIDATE_COUNT" => Self::CandidateCount,
            "COMPILE_COUNT" => Self::CompileCount,
            _ => return None,
        })
    }
}

/// A whole-round defer caused by a fixed budget that the complete candidate set
/// cannot fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetExhaustion {
    limit: BudgetLimit,
    cost: AcquisitionCost,
    budget: AcquisitionBudget,
}

impl BudgetExhaustion {
    pub fn limit(&self) -> BudgetLimit {
        self.limit
    }

    pub fn cost(&self) -> AcquisitionCost {
        self.cost
    }

    pub fn budget(&self) -> AcquisitionBudget {
        self.budget
    }
}

/// One durable, operator-actionable security-acquisition block.
///
/// This is a liveness diagnostic, not Task semantic authority: it creates no
/// Attempt, emits no Task NACK, and never selects among a compiled subset. It is
/// released only by an explicit operator action or a configuration/profile
/// change (ADR-0014 §4).
#[derive(Clone, Debug, PartialEq)]
pub struct SecurityAcquisitionBlock {
    task_id: TaskId,
    generation_id: GenerationId,
    domain: CandidateDomain,
    binding_profile_digest: String,
    cost: AcquisitionCost,
    budget: AcquisitionBudget,
    limit: BudgetLimit,
    status: SecurityAcquisitionBlockStatus,
    created_at: UnixTime,
    released_at: Option<UnixTime>,
    release_reason: Option<SecurityAcquisitionRelease>,
}

impl SecurityAcquisitionBlock {
    /// Build the block from a whole-round defer. The generation is the Task's
    /// durable dispatch generation; it is recorded for operator context only.
    #[allow(clippy::too_many_arguments)]
    pub fn from_budget_defer(
        task_id: TaskId,
        generation_id: GenerationId,
        domain: CandidateDomain,
        binding_profile_digest: impl Into<String>,
        exhaustion: BudgetExhaustion,
        now: UnixTime,
    ) -> Result<Self, ContractError> {
        let binding_profile_digest = binding_profile_digest.into();
        if binding_profile_digest.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "security acquisition block binding/profile digest cannot be empty".into(),
            });
        }
        if !now.is_finite() || now < 0.0 {
            return Err(ContractError::InvalidNumber {
                field: "security acquisition block timestamp".into(),
            });
        }
        Ok(Self {
            task_id,
            generation_id,
            domain,
            binding_profile_digest,
            cost: exhaustion.cost,
            budget: exhaustion.budget,
            limit: exhaustion.limit,
            status: SecurityAcquisitionBlockStatus::Blocked,
            created_at: now,
            released_at: None,
            release_reason: None,
        })
    }

    /// Reconstruct a durable row. Used by the storage read path.
    #[allow(clippy::too_many_arguments)]
    pub fn from_durable_parts(
        task_id: TaskId,
        generation_id: GenerationId,
        candidate_domain_digest: String,
        candidate_count: usize,
        binding_profile_digest: String,
        cost: AcquisitionCost,
        budget: AcquisitionBudget,
        limit: BudgetLimit,
        status: SecurityAcquisitionBlockStatus,
        created_at: UnixTime,
        released_at: Option<UnixTime>,
        release_reason: Option<SecurityAcquisitionRelease>,
    ) -> Result<Self, ContractError> {
        if candidate_domain_digest.trim().is_empty() || binding_profile_digest.trim().is_empty() {
            return Err(ContractError::InvariantViolation(
                "a durable security acquisition block has an empty key digest".into(),
            ));
        }
        match (status, released_at, release_reason) {
            (SecurityAcquisitionBlockStatus::Blocked, None, None)
            | (SecurityAcquisitionBlockStatus::Released, Some(_), Some(_)) => {}
            _ => {
                return Err(ContractError::InvariantViolation(
                    "a durable security acquisition block has inconsistent release fields".into(),
                ))
            }
        }
        Ok(Self {
            task_id,
            generation_id,
            domain: CandidateDomain {
                digest: candidate_domain_digest,
                candidate_count,
            },
            binding_profile_digest,
            cost,
            budget,
            limit,
            status,
            created_at,
            released_at,
            release_reason,
        })
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn generation_id(&self) -> &GenerationId {
        &self.generation_id
    }

    pub fn candidate_domain(&self) -> &CandidateDomain {
        &self.domain
    }

    pub fn candidate_domain_digest(&self) -> &str {
        self.domain.digest()
    }

    pub fn candidate_count(&self) -> usize {
        self.domain.candidate_count()
    }

    pub fn binding_profile_digest(&self) -> &str {
        &self.binding_profile_digest
    }

    pub fn cost(&self) -> AcquisitionCost {
        self.cost
    }

    pub fn budget(&self) -> AcquisitionBudget {
        self.budget
    }

    pub fn limit(&self) -> BudgetLimit {
        self.limit
    }

    pub fn status(&self) -> SecurityAcquisitionBlockStatus {
        self.status
    }

    pub fn created_at(&self) -> UnixTime {
        self.created_at
    }

    pub fn released_at(&self) -> Option<UnixTime> {
        self.released_at
    }

    pub fn release_reason(&self) -> Option<SecurityAcquisitionRelease> {
        self.release_reason
    }

    /// The diagnostic reason. It is derived, not stored: the durable row already
    /// records the budget and the observed cost class.
    pub fn reason(&self) -> String {
        format!(
            "the complete security candidate set exceeds the fixed {} budget",
            match self.limit {
                BudgetLimit::CandidateCount => "candidate-count",
                BudgetLimit::CompileCount => "compile-count",
            }
        )
    }

    /// The only ways this block may be cleared.
    pub fn release_condition(&self) -> &'static str {
        "an explicit operator action, or a configuration/profile change that narrows the candidate domain or raises the budget"
    }

    pub fn is_blocked(&self) -> bool {
        self.status == SecurityAcquisitionBlockStatus::Blocked
    }

    /// Apply an explicit release. Idempotent for an already-released block.
    pub fn release(
        &mut self,
        release: SecurityAcquisitionRelease,
        now: UnixTime,
    ) -> Result<(), ContractError> {
        if !now.is_finite() || now < 0.0 {
            return Err(ContractError::InvalidNumber {
                field: "security acquisition block release timestamp".into(),
            });
        }
        if self.status == SecurityAcquisitionBlockStatus::Released {
            return Ok(());
        }
        self.status = SecurityAcquisitionBlockStatus::Released;
        self.released_at = Some(now);
        self.release_reason = Some(release);
        Ok(())
    }

    /// Whether a newly observed round is the same indistinguishable condition.
    /// An identical key MUST NOT emit a second diagnostic.
    pub fn same_condition(&self, domain: &CandidateDomain, binding_profile_digest: &str) -> bool {
        self.domain.digest() == domain.digest()
            && self.binding_profile_digest == binding_profile_digest
    }
}

/// Why a security acquisition block was released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecurityAcquisitionRelease {
    OperatorAction,
    ConfigurationChanged,
}

impl SecurityAcquisitionRelease {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OperatorAction => "OPERATOR_ACTION",
            Self::ConfigurationChanged => "CONFIGURATION_CHANGED",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "OPERATOR_ACTION" => Self::OperatorAction,
            "CONFIGURATION_CHANGED" => Self::ConfigurationChanged,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecurityAcquisitionBlockStatus {
    Blocked,
    Released,
}

impl SecurityAcquisitionBlockStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "BLOCKED",
            Self::Released => "RELEASED",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "BLOCKED" => Self::Blocked,
            "RELEASED" => Self::Released,
            _ => return None,
        })
    }
}

/// The durable key that scopes one block's condition: the exact adapter
/// binding/policy and imported profile. A change here is a new diagnostic, not
/// a retry of the old one.
pub fn binding_profile_digest(
    binding: &AdapterBindingPolicy,
    profile: &ImportedEnforcementProfile,
) -> String {
    canonical_json_body_digest(&json!({
        "kind": "SECURITY_BINDING_PROFILE",
        "schema": ENFORCEMENT_PLAN_SCHEMA_VERSION,
        "adapter_binding_policy_digest": adapter_binding_policy_content_digest(binding),
        "enforcement_profile_digest": profile.profile_digest(),
        "compiler_protocol": ENFORCEMENT_COMPILER_PROTOCOL,
    }))
}
