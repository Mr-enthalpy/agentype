//! M6-B.5 B5.2 imported enforcement qualification and the pure root-relative
//! compiler (ADR-0014 §2–§5, §13–§14).
//!
//! Nothing in this module performs I/O or creates physical objects. The
//! compiler consumes an [`EffectiveTaskAuthority`] and a
//! [`ValidatedSourceBinding`] — the source's own adapter policy, the exact
//! [`AdapterBindingPolicy`], and the [`ImportedEnforcementProfile`] that was
//! imported for that binding — and returns a **root-relative**
//! [`PolicyEnforcementPlan`] plus the evidence subject and the memoization key.
//! `prepare` and physical start are later phases with different powers.
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
    AdapterBindingPolicy, ConfigDigest, ConfigStatus, SourceConfig, SourceConfigRef, SourceStatus,
    SpawnSource,
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
///
/// `/2` records that a profile names the adapter binding it was imported for.
/// `/3` corrects *which* identity that is: B.4 `binding_ref` is a stable alias
/// that resolves to the exact installed physical `adapter_binding_key`, so the
/// profile, the evidence subject and the compile cache key must commit to the
/// resolved physical key, not the alias. A `/2` profile digest and every digest
/// that commits to it (the evidence subject, the compile cache key, the plan,
/// and the durable binding/profile key) are different identities.
pub const ENFORCEMENT_PROFILE_SCHEMA_VERSION: u32 = 3;
/// Version tag of the canonical trusted-importer attestation document.
pub const ENFORCEMENT_PROFILE_ATTESTATION_SCHEMA_VERSION: u32 = 1;
/// The attestation protocol this build accepts.
///
/// An attestation is a separate imported fact from the profile it names: the
/// profile digest proves only internal consistency, while the attestation says
/// which trusted importer observed that exact physical domain and when the
/// observation is valid. A row written under another protocol cannot be
/// interpreted under these rules and is rejected rather than reinterpreted.
pub const ENFORCEMENT_PROFILE_ATTESTATION_PROTOCOL: &str = "agentype-enforcement-profile-attest/1";
/// Version tag of the compiled plan document.
pub const ENFORCEMENT_PLAN_SCHEMA_VERSION: u32 = 2;
/// Version tag of the canonical execution-guarantee document that the compile
/// cache key commits to.
pub const ENFORCEMENT_GUARANTEES_SCHEMA_VERSION: u32 = 1;
/// The compiler protocol this build implements. It is part of the plan digest
/// and the memoization key: a compiler semantic change cannot reuse an old
/// result even when the inputs are byte-identical.
///
/// `/2` records that the plan and the cache key now carry the composed
/// execution guarantees, so a `/1` result must never be reused. `/3` records
/// that credential delivery is a request-scoped obligation checked against the
/// attested `SourceConfig`, not a policy-derived one, so a `/2` plan for a
/// credential-bearing policy is a different plan.
pub const ENFORCEMENT_COMPILER_PROTOCOL: &str = "agentype-enforcement-compile/3";

/// The frozen `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` profile identity
/// (ADR-0014 §14). This is a positive minimum target, not a claim that a
/// particular build already enforces it.
pub const POLICY_ONLY_REFERENCE_PROTOCOL: &str = "agentype-enforcement";
pub const POLICY_ONLY_REFERENCE_PROFILE: &str = "SECURITY_V1_POLICY_ONLY_REFERENCE_V1";
pub const POLICY_ONLY_REFERENCE_VERSION: u32 = 1;

/// Whether `reference` is the *exact* frozen
/// `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` identity.
///
/// The comparison is over all three identity components. A profile that reuses
/// the name with another protocol or version is a different profile and is not
/// silently held to (or excused from) the frozen ceiling; only the exact
/// identity carries the fixed capability claim of ADR-0014 §14.
#[cfg(any(test, feature = "provisioning-producer"))]
fn is_policy_only_reference(reference: &EnforcementProfileRef) -> bool {
    reference.protocol() == POLICY_ONLY_REFERENCE_PROTOCOL
        && reference.profile() == POLICY_ONLY_REFERENCE_PROFILE
        && reference.version() == POLICY_ONLY_REFERENCE_VERSION
}

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
    /// The domain can deliver an Agentype-managed credential when the exact
    /// attested config requests one. Authorization alone does not create a
    /// delivery obligation.
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

