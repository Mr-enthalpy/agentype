//! M6-B.5 B5.2 composite Task authority conformance.
//!
//! These tests pin the four-term fold ADR-0013 §5 requires:
//!
//! ```text
//! P_eff = TaskAuthority(meet(P_binding, P_agent, P_source))
//! ```
//!
//! They also pin that the composite is only constructible from a supported
//! derivation and from a source declaration whose authored digest matches the
//! policy bytes presented with it.

use agentype_agent_contract::*;
use agentype_core::{GenerationId, InformationFunction, LogicalAgentId, TaskId, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn policy_ref(id: &str, revision: u64) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, revision).unwrap()
}

fn endpoint() -> NetworkEndpointSet {
    NetworkEndpointSet::allowed([NetworkEndpoint::new(
        NetworkTransport::Tcp,
        "example.com",
        Some(443),
    )
    .unwrap()])
}

fn workspace(components: &[&str]) -> PathSet {
    PathSet::allowed([PathSelector::workspace(components.iter().copied()).unwrap()])
}

fn policy(write: PathSet, network: NetworkEndpointSet) -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            write,
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network,
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn agent_type_with_policy(
    id: &str,
    reference: SandboxPolicyRef,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
) -> AgentType {
    AgentType {
        type_ref: type_ref(id, 1),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace,
                network,
                requires_attempt_isolation: false,
            },
            lifecycle: [LifecycleMode::Resident].into_iter().collect(),
            continuity: ContinuityMode::Logical,
            sandbox_policy: Some(reference),
            anchor_constraint: None,
        },
    }
}

fn task_requirement(workspace: WorkspaceMode, network: NetworkPolicy) -> TaskAgentRequirement {
    TaskAgentRequirement {
        required_type: type_ref("pinned", 1),
        hard: TaskRequirement {
            information_function: InformationFunction::Expand,
            required_capabilities: BTreeMap::new(),
            required_affinity: BTreeSet::new(),
            required_workspace: workspace,
            required_network: network,
            required_attempt_isolation: false,
            required_continuity: ContinuityMode::None,
            sandbox_policy: None,
            required_anchor: None,
            budget: Budget::new(50.0).unwrap(),
        },
    }
}

fn source() -> SpawnSource {
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

fn binding(policy: NormalizedSandboxPolicy, reference: SandboxPolicyRef) -> TaskSecurityBinding {
    let input = PolicyInput::authored(reference, &policy);
    let derivation = SecurityDerivation::new([input], &policy, &RequiredAccess::empty()).unwrap();
    TaskSecurityBinding::new(policy, derivation, []).unwrap()
}

fn identity() -> (TaskId, GenerationId, LogicalAgentId) {
    (
        TaskId::from_string("task-1"),
        GenerationId::from_string("generation-1"),
        LogicalAgentId::from_string("agent-1"),
    )
}

#[test]
fn the_composite_folds_binding_agent_source_and_task_coarse_modes() {
    let binding_policy = policy(workspace(&["src"]), endpoint());
    let reference = policy_ref("binding", 1);
    let binding = binding(binding_policy.clone(), reference.clone());
    let requirement = task_requirement(WorkspaceMode::ReadOnly, NetworkPolicy::Disabled);

    // The AgentType and source both grant more authority than the binding. The
    // fold must never add any of it.
    let agent = agent_type_with_policy(
        "pinned",
        policy_ref("agent-policy", 1),
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );
    let agent_policy = policy(PathSet::any(), NetworkEndpointSet::any());
    let agent_revision = SecurityRevision::new(
        &agent,
        SecurityContractMode::SecurityV1,
        Some(&agent_policy),
    )
    .unwrap();

    let source = source();
    let source_policy = policy(workspace(&["src", "tmp"]), NetworkEndpointSet::any());
    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(policy_ref("source-policy", 1), &source_policy),
        EnforcementProfileRef::new("enforcement", "profile", 1).unwrap(),
    );

    let (task_id, generation_id, logical_agent_id) = identity();
    let authority = EffectiveTaskAuthority::new(
        task_id.clone(),
        generation_id.clone(),
        logical_agent_id.clone(),
        &binding,
        &requirement,
        &agent_revision,
        &source,
        &declaration,
        Some(&source_policy),
    )
    .unwrap();

    // ReadOnly erases write/delete; Disabled erases network.
    assert!(authority.policy().files().write().is_empty());
    assert!(authority.policy().files().delete().is_empty());
    assert_eq!(
        authority.policy().network(),
        &NetworkEndpointSet::deny_all()
    );
    // Reads are preserved from the binding.
    assert_eq!(
        authority.policy().files().read(),
        binding_policy.files().read()
    );

    assert_eq!(authority.task_id(), &task_id);
    assert_eq!(authority.generation_id(), &generation_id);
    assert_eq!(authority.logical_agent_id(), &logical_agent_id);
    assert_eq!(authority.agent_type_ref(), &agent.type_ref);
    assert_eq!(authority.spawn_source_ref(), &source.source_ref);
    assert_eq!(
        authority.effective_policy_digest(),
        sandbox_policy_content_digest(authority.policy())
    );
    assert_eq!(
        authority.binding_policy_digest(),
        sandbox_policy_content_digest(&binding_policy)
    );
    assert_eq!(
        authority.agent_policy_digest(),
        sandbox_policy_content_digest(&agent_policy)
    );
    assert_eq!(
        authority.source_policy_digest(),
        Some(sandbox_policy_content_digest(&source_policy).as_str())
    );
    assert_eq!(authority.enforcement_profile().protocol(), "enforcement");
    authority.validate_coarse_modes(&requirement).unwrap();
}

