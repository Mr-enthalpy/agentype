//! M6-B.1 conformance: AgentType purity, four independent relations,
//! refinement monotonicity, matcher semantics, exact-revision capability proof
//! paths, enforceability (never "stronger capability implies restriction"),
//! and validated numeric values.

use agentype_agent_contract::*;
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn cref(id: &str, revision: u64) -> CapabilityRef {
    CapabilityRef::new(id, revision).unwrap()
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn budget(value: f64) -> Budget {
    Budget::new(value).unwrap()
}

fn quantity(value: f64) -> Quantity {
    Quantity::new(value).unwrap()
}

fn spec(id: &str, revision: u64, matcher: MatcherKind, class: SecurityClass) -> CapabilitySpec {
    CapabilitySpec {
        reference: cref(id, revision),
        matcher_kind: matcher,
        security_class: class,
    }
}

fn claim(id: &str, revision: u64, value: CapabilityValue, assurance: Assurance) -> CapabilityClaim {
    CapabilityClaim {
        reference: cref(id, revision),
        value,
        assurance,
        evidence_ref: None,
    }
}

fn base_contract() -> AgentTypeContract {
    AgentTypeContract {
        allowed_information_functions: vec![InformationFunction::Expand],
        required_capabilities: BTreeMap::new(),
        capability_specs: BTreeMap::new(),
        permission_ceiling: set(&["read"]),
        visibility: set(&["public"]),
        tools: set(&["git"]),
        roots: set(&["repo"]),
        budget_ceiling: budget(100.0),
        security: SecurityContract {
            workspace: WorkspaceMode::ReadOnly,
            network: NetworkPolicy::Restricted,
            tool_roots: set(&["repo"]),
            requires_attempt_isolation: false,
        },
        lifecycle: [LifecycleMode::Resident].into_iter().collect(),
        continuity: ContinuityMode::Logical,
        anchor_constraint: None,
    }
}

fn base_agent() -> AgentType {
    AgentType {
        type_ref: type_ref("general-reviewer", 1),
        based_on: None,
        contract: base_contract(),
        status: AgentTypeStatus::Published,
    }
}

fn base_task() -> TaskRequirement {
    TaskRequirement {
        information_function: InformationFunction::Expand,
        required_capabilities: BTreeMap::new(),
        required_permissions: set(&["read"]),
        required_tools: set(&["git"]),
        required_workspace: WorkspaceMode::ReadOnly,
        required_network: NetworkPolicy::Restricted,
        required_continuity: ContinuityMode::None,
        required_anchor: None,
        budget: budget(50.0),
    }
}

fn base_source() -> SpawnSource {
    SpawnSource {
        source_ref: SpawnSourceRef::new("codex-local", 2).unwrap(),
        adapter_policy: AdapterPolicyRef::new("codex-local-adapter", 3).unwrap(),
        lifecycle_modes: [LifecycleMode::Resident, LifecycleMode::Ephemeral]
            .into_iter()
            .collect(),
        continuity_modes: [ContinuityMode::None, ContinuityMode::Logical]
            .into_iter()
            .collect(),
        functional_envelope: BTreeMap::new(),
        claims: Vec::new(),
        status: SourceStatus::Active,
    }
}

fn base_config() -> SourceConfig {
    SourceConfig {
        config_ref: SourceConfigRef::new(base_source().source_ref, "deep-reasoning", 7).unwrap(),
        config_digest: "sha256:abc".into(),
        credential_refs: vec![CredentialRef::new("vault://codex-prod").unwrap()],
        claims: Vec::new(),
        status: ConfigStatus::Active,
    }
}

fn base_physical() -> PhysicalSafety {
    PhysicalSafety {
        attempt_isolation: false,
        enforceable_workspace_modes: vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write],
        enforceable_network_modes: [
            NetworkPolicy::Disabled,
            NetworkPolicy::Restricted,
            NetworkPolicy::Enabled,
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn test_base_contract_satisfies_base_task() {
    assert!(can_execute(&base_agent(), &base_task()).is_ok());
    assert!(can_provision(
        &base_agent(),
        &base_source(),
        &base_config(),
        &base_physical(),
        true
    )
    .is_ok());
}

#[test]
fn test_can_execute_gates() {
    let mut req = base_task();
    req.information_function = InformationFunction::CompressNegative;
    assert!(matches!(
        can_execute(&base_agent(), &req),
        Err(ContractError::CapabilityMismatch { .. })
    ));

    let mut req = base_task();
    req.required_permissions = set(&["write"]);
    assert!(can_execute(&base_agent(), &req).is_err());

    let mut req = base_task();
    req.required_tools = set(&["ripgrep"]);
    assert!(can_execute(&base_agent(), &req).is_err());

    let mut req = base_task();
    req.required_workspace = WorkspaceMode::Write;
    assert!(matches!(
        can_execute(&base_agent(), &req),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    let mut req = base_task();
    req.required_network = NetworkPolicy::Enabled;
    assert!(can_execute(&base_agent(), &req).is_err());

    let mut req = base_task();
    req.budget = budget(200.0);
    assert!(can_execute(&base_agent(), &req).is_err());

    // An unconstrained agent accepts any anchor requirement ...
    let mut req = base_task();
    req.required_anchor = Some("other".into());
    assert!(can_execute(&base_agent(), &req).is_ok());
    // ... but a constrained agent must match exactly.
    let mut constrained = base_agent();
    constrained.contract.anchor_constraint = Some("region-a".into());
    req.required_anchor = Some("region-b".into());
    assert!(can_execute(&constrained, &req).is_err());
    req.required_anchor = Some("region-a".into());
    assert!(can_execute(&constrained, &req).is_ok());
}

#[test]
fn test_can_execute_requires_capability_value() {
    let reference = cref("workspace.write", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec(
            "workspace.write",
            1,
            MatcherKind::Bool,
            SecurityClass::Authority,
        ),
    );

    let mut req = base_task();
    req.required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    assert!(can_execute(&agent, &req).is_ok());

    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(false));
    assert!(can_execute(&agent, &req).is_err());

    req.required_capabilities
        .insert(cref("terminal.attach", 1), CapabilityValue::Bool(true));
    assert!(can_execute(&agent, &req).is_err());
}

#[test]
fn test_refinement_accepts_narrowing() {
    let base = base_agent();
    let mut derived = base_agent();
    derived.type_ref = type_ref("readonly-rust-reviewer", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived.contract.permission_ceiling = set(&["read"]);
    derived.contract.tools = set(&["git"]);
    derived.contract.roots = set(&["repo"]);
    derived.contract.budget_ceiling = budget(25.0);
    derived.contract.lifecycle = [LifecycleMode::Resident].into_iter().collect();

    assert!(is_valid_refinement(&base, &derived).is_ok());
}

#[test]
fn test_refinement_rejects_each_widening() {
    let base = base_agent();

    let mut d = base_agent();
    d.contract.permission_ceiling = set(&["read", "write"]);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.visibility = set(&["public", "secret"]);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.tools = set(&["git", "shell"]);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.roots = set(&["repo", "/"]);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.budget_ceiling = budget(500.0);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.lifecycle = [LifecycleMode::Resident, LifecycleMode::Revivable]
        .into_iter()
        .collect();
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.security.workspace = WorkspaceMode::Write;
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.security.network = NetworkPolicy::Enabled;
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.security.tool_roots = set(&["repo", "/etc"]);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.allowed_information_functions = vec![
        InformationFunction::Expand,
        InformationFunction::CompressNegative,
    ];
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut base_isolated = base_agent();
    base_isolated.contract.security.requires_attempt_isolation = true;
    let mut d = base_agent();
    d.contract.security.requires_attempt_isolation = false;
    assert!(is_valid_refinement(&base_isolated, &d).is_err());
}

#[test]
fn test_refinement_may_strengthen_but_not_weaken_continuity() {
    let mut base_none = base_agent();
    base_none.contract.continuity = ContinuityMode::None;
    let mut d_strengthened = base_agent();
    d_strengthened.contract.continuity = ContinuityMode::Logical;
    assert!(is_valid_refinement(&base_none, &d_strengthened).is_ok());

    let mut base_logical = base_agent();
    base_logical.contract.continuity = ContinuityMode::Logical;
    let mut d_weakened = base_agent();
    d_weakened.contract.continuity = ContinuityMode::None;
    assert!(matches!(
        is_valid_refinement(&base_logical, &d_weakened),
        Err(ContractError::InvalidRefinement { .. })
    ));
}

#[test]
fn test_refinement_anchor_must_satisfy_base() {
    let mut base = base_agent();
    base.contract.anchor_constraint = Some("region-a".into());

    let mut ok = base_agent();
    ok.contract.anchor_constraint = Some("region-a".into());
    assert!(is_valid_refinement(&base, &ok).is_ok());

    let mut bad = base_agent();
    bad.contract.anchor_constraint = Some("region-b".into());
    assert!(is_valid_refinement(&base, &bad).is_err());

    let mut missing = base_agent();
    missing.contract.anchor_constraint = None;
    assert!(is_valid_refinement(&base, &missing).is_err());
}

#[test]
fn test_can_provision_gates() {
    let agent = base_agent();
    let physical = base_physical();

    let mut inactive = base_source();
    inactive.status = SourceStatus::Draining;
    assert!(matches!(
        can_provision(&agent, &inactive, &base_config(), &physical, true),
        Err(ContractError::SourceConfigInvalid { .. })
    ));

    let mut narrow_life = base_source();
    narrow_life.lifecycle_modes = [LifecycleMode::Ephemeral].into_iter().collect();
    assert!(can_provision(&agent, &narrow_life, &base_config(), &physical, true).is_err());

    let mut narrow_cont = base_source();
    narrow_cont.continuity_modes = [ContinuityMode::None].into_iter().collect();
    assert!(can_provision(&agent, &narrow_cont, &base_config(), &physical, true).is_err());

    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &physical, false),
        Err(ContractError::AdapterBindingMissing)
    ));

    // Workspace policy must be in the enforceable set, not implied by rank.
    let mut write_agent = base_agent();
    write_agent.contract.security.workspace = WorkspaceMode::Write;
    let readonly_only = PhysicalSafety {
        attempt_isolation: false,
        enforceable_workspace_modes: vec![WorkspaceMode::ReadOnly],
        enforceable_network_modes: [NetworkPolicy::Restricted].into_iter().collect(),
    };
    assert!(matches!(
        can_provision(
            &write_agent,
            &base_source(),
            &base_config(),
            &readonly_only,
            true
        ),
        Err(ContractError::SecurityUnenforceable { .. })
    ));
}

#[test]
fn test_functional_envelope_must_cover_agent_capabilities() {
    let reference = cref("tools", 1);
    let mut agent = base_agent();
    agent.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Set(set(&["git", "ripgrep"])),
    );
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec("tools", 1, MatcherKind::Set, SecurityClass::Functional),
    );

    let mut source = base_source();
    source.functional_envelope.insert(
        reference.clone(),
        CapabilityValue::Set(set(&["git", "ripgrep", "jq"])),
    );
    assert!(can_provision(&agent, &source, &base_config(), &base_physical(), true).is_ok());

    let mut weak = base_source();
    weak.functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    assert!(can_provision(&agent, &weak, &base_config(), &base_physical(), true).is_err());
}