/// The composed execution guarantees a Task imposes.
///
/// A guarantee is a disjunction, not an authorization intersection. It is carried
/// separately from the operation-set policy bytes so that a Task-only or
/// AgentType-only requirement survives a wider effective policy, and it also
/// includes the effective policy bit, which already folds every authored layer
/// (Generation, Task and SpawnSource). The plan accessor and the obligation set
/// therefore read the same value and cannot disagree.
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
    /// requirement, the bound AgentType's contract and the effective policy bit
    /// all count. A meet is a disjunction for a guarantee, so no layer may clear
    /// one.
    pub fn from_authority(authority: &EffectiveTaskAuthority) -> Self {
        Self::new(authority.requires_attempt_isolation() || authority.policy().attempt_isolation())
    }

    pub fn requires_attempt_isolation(&self) -> bool {
        self.requires_attempt_isolation
    }
}

/// Canonical identity of the composed execution guarantees.
///
/// A guarantee contributes obligations without changing the effective-policy
/// bytes, so the policy digest alone cannot distinguish two compile inputs whose
/// obligation sets differ. The compile cache key therefore commits to this
/// digest is an explicit key component (ADR-0014 §4).
pub fn effective_execution_guarantees_digest(guarantees: EffectiveExecutionGuarantees) -> String {
    canonical_json_body_digest(&json!({
        "kind": "EFFECTIVE_EXECUTION_GUARANTEES",
        "schema": ENFORCEMENT_GUARANTEES_SCHEMA_VERSION,
        "requires_attempt_isolation": guarantees.requires_attempt_isolation(),
    }))
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
        // Credential delivery is deliberately *not* derived here. The effective
        // policy's credential map is an authorization ceiling; only a non-empty
        // `SourceConfig.credential_refs` requests a delivery (ADR-0013 §5), and
        // that request is not known until the exact config is attested
        // (ADR-0014 §3). Deriving the obligation from the ceiling would refuse a
        // policy-only profile for a config that asks for no credential. The
        // request is checked when the config is bound to the compiled plan.
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

    /// `self ⊆ ceiling`. A profile may be more conservative than its reference
    /// matrix, never wider.
    pub fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.supported.is_subset(&ceiling.supported)
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
/// the exact physical adapter domain it was imported for, the domain's support
/// matrix, and the subjects its claims are relative to.
///
/// A profile is an imported fact about one physical domain, so it records the
/// `(adapter_kind, adapter_binding_key)` pair that domain was imported under.
/// B.4 `AdapterBindingPolicy.binding_ref` is only a stable alias that resolves to
/// that key, and the key — not the alias — is what the Execution and its
/// `BindingSnapshot` freeze (ADR-0008). A profile imported for physical domain
/// K1 therefore cannot be compiled against a selection for K2, even when both
/// are reached through the same alias: the [`ValidatedSourceBinding`] composite
/// cross-checks both fields before the support matrix is used.
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
    adapter_kind: String,
    adapter_binding_key: String,
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
        adapter_kind: impl Into<String>,
        adapter_binding_key: impl Into<String>,
        support: EnforcementSupport,
        covered_subjects: impl IntoIterator<Item = EnforcementSubject>,
    ) -> Result<Self, ContractError> {
        if protocol_version == 0 {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "enforcement protocol version must be at least 1".into(),
            });
        }
        let adapter_kind = adapter_kind.into();
        if adapter_kind.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "an imported enforcement profile must name its adapter kind".into(),
            });
        }
        let adapter_binding_key = adapter_binding_key.into();
        if adapter_binding_key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "an imported enforcement profile must name its exact adapter binding key"
                    .into(),
            });
        }
        let covered_subjects: BTreeSet<EnforcementSubject> = covered_subjects.into_iter().collect();
        if covered_subjects.is_empty() {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "an imported enforcement profile must name its subject".into(),
            });
        }
        // ADR-0014 §14: the frozen reference profile is a **fixed** capability
        // claim, not a floor to widen. It is a positive minimum target for a
        // single policy/receipt protocol, and the platform may be more
        // conservative, never broader. A profile that needs another dimension
        // must be a new versioned profile identity, not a wider support matrix
        // on this one.
        if is_policy_only_reference(&reference)
            && !support.is_subset_of(&EnforcementSupport::policy_only_reference())
        {
            let over: Vec<&str> = support
                .features()
                .iter()
                .filter(|feature| !EnforcementSupport::policy_only_reference().supports(**feature))
                .map(|feature| feature.as_str())
                .collect();
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "the frozen {} profile cannot claim {}",
                    POLICY_ONLY_REFERENCE_PROFILE,
                    over.join(", ")
                ),
            });
        }
        // ADR-0014 §14: the policy-only reference protocol claims only the proven
        // domain. An externally delegated or unproven subject requires a
        // separately authored profile, not a wider subject set on this protocol.
        if reference.protocol() == POLICY_ONLY_REFERENCE_PROTOCOL
            && covered_subjects
                .iter()
                .any(|subject| !subject.is_inside_proven_domain())
        {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "the policy-only reference protocol cannot claim a subject outside the \
                         proven domain"
                    .into(),
            });
        }
        let profile_digest = profile_digest(
            &reference,
            protocol_version,
            &adapter_kind,
            &adapter_binding_key,
            &support,
            &covered_subjects,
        );
        Ok(Self {
            reference,
            protocol_version,
            adapter_kind,
            adapter_binding_key,
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
        adapter_kind: impl Into<String>,
        adapter_binding_key: impl Into<String>,
        support: EnforcementSupport,
        covered_subjects: impl IntoIterator<Item = EnforcementSubject>,
        expected_digest: &str,
    ) -> Result<Self, ContractError> {
        let profile = Self::import(
            reference,
            protocol_version,
            adapter_kind,
            adapter_binding_key,
            support,
            covered_subjects,
        )?;
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

    /// The adapter kind this profile was imported for.
    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    /// The exact physical adapter binding key this profile was imported for.
    ///
    /// This is the resolved `AdapterBindingKey`, never the B.4 `binding_ref`
    /// alias: the alias can point at a different physical domain after
    /// re-resolution, while the key is what the Execution freezes.
    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
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
    adapter_kind: &str,
    adapter_binding_key: &str,
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
        "adapter_kind": adapter_kind,
        "adapter_binding_key": adapter_binding_key,
        "supported": support.features().iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "subjects": covered_subjects.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    }))
}

