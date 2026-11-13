//! M6-B.5 B5.2 imported-enforcement qualification and the pure root-relative
//! compiler conformance.
//!
//! These tests pin ADR-0014 §2–§5 and §13–§14:
//!
//! * obligations are derived from imposed restrictions, not from the mere
//!   presence of a field;
//! * the frozen `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` support matrix covers
//!   workspace-write confinement and an environment allowlist, and nothing else;
//! * compilation is pure and root-relative, returns a partial evidence subject,
//!   and completes only with its own exact SourceConfig;
//! * the whole-round budget has no partial-success path, and one durable
//!   diagnostic is released only by operator action or configuration change.

// The imported enforcement profile is an external fact whose only constructor is
// gated behind `provisioning-producer` (enabled by `test-support`), so this
// conformance file is only meaningful with that producer available.
#![cfg(feature = "test-support")]

use agentype_agent_contract::*;
use agentype_core::{GenerationId, InformationFunction, LogicalAgentId, TaskId, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

// =============================================================================
// Fixtures
// =============================================================================

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn policy_ref(id: &str, revision: u64) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, revision).unwrap()
}

fn workspace(components: &[&str]) -> PathSet {
    PathSet::allowed([PathSelector::workspace(components.iter().copied()).unwrap()])
}

fn endpoint() -> NetworkEndpointSet {
    NetworkEndpointSet::allowed([NetworkEndpoint::new(
        NetworkTransport::Tcp,
        "example.com",
        Some(443),
    )
    .unwrap()])
}

/// A policy whose only restrictions are a workspace write region and an
/// explicit environment allowlist. That is exactly the reference matrix.
fn confined_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            workspace(&["src"]),
            PathSet::any(),
            PathSet::any(),
            PathSet::any(),
        ),
        tools: ToolPolicy::top(),
        network: NetworkEndpointSet::any(),
        environment: EnvironmentPolicy::new(NameSet::allowed(["PATH"]).unwrap(), BTreeMap::new())
            .unwrap(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

/// The same shape plus a narrowed read set, which the reference profile does
/// not claim.
fn read_confined_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            workspace(&["src"]),
            workspace(&["src"]),
            PathSet::any(),
            PathSet::any(),
            PathSet::any(),
        ),
        tools: ToolPolicy::top(),
        network: NetworkEndpointSet::any(),
        environment: EnvironmentPolicy::new(NameSet::allowed(["PATH"]).unwrap(), BTreeMap::new())
            .unwrap(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn agent_type_with_policy(id: &str, reference: SandboxPolicyRef) -> AgentType {
    AgentType {
        type_ref: type_ref(id, 1),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace: WorkspaceMode::Write,
                network: NetworkPolicy::Enabled,
                requires_attempt_isolation: false,
            },
            lifecycle: [LifecycleMode::Resident].into_iter().collect(),
            continuity: ContinuityMode::Logical,
            sandbox_policy: Some(reference),
            anchor_constraint: None,
        },
    }
}

fn requirement() -> TaskAgentRequirement {
    TaskAgentRequirement {
        required_type: type_ref("pinned", 1),
        hard: TaskRequirement {
            information_function: InformationFunction::Expand,
            required_capabilities: BTreeMap::new(),
            required_affinity: BTreeSet::new(),
            required_workspace: WorkspaceMode::Write,
            required_network: NetworkPolicy::Enabled,
            required_attempt_isolation: false,
            required_continuity: ContinuityMode::None,
            sandbox_policy: None,
            required_anchor: None,
            budget: Budget::new(50.0).unwrap(),
        },
    }
}

fn spawn_source() -> SpawnSource {
    SpawnSource {
        source_ref: SpawnSourceRef::new("codex-local", 1).unwrap(),
        adapter_policy: AdapterPolicyRef::new("codex-local-adapter", 1).unwrap(),
        lifecycle_modes: [LifecycleMode::Resident].into_iter().collect(),
        continuity_modes: [ContinuityMode::None, ContinuityMode::Logical]
            .into_iter()
            .collect(),
        functional_envelope: BTreeMap::new(),
        claims: Vec::new(),
        status: SourceStatus::Active,
    }
}

