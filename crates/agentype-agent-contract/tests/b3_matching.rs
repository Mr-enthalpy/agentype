//! M6-B.3 pure-layer conformance: Task agent requirement canonicalization, the
//! Generation policy fold, and existing-agent matching order.

use agentype_agent_contract::*;
use agentype_core::{InformationFunction, LogicalAgentId, WorkspaceMode};
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
        type_ref: AgentTypeRef::new(id, revision).unwrap(),
        based_on: None,
        contract: contract(affinity, ceiling),
    }
}

fn candidate(
    id: &str,
    agent_type: AgentType,
    available_since: Option<f64>,
) -> ExistingAgentCandidate {
    ExistingAgentCandidate {
        logical_agent_id: LogicalAgentId::from_string(id),
        agent_type,
        lifecycle: LifecycleMode::Resident,
        available_since,
        created_at: 1.0,
    }
}

#[test]
fn requirement_digest_ignores_affinity_order_and_bool_false() {
    let catalog = catalog();
    let pinned = AgentTypeRef::new("reviewer", 1).unwrap();

    let mut with_false = task_requirement();
    with_false
        .required_capabilities
        .insert(cref("attempt.isolation", 1), CapabilityValue::Bool(false));
    with_false.required_affinity = set(&["b", "a"]);

    let mut omitted = task_requirement();
    omitted.required_affinity = set(&["a", "b"]);

    let mut a = TaskAgentRequirement {
        required_type: None,
        hard: with_false,
    };
    let mut b = TaskAgentRequirement {
        required_type: Some(pinned.clone()),
        hard: omitted.clone(),
    };
    // `required_type` is part of the identity, so pin both to compare content.
    let mut c = TaskAgentRequirement {
        required_type: Some(pinned),
        hard: omitted,
    };

    canonicalize_task_agent_requirement(&mut a, &catalog).unwrap();
    canonicalize_task_agent_requirement(&mut b, &catalog).unwrap();
    canonicalize_task_agent_requirement(&mut c, &catalog).unwrap();

    // `Bool(false)` normalizes to omission; `b` and `c` are the same content.
    assert!(a.hard.required_capabilities.is_empty());
    assert_eq!(
        task_agent_requirement_content_digest(&b),
        task_agent_requirement_content_digest(&c)
    );
    // A different pin changes the digest.
    assert_ne!(
        task_agent_requirement_content_digest(&a),
        task_agent_requirement_content_digest(&b)
    );

    // Round-trip through the canonical document.
    let json = String::from_utf8(canonical_task_agent_requirement_bytes(&b)).unwrap();
    let decoded = task_agent_requirement_from_canonical_json(&json).unwrap();
    assert_eq!(decoded, b);
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
    // Generation caps at Write/Enabled; a ReadOnly Task keeps ReadOnly.
    let effective = fold_generation_policy(&policy(), &task_requirement()).unwrap();
    assert_eq!(effective.required_workspace, WorkspaceMode::ReadOnly);
    assert_eq!(effective.required_network, NetworkPolicy::Restricted);
    assert_eq!(effective.required_continuity, ContinuityMode::Logical);
    assert_eq!(effective.budget, budget(10.0));
}

#[test]
fn generation_policy_fold_rejects_a_task_that_exceeds_the_ceiling() {
    // Generation forbids Write: a Write Task must fail closed.
    let mut read_only_policy = policy();
    read_only_policy.max_workspace = WorkspaceMode::ReadOnly;
    let mut write_task = task_requirement();
    write_task.required_workspace = WorkspaceMode::Write;
    assert!(matches!(
        fold_generation_policy(&read_only_policy, &write_task),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));

    // Generation forbids network: an Enabled Task must fail closed.
    let mut no_network_policy = policy();
    no_network_policy.max_network = NetworkPolicy::Disabled;
    let mut networked_task = task_requirement();
    networked_task.required_network = NetworkPolicy::Enabled;
    assert!(matches!(
        fold_generation_policy(&no_network_policy, &networked_task),
        Err(ContractError::GenerationPolicyConflict { .. })
    ));

    // A stricter Task under a wider ceiling keeps its own value.
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
fn matching_requires_pin_and_filters_incompatible_contracts() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);

    // No pinned type: nominal selection is deferred, matching returns nothing,
    // but this is still a typed (constrained) Task, never a legacy Task.
    let untyped = TaskAgentRequirement {
        required_type: None,
        hard: task_requirement(),
    };
    assert!(match_existing_agents(&required, &untyped, &[], &catalog)
        .unwrap()
        .is_empty());

    // Mismatched pin is a caller invariant violation, not an empty result.
    let wrong_pin = TaskAgentRequirement {
        required_type: Some(AgentTypeRef::new("other", 1).unwrap()),
        hard: task_requirement(),
    };
    assert!(matches!(
        match_existing_agents(&required, &wrong_pin, &[], &catalog),
        Err(ContractError::InvariantViolation(_))
    ));

    // A candidate whose contract cannot execute the task is filtered out.
    let mut incapable_contract = contract(AffinityConstraint::Any, 100.0);
    incapable_contract.allowed_information_functions = vec![InformationFunction::CompressPositive];
    let incapable = ExistingAgentCandidate {
        logical_agent_id: LogicalAgentId::from_string("agent-incapable"),
        agent_type: AgentType {
            type_ref: AgentTypeRef::new("reviewer", 1).unwrap(),
            based_on: None,
            contract: incapable_contract,
        },
        lifecycle: LifecycleMode::Resident,
        available_since: Some(0.0),
        created_at: 1.0,
    };

    let req = TaskAgentRequirement {
        required_type: Some(required.type_ref.clone()),
        hard: task_requirement(),
    };
    let matched = match_existing_agents(&required, &req, &[incapable], &catalog).unwrap();
    assert!(matched.is_empty());
}