/// A trusted-importer attestation for one exact imported profile.
///
/// A profile digest proves only that the document is internally consistent: the
/// same caller that supplies the support matrix, covered subjects and physical
/// binding key also computes the digest. This record is the missing external
/// fact. The provisioning producer writes it once, keyed by the exact profile
/// digest, and binds it to the importer identity, the attestation protocol, the
/// exact physical domain and a validity window. Acquisition reads it
/// transactionally and rejects a profile with no matching fresh row, so a caller
/// cannot widen support, forge subject coverage, replay stale physical-domain
/// evidence, or move the physical key under a stable alias and still be
/// accepted.
///
/// The constructor is gated with the profile importer because it is the
/// producer's write path. [`Self::from_parts`] is the durable read path: it
/// recomputes the digest from the stored parts rather than trusting the stored
/// value, so a tampered row fails closed. Constructing an attestation is not
/// authority by itself; only the durable row that acquisition re-reads is.
#[derive(Clone, Debug, PartialEq)]
pub struct EnforcementProfileAttestation {
    profile_digest: String,
    importer_id: String,
    importer_protocol: String,
    adapter_kind: String,
    adapter_binding_key: String,
    issued_at: UnixTime,
    valid_until: UnixTime,
    attestation_digest: String,
}

impl EnforcementProfileAttestation {
    /// Attest one imported profile. Only the internal provisioning producer
    /// may write this fact.
    #[cfg(any(test, feature = "provisioning-producer"))]
    pub fn new(
        profile: &ImportedEnforcementProfile,
        importer_id: impl Into<String>,
        importer_protocol: impl Into<String>,
        issued_at: UnixTime,
        valid_until: UnixTime,
    ) -> Result<Self, ContractError> {
        Self::validated_parts(
            profile.profile_digest().to_string(),
            importer_id.into(),
            importer_protocol.into(),
            profile.adapter_kind().to_string(),
            profile.adapter_binding_key().to_string(),
            issued_at,
            valid_until,
        )
    }