#[test]
fn a_write_task_keeps_only_the_intersection_of_all_three_layers() {
    let binding_policy = policy(workspace(&["src"]), endpoint());
    let binding = binding(binding_policy.clone(), policy_ref("binding", 1));
    let requirement = task_requirement(WorkspaceMode::Write, NetworkPolicy::Restricted);

    let agent = agent_type_with_policy(
        "pinned",
        policy_ref("agent-policy", 1),
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );
    let agent_policy = policy(PathSet::any(), NetworkEndpointSet::any());
    let agent_revision = SecurityRevision::new(
        &agent,
        SecurityContractMode::SecurityV1,
        Some(&agent_policy),
    )
    .unwrap();
    let source = source();
    let source_policy = policy(workspace(&["src", "tmp"]), NetworkEndpointSet::any());
    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(policy_ref("source-policy", 1), &source_policy),
        EnforcementProfileRef::new("enforcement", "profile", 1).unwrap(),
    );

    let (task_id, generation_id, logical_agent_id) = identity();
    let authority = EffectiveTaskAuthority::new(
        task_id,
        generation_id,
        logical_agent_id,
        &binding,
        &requirement,
        &agent_revision,
        &source,
        &declaration,
        Some(&source_policy),
    )
    .unwrap();

    // The source layer writes only the `src/tmp` subtree, so the meet narrows
    // the binding's whole `src` grant to that subtree.
    assert_eq!(
        authority.policy().files().write(),
        &workspace(&["src", "tmp"])
    );
    assert!(is_within(authority.policy(), &binding_policy));
    // `Write`/`Enabled` do not narrow anything by themselves.
    assert_eq!(authority.policy().network(), &endpoint());
    authority.validate_coarse_modes(&requirement).unwrap();
}

#[test]
fn a_coarse_agent_revision_can_never_carry_security_v1_authority() {
    let policy = policy(workspace(&["src"]), endpoint());
    let binding = binding(policy.clone(), policy_ref("binding", 1));
    let requirement = task_requirement(WorkspaceMode::ReadOnly, NetworkPolicy::Disabled);
    let agent = agent_type_with_policy(
        "pinned",
        policy_ref("agent-policy", 1),
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );
    let coarse = SecurityRevision::new(&agent, SecurityContractMode::CoarseV1, None).unwrap();
    let declaration = SpawnSourceSecurityDeclaration::without_policy(
        EnforcementProfileRef::new("enforcement", "profile", 1).unwrap(),
    );

    let (task_id, generation_id, logical_agent_id) = identity();
    let error = EffectiveTaskAuthority::new(
        task_id,
        generation_id,
        logical_agent_id,
        &binding,
        &requirement,
        &coarse,
        &source(),
        &declaration,
        None,
    )
    .unwrap_err();
    assert!(matches!(error, ContractError::SecurityUnenforceable { .. }));
}

#[test]
fn a_security_revision_requires_its_authored_policy() {
    let agent = agent_type_with_policy(
        "pinned",
        policy_ref("agent-policy", 1),
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );
    assert!(matches!(
        SecurityRevision::new(&agent, SecurityContractMode::SecurityV1, None).unwrap_err(),
        ContractError::InvariantViolation(_)
    ));

    // A COARSE_V1 revision ignores a supplied policy rather than borrowing it.
    let stray = policy(PathSet::any(), NetworkEndpointSet::any());
    let coarse =
        SecurityRevision::new(&agent, SecurityContractMode::CoarseV1, Some(&stray)).unwrap();
    assert!(coarse.authored_policy().is_none());
}

#[test]
fn an_unsupported_derivation_version_is_not_authority() {
    let policy = policy(workspace(&["src"]), endpoint());
    let reference = policy_ref("binding", 1);
    let input = PolicyInput::authored(reference, &policy);
    let deriv = SecurityDerivation::from_parts(
        [input],
        SANDBOX_POLICY_NORMALIZATION_VERSION + 1,
        SANDBOX_POLICY_MEET_ALGORITHM_VERSION,
        sandbox_policy_content_digest(&policy),
        required_access_content_digest(&RequiredAccess::empty()),
    );
    let binding = TaskSecurityBinding::new(policy, deriv, []).unwrap();
    let requirement = task_requirement(WorkspaceMode::ReadOnly, NetworkPolicy::Restricted);

    let error =
        TaskSecurityCeiling::from_binding(TaskId::from_string("task-1"), &binding, &requirement)
            .unwrap_err();
    assert!(matches!(
        error,
        ContractError::UnsupportedPolicyDerivationVersion { .. }
    ));
}