fn reference_profile_ref() -> EnforcementProfileRef {
    EnforcementProfileRef::new(
        POLICY_ONLY_REFERENCE_PROTOCOL,
        POLICY_ONLY_REFERENCE_PROFILE,
        POLICY_ONLY_REFERENCE_VERSION,
    )
    .unwrap()
}

fn reference_profile(subjects: [EnforcementSubject; 1]) -> ImportedEnforcementProfile {
    ImportedEnforcementProfile::import(
        reference_profile_ref(),
        1,
        EnforcementSupport::policy_only_reference(),
        subjects,
    )
    .unwrap()
}

fn task_binding(
    policy: NormalizedSandboxPolicy,
    reference: SandboxPolicyRef,
) -> TaskSecurityBinding {
    let input = PolicyInput::authored(reference, &policy);
    let derivation = SecurityDerivation::new([input], &policy, &RequiredAccess::empty()).unwrap();
    TaskSecurityBinding::new(policy, derivation, []).unwrap()
}

fn adapter_binding(status: ConfigStatus) -> AdapterBindingPolicy {
    AdapterBindingPolicy::new(
        AdapterPolicyRef::new("codex-local-adapter", 1).unwrap(),
        "local-process",
        "codex-local/binding",
        PhysicalSafety::new(
            false,
            vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write],
            [
                NetworkPolicy::Disabled,
                NetworkPolicy::Restricted,
                NetworkPolicy::Enabled,
            ]
            .into_iter()
            .collect(),
        )
        .unwrap(),
        status,
    )
    .unwrap()
}