    /// Reconstruct an attestation from its durable parts and verify that the
    /// stored digest describes them. This is the read path; it never trusts a
    /// stored digest on its own.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        profile_digest: impl Into<String>,
        importer_id: impl Into<String>,
        importer_protocol: impl Into<String>,
        adapter_kind: impl Into<String>,
        adapter_binding_key: impl Into<String>,
        issued_at: UnixTime,
        valid_until: UnixTime,
        expected_digest: &str,
    ) -> Result<Self, ContractError> {
        let attestation = Self::validated_parts(
            profile_digest.into(),
            importer_id.into(),
            importer_protocol.into(),
            adapter_kind.into(),
            adapter_binding_key.into(),
            issued_at,
            valid_until,
        )?;
        if attestation.attestation_digest() != expected_digest {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "trusted-importer attestation digest does not describe its parts".into(),
            });
        }
        Ok(attestation)
    }

    fn validated_parts(
        profile_digest: String,
        importer_id: String,
        importer_protocol: String,
        adapter_kind: String,
        adapter_binding_key: String,
        issued_at: UnixTime,
        valid_until: UnixTime,
    ) -> Result<Self, ContractError> {
        if profile_digest.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "a trusted-importer attestation must name its imported profile".into(),
            });
        }
        if importer_id.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "a trusted-importer attestation must name its importer".into(),
            });
        }
        if importer_protocol != ENFORCEMENT_PROFILE_ATTESTATION_PROTOCOL {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "the trusted-importer attestation names an unsupported protocol".into(),
            });
        }
        if adapter_kind.trim().is_empty() || adapter_binding_key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason:
                    "a trusted-importer attestation must name the exact physical adapter domain"
                        .into(),
            });
        }
        if !issued_at.is_finite() || issued_at < 0.0 {
            return Err(ContractError::InvalidNumber {
                field: "trusted-importer attestation issued_at".into(),
            });
        }
        if !valid_until.is_finite() || valid_until <= issued_at {
            return Err(ContractError::InvalidNumber {
                field: "trusted-importer attestation valid_until".into(),
            });
        }
        let attestation_digest = attestation_digest(
            &profile_digest,
            &importer_id,
            &importer_protocol,
            &adapter_kind,
            &adapter_binding_key,
            issued_at,
            valid_until,
        );
        Ok(Self {
            profile_digest,
            importer_id,
            importer_protocol,
            adapter_kind,
            adapter_binding_key,
            issued_at,
            valid_until,
            attestation_digest,
        })
    }

    /// The exact imported profile digest this attestation covers.
    pub fn profile_digest(&self) -> &str {
        &self.profile_digest
    }

    /// The trusted importer that observed this physical domain.
    pub fn importer_id(&self) -> &str {
        &self.importer_id
    }

    pub fn importer_protocol(&self) -> &str {
        &self.importer_protocol
    }

    /// The exact adapter kind the profile was imported for.
    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    /// The exact resolved physical adapter binding key the profile was imported
    /// for. This is never the B.4 `binding_ref` alias.
    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
    }

    pub fn issued_at(&self) -> UnixTime {
        self.issued_at
    }

    pub fn valid_until(&self) -> UnixTime {
        self.valid_until
    }

    pub fn attestation_digest(&self) -> &str {
        &self.attestation_digest
    }

    /// Whether the observation is valid at `now`. The interval is half-open:
    /// `issued_at <= now < valid_until`.
    pub fn is_fresh(&self, now: UnixTime) -> bool {
        now.is_finite() && now >= self.issued_at && now < self.valid_until
    }

    /// Whether this attestation covers exactly `profile` and its physical
    /// domain. The digest already commits to the support matrix and subjects,
    /// so an internally consistent but wider profile cannot match.
    pub fn validate_for_profile(
        &self,
        profile: &ImportedEnforcementProfile,
    ) -> Result<(), ContractError> {
        if self.profile_digest != profile.profile_digest() {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "the trusted-importer attestation names a different imported profile"
                    .into(),
            });
        }
        if self.adapter_kind != profile.adapter_kind()
            || self.adapter_binding_key != profile.adapter_binding_key()
        {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason:
                    "the trusted-importer attestation names a different physical adapter domain"
                        .into(),
            });
        }
        Ok(())
    }

    /// The acquisition check: exact profile, exact physical domain, and a
    /// currently valid observation.
    pub fn validate_at(
        &self,
        profile: &ImportedEnforcementProfile,
        now: UnixTime,
    ) -> Result<(), ContractError> {
        self.validate_for_profile(profile)?;
        if !self.is_fresh(now) {
            return Err(ContractError::SecurityUnenforceable {
                reason: "the trusted-importer attestation is not fresh at acquisition time".into(),
            });
        }
        Ok(())
    }
}

