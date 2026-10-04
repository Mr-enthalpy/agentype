//! M6-B.3 pure-layer conformance: Task agent requirement canonicalization, the
//! Generation policy fold, M5 placement composition, and existing-agent ranking.

use agentype_agent_contract::*;
use agentype_core::{
    ContinuityPreference, InformationFunction, LogicalAgentId, PartitionId, WorkspaceMode,
    WorkstreamId,
};
use std::collections::{BTreeMap, BTreeSet};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn cref(id: &str, revision: u64) -> CapabilityRef {
    CapabilityRef::new(id, revision).unwrap()
}

fn budget(value: f64) -> Budget {
    Budget::new(value).unwrap()
}

fn catalog() -> CapabilityCatalog {
    let mut catalog = CapabilityCatalog::new();
    catalog
        .define(
            cref("tools", 1),
            MatcherKind::Set,
            SecurityClass::Functional,
            CapabilityPolarity::Ability,
        )
        .unwrap();
    catalog
        .define(
            cref("attempt.isolation", 1),
            MatcherKind::Bool,
            SecurityClass::Sandbox,
            CapabilityPolarity::Restriction,
        )
        .unwrap();
    catalog
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn task_requirement() -> TaskRequirement {
    TaskRequirement {
        information_function: InformationFunction::Expand,
        required_capabilities: BTreeMap::new(),
        required_affinity: BTreeSet::new(),
        required_workspace: WorkspaceMode::ReadOnly,
        required_network: NetworkPolicy::Restricted,
        required_attempt_isolation: false,
        required_continuity: ContinuityMode::None,
        sandbox_policy: None,
        required_anchor: None,
        budget: budget(50.0),
    }
}

fn contract(affinity: AffinityConstraint, budget_ceiling: f64) -> AgentTypeContract {
    AgentTypeContract {
        allowed_information_functions: vec![InformationFunction::Expand],
        required_capabilities: BTreeMap::new(),
        affinity,
        budget_ceiling: budget(budget_ceiling),
        security: SecurityContract {
            workspace: WorkspaceMode::ReadOnly,
            network: NetworkPolicy::Restricted,
            requires_attempt_isolation: false,
        },
        lifecycle: [LifecycleMode::Resident].into_iter().collect(),
        continuity: ContinuityMode::Logical,
        sandbox_policy: None,
        anchor_constraint: None,
    }
}

fn agent(id: &str, revision: u64, affinity: AffinityConstraint, ceiling: f64) -> AgentType {
    AgentType {
        type_ref: type_ref(id, revision),
        based_on: None,
        contract: contract(affinity, ceiling),
    }
}

fn placement(continuity: ContinuityPreference) -> TaskPlacement {
    TaskPlacement {
        partition: PartitionId::new("p1"),
        required_tags: BTreeSet::new(),
        workstream_id: None,
        continuity,
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

fn pinned(required: &AgentType) -> TaskAgentRequirement {
    TaskAgentRequirement {
        required_type: required.type_ref.clone(),
        hard: task_requirement(),
    }
}

#[test]
fn requirement_digest_ignores_affinity_order_and_bool_false() {
    let catalog = catalog();
    let pin = type_ref("reviewer", 1);

    let mut with_false = task_requirement();
    with_false
        .required_capabilities
        .insert(cref("attempt.isolation", 1), CapabilityValue::Bool(false));
    with_false.required_affinity = set(&["b", "a"]);

    let mut omitted = task_requirement();
    omitted.required_affinity = set(&["a", "b"]);

    let mut a = TaskAgentRequirement {
        required_type: pin.clone(),
        hard: with_false,
    };
    let mut b = TaskAgentRequirement {
        required_type: pin,
        hard: omitted,
    };

    canonicalize_task_agent_requirement(&mut a, &catalog).unwrap();
    canonicalize_task_agent_requirement(&mut b, &catalog).unwrap();

    assert!(a.hard.required_capabilities.is_empty());
    assert_eq!(
        task_agent_requirement_content_digest(&a),
        task_agent_requirement_content_digest(&b)
    );

    let json = String::from_utf8(canonical_task_agent_requirement_bytes(&a)).unwrap();
    let decoded = task_agent_requirement_from_canonical_json(&json).unwrap();
    assert_eq!(decoded, a);
}

fn policy() -> GenerationPolicy {
    GenerationPolicy {
        allowed_information_functions: vec![InformationFunction::Expand],
        max_workspace: WorkspaceMode::Write,
        max_network: NetworkPolicy::Enabled,
        requires_attempt_isolation: false,
        min_continuity: ContinuityMode::Logical,
        sandbox_policy: None,
        budget_ceiling: budget(10.0),
        allowed_affinity: None,
        anchor_constraint: None,
    }
}

#[test]
fn generation_policy_fold_keeps_the_stricter_task_and_caps_authority() {
    let effective = fold_generation_policy(&policy(), &task_requirement()).unwrap();
    assert_eq!(effective.required_workspace, WorkspaceMode::ReadOnly);
    assert_eq!(effective.required_network, NetworkPolicy::Restricted);
    assert_eq!(effective.required_continuity, ContinuityMode::Logical);
    assert_eq!(effective.budget, budget(10.0));
}

#[test]
fn generation_policy_fold_rejects_a_task_that_exceeds_the_ceiling() {
    let mut read_only_policy = policy();
    read_only_policy.max_workspace = WorkspaceMode::ReadOnly;
    let mut write_task = task_requirement();
    write_task.required_workspace = WorkspaceMode::Write;
    assert!(matches!(
        fold_generation_policy(&read_only_policy, &write_task),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));

    let mut no_network_policy = policy();
    no_network_policy.max_network = NetworkPolicy::Disabled;
    let mut networked_task = task_requirement();
    networked_task.required_network = NetworkPolicy::Enabled;
    assert!(matches!(
        fold_generation_policy(&no_network_policy, &networked_task),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));

    let mut disabled_task = task_requirement();
    disabled_task.required_network = NetworkPolicy::Disabled;
    let effective = fold_generation_policy(&policy(), &disabled_task).unwrap();
    assert_eq!(effective.required_network, NetworkPolicy::Disabled);
}

#[test]
fn generation_policy_fold_rejects_widening() {
    let mut req = task_requirement();
    req.information_function = InformationFunction::CompressNegative;
    assert!(matches!(
        fold_generation_policy(&policy(), &req),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));

    let mut affinity_policy = policy();
    affinity_policy.allowed_affinity = Some(set(&["rust"]));
    let mut req = task_requirement();
    req.required_affinity = set(&["crypto"]);
    assert!(matches!(
        fold_generation_policy(&affinity_policy, &req),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));

    let mut sandbox_policy = policy();
    sandbox_policy.sandbox_policy = Some(SandboxPolicyRef::new("strict", 1).unwrap());
    let mut req = task_requirement();
    req.sandbox_policy = Some(SandboxPolicyRef::new("other", 1).unwrap());
    assert!(matches!(
        fold_generation_policy(&sandbox_policy, &req),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));
}

#[test]
fn generation_policy_round_trips_canonically() {
    let catalog = catalog();
    let mut canonical = policy();
    canonicalize_generation_policy(&mut canonical, &catalog).unwrap();
    let json = String::from_utf8(canonical_generation_policy_bytes(&canonical)).unwrap();
    assert_eq!(
        generation_policy_from_canonical_json(&json).unwrap(),
        canonical
    );
}

#[test]
fn matching_filters_incompatible_contracts_and_pins() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);

    let mut incapable_contract = contract(AffinityConstraint::Any, 100.0);
    incapable_contract.allowed_information_functions = vec![InformationFunction::CompressPositive];
    let incapable = ExistingAgentCandidate {
        logical_agent_id: LogicalAgentId::from_string("agent-incapable"),
        agent_type: AgentType {
            type_ref: type_ref("reviewer", 1),
            based_on: None,
            contract: incapable_contract,
        },
        partition: PartitionId::new("p1"),
        tags: BTreeSet::new(),
        workstream_id: None,
        available_since: Some(0.0),
        created_at: 1.0,
    };

    let matched = match_existing_agents(
        &required,
        &pinned(&required),
        &placement(ContinuityPreference::None),
        &[incapable],
        &catalog,
    )
    .unwrap();
    assert!(matched.is_empty());

    let wrong_pin = TaskAgentRequirement {
        required_type: type_ref("other", 1),
        hard: task_requirement(),
    };
    assert!(matches!(
        match_existing_agents(
            &required,
            &wrong_pin,
            &placement(ContinuityPreference::None),
            &[],
            &catalog
        ),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn matching_orders_exact_then_narrower_then_broader() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);

    let exact = candidate(
        "agent-exact",
        agent("reviewer", 1, AffinityConstraint::Any, 100.0),
    );
    let narrower = candidate(
        "agent-narrow",
        agent(
            "rust-reviewer",
            2,
            AffinityConstraint::Only(set(&["rust"])),
            50.0,
        ),
    );
    let broader = candidate(
        "agent-broad",
        agent("generalist", 1, AffinityConstraint::Any, 300.0),
    );

    let matched = match_existing_agents(
        &required,
        &pinned(&required),
        &placement(ContinuityPreference::None),
        &[broader, narrower, exact],
        &catalog,
    )
    .unwrap();
    let order: Vec<&str> = matched
        .iter()
        .map(|c| c.logical_agent_id.as_str())
        .collect();
    assert_eq!(order, vec!["agent-exact", "agent-narrow", "agent-broad"]);
}

#[test]
fn matching_prefers_candidate_specificity_before_availability() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);

    let mut a = candidate(
        "agent-a",
        agent(
            "rust-reviewer",
            2,
            AffinityConstraint::Only(set(&["rust"])),
            50.0,
        ),
    );
    a.available_since = Some(9.0);
    let mut b = candidate(
        "agent-b",
        agent(
            "rust-reviewer",
            3,
            AffinityConstraint::Only(set(&["rust"])),
            80.0,
        ),
    );
    b.available_since = Some(1.0);

    let matched = match_existing_agents(
        &required,
        &pinned(&required),
        &placement(ContinuityPreference::None),
        &[b, a],
        &catalog,
    )
    .unwrap();
    let order: Vec<&str> = matched
        .iter()
        .map(|c| c.logical_agent_id.as_str())
        .collect();
    assert_eq!(order, vec!["agent-a", "agent-b"]);
}

