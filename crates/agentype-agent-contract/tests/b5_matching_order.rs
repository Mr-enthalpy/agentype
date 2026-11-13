//! M6-B.5 B5.2 policy-order preselection conformance.
//!
//! These tests pin the four hard acceptance items from ADR-0013 §9 that are
//! observable in the pure layer:
//!
//! 1. mode-aware preselection over the policy order;
//! 2. policy-aware `more_specific_for`;
//! 4. no `SECURITY_V1` candidate dropped merely because its policy reference
//!    differs while its policy is a legal bound;
//!
//! and the cross-mode exclusion from §11.

use agentype_agent_contract::*;
use agentype_core::{
    ContinuityPreference, InformationFunction, LogicalAgentId, PartitionId, TaskId, WorkspaceMode,
};
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

fn policy(read: PathSet, write: PathSet, network: NetworkEndpointSet) -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            read,
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

fn wide_policy() -> NormalizedSandboxPolicy {
    policy(PathSet::any(), PathSet::any(), NetworkEndpointSet::any())
}

fn narrow_policy() -> NormalizedSandboxPolicy {
    policy(PathSet::any(), workspace(&["src"]), endpoint())
}

fn ceiling_policy() -> NormalizedSandboxPolicy {
    // `ReadOnly` + `Restricted` applied to `narrow_policy`.
    policy(PathSet::any(), PathSet::deny_all(), endpoint())
}

fn agent_type(
    id: &str,
    reference: SandboxPolicyRef,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
    policy_ref: Option<SandboxPolicyRef>,
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
            sandbox_policy: policy_ref.or(Some(reference)),
            anchor_constraint: None,
        },
    }
}

fn requirement(pin: &AgentTypeRef, fine: Option<SandboxPolicyRef>) -> TaskAgentRequirement {
    TaskAgentRequirement {
        required_type: pin.clone(),
        hard: TaskRequirement {
            information_function: InformationFunction::Expand,
            required_capabilities: BTreeMap::new(),
            required_affinity: BTreeSet::new(),
            required_workspace: WorkspaceMode::ReadOnly,
            required_network: NetworkPolicy::Restricted,
            required_attempt_isolation: false,
            required_continuity: ContinuityMode::None,
            sandbox_policy: fine,
            required_anchor: None,
            budget: Budget::new(50.0).unwrap(),
        },
    }
}

fn binding(policy: NormalizedSandboxPolicy, reference: SandboxPolicyRef) -> TaskSecurityBinding {
    let input = PolicyInput::authored(reference, &policy);
    let derivation = SecurityDerivation::new([input], &policy, &RequiredAccess::empty()).unwrap();
    TaskSecurityBinding::new(policy, derivation, []).unwrap()
}

fn placement() -> TaskPlacement {
    TaskPlacement {
        partition: PartitionId::new("p1"),
        required_tags: BTreeSet::new(),
        workstream_id: None,
        continuity: ContinuityPreference::None,
    }
}

fn candidate(id: &str, agent_type: AgentType) -> ExistingAgentCandidate {
    ExistingAgentCandidate {
        logical_agent_id: LogicalAgentId::from_string(id),
        agent_type,
        partition: PartitionId::new("p1"),
        tags: BTreeSet::new(),
        workstream_id: None,
        available_since: Some(1.0),
        created_at: 1.0,
    }
}

struct Fixture {
    pinned: AgentType,
    ceiling: TaskSecurityCeiling,
    request: TaskAgentRequirement,
}

fn fixture(fine: Option<SandboxPolicyRef>) -> Fixture {
    let binding_policy = narrow_policy();
    let binding = binding(binding_policy, policy_ref("binding", 1));
    let pinned = agent_type(
        "pinned",
        policy_ref("pinned", 1),
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
        None,
    );
    let request = requirement(&type_ref("pinned", 1), fine);
    let ceiling =
        TaskSecurityCeiling::from_binding(TaskId::from_string("task-1"), &binding, &request)
            .unwrap();
    Fixture {
        pinned,
        ceiling,
        request,
    }
}

