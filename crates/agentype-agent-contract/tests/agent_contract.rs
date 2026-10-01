//! M6-B.1 conformance: AgentType purity, four independent relations,
//! refinement monotonicity, matcher semantics, and DECLARED != ENFORCED.

use agentype_agent_contract::*;
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn cap(id: &str) -> CapabilityId {
    CapabilityId::from_string(id)
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
        budget_ceiling: 100.0,
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
        type_ref: AgentTypeRef::new("general-reviewer", 1),
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
        budget: 50.0,
    }
}

fn base_source() -> SpawnSource {
    SpawnSource {
        source_ref: SpawnSourceRef {
            source_id: SpawnSourceId::from_string("codex-local"),
            revision: 2,
        },
        adapter_policy: AdapterPolicyRef {
            policy_id: AdapterPolicyId::from_string("codex-local-adapter"),
            revision: 3,
        },
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
    let source = base_source().source_ref;
    SourceConfig {
        config_ref: SourceConfigRef {
            source,
            config_id: SourceConfigId::from_string("deep-reasoning"),
            revision: 7,
        },
        config_digest: "sha256:abc".into(),
        credential_refs: vec!["vault://codex-prod".into()],
        status: ConfigStatus::Active,
    }
}

fn base_physical() -> PhysicalSafety {
    PhysicalSafety {
        attempt_isolation: false,
        workspace: WorkspaceMode::Write,
        network: NetworkPolicy::Enabled,
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
    req.required_continuity = ContinuityMode::Logical;
    assert!(can_execute(&base_agent(), &req).is_ok());
    req.required_continuity = ContinuityMode::None;
    assert!(can_execute(&base_agent(), &req).is_ok());

    let mut req = base_task();
    req.budget = 200.0;
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
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(cap("workspace.write"), CapabilityValue::Bool(true));
    agent.contract.capability_specs.insert(
        cap("workspace.write"),
        CapabilitySpec {
            capability_id: cap("workspace.write"),
            revision: 1,
            matcher_kind: MatcherKind::Bool,
            security_class: SecurityClass::Authority,
        },
    );

    let mut req = base_task();
    req.required_capabilities
        .insert(cap("workspace.write"), CapabilityValue::Bool(true));
    assert!(can_execute(&agent, &req).is_ok());

    agent
        .contract
        .required_capabilities
        .insert(cap("workspace.write"), CapabilityValue::Bool(false));
    assert!(can_execute(&agent, &req).is_err());

    req.required_capabilities
        .insert(cap("terminal.attach"), CapabilityValue::Bool(true));
    assert!(can_execute(&agent, &req).is_err());
}

#[test]
fn test_refinement_accepts_narrowing() {
    let base = base_agent();
    let mut derived = base_agent();
    derived.type_ref = AgentTypeRef::new("readonly-rust-reviewer", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived.contract.permission_ceiling = set(&["read"]);
    derived.contract.tools = set(&["git"]);
    derived.contract.roots = set(&["repo"]);
    derived.contract.budget_ceiling = 25.0;
    derived.contract.lifecycle = [LifecycleMode::Resident].into_iter().collect();

    assert!(is_valid_refinement(&base, &derived).is_ok());
}

#[test]
fn test_refinement_rejects_each_widening() {
    let base = base_agent();

    let mut d = base_agent();
    d.contract.permission_ceiling = set(&["read", "write"]);
    assert!(matches!(
        is_valid_refinement(&base, &d),
        Err(ContractError::InvalidRefinement { .. })
    ));

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
    d.contract.budget_ceiling = 500.0;
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.lifecycle = [LifecycleMode::Resident, LifecycleMode::Revivable]
        .into_iter()
        .collect();
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.continuity = ContinuityMode::Logical;
    let mut base_lower = base_agent();
    base_lower.contract.continuity = ContinuityMode::None;
    assert!(is_valid_refinement(&base_lower, &d).is_err());

    let mut d = base_agent();
    d.contract.security.workspace = WorkspaceMode::Write;
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.security.network = NetworkPolicy::Enabled;
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut d = base_agent();
    d.contract.security.tool_roots = set(&["repo", "/etc"]);
    assert!(is_valid_refinement(&base, &d).is_err());

    let mut base_isolated = base_agent();
    base_isolated.contract.security.requires_attempt_isolation = true;
    let mut d = base_agent();
    d.contract.security.requires_attempt_isolation = false;
    assert!(is_valid_refinement(&base_isolated, &d).is_err());
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

    // Inactive source.
    let mut inactive = base_source();
    inactive.status = SourceStatus::Draining;
    assert!(matches!(
        can_provision(&agent, &inactive, &base_config(), &physical, true),
        Err(ContractError::SourceConfigInvalid { .. })
    ));

    // Lifecycle not covered.
    let mut narrow_life = base_source();
    narrow_life.lifecycle_modes = [LifecycleMode::Ephemeral].into_iter().collect();
    assert!(can_provision(&agent, &narrow_life, &base_config(), &physical, true).is_err());

    // Continuity not covered.
    let mut narrow_cont = base_source();
    narrow_cont.continuity_modes = [ContinuityMode::None].into_iter().collect();
    assert!(can_provision(&agent, &narrow_cont, &base_config(), &physical, true).is_err());

    // Exact binding unavailable.
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &physical, false),
        Err(ContractError::AdapterBindingMissing)
    ));

    // Physical safety too weak for the agent workspace policy.
    let mut write_agent = base_agent();
    write_agent.contract.security.workspace = WorkspaceMode::Write;
    let mut readonly_physical = physical;
    readonly_physical.workspace = WorkspaceMode::ReadOnly;
    assert!(matches!(
        can_provision(
            &write_agent,
            &base_source(),
            &base_config(),
            &readonly_physical,
            true
        ),
        Err(ContractError::SecurityUnenforceable { .. })
    ));
}