#[test]
fn test_security_class_requires_enforced_for_generic_capability() {
    let reference = cref("sandbox.network_lock", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec(
            "sandbox.network_lock",
            1,
            MatcherKind::Bool,
            SecurityClass::Sandbox,
        ),
    );

    // A functional envelope entry is not an ENFORCED proof.
    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Bool(true));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_physical(), true),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // A DECLARED claim is not an ENFORCED proof either.
    source.functional_envelope.clear();
    source.claims.push(claim(
        "sandbox.network_lock",
        1,
        CapabilityValue::Bool(true),
        Assurance::Declared,
    ));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_physical(), true),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // Only an ENFORCED claim at the exact revision proves it.
    source.claims[0].assurance = Assurance::Enforced;
    assert!(can_provision(&agent, &source, &base_config(), &base_physical(), true).is_ok());
}

#[test]
fn test_capability_claim_revision_must_match_spec_revision() {
    let reference = cref("tools", 2);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec("tools", 2, MatcherKind::Set, SecurityClass::Functional),
    );

    // A v1 claim must not satisfy a v2 requirement.
    let mut source = base_source();
    source
        .functional_envelope
        .insert(cref("tools", 1), CapabilityValue::Set(set(&["git", "jq"])));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_physical(), true),
        Err(ContractError::CapabilityMismatch { .. })
    ));

    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
    assert!(can_provision(&agent, &source, &base_config(), &base_physical(), true).is_ok());
}