#[test]
fn policy_order_keeps_a_legal_candidate_whose_reference_differs() {
    let fixture = fixture(Some(policy_ref("task-fine", 1)));
    let catalog = CapabilityCatalog::new();

    // Equal to the Task ceiling but a *different* catalog revision from the
    // Task's authored fine policy. Reference equality would drop it.
    let narrow_policy = ceiling_policy();
    let narrow_type = agent_type(
        "narrow",
        policy_ref("narrow-policy", 1),
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        None,
    );
    let narrow = candidate("agent-narrow", narrow_type);
    let narrow_revision = QualifiedExistingAgentCandidate::new(
        &narrow,
        SecurityContractMode::SecurityV1,
        Some(&narrow_policy),
    )
    .unwrap();

    let pinned_policy = wide_policy();
    let pinned_revision = SecurityRevision::new(
        &fixture.pinned,
        SecurityContractMode::SecurityV1,
        Some(&pinned_policy),
    )
    .unwrap();

    // The frozen equality relation drops it: the Task authors `task-fine`, the
    // candidate authors `narrow-policy`.
    let frozen = match_existing_agents(
        &fixture.pinned,
        &fixture.request,
        &placement(),
        std::slice::from_ref(&narrow),
        &catalog,
    )
    .unwrap();
    assert!(frozen.is_empty(), "reference equality drops the candidate");

    let qualified = match_existing_agents_security_v1(
        &pinned_revision,
        &fixture.ceiling,
        &fixture.request,
        &placement(),
        &[narrow_revision],
        &catalog,
    )
    .unwrap();
    assert_eq!(qualified.len(), 1);
    assert_eq!(qualified[0].logical_agent_id, narrow.logical_agent_id);

    // Sanity: the candidate really is a legal bound of the ceiling.
    assert!(is_within(fixture.ceiling.policy(), &narrow_policy));
}

#[test]
fn policy_order_ranks_narrower_first_and_excludes_non_bounds() {
    let fixture = fixture(None);
    let catalog = CapabilityCatalog::new();

    let narrow_policy = ceiling_policy();
    let broad_policy = wide_policy();
    // Read is narrower than the ceiling's `any`, so it cannot bound the Task.
    let too_narrow_policy = policy(workspace(&["sandbox"]), PathSet::deny_all(), endpoint());

    let narrow = candidate(
        "agent-narrow",
        agent_type(
            "narrow",
            policy_ref("narrow-policy", 1),
            WorkspaceMode::ReadOnly,
            NetworkPolicy::Restricted,
            None,
        ),
    );
    let broad = candidate(
        "agent-broad",
        agent_type(
            "broad",
            policy_ref("broad-policy", 1),
            WorkspaceMode::Write,
            NetworkPolicy::Enabled,
            None,
        ),
    );
    let too_narrow = candidate(
        "agent-too-narrow",
        agent_type(
            "too-narrow",
            policy_ref("too-narrow-policy", 1),
            WorkspaceMode::ReadOnly,
            NetworkPolicy::Restricted,
            None,
        ),
    );
    let coarse = candidate(
        "agent-coarse",
        agent_type(
            "coarse",
            policy_ref("ignored", 1),
            WorkspaceMode::ReadOnly,
            NetworkPolicy::Restricted,
            None,
        ),
    );

    let qualified = vec![
        QualifiedExistingAgentCandidate::new(
            &narrow,
            SecurityContractMode::SecurityV1,
            Some(&narrow_policy),
        )
        .unwrap(),
        QualifiedExistingAgentCandidate::new(
            &broad,
            SecurityContractMode::SecurityV1,
            Some(&broad_policy),
        )
        .unwrap(),
        QualifiedExistingAgentCandidate::new(
            &too_narrow,
            SecurityContractMode::SecurityV1,
            Some(&too_narrow_policy),
        )
        .unwrap(),
        // A COARSE_V1 revision carries no authored policy.
        QualifiedExistingAgentCandidate::new(&coarse, SecurityContractMode::CoarseV1, None)
            .unwrap(),
    ];

    let pinned_revision = SecurityRevision::new(
        &fixture.pinned,
        SecurityContractMode::SecurityV1,
        Some(&broad_policy),
    )
    .unwrap();

    let ranked = match_existing_agents_security_v1(
        &pinned_revision,
        &fixture.ceiling,
        &fixture.request,
        &placement(),
        &qualified,
        &catalog,
    )
    .unwrap();

    // The narrower candidate sorts before the broader one; the non-bound and
    // the COARSE cross-mode candidate are absent.
    let ids: Vec<&str> = ranked.iter().map(|c| c.logical_agent_id.as_str()).collect();
    assert_eq!(ids, vec!["agent-narrow", "agent-broad"]);
}