fn attestation_digest(
    profile_digest: &str,
    importer_id: &str,
    importer_protocol: &str,
    adapter_kind: &str,
    adapter_binding_key: &str,
    issued_at: UnixTime,
    valid_until: UnixTime,
) -> String {
    canonical_json_body_digest(&json!({
        "kind": "ENFORCEMENT_PROFILE_ATTESTATION",
        "schema": ENFORCEMENT_PROFILE_ATTESTATION_SCHEMA_VERSION,
        "protocol": importer_protocol,
        "importer_id": importer_id,
        "profile_digest": profile_digest,
        "adapter_kind": adapter_kind,
        "adapter_binding_key": adapter_binding_key,
        "issued_at": issued_at,
        "valid_until": valid_until,
    }))
}

/// One validated source/binding/profile triple.
///
/// The compiler's physical inputs are related facts, not independent values: the
/// exact adapter binding policy must be the policy the [`SpawnSource`] names,
/// both must be active, and the imported profile must have been imported for
/// that exact `(adapter_kind, adapter_binding_key)` physical domain. This type
/// is the only way to reach [`compile_execution_security`], so a caller cannot
/// compile a profile against a binding it was not imported for, nor a binding
/// the source does not own.
///
/// B.4 distinguishes the `AdapterBindingPolicy.binding_ref` **alias** from the
/// resolved physical `AdapterBindingKey`. The alias is a source-policy input;
/// the key is what the Execution and its `BindingSnapshot` freeze. This type
/// therefore carries the resolved key explicitly and requires the imported
/// profile to name that same key. The resolution itself (alias → key) belongs to
/// the resolver, which produced the durable selection; the authority transaction
/// separately proves `evidence.adapter_binding_key == selection.adapter_binding_key`.
/// Here the key is the shared identity that ties the profile to the same
/// physical domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedSourceBinding {
    source: crate::records::SpawnSourceRef,
    adapter_policy: crate::records::AdapterPolicyRef,
    binding: AdapterBindingPolicy,
    adapter_binding_key: String,
    profile: ImportedEnforcementProfile,
}

impl ValidatedSourceBinding {
    /// The only constructor. Every mismatch is rejected here, before policy
    /// composition or obligation derivation, so a successful value is a
    /// consistent physical description.
    ///
    /// `adapter_binding_key` is the resolver-resolved physical key for
    /// `binding.binding_ref`. It must be the key the imported profile names;
    /// an alias-only match is not sufficient, because the same alias can resolve
    /// to a different physical domain.
    pub fn validate(
        source: &SpawnSource,
        binding: &AdapterBindingPolicy,
        adapter_binding_key: &str,
        profile: &ImportedEnforcementProfile,
    ) -> Result<Self, ContractError> {
        binding.validate()?;
        if source.status != SourceStatus::Active {
            return Err(ContractError::SecurityUnenforceable {
                reason: "the spawn source is not active".into(),
            });
        }
        if binding.status != ConfigStatus::Active {
            return Err(ContractError::SecurityUnenforceable {
                reason: "the exact adapter binding is not active".into(),
            });
        }
        if binding.policy_ref != source.adapter_policy {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "the adapter binding does not implement the source's adapter policy".into(),
            });
        }
        if adapter_binding_key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason:
                    "the validated source binding requires the resolved exact adapter binding key"
                        .into(),
            });
        }
        if profile.adapter_kind() != binding.adapter_kind
            || profile.adapter_binding_key() != adapter_binding_key
        {
            return Err(ContractError::EvidenceSubjectMismatch {
                reason: "the imported enforcement profile was imported for a different physical \
                         adapter binding"
                    .into(),
            });
        }
        Ok(Self {
            source: source.source_ref.clone(),
            adapter_policy: source.adapter_policy.clone(),
            binding: binding.clone(),
            adapter_binding_key: adapter_binding_key.to_string(),
            profile: profile.clone(),
        })
    }

    /// The source revision the binding belongs to.
    pub fn source(&self) -> &crate::records::SpawnSourceRef {
        &self.source
    }

    /// The adapter policy the source declares and the binding implements.
    pub fn adapter_policy(&self) -> &crate::records::AdapterPolicyRef {
        &self.adapter_policy
    }

    pub fn binding(&self) -> &AdapterBindingPolicy {
        &self.binding
    }

    /// The resolved exact physical domain this tuple was validated against.
    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
    }

    pub fn profile(&self) -> &ImportedEnforcementProfile {
        &self.profile
    }
}