#[test]
fn test_source_config_must_belong_to_exact_source_revision() {
    let agent = base_agent();
    let mut other_source = base_source();
    other_source.source_ref = SpawnSourceRef::new("other-source", 1).unwrap();
    assert!(matches!(
        can_provision(
            &agent,
            &other_source,
            &base_config(),
            &base_physical(),
            true
        ),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_config_specific_claims_affect_provisioning() {
    let reference = cref("sandbox.network_lock", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec(
            "sandbox.network_lock",
            1,
            MatcherKind::Bool,
            SecurityClass::Sandbox,
        ),
    );

    let source = base_source();

    // Config A enforces it; config B does not. Same source, different result.
    let mut config_a = base_config();
    config_a.claims.push(claim(
        "sandbox.network_lock",
        1,
        CapabilityValue::Bool(true),
        Assurance::Enforced,
    ));
    let config_b = base_config();

    assert!(can_provision(&agent, &source, &config_a, &base_physical(), true).is_ok());
    assert!(can_provision(&agent, &source, &config_b, &base_physical(), true).is_err());
}

#[test]
fn test_conflicting_claims_fail_closed() {
    let reference = cref("tools", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec("tools", 1, MatcherKind::Set, SecurityClass::Functional),
    );

    let mut source = base_source();
    source.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git"])),
        Assurance::Declared,
    ));
    source.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git", "jq"])),
        Assurance::Declared,
    ));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_physical(), true),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_relations_are_independent() {
    let agent = base_agent();
    assert!(can_execute(&agent, &base_task()).is_ok());
    assert!(can_provision(
        &agent,
        &base_source(),
        &base_config(),
        &base_physical(),
        false
    )
    .is_err());

    let mut req = base_task();
    req.information_function = InformationFunction::CompressPositive;
    assert!(can_execute(&agent, &req).is_err());
    assert!(can_provision(
        &agent,
        &base_source(),
        &base_config(),
        &base_physical(),
        true
    )
    .is_ok());
}