#[test]
fn matching_enforces_m5_placement_and_prefers_workstream() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);
    let req = pinned(&required);

    let w1 = WorkstreamId::from_string("w1");
    let w2 = WorkstreamId::from_string("w2");

    let mut placement = placement(ContinuityPreference::Preferred);
    placement.required_tags = set(&["rust"]);
    placement.workstream_id = Some(w1.clone());

    let mut same_ws = candidate("agent-w1", required.clone());
    same_ws.tags = set(&["rust"]);
    same_ws.workstream_id = Some(w1.clone());
    let mut other_ws = candidate("agent-w2", required.clone());
    other_ws.tags = set(&["rust"]);
    other_ws.workstream_id = Some(w2);

    let mut wrong_partition = candidate("agent-part", required.clone());
    wrong_partition.tags = set(&["rust"]);
    wrong_partition.partition = PartitionId::new("p2");
    wrong_partition.workstream_id = Some(w1.clone());

    let mut missing_tag = candidate("agent-tag", required.clone());
    missing_tag.workstream_id = Some(w1.clone());

    let matched = match_existing_agents(
        &required,
        &req,
        &placement,
        &[
            other_ws.clone(),
            wrong_partition,
            missing_tag,
            same_ws.clone(),
        ],
        &catalog,
    )
    .unwrap();
    let order: Vec<&str> = matched
        .iter()
        .map(|c| c.logical_agent_id.as_str())
        .collect();
    assert_eq!(order, vec!["agent-w1", "agent-w2"]);

    // `Required` continuity is a hard workstream gate.
    let mut required_ws = placement.clone();
    required_ws.continuity = ContinuityPreference::Required;
    let matched = match_existing_agents(
        &required,
        &req,
        &required_ws,
        &[same_ws, other_ws],
        &catalog,
    )
    .unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].logical_agent_id.as_str(), "agent-w1");
}

#[test]
fn matching_does_not_conflate_agent_type_lifecycle_with_realized_mode() {
    let catalog = catalog();
    // The AgentType `lifecycle` set is a required source envelope (spec 06), not
    // a realized-instance mode whitelist. A candidate must not be rejected merely
    // because its M5 retention is not a member of the envelope.
    let mut ephemeral_contract = contract(AffinityConstraint::Any, 100.0);
    ephemeral_contract.lifecycle = [LifecycleMode::Ephemeral].into_iter().collect();
    let required = AgentType {
        type_ref: type_ref("reviewer", 1),
        based_on: None,
        contract: ephemeral_contract,
    };
    let matched = match_existing_agents(
        &required,
        &pinned(&required),
        &placement(ContinuityPreference::None),
        &[candidate("agent-resident", required.clone())],
        &catalog,
    )
    .unwrap();
    assert_eq!(matched.len(), 1);
}