#[test]
fn the_source_layer_must_match_its_own_declaration() {
    let binding_policy = policy(workspace(&["src"]), endpoint());
    let binding = binding(binding_policy, policy_ref("binding", 1));
    let requirement = task_requirement(WorkspaceMode::Write, NetworkPolicy::Enabled);

    let agent = agent_type_with_policy(
        "pinned",
        policy_ref("agent-policy", 1),
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );
    let agent_policy = policy(PathSet::any(), NetworkEndpointSet::any());
    let agent_revision = SecurityRevision::new(
        &agent,
        SecurityContractMode::SecurityV1,
        Some(&agent_policy),
    )
    .unwrap();

    let declared = policy(workspace(&["src"]), endpoint());
    let presented = policy(workspace(&["src", "tmp"]), endpoint());
    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(policy_ref("source-policy", 1), &declared),
        EnforcementProfileRef::new("enforcement", "profile", 1).unwrap(),
    );

    let (task_id, generation_id, logical_agent_id) = identity();
    let error = EffectiveTaskAuthority::new(
        task_id,
        generation_id,
        logical_agent_id,
        &binding,
        &requirement,
        &agent_revision,
        &source(),
        &declaration,
        Some(&presented),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ContractError::EvidenceSubjectMismatch { .. }
    ));

    // A declaration with a policy but no policy bytes is equally refused.
    let (task_id, generation_id, logical_agent_id) = identity();
    let error = EffectiveTaskAuthority::new(
        task_id,
        generation_id,
        logical_agent_id,
        &binding,
        &requirement,
        &agent_revision,
        &source(),
        &declaration,
        None,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ContractError::EvidenceSubjectMismatch { .. }
    ));
}

#[test]
fn the_composite_rechecks_the_task_coarse_modes_after_the_meet() {
    // The Task asks for full network, but the durable binding is restricted. The
    // fine-derived policy is valid on its own; only the composed policy proves
    // the coarse requirement unsatisfiable.
    let binding_policy = policy(workspace(&["src"]), endpoint());
    let binding = binding(binding_policy, policy_ref("binding", 1));
    let requirement = task_requirement(WorkspaceMode::ReadOnly, NetworkPolicy::Enabled);

    let agent = agent_type_with_policy(
        "pinned",
        policy_ref("agent-policy", 1),
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );
    let agent_policy = policy(PathSet::any(), NetworkEndpointSet::any());
    let agent_revision = SecurityRevision::new(
        &agent,
        SecurityContractMode::SecurityV1,
        Some(&agent_policy),
    )
    .unwrap();
    let declaration = SpawnSourceSecurityDeclaration::without_policy(
        EnforcementProfileRef::new("enforcement", "profile", 1).unwrap(),
    );

    let (task_id, generation_id, logical_agent_id) = identity();
    let authority = EffectiveTaskAuthority::new(
        task_id,
        generation_id,
        logical_agent_id,
        &binding,
        &requirement,
        &agent_revision,
        &source(),
        &declaration,
        None,
    )
    .unwrap();
    assert!(matches!(
        authority.validate_coarse_modes(&requirement).unwrap_err(),
        ContractError::SecurityUnenforceable { .. }
    ));
}

#[test]
fn the_task_ceiling_applies_the_coarse_modes_and_records_identity() {
    let binding_policy = policy(workspace(&["src"]), endpoint());
    let binding = binding(binding_policy.clone(), policy_ref("binding", 1));
    let requirement = task_requirement(WorkspaceMode::ReadOnly, NetworkPolicy::Disabled);

    let ceiling =
        TaskSecurityCeiling::from_binding(TaskId::from_string("task-1"), &binding, &requirement)
            .unwrap();
    assert!(ceiling.policy().files().write().is_empty());
    assert_eq!(ceiling.policy().network(), &NetworkEndpointSet::deny_all());
    assert_eq!(
        ceiling.binding_policy_digest(),
        sandbox_policy_content_digest(&binding_policy)
    );
    assert_eq!(
        ceiling.policy_digest(),
        sandbox_policy_content_digest(ceiling.policy())
    );
    assert_eq!(ceiling.required_workspace(), WorkspaceMode::ReadOnly);
    assert_eq!(ceiling.required_network(), NetworkPolicy::Disabled);
    assert_eq!(
        ceiling.requirement_digest(),
        task_agent_requirement_content_digest(&requirement)
    );
}