#[test]
fn test_equal_contracts_are_not_mutually_more_specific() {
    let req = base_task();
    let a = base_agent();
    let mut b = base_agent();
    b.type_ref = type_ref("identical-twin", 1);

    assert!(!more_specific_for(&a, &b, &req));
    assert!(!more_specific_for(&b, &a, &req));
}

#[test]
fn test_more_specific_rejects_wider_security_envelope() {
    let req = base_task();
    let broad = base_agent();

    // A is narrower on tools/budget but WIDER on network authority: it must not
    // be considered more specific for a task both can execute.
    let mut wider_security = base_agent();
    wider_security.type_ref = type_ref("wider-security", 1);
    wider_security.contract.tools = set(&["git"]);
    wider_security.contract.budget_ceiling = budget(50.0);
    wider_security.contract.security.network = NetworkPolicy::Enabled;

    assert!(!more_specific_for(&wider_security, &broad, &req));
}

#[test]
fn test_more_specific_requires_executability_and_is_not_inheritance_depth() {
    let req = base_task();
    let broad = base_agent();

    let mut narrow = base_agent();
    narrow.type_ref = type_ref("readonly-rust-reviewer", 1);
    narrow.based_on = Some(broad.type_ref.clone());
    narrow.contract.permission_ceiling = set(&["read"]);
    narrow.contract.budget_ceiling = budget(60.0);

    assert!(more_specific_for(&narrow, &broad, &req));
    assert!(!more_specific_for(&broad, &narrow, &req));

    // Deeper nominal inheritance does not by itself make a type more specific.
    let mut wider = base_agent();
    wider.type_ref = type_ref("deeper", 1);
    wider.based_on = Some(narrow.type_ref.clone());
    wider.contract.budget_ceiling = budget(500.0);
    assert!(!more_specific_for(&wider, &broad, &req));

    // A non-executable type is never "more specific".
    let mut detached = base_agent();
    detached.type_ref = type_ref("detached", 1);
    detached.contract.allowed_information_functions = vec![InformationFunction::CompressNegative];
    assert!(!more_specific_for(&detached, &broad, &req));
}