/// The eight-part compile memoization key (ADR-0014 §4).
///
/// The effective-policy digest commits to canonical bytes, not to the
/// normalization and meet algorithm versions that produced them, so both
/// versions are explicit components. The same reasoning applies to the composed
/// guarantees: they change the obligations without changing the policy bytes, so
/// their digest is an explicit component too.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompileCacheKey {
    effective_policy_digest: String,
    normalization_version: u32,
    meet_algorithm_version: u32,
    adapter_kind: String,
    adapter_binding_key: String,
    enforcement_profile_digest: String,
    execution_guarantees_digest: String,
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
        execution_guarantees_digest: impl Into<String>,
    ) -> Result<Self, ContractError> {
        let key = Self {
            effective_policy_digest: effective_policy_digest.into(),
            normalization_version,
            meet_algorithm_version,
            adapter_kind: adapter_kind.into(),
            adapter_binding_key: adapter_binding_key.into(),
            enforcement_profile_digest: enforcement_profile_digest.into(),
            execution_guarantees_digest: execution_guarantees_digest.into(),
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
            (
                &key.execution_guarantees_digest,
                "execution guarantees digest",
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

    pub fn execution_guarantees_digest(&self) -> &str {
        &self.execution_guarantees_digest
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
    guarantees: EffectiveExecutionGuarantees,
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

    /// The composed execution guarantees, not the bare effective-policy bit. A
    /// Task-only or AgentType-only requirement therefore reaches the committed
    /// execution request even when every authored policy bit is false.
    pub fn guarantees(&self) -> EffectiveExecutionGuarantees {
        self.guarantees
    }

    pub fn attempt_isolation(&self) -> bool {
        self.guarantees.requires_attempt_isolation()
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
///
/// Compilation proves the profile's support matrix covers the obligations of the
/// effective policy. It does **not** yet name the enforcement subject the claim
/// is relative to; call [`CompiledExecutionSecurity::prove_subject`] for that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledExecutionSecurity {
    plan: PolicyEnforcementPlan,
    cache_key: CompileCacheKey,
    evidence_subject: SecurityEvidenceSubject,
    covered_subjects: BTreeSet<EnforcementSubject>,
    supports_credential_delivery: bool,
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

    /// Complete the evidence subject once the exact SourceConfig is attested,
    /// and settle the one obligation the config itself decides.
    ///
    /// A non-empty `credential_refs` is a delivery request (ADR-0013 §5), not an
    /// authorization. The compile step cannot see it, so this step refuses a
    /// request the profile cannot deliver instead of silently compiling a
    /// credential-bearing execution. No request means no obligation, which is
    /// why an authorization ceiling alone never makes a candidate ineligible.
    ///
    /// This is a **pure qualification** necessary condition, not an authority
    /// grant: a profile that *claims* `CredentialDelivery` satisfies it without
    /// proving that this exact request is authorized. The authority-bearing
    /// acquisition entry therefore stays fail-closed for every non-empty
    /// `credential_refs` until B5.3 introduces the canonical `CredentialRequest`,
    /// `CredentialEligibilityEvidence` and broker qualification (ADR-0015 §5/§9).
    pub fn with_source_config(mut self, config: &SourceConfig) -> Result<Self, ContractError> {
        self.evidence_subject = self.evidence_subject.with_source_config(config)?;
        if !config.credential_refs.is_empty() && !self.supports_credential_delivery {
            return Err(ContractError::SecurityUnenforceable {
                reason: format!(
                    "the selected config requests {} credential reference(s) but enforcement \
                     profile {} does not support credential delivery",
                    config.credential_refs.len(),
                    self.plan.reference.profile()
                ),
            });
        }
        Ok(self)
    }

    /// Prove that the profile this plan was compiled with claims the exact
    /// enforcement subject (ADR-0014 §14).
    ///
    /// A covered subject is a candidate qualification fact, not a receipt: the
    /// returned value still carries no Attempt/Lease authority and no claim that
    /// the restrictions were applied to a live process.
    pub fn prove_subject(
        self,
        subject: EnforcementSubject,
    ) -> Result<ProvenExecutionSecurity, ContractError> {
        if !self.covered_subjects.contains(&subject) {
            return Err(ContractError::SecurityUnenforceable {
                reason: format!(
                    "the imported enforcement profile does not claim the executing subject {}",
                    subject.as_str()
                ),
            });
        }
        Ok(ProvenExecutionSecurity {
            compiled: self,
            subject,
        })
    }
}

/// A compiled candidate whose profile claims the exact execution subject.
///
/// This is the strongest value the B5.2 compiler can produce: the plan, the
/// memoization key, and the subject are all bound together, so a caller cannot
/// read a successful [`CompiledExecutionSecurity`] as an applied-security claim
/// for a subject the profile never named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenExecutionSecurity {
    compiled: CompiledExecutionSecurity,
    subject: EnforcementSubject,
}

impl ProvenExecutionSecurity {
    /// The exact subject the profile's claims are relative to.
    pub fn subject(&self) -> EnforcementSubject {
        self.subject
    }

    pub fn compiled(&self) -> &CompiledExecutionSecurity {
        &self.compiled
    }

    /// Drop the subject proof. The result is a plain compile, not evidence.
    pub fn into_compiled(self) -> CompiledExecutionSecurity {
        self.compiled
    }

    pub fn plan(&self) -> &PolicyEnforcementPlan {
        self.compiled.plan()
    }

    pub fn plan_digest(&self) -> &str {
        self.compiled.plan_digest()
    }

    pub fn cache_key(&self) -> &CompileCacheKey {
        self.compiled.cache_key()
    }

    pub fn evidence_subject(&self) -> &SecurityEvidenceSubject {
        self.compiled.evidence_subject()
    }

    /// Complete the evidence subject once the exact SourceConfig is attested.
    pub fn with_source_config(mut self, config: &SourceConfig) -> Result<Self, ContractError> {
        self.compiled = self.compiled.with_source_config(config)?;
        Ok(self)
    }
}

/// Compile the root-relative plan and the evidence subject.
///
/// The input is a [`ValidatedSourceBinding`], so the physical facts are already
/// related: the binding is the source's own adapter policy, both are active,
/// and the profile was imported for the same resolved physical
/// `(adapter_kind, adapter_binding_key)` domain as the selection. Steps, in the
/// ADR-0014 §3 order:
/// 1. the validated tuple must describe the authority's own spawn source;
/// 2. the imported profile must be the one the SpawnSource declaration named;
/// 3. the effective policy's obligations must be covered by the profile;
/// 4. the plan is the logical effective policy; its digest commits to the
///    policy, the profile, and the compiler protocol.
///
/// A missing feature is a deterministic candidate ineligibility
/// (`SecurityUnenforceable`). A profile/authority/source mismatch is a subject
/// mismatch. Neither is a Task failure.
pub fn compile_execution_security(
    authority: &EffectiveTaskAuthority,
    domain: &ValidatedSourceBinding,
) -> Result<CompiledExecutionSecurity, ContractError> {
    if domain.source() != authority.spawn_source_ref() {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the validated source/binding belongs to a different spawn source".into(),
        });
    }
    let profile = domain.profile();
    let binding = domain.binding();
    if authority.enforcement_profile() != profile.reference() {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "the imported enforcement profile is not the one the source declared".into(),
        });
    }

    let guarantees = EffectiveExecutionGuarantees::from_authority(authority);
    if authority.policy().attempt_isolation() && !guarantees.requires_attempt_isolation() {
        return Err(ContractError::InvariantViolation(
            "the effective policy requires attempt isolation but no composed guarantee carries it"
                .into(),
        ));
    }
    let obligations = RequiredEnforcementFeatures::derive(authority.policy(), guarantees);
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
        "requires_attempt_isolation": guarantees.requires_attempt_isolation(),
        "obligations": obligations.features().iter().map(|f| f.as_str()).collect::<Vec<_>>(),
    }));
    let cache_key = CompileCacheKey::new(
        authority.effective_policy_digest(),
        authority.normalization_version(),
        authority.meet_algorithm_version(),
        &binding.adapter_kind,
        domain.adapter_binding_key(),
        profile.profile_digest(),
        effective_execution_guarantees_digest(guarantees),
    )?;
    let plan = PolicyEnforcementPlan {
        effective_policy,
        guarantees,
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
        domain.adapter_binding_key().to_string(),
        authority.effective_policy_digest().to_string(),
        profile.reference().clone(),
        profile.profile_digest().to_string(),
        plan.plan_digest.clone(),
    );

    Ok(CompiledExecutionSecurity {
        plan,
        cache_key,
        evidence_subject,
        covered_subjects: profile.covered_subjects().clone(),
        supports_credential_delivery: profile
            .support()
            .supports(EnforcementFeature::CredentialDelivery),
    })
}