#[test]
fn matching_orders_exact_then_specific_then_deterministic() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);
    let req = TaskAgentRequirement {
        required_type: Some(required.type_ref.clone()),
        hard: task_requirement(),
    };

    // The exact type wins over a narrower (more specific) candidate.
    let exact = candidate(
        "agent-exact",
        agent("reviewer", 1, AffinityConstraint::Any, 100.0),
        Some(5.0),
    );
    let narrower = candidate(
        "agent-narrow",
        agent(
            "rust-reviewer",
            2,
            AffinityConstraint::Only(set(&["rust"])),
            50.0,
        ),
        Some(1.0),
    );

    let matched = match_existing_agents(&required, &req, &[narrower, exact], &catalog).unwrap();
    let order: Vec<&str> = matched
        .iter()
        .map(|c| c.logical_agent_id.as_str())
        .collect();
    assert_eq!(order, vec!["agent-exact", "agent-narrow"]);
    // Only immediately usable (READY, unassigned) candidates are returned; the
    // caller never receives a "cold" candidate it did not prove.
    assert!(matched
        .iter()
        .all(|c| c.lifecycle == LifecycleMode::Resident));

    // Two equivalent exact candidates order deterministically by id.
    let a = candidate(
        "agent-a",
        agent("reviewer", 1, AffinityConstraint::Any, 100.0),
        Some(1.0),
    );
    let b = candidate(
        "agent-b",
        agent("reviewer", 1, AffinityConstraint::Any, 100.0),
        Some(1.0),
    );
    let matched = match_existing_agents(&required, &req, &[b, a], &catalog).unwrap();
    assert_eq!(matched[0].logical_agent_id.as_str(), "agent-a");
}

#[test]
fn matching_prefers_candidate_specificity_before_availability() {
    let catalog = catalog();
    let required = agent("reviewer", 1, AffinityConstraint::Any, 100.0);
    let req = TaskAgentRequirement {
        required_type: Some(required.type_ref.clone()),
        hard: task_requirement(),
    };

    // Both A and B are refinements of the required type; A is strictly more
    // specific (subset affinity, lower budget). B has an earlier available_since.
    let a = candidate(
        "agent-a",
        agent(
            "rust-reviewer",
            2,
            AffinityConstraint::Only(set(&["rust"])),
            50.0,
        ),
        Some(9.0),
    );
    let b = candidate(
        "agent-b",
        agent(
            "rust-reviewer",
            3,
            AffinityConstraint::Only(set(&["rust"])),
            80.0,
        ),
        Some(1.0),
    );
    // A broader type and a semantically incomparable type are NOT substitutes
    // for a pinned requirement, even though both can execute the Task.
    let broader = candidate(
        "agent-broad",
        agent("generalist", 1, AffinityConstraint::Any, 300.0),
        Some(0.0),
    );
    let incomparable = candidate(
        "agent-incomparable",
        agent("odd", 1, AffinityConstraint::Only(set(&["rust"])), 300.0),
        Some(0.0),
    );

    let matched = match_existing_agents(
        &required,
        &req,
        &[broader, incomparable, b.clone(), a.clone()],
        &catalog,
    )
    .unwrap();
    let order: Vec<&str> = matched
        .iter()
        .map(|c| c.logical_agent_id.as_str())
        .collect();
    assert_eq!(order, vec!["agent-a", "agent-b"]);
}