#[test]
fn test_physical_enabled_network_does_not_prove_restricted_enforcement() {
    let mut agent = base_agent();
    agent.contract.security.network = NetworkPolicy::Restricted;

    // The environment can only do "all or nothing": Enabled is present but
    // Restricted is not. Enabled must NOT be read as proving Restricted.
    let physical = PhysicalSafety {
        attempt_isolation: false,
        enforceable_workspace_modes: vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write],
        enforceable_network_modes: [NetworkPolicy::Disabled, NetworkPolicy::Enabled]
            .into_iter()
            .collect(),
    };
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &physical, true),
        Err(ContractError::SecurityUnenforceable { .. })
    ));
}

#[test]
fn test_filter_precedes_ranking() {
    let agent = base_agent();
    let physical = base_physical();

    let mut inactive = base_source();
    inactive.status = SourceStatus::Disabled;
    let eligible = base_source();

    let candidates = [inactive, eligible];
    let feasible: Vec<&SpawnSource> = candidates
        .iter()
        .filter(|s| can_provision(&agent, s, &base_config(), &physical, true).is_ok())
        .collect();
    assert_eq!(feasible.len(), 1);
}

#[test]
fn test_matcher_semantics() {
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(false),
        &CapabilityValue::Bool(true)
    ));
    assert!(!value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(true),
        &CapabilityValue::Bool(false)
    ));

    assert!(value_satisfies(
        MatcherKind::Set,
        &CapabilityValue::Set(set(&["git"])),
        &CapabilityValue::Set(set(&["git", "jq"]))
    ));
    assert!(!value_satisfies(
        MatcherKind::Set,
        &CapabilityValue::Set(set(&["git", "ripgrep"])),
        &CapabilityValue::Set(set(&["git"]))
    ));

    let ordered = |rank| CapabilityValue::Ordered {
        class: "continuity".into(),
        rank,
    };
    assert!(value_satisfies(
        MatcherKind::Ordered,
        &ordered(1),
        &ordered(2)
    ));
    assert!(!value_satisfies(
        MatcherKind::Ordered,
        &ordered(2),
        &ordered(1)
    ));

    assert!(value_satisfies(
        MatcherKind::Quantity,
        &CapabilityValue::Quantity(quantity(4096.0)),
        &CapabilityValue::Quantity(quantity(8192.0))
    ));
    assert!(!value_satisfies(
        MatcherKind::Quantity,
        &CapabilityValue::Quantity(quantity(4096.0)),
        &CapabilityValue::Quantity(quantity(2048.0))
    ));

    assert!(value_satisfies(
        MatcherKind::Exact,
        &CapabilityValue::Exact(serde_json::json!("read_only")),
        &CapabilityValue::Exact(serde_json::json!("read_only"))
    ));
    assert!(!value_satisfies(
        MatcherKind::Exact,
        &CapabilityValue::Exact(serde_json::json!("read_only")),
        &CapabilityValue::Exact(serde_json::json!("write"))
    ));

    assert!(!value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(true),
        &CapabilityValue::Quantity(quantity(1.0))
    ));
}