/// Qualify one candidate: prove the candidate's authored ceiling bounds the
/// effective Task policy with the real policy order, re-check the Task's coarse
/// modes and its positive access floor on the complete policy, then compile and
/// prove the profile claims the executing subject.
///
/// This is the acquisition-side predicate. It grants no Attempt/Lease
/// authority; the authority transaction still owns that.
#[allow(clippy::too_many_arguments)]
pub fn qualify_security_candidate(
    authority: &EffectiveTaskAuthority,
    requirement: &crate::requirement::TaskAgentRequirement,
    agent: &crate::authority::SecurityRevision<'_>,
    domain: &ValidatedSourceBinding,
    subject: EnforcementSubject,
    catalog: &crate::capability::CapabilityCatalog,
) -> Result<ProvenExecutionSecurity, ContractError> {
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
    compile_execution_security(authority, domain)?.prove_subject(subject)
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
        let block = Self {
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
        };
        block.validate_durable_truth()?;
        Ok(block)
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
        let block = Self {
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
        };
        block.validate_durable_truth()?;
        Ok(block)
    }

    /// Re-prove that this row is a real whole-round defer.
    ///
    /// The recorded cost must actually exceed the recorded budget, the recorded
    /// limit must be exactly the bound that `require_whole_round` reports, and
    /// the observed candidate cost must equal the recorded domain size: a budget
    /// block means the complete candidate set was attempted, so a row that
    /// records fewer attempted candidates than its domain describes is not a
    /// budget block at all. The in-memory producer path cannot reach an
    /// inconsistent state, because `BudgetExhaustion` has private fields and only
    /// `require_whole_round` constructs it; this check exists for the durable
    /// boundary, where the fields arrive independently and a hand-written row is
    /// indistinguishable from a real diagnostic without it.
    pub fn validate_durable_truth(&self) -> Result<(), ContractError> {
        if !self.created_at.is_finite() || self.created_at < 0.0 {
            return Err(ContractError::InvariantViolation(
                "a durable security acquisition block has an invalid creation timestamp".into(),
            ));
        }
        if let Some(released_at) = self.released_at {
            if !released_at.is_finite() || released_at < 0.0 {
                return Err(ContractError::InvariantViolation(
                    "a durable security acquisition block has an invalid release timestamp".into(),
                ));
            }
        }
        if self.cost.candidates() != self.domain.candidate_count() {
            return Err(ContractError::InvariantViolation(format!(
                "a durable security acquisition block records {} observed candidates but a domain of {}: a budget block means the complete round was attempted",
                self.cost.candidates(),
                self.domain.candidate_count()
            )));
        }
        match self.budget.require_whole_round(self.cost) {
            Ok(()) => Err(ContractError::InvariantViolation(
                "a durable security acquisition block records a cost inside its budget".into(),
            )),
            Err(exhaustion) if exhaustion.limit() == self.limit => Ok(()),
            Err(exhaustion) => Err(ContractError::InvariantViolation(format!(
                "a durable security acquisition block records the {} limit but its budget is exhausted at {}",
                self.limit.as_str(),
                exhaustion.limit().as_str()
            ))),
        }
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