#[test]
fn test_functional_envelope_must_cover_agent_capabilities() {
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(cap("tools"), CapabilityValue::Set(set(&["git", "ripgrep"])));
    agent.contract.capability_specs.insert(
        cap("tools"),
        CapabilitySpec {
            capability_id: cap("tools"),
            revision: 1,
            matcher_kind: MatcherKind::Set,
            security_class: SecurityClass::Functional,
        },
    );

    let mut source = base_source();
    source.functional_envelope.insert(
        cap("tools"),
        CapabilityValue::Set(set(&["git", "ripgrep", "jq"])),
    );
    assert!(can_provision(&agent, &source, &base_config(), &base_physical(), true).is_ok());

    let mut weak = base_source();
    weak.functional_envelope
        .insert(cap("tools"), CapabilityValue::Set(set(&["git"])));
    assert!(can_provision(&agent, &weak, &base_config(), &base_physical(), true).is_err());
}

#[test]
fn test_declared_claim_cannot_authorize_sandbox() {
    let mut agent = base_agent();
    agent.contract.security.requires_attempt_isolation = true;

    let mut source = base_source();
    source.claims.push(CapabilityClaim {
        capability_id: cap("attempt_isolation"),
        capability_revision: 1,
        value: CapabilityValue::Bool(true),
        assurance: Assurance::Declared,
        evidence_ref: None,
    });

    let mut physical = base_physical();
    physical.attempt_isolation = true;
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &physical, true),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // Upgrade the claim to ENFORCED; still needs the physical fact.
    source.claims[0].assurance = Assurance::Enforced;
    assert!(can_provision(&agent, &source, &base_config(), &physical, true).is_ok());

    physical.attempt_isolation = false;
    assert!(can_provision(&agent, &source, &base_config(), &physical, true).is_err());
}

#[test]
fn test_relations_are_independent() {
    // can_execute true but can_provision false (no exact binding) ...
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

    // ... and can_provision true while can_execute false for a different Task.
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
fn test_more_specific_requires_executability_and_is_not_inheritance_depth() {
    let req = base_task();
    let broad = base_agent();

    let mut narrow = base_agent();
    narrow.type_ref = AgentTypeRef::new("readonly-rust-reviewer", 1);
    narrow.based_on = Some(broad.type_ref.clone());
    narrow.contract.tools = set(&["git"]);
    narrow.contract.permission_ceiling = set(&["read"]);
    narrow.contract.budget_ceiling = 60.0;

    assert!(more_specific_for(&narrow, &broad, &req));
    assert!(!more_specific_for(&broad, &narrow, &req));

    // Deep nominal inheritance does not by itself make a type more specific:
    // here the "deeper" type has a wider budget ceiling.
    let mut wider = base_agent();
    wider.type_ref = AgentTypeRef::new("deeper", 1);
    wider.based_on = Some(narrow.type_ref.clone());
    wider.contract.budget_ceiling = 500.0;
    assert!(!more_specific_for(&wider, &broad, &req));

    // A non-executable type is never "more specific".
    let mut detached = base_agent();
    detached.type_ref = AgentTypeRef::new("detached", 1);
    detached.contract.allowed_information_functions = vec![InformationFunction::CompressNegative];
    assert!(!more_specific_for(&detached, &broad, &req));
}

#[test]
fn test_filter_precedes_ranking() {
    let agent = base_agent();
    let physical = base_physical();

    // Infeasible candidates (cheap or not) are excluded before any ranking.
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
        &CapabilityValue::Quantity(4096.0),
        &CapabilityValue::Quantity(8192.0)
    ));
    assert!(!value_satisfies(
        MatcherKind::Quantity,
        &CapabilityValue::Quantity(4096.0),
        &CapabilityValue::Quantity(2048.0)
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

    // Mismatched shapes never satisfy.
    assert!(!value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(true),
        &CapabilityValue::Quantity(1.0)
    ));
}

#[test]
fn test_selector_resolution() {
    let mut catalog = PublishedCatalog::new();
    catalog.publish(AgentTypeRef::new("auditor", 2));
    catalog.publish(AgentTypeRef::new("auditor", 3));

    let exact = AgentTypeSelector::Exact(AgentTypeRef::new("auditor", 2));
    assert_eq!(
        resolve_selector(&exact, &catalog).unwrap(),
        AgentTypeRef::new("auditor", 2)
    );

    let latest = AgentTypeSelector::Latest(AgentTypeId::from_string("auditor"));
    assert_eq!(
        resolve_selector(&latest, &catalog).unwrap(),
        AgentTypeRef::new("auditor", 3)
    );

    let missing = AgentTypeSelector::Exact(AgentTypeRef::new("auditor", 9));
    assert!(matches!(
        resolve_selector(&missing, &catalog),
        Err(ContractError::AgentTypeNotFound { .. })
    ));

    let unknown = AgentTypeSelector::Latest(AgentTypeId::from_string("ghost"));
    assert!(resolve_selector(&unknown, &catalog).is_err());
}