#[test]
fn test_nan_budget_is_rejected() {
    assert!(Budget::new(f64::NAN).is_err());
    assert!(Budget::new(f64::INFINITY).is_err());
    assert!(Budget::new(-1.0).is_err());
    assert!(Budget::new(0.0).is_ok());
    assert!(Budget::new(1.0).is_ok());
}

#[test]
fn test_nan_quantity_is_rejected() {
    assert!(Quantity::new(f64::NAN).is_err());
    assert!(Quantity::new(f64::INFINITY).is_err());
    assert!(Quantity::new(-1.0).is_err());
    assert!(Quantity::new(0.0).is_ok());
}

#[test]
fn test_information_functions_are_canonicalized() {
    let mut contract = base_contract();
    contract.allowed_information_functions = vec![
        InformationFunction::CompressPositive,
        InformationFunction::Expand,
        InformationFunction::Expand,
    ];
    contract.normalize();
    // Canonical order is by the persisted SQL token: COMPRESS_POSITIVE < EXPAND.
    assert_eq!(
        contract.allowed_information_functions,
        vec![
            InformationFunction::CompressPositive,
            InformationFunction::Expand
        ]
    );
}

#[test]
fn test_selector_resolution() {
    let mut catalog = PublishedCatalog::new();
    catalog.publish(type_ref("auditor", 2));
    catalog.publish(type_ref("auditor", 3));

    let exact = AgentTypeSelector::Exact(type_ref("auditor", 2));
    assert_eq!(
        resolve_selector(&exact, &catalog).unwrap(),
        type_ref("auditor", 2)
    );

    let latest = AgentTypeSelector::Latest(AgentTypeId::from_string("auditor"));
    assert_eq!(
        resolve_selector(&latest, &catalog).unwrap(),
        type_ref("auditor", 3)
    );

    let missing = AgentTypeSelector::Exact(type_ref("auditor", 9));
    assert!(matches!(
        resolve_selector(&missing, &catalog),
        Err(ContractError::AgentTypeNotFound { .. })
    ));

    let unknown = AgentTypeSelector::Latest(AgentTypeId::from_string("ghost"));
    assert!(resolve_selector(&unknown, &catalog).is_err());
}

#[test]
fn test_identity_validation() {
    assert!(AgentTypeRef::new("", 1).is_err());
    assert!(AgentTypeRef::new("x", 0).is_err());
    assert!(AgentTypeRef::new("x", 1).is_ok());
    assert!(CapabilityRef::new("", 1).is_err());
    assert!(CapabilityRef::new("x", 0).is_err());
    assert!(CredentialRef::new("").is_err());
    assert!(CredentialRef::new("vault://x").is_ok());
}

#[test]
fn test_deprecation_is_future_selection_only() {
    let mut catalog = PublishedCatalog::new();
    let r2 = type_ref("auditor", 2);
    catalog.publish(r2.clone());
    catalog.publish(type_ref("auditor", 3));

    catalog.deprecate(&r2);
    // Deprecated: no longer selectable, and not the latest.
    assert!(!catalog.is_published(&r2));
    assert!(catalog.is_deprecated(&r2));
    let latest = AgentTypeSelector::Latest(AgentTypeId::from_string("auditor"));
    assert_eq!(
        resolve_selector(&latest, &catalog).unwrap(),
        type_ref("auditor", 3)
    );
}