#[test]
fn policy_aware_specificity_uses_the_algebra_not_the_reference() {
    let fixture = fixture(None);
    let catalog = CapabilityCatalog::new();

    let narrow_policy = ceiling_policy();
    let broad_policy = wide_policy();
    let narrow = candidate(
        "agent-narrow",
        agent_type(
            "narrow",
            policy_ref("narrow-policy", 1),
            WorkspaceMode::ReadOnly,
            NetworkPolicy::Restricted,
            None,
        ),
    );
    let broad = candidate(
        "agent-broad",
        agent_type(
            "broad",
            policy_ref("broad-policy", 1),
            WorkspaceMode::Write,
            NetworkPolicy::Enabled,
            None,
        ),
    );

    let pinned = SecurityRevision::new(
        &fixture.pinned,
        SecurityContractMode::SecurityV1,
        Some(&broad_policy),
    )
    .unwrap();
    let narrow_revision = QualifiedExistingAgentCandidate::new(
        &narrow,
        SecurityContractMode::SecurityV1,
        Some(&narrow_policy),
    )
    .unwrap();
    let broad_revision = QualifiedExistingAgentCandidate::new(
        &broad,
        SecurityContractMode::SecurityV1,
        Some(&broad_policy),
    )
    .unwrap();

    assert!(more_specific_for_with_policies(
        &narrow_revision.revision(),
        &pinned,
        &fixture.request.hard,
        &fixture.ceiling,
        &catalog,
    ));
    assert!(!more_specific_for_with_policies(
        &pinned,
        &narrow_revision.revision(),
        &fixture.request.hard,
        &fixture.ceiling,
        &catalog,
    ));
    assert!(!more_specific_for_with_policies(
        &broad_revision.revision(),
        &pinned,
        &fixture.request.hard,
        &fixture.ceiling,
        &catalog,
    ));
}

#[test]
fn a_coarse_candidate_cannot_be_qualified_for_a_security_task() {
    let fixture = fixture(None);
    let catalog = CapabilityCatalog::new();
    let coarse = candidate(
        "agent-coarse",
        agent_type(
            "coarse",
            policy_ref("coarse", 1),
            WorkspaceMode::ReadOnly,
            NetworkPolicy::Restricted,
            // Even a declared reference does not make it SECURITY_V1: the mode
            // owns that, and the stored mode is COARSE.
            None,
        ),
    );
    let view = QualifiedExistingAgentCandidate::new(&coarse, SecurityContractMode::CoarseV1, None)
        .unwrap();
    let coarse_policy = wide_policy();
    let pinned = SecurityRevision::new(
        &fixture.pinned,
        SecurityContractMode::SecurityV1,
        Some(&coarse_policy),
    )
    .unwrap();
    let ranked = match_existing_agents_security_v1(
        &pinned,
        &fixture.ceiling,
        &fixture.request,
        &placement(),
        &[view],
        &catalog,
    )
    .unwrap();
    assert!(ranked.is_empty());
}

#[test]
fn a_security_qualified_candidate_must_project_to_its_coarse_contract() {
    let candidate = candidate(
        "agent-mismatch",
        agent_type(
            "mismatch",
            policy_ref("mismatch-policy", 1),
            WorkspaceMode::ReadOnly,
            NetworkPolicy::Restricted,
            None,
        ),
    );
    // The policy allows writing, but the revision's coarse contract says
    // ReadOnly. Qualifying it would fabricate authority the revision never
    // declared.
    let wider = wide_policy();
    assert!(matches!(
        QualifiedExistingAgentCandidate::new(
            &candidate,
            SecurityContractMode::SecurityV1,
            Some(&wider)
        )
        .unwrap_err(),
        ContractError::InvariantViolation(_)
    ));
}