fn source_config(source: &SpawnSourceRef) -> SourceConfig {
    SourceConfig {
        config_ref: SourceConfigRef::new(source.clone(), "deep", 1).unwrap(),
        config_digest: ConfigDigest::new(
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: Vec::new(),
        claims: Vec::new(),
        status: ConfigStatus::Active,
    }
}

/// One fully composed candidate tuple for a `SECURITY_V1` Task.
struct Fixture {
    binding: TaskSecurityBinding,
    requirement: TaskAgentRequirement,
    agent: AgentType,
    agent_policy: NormalizedSandboxPolicy,
    source: SpawnSource,
    source_policy: NormalizedSandboxPolicy,
    declaration: SpawnSourceSecurityDeclaration,
    profile: ImportedEnforcementProfile,
}

impl Fixture {
    fn new(
        binding_policy: NormalizedSandboxPolicy,
        requirement: TaskAgentRequirement,
        profile: ImportedEnforcementProfile,
    ) -> Self {
        let reference = policy_ref("binding", 1);
        let binding = task_binding(binding_policy.clone(), reference);
        let agent = agent_type_with_policy("pinned", policy_ref("agent-policy", 1));
        let agent_policy = NormalizedSandboxPolicy::top();
        let source = spawn_source();
        let source_policy = NormalizedSandboxPolicy::top();
        let declaration = SpawnSourceSecurityDeclaration::with_policy(
            PolicyInput::authored(policy_ref("source-policy", 1), &source_policy),
            profile.reference().clone(),
        );
        Self {
            binding,
            requirement,
            agent,
            agent_policy,
            source,
            source_policy,
            declaration,
            profile,
        }
    }

    fn authority(&self) -> EffectiveTaskAuthority {
        let revision = SecurityRevision::new(
            &self.agent,
            SecurityContractMode::SecurityV1,
            Some(&self.agent_policy),
        )
        .unwrap();
        EffectiveTaskAuthority::new(
            TaskId::from_string("task-1"),
            GenerationId::from_string("generation-1"),
            LogicalAgentId::from_string("agent-1"),
            &self.binding,
            &self.requirement,
            &revision,
            &self.source,
            &self.declaration,
            Some(&self.source_policy),
        )
        .unwrap()
    }
}

fn confined_fixture() -> Fixture {
    Fixture::new(
        confined_policy(),
        requirement(),
        reference_profile([EnforcementSubject::ControlledPrimaryProcess]),
    )
}

// =============================================================================
// Obligation derivation and the support matrix
// =============================================================================

#[test]
fn obligations_follow_imposed_restrictions_not_field_presence() {
    let guarantees = EffectiveExecutionGuarantees::new(false);
    assert!(
        RequiredEnforcementFeatures::derive(&NormalizedSandboxPolicy::top(), guarantees).is_empty()
    );

    let deny_all =
        RequiredEnforcementFeatures::derive(&NormalizedSandboxPolicy::deny_all(), guarantees);
    let expected: BTreeSet<EnforcementFeature> = [
        EnforcementFeature::WorkspaceWriteConfinement,
        EnforcementFeature::FileReadConfinement,
        EnforcementFeature::FileExecuteConfinement,
        EnforcementFeature::FileVisibilityConfinement,
        EnforcementFeature::ToolCommandAllowlist,
        EnforcementFeature::NetworkEgressRestriction,
        EnforcementFeature::EnvironmentAllowlist,
    ]
    .into_iter()
    .collect();
    assert_eq!(deny_all.features(), &expected);
    // Deny-all authorizes nothing, so it requires no credential delivery and no
    // resource ceiling.
    assert!(!deny_all.contains(EnforcementFeature::CredentialDelivery));
    assert!(!deny_all.contains(EnforcementFeature::ResourceLimit));

    let confined = RequiredEnforcementFeatures::derive(&confined_policy(), guarantees);
    assert_eq!(
        confined.features(),
        &[
            EnforcementFeature::WorkspaceWriteConfinement,
            EnforcementFeature::EnvironmentAllowlist,
        ]
        .into_iter()
        .collect()
    );

    // A restricted network is one dimension, not a whole deny-all.
    let restricted_network = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::top(),
        tools: ToolPolicy::top(),
        network: endpoint(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap();
    assert_eq!(
        RequiredEnforcementFeatures::derive(&restricted_network, guarantees).features(),
        &[EnforcementFeature::NetworkEgressRestriction]
            .into_iter()
            .collect()
    );

    // Isolation is an execution guarantee, not a policy dimension: it is
    // required even when the policy bytes are top.
    let isolated = RequiredEnforcementFeatures::derive(
        &NormalizedSandboxPolicy::top(),
        EffectiveExecutionGuarantees::new(true),
    );
    assert_eq!(
        isolated.features(),
        &[EnforcementFeature::AttemptIsolation].into_iter().collect()
    );
}

#[test]
fn the_reference_profile_claims_only_workspace_write_and_environment() {
    let support = EnforcementSupport::policy_only_reference();
    assert_eq!(support.features().len(), 2);
    assert!(support.supports(EnforcementFeature::WorkspaceWriteConfinement));
    assert!(support.supports(EnforcementFeature::EnvironmentAllowlist));
    assert!(!support.supports(EnforcementFeature::FileReadConfinement));

    let confined = RequiredEnforcementFeatures::derive(
        &confined_policy(),
        EffectiveExecutionGuarantees::new(false),
    );
    assert!(support.covers(&confined));

    // Deny-all is a stronger policy than the matrix claims, so a reference
    // profile MUST reject it rather than silently weakening it.
    let deny_all = RequiredEnforcementFeatures::derive(
        &NormalizedSandboxPolicy::deny_all(),
        EffectiveExecutionGuarantees::new(false),
    );
    let missing = support.missing(&deny_all);
    assert!(!support.covers(&deny_all));
    assert!(missing.contains(&EnforcementFeature::FileReadConfinement));
    assert!(missing.contains(&EnforcementFeature::NetworkEgressRestriction));
    assert!(!missing.contains(&EnforcementFeature::EnvironmentAllowlist));
}

// =============================================================================
// The pure compiler
// =============================================================================

#[test]
fn compilation_returns_a_root_relative_plan_and_a_partial_subject() {
    let fixture = confined_fixture();
    let authority = fixture.authority();
    let binding = adapter_binding(ConfigStatus::Active);
    let compiled = compile_execution_security(&authority, &binding, &fixture.profile).unwrap();

    // The plan is the logical effective policy; no concrete jail path exists.
    let plan = compiled.plan();
    assert_eq!(plan.effective_policy(), authority.policy());
    assert_eq!(plan.files(), authority.policy().files());
    assert_eq!(plan.working_directory(), &WorkingDirectory::Any);
    assert!(plan.credential_authorizations().is_empty());
    assert_eq!(plan.enforcement_profile(), fixture.profile.reference());
    assert_eq!(
        plan.obligations().features(),
        &[
            EnforcementFeature::WorkspaceWriteConfinement,
            EnforcementFeature::EnvironmentAllowlist,
        ]
        .into_iter()
        .collect()
    );
    assert!(plan.plan_digest().starts_with("sha256:"));

    // The cache key commits to all seven components.
    let key = compiled.cache_key();
    assert_eq!(
        key.effective_policy_digest(),
        authority.effective_policy_digest()
    );
    assert_eq!(
        key.normalization_version(),
        authority.normalization_version()
    );
    assert_eq!(
        key.meet_algorithm_version(),
        authority.meet_algorithm_version()
    );
    assert_eq!(key.adapter_kind(), "local-process");
    assert_eq!(key.adapter_binding_key(), "codex-local/binding");
    assert_eq!(
        key.enforcement_profile_digest(),
        fixture.profile.profile_digest()
    );
    assert_eq!(
        key.compiler_protocol_version(),
        ENFORCEMENT_COMPILER_PROTOCOL
    );

    // Compilation happens before attestation, so the subject is deliberately
    // incomplete and MUST NOT be used as acquisition evidence yet.
    let subject = compiled.evidence_subject();
    assert!(!subject.is_complete());
    assert!(subject.source_config().is_none());
    assert!(matches!(
        subject.digest().unwrap_err(),
        ContractError::EvidenceSubjectMismatch { .. }
    ));
    assert_eq!(subject.agent_type(), &fixture.agent.type_ref);
    assert_eq!(subject.spawn_source(), &fixture.source.source_ref);
    assert_eq!(subject.plan_digest(), plan.plan_digest());
    assert_eq!(
        subject.effective_policy_digest(),
        authority.effective_policy_digest()
    );
    assert_eq!(
        subject.enforcement_profile_digest(),
        fixture.profile.profile_digest()
    );

    // The same inputs compile to the same bytes.
    let again = compile_execution_security(&authority, &binding, &fixture.profile).unwrap();
    assert_eq!(again.plan_digest(), compiled.plan_digest());
    assert_eq!(again.cache_key(), compiled.cache_key());
}

#[test]
fn the_subject_completes_only_with_its_own_source_config() {
    let fixture = confined_fixture();
    let authority = fixture.authority();
    let binding = adapter_binding(ConfigStatus::Active);
    let compiled = compile_execution_security(&authority, &binding, &fixture.profile).unwrap();

    let config = source_config(&fixture.source.source_ref);
    let completed = compiled.clone().with_source_config(&config).unwrap();
    assert!(completed.evidence_subject().is_complete());
    assert_eq!(
        completed.evidence_subject().source_config(),
        Some(&config.config_ref)
    );
    let digest = completed.evidence_subject().digest().unwrap();
    assert!(digest.starts_with("sha256:"));

    // A config from a different source is a subject mismatch, not a weaker
    // match and not a silent re-binding.
    let foreign_source = SpawnSourceRef::new("other-source", 1).unwrap();
    let foreign = source_config(&foreign_source);
    assert!(matches!(
        compiled.clone().with_source_config(&foreign).unwrap_err(),
        ContractError::EvidenceSubjectMismatch { .. }
    ));

    // The subject digest commits to the exact config: a different revision of
    // the same config is different evidence.
    let mut other_revision = config.clone();
    other_revision.config_ref =
        SourceConfigRef::new(fixture.source.source_ref.clone(), "deep", 2).unwrap();
    let other = compiled.with_source_config(&other_revision).unwrap();
    assert_ne!(other.evidence_subject().digest().unwrap(), digest);
}

#[test]
fn a_different_imported_profile_is_a_subject_mismatch() {
    let fixture = confined_fixture();
    let authority = fixture.authority();
    let binding = adapter_binding(ConfigStatus::Active);

    let other = ImportedEnforcementProfile::import(
        EnforcementProfileRef::new(
            POLICY_ONLY_REFERENCE_PROTOCOL,
            "SOME_OTHER_PROFILE",
            POLICY_ONLY_REFERENCE_VERSION,
        )
        .unwrap(),
        1,
        EnforcementSupport::policy_only_reference(),
        [EnforcementSubject::ControlledPrimaryProcess],
    )
    .unwrap();

    assert!(matches!(
        compile_execution_security(&authority, &binding, &other).unwrap_err(),
        ContractError::EvidenceSubjectMismatch { .. }
    ));
}

#[test]
fn an_inactive_binding_is_never_compiled() {
    let fixture = confined_fixture();
    let authority = fixture.authority();
    let inactive = adapter_binding(ConfigStatus::Disabled);
    assert!(matches!(
        compile_execution_security(&authority, &inactive, &fixture.profile).unwrap_err(),
        ContractError::SecurityUnenforceable { .. }
    ));
}

#[test]
fn a_missing_dimension_makes_the_candidate_ineligible_not_the_task_failed() {
    let fixture = Fixture::new(
        read_confined_policy(),
        requirement(),
        reference_profile([EnforcementSubject::ControlledPrimaryProcess]),
    );
    let authority = fixture.authority();
    let error = compile_execution_security(
        &authority,
        &adapter_binding(ConfigStatus::Active),
        &fixture.profile,
    )
    .unwrap_err();
    match error {
        ContractError::SecurityUnenforceable { reason } => {
            assert!(reason.contains("FILE_READ_CONFINEMENT"), "{reason}");
        }
        other => panic!("expected candidate-local ineligibility, got {other:?}"),
    }
}

#[test]
fn the_cache_key_rejects_an_incomplete_component() {
    assert!(matches!(
        CompileCacheKey::new(
            "sha256:policy",
            SANDBOX_POLICY_NORMALIZATION_VERSION,
            SANDBOX_POLICY_MEET_ALGORITHM_VERSION,
            "",
            "binding",
            "sha256:profile",
        )
        .unwrap_err(),
        ContractError::InvalidRef { .. }
    ));
    assert!(matches!(
        CompileCacheKey::new(
            "sha256:policy",
            0,
            SANDBOX_POLICY_MEET_ALGORITHM_VERSION,
            "local-process",
            "binding",
            "sha256:profile",
        )
        .unwrap_err(),
        ContractError::InvalidRef { .. }
    ));
}

// =============================================================================
// Acquisition qualification
// =============================================================================

#[test]
fn qualification_re_proves_the_requirement_and_agent_identity_before_compiling() {
    let fixture = confined_fixture();
    let authority = fixture.authority();
    let binding = adapter_binding(ConfigStatus::Active);
    let catalog = CapabilityCatalog::new();
    let revision = SecurityRevision::new(
        &fixture.agent,
        SecurityContractMode::SecurityV1,
        Some(&fixture.agent_policy),
    )
    .unwrap();

    let qualified = qualify_security_candidate(
        &authority,
        &fixture.requirement,
        &revision,
        &binding,
        &fixture.profile,
        &catalog,
    )
    .unwrap();
    assert_eq!(
        qualified.plan_digest(),
        compile_execution_security(&authority, &binding, &fixture.profile)
            .unwrap()
            .plan_digest()
    );

    // A different Task requirement cannot borrow this composite.
    let mut other_requirement = fixture.requirement.clone();
    other_requirement.hard.budget = Budget::new(51.0).unwrap();
    assert!(matches!(
        qualify_security_candidate(
            &authority,
            &other_requirement,
            &revision,
            &binding,
            &fixture.profile,
            &catalog,
        )
        .unwrap_err(),
        ContractError::EvidenceSubjectMismatch { .. }
    ));

    // A different AgentType revision cannot borrow this composite either.
    let other_agent = agent_type_with_policy("other", policy_ref("other-policy", 1));
    let other_revision = SecurityRevision::new(
        &other_agent,
        SecurityContractMode::SecurityV1,
        Some(&fixture.agent_policy),
    )
    .unwrap();
    assert!(matches!(
        qualify_security_candidate(
            &authority,
            &fixture.requirement,
            &other_revision,
            &binding,
            &fixture.profile,
            &catalog,
        )
        .unwrap_err(),
        ContractError::EvidenceSubjectMismatch { .. }
    ));

    // A COARSE_V1 revision is never qualified against a SECURITY_V1 Task, even
    // when it names the same AgentType revision.
    let coarse =
        SecurityRevision::new(&fixture.agent, SecurityContractMode::CoarseV1, None).unwrap();
    assert!(matches!(
        qualify_security_candidate(
            &authority,
            &fixture.requirement,
            &coarse,
            &binding,
            &fixture.profile,
            &catalog,
        )
        .unwrap_err(),
        ContractError::SecurityUnenforceable { .. }
    ));
}

#[test]
fn qualification_rechecks_the_task_coarse_modes_on_the_complete_policy() {
    // The Task requires network, the AgentType policy is top, but the source
    // ceiling denies all egress. The fold satisfies every fine layer yet the
    // complete policy cannot satisfy `NetworkPolicy::Enabled`.
    let requirement = requirement();
    let mut fixture = Fixture::new(
        confined_policy(),
        requirement.clone(),
        reference_profile([EnforcementSubject::ControlledPrimaryProcess]),
    );
    fixture.source_policy = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::top(),
        tools: ToolPolicy::top(),
        network: NetworkEndpointSet::deny_all(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap();
    fixture.declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(policy_ref("source-policy", 1), &fixture.source_policy),
        fixture.profile.reference().clone(),
    );

    let authority = fixture.authority();
    // The composite itself is constructible: each layer is individually valid.
    assert!(authority.policy().network().is_empty());

    let revision = SecurityRevision::new(
        &fixture.agent,
        SecurityContractMode::SecurityV1,
        Some(&fixture.agent_policy),
    )
    .unwrap();
    assert!(matches!(
        qualify_security_candidate(
            &authority,
            &fixture.requirement,
            &revision,
            &adapter_binding(ConfigStatus::Active),
            &fixture.profile,
            &CapabilityCatalog::new(),
        )
        .unwrap_err(),
        ContractError::SecurityUnenforceable { .. }
    ));
}

// =============================================================================
// Budget and the durable diagnostic
// =============================================================================

#[test]
fn the_budget_has_no_partial_success_path() {
    let budget = AcquisitionBudget::new(2, 1).unwrap();
    assert!(budget
        .require_whole_round(AcquisitionCost::new(2, 1))
        .is_ok());

    let over_candidates = budget
        .require_whole_round(AcquisitionCost::new(3, 0))
        .unwrap_err();
    assert_eq!(over_candidates.limit(), BudgetLimit::CandidateCount);
    assert_eq!(over_candidates.cost(), AcquisitionCost::new(3, 0));
    assert_eq!(over_candidates.budget(), budget);

    let over_compiles = budget
        .require_whole_round(AcquisitionCost::new(1, 2))
        .unwrap_err();
    assert_eq!(over_compiles.limit(), BudgetLimit::CompileCount);

    assert!(matches!(
        AcquisitionBudget::new(0, 1).unwrap_err(),
        ContractError::InvalidNumber { .. }
    ));
    assert!(matches!(
        AcquisitionBudget::new(1, 0).unwrap_err(),
        ContractError::InvalidNumber { .. }
    ));
}

#[test]
fn the_candidate_domain_digest_is_a_stable_identity_set() {
    let a = LogicalAgentId::from_string("agent-a");
    let b = LogicalAgentId::from_string("agent-b");

    let forward = CandidateDomain::from_ids([a.clone(), b.clone()]);
    let reverse = CandidateDomain::from_ids([b.clone(), a.clone()]);
    assert_eq!(forward.digest(), reverse.digest());
    assert_eq!(forward.candidate_count(), 2);

    // Duplicates are one identity, and a different set is a different domain.
    let deduped = CandidateDomain::from_ids([a.clone(), b.clone(), a.clone()]);
    assert_eq!(deduped.digest(), forward.digest());
    assert_ne!(CandidateDomain::from_ids([a]).digest(), forward.digest());
    assert_eq!(CandidateDomain::from_ids([]).candidate_count(), 0);
}

#[test]
fn a_block_is_released_only_by_operator_action_or_configuration_change() {
    let budget = AcquisitionBudget::new(1, 1).unwrap();
    let exhaustion = budget
        .require_whole_round(AcquisitionCost::new(4, 4))
        .unwrap_err();
    let domain = CandidateDomain::from_ids([
        LogicalAgentId::from_string("agent-a"),
        LogicalAgentId::from_string("agent-b"),
    ]);
    let binding_profile = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let mut block = SecurityAcquisitionBlock::from_budget_defer(
        TaskId::from_string("task-1"),
        GenerationId::from_string("generation-1"),
        domain.clone(),
        binding_profile,
        exhaustion,
        100.0,
    )
    .unwrap();

    assert!(block.is_blocked());
    assert_eq!(block.status(), SecurityAcquisitionBlockStatus::Blocked);
    assert_eq!(block.limit(), BudgetLimit::CandidateCount);
    assert_eq!(block.candidate_count(), 2);
    assert!(block.reason().contains("candidate-count"));
    assert!(block.release_condition().contains("operator action"));

    // The same indistinguishable condition is one diagnostic; a different
    // candidate domain is a new condition, not a retry of the old one.
    assert!(block.same_condition(&domain, binding_profile));
    let other = CandidateDomain::from_ids([LogicalAgentId::from_string("agent-a")]);
    assert!(!block.same_condition(&other, binding_profile));

    block
        .release(SecurityAcquisitionRelease::OperatorAction, 101.0)
        .unwrap();
    assert!(!block.is_blocked());
    assert_eq!(block.released_at(), Some(101.0));
    assert_eq!(
        block.release_reason(),
        Some(SecurityAcquisitionRelease::OperatorAction)
    );

    // A second release is a no-op: the first release identity is durable.
    block
        .release(SecurityAcquisitionRelease::ConfigurationChanged, 200.0)
        .unwrap();
    assert_eq!(block.released_at(), Some(101.0));
    assert_eq!(
        block.release_reason(),
        Some(SecurityAcquisitionRelease::OperatorAction)
    );

    assert!(matches!(
        block
            .release(SecurityAcquisitionRelease::OperatorAction, f64::NAN)
            .unwrap_err(),
        ContractError::InvalidNumber { .. }
    ));
}

#[test]
fn a_durable_block_row_cannot_have_inconsistent_release_fields() {
    let error = SecurityAcquisitionBlock::from_durable_parts(
        TaskId::from_string("task-1"),
        GenerationId::from_string("generation-1"),
        "sha256:domain".into(),
        1,
        "sha256:binding".into(),
        AcquisitionCost::new(4, 4),
        AcquisitionBudget::new(1, 1).unwrap(),
        BudgetLimit::CandidateCount,
        SecurityAcquisitionBlockStatus::Released,
        100.0,
        None,
        None,
    )
    .unwrap_err();
    assert!(matches!(error, ContractError::InvariantViolation(_)));

    let error = SecurityAcquisitionBlock::from_durable_parts(
        TaskId::from_string("task-1"),
        GenerationId::from_string("generation-1"),
        "sha256:domain".into(),
        1,
        "".into(),
        AcquisitionCost::new(4, 4),
        AcquisitionBudget::new(1, 1).unwrap(),
        BudgetLimit::CandidateCount,
        SecurityAcquisitionBlockStatus::Blocked,
        100.0,
        None,
        None,
    )
    .unwrap_err();
    assert!(matches!(error, ContractError::InvariantViolation(_)));
}
