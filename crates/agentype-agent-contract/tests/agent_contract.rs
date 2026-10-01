//! M6-B.1 conformance: AgentType purity, four independent relations,
//! refinement monotonicity, the shared capability-constraint order, exact
//! capability-revision proof paths, imported (never asserted) enforcement
//! evidence, affinity, and validated construction.

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

fn policy_ref(id: &str, revision: u64) -> AdapterPolicyRef {
    AdapterPolicyRef::new(id, revision).unwrap()
}

fn budget(value: f64) -> Budget {
    Budget::new(value).unwrap()
}

fn quantity(value: f64) -> Quantity {
    Quantity::new(value).unwrap()
}

fn spec(matcher: MatcherKind, class: SecurityClass) -> CapabilitySpec {
    CapabilitySpec {
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

/// Trusted-integration stand-in: mirrors how M6-B.4 wires an imported adapter.
struct TestEvidence {
    policy: AdapterPolicyRef,
    kind: String,
    key: AdapterBindingKey,
    safety: PhysicalSafety,
    enforced: Vec<(CapabilityRef, CapabilityValue)>,
}

impl ProvisioningEvidenceSource for TestEvidence {
    fn adapter_policy(&self) -> AdapterPolicyRef {
        self.policy.clone()
    }
    fn adapter_kind(&self) -> String {
        self.kind.clone()
    }
    fn adapter_binding_key(&self) -> AdapterBindingKey {
        self.key.clone()
    }
    fn enforceable_safety(&self) -> PhysicalSafety {
        self.safety.clone()
    }
    fn enforced_capabilities(&self) -> Vec<(CapabilityRef, CapabilityValue)> {
        self.enforced.clone()
    }
}

fn base_physical() -> PhysicalSafety {
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
    .unwrap()
}

fn resolve(evidence: TestEvidence) -> ResolvedProvisioningEvidence {
    ResolvedProvisioningEvidence::from_source(&evidence).unwrap()
}

fn base_evidence() -> ResolvedProvisioningEvidence {
    resolve(TestEvidence {
        policy: policy_ref("codex-local-adapter", 3),
        kind: "codex_cli".into(),
        key: AdapterBindingKey::new("linux:boot:pidns").unwrap(),
        safety: base_physical(),
        enforced: Vec::new(),
    })
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
        affinity: BTreeSet::new(),
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
    }
}

fn base_task() -> TaskRequirement {
    TaskRequirement {
        information_function: InformationFunction::Expand,
        required_capabilities: BTreeMap::new(),
        required_permissions: set(&["read"]),
        required_tools: set(&["git"]),
        required_affinity: BTreeSet::new(),
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
        adapter_policy: policy_ref("codex-local-adapter", 3),
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

#[test]
fn test_base_contract_satisfies_base_task() {
    assert!(can_execute(&base_agent(), &base_task()).is_ok());
    assert!(can_provision(
        &base_agent(),
        &base_source(),
        &base_config(),
        &base_evidence()
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
    req.required_affinity = set(&["sqlite"]);
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

    let mut req = base_task();
    req.required_anchor = Some("other".into());
    assert!(can_execute(&base_agent(), &req).is_ok());
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
        spec(MatcherKind::Bool, SecurityClass::Authority),
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
    derived.contract.budget_ceiling = budget(25.0);
    derived.contract.affinity = set(&["rust"]);

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
    let evidence = base_evidence();

    let mut inactive = base_source();
    inactive.status = SourceStatus::Draining;
    assert!(matches!(
        can_provision(&agent, &inactive, &base_config(), &evidence),
        Err(ContractError::SourceConfigInvalid { .. })
    ));

    let mut narrow_life = base_source();
    narrow_life.lifecycle_modes = [LifecycleMode::Ephemeral].into_iter().collect();
    assert!(can_provision(&agent, &narrow_life, &base_config(), &evidence).is_err());

    let mut narrow_cont = base_source();
    narrow_cont.continuity_modes = [ContinuityMode::None].into_iter().collect();
    assert!(can_provision(&agent, &narrow_cont, &base_config(), &evidence).is_err());
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
        spec(MatcherKind::Set, SecurityClass::Functional),
    );

    let mut source = base_source();
    source.functional_envelope.insert(
        reference.clone(),
        CapabilityValue::Set(set(&["git", "ripgrep", "jq"])),
    );
    assert!(can_provision(&agent, &source, &base_config(), &base_evidence()).is_ok());

    let mut weak = base_source();
    weak.functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    assert!(can_provision(&agent, &weak, &base_config(), &base_evidence()).is_err());
}

#[test]
fn test_security_class_requires_imported_evidence() {
    let reference = cref("sandbox.network_lock", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Bool, SecurityClass::Sandbox),
    );

    // A functional envelope is not evidence.
    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Bool(true));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence()),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // Neither is a DECLARED/ENFORCED claim label.
    source.functional_envelope.clear();
    source.claims.push(claim(
        "sandbox.network_lock",
        1,
        CapabilityValue::Bool(true),
        Assurance::Enforced,
    ));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence()),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // Only imported enforcement evidence proves it.
    let evidence = resolve(TestEvidence {
        policy: policy_ref("codex-local-adapter", 3),
        kind: "codex_cli".into(),
        key: AdapterBindingKey::new("linux:boot:pidns").unwrap(),
        safety: base_physical(),
        enforced: vec![(reference.clone(), CapabilityValue::Bool(true))],
    });
    assert!(can_provision(&agent, &source, &base_config(), &evidence).is_ok());
}

#[test]
fn test_enforced_requires_imported_evidence_not_claim_label() {
    let reference = cref("authority.deploy", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Bool, SecurityClass::Authority),
    );

    let mut source = base_source();
    source.claims.push(claim(
        "authority.deploy",
        1,
        CapabilityValue::Bool(true),
        Assurance::Enforced,
    ));

    let no_evidence = base_evidence();
    assert!(can_provision(&agent, &source, &base_config(), &no_evidence).is_err());

    let evidence = resolve(TestEvidence {
        policy: policy_ref("codex-local-adapter", 3),
        kind: "codex_cli".into(),
        key: AdapterBindingKey::new("linux:boot:pidns").unwrap(),
        safety: base_physical(),
        enforced: vec![(reference.clone(), CapabilityValue::Bool(true))],
    });
    assert!(can_provision(&agent, &source, &base_config(), &evidence).is_ok());
}

#[test]
fn test_evidence_must_match_source_adapter_policy() {
    let agent = base_agent();
    let evidence = resolve(TestEvidence {
        policy: policy_ref("other-adapter", 9),
        kind: "other".into(),
        key: AdapterBindingKey::new("other-domain").unwrap(),
        safety: base_physical(),
        enforced: Vec::new(),
    });
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &evidence),
        Err(ContractError::EvidencePolicyMismatch { .. })
    ));
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
        spec(MatcherKind::Set, SecurityClass::Functional),
    );

    let mut source = base_source();
    source
        .functional_envelope
        .insert(cref("tools", 1), CapabilityValue::Set(set(&["git", "jq"])));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence()),
        Err(ContractError::CapabilityMismatch { .. })
    ));

    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
    assert!(can_provision(&agent, &source, &base_config(), &base_evidence()).is_ok());
}

#[test]
fn test_source_config_must_belong_to_exact_source_revision() {
    let agent = base_agent();
    let mut other_source = base_source();
    other_source.source_ref = SpawnSourceRef::new("other-source", 1).unwrap();
    assert!(matches!(
        can_provision(&agent, &other_source, &base_config(), &base_evidence()),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_config_specific_claims_affect_provisioning() {
    let reference = cref("tools", 1);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
    agent.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Set, SecurityClass::Functional),
    );

    let source = base_source();
    let mut config_a = base_config();
    config_a.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git", "jq"])),
        Assurance::Declared,
    ));
    let config_b = base_config();

    assert!(can_provision(&agent, &source, &config_a, &base_evidence()).is_ok());
    assert!(can_provision(&agent, &source, &config_b, &base_evidence()).is_err());
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
        spec(MatcherKind::Set, SecurityClass::Functional),
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
        can_provision(&agent, &source, &base_config(), &base_evidence()),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_relations_are_independent() {
    let agent = base_agent();
    assert!(can_execute(&agent, &base_task()).is_ok());

    let mut req = base_task();
    req.information_function = InformationFunction::CompressPositive;
    assert!(can_execute(&agent, &req).is_err());
    assert!(can_provision(&agent, &base_source(), &base_config(), &base_evidence()).is_ok());
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

    let mut wider_security = base_agent();
    wider_security.type_ref = type_ref("wider-security", 1);
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
    narrow.contract.budget_ceiling = budget(60.0);

    assert!(more_specific_for(&narrow, &broad, &req));
    assert!(!more_specific_for(&broad, &narrow, &req));

    let mut wider = base_agent();
    wider.type_ref = type_ref("deeper", 1);
    wider.based_on = Some(narrow.type_ref.clone());
    wider.contract.budget_ceiling = budget(500.0);
    assert!(!more_specific_for(&wider, &broad, &req));

    let mut detached = base_agent();
    detached.type_ref = type_ref("detached", 1);
    detached.contract.allowed_information_functions = vec![InformationFunction::CompressNegative];
    assert!(!more_specific_for(&detached, &broad, &req));
}

#[test]
fn test_physical_enabled_network_does_not_prove_restricted_enforcement() {
    let mut agent = base_agent();
    agent.contract.security.network = NetworkPolicy::Restricted;

    let safety = PhysicalSafety::new(
        false,
        vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write],
        [NetworkPolicy::Disabled, NetworkPolicy::Enabled]
            .into_iter()
            .collect(),
    )
    .unwrap();
    let evidence = resolve(TestEvidence {
        policy: policy_ref("codex-local-adapter", 3),
        kind: "codex_cli".into(),
        key: AdapterBindingKey::new("linux:boot:pidns").unwrap(),
        safety,
        enforced: Vec::new(),
    });
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &evidence),
        Err(ContractError::SecurityUnenforceable { .. })
    ));
}

#[test]
fn test_filter_precedes_ranking() {
    let agent = base_agent();
    let evidence = base_evidence();

    let mut inactive = base_source();
    inactive.status = SourceStatus::Disabled;
    let eligible = base_source();

    let candidates = [inactive, eligible];
    let feasible: Vec<&SpawnSource> = candidates
        .iter()
        .filter(|s| can_provision(&agent, s, &base_config(), &evidence).is_ok())
        .collect();
    assert_eq!(feasible.len(), 1);
}

#[test]
fn test_matcher_semantics() {
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(true),
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
}

#[test]
fn test_bool_matcher_is_exact_equality() {
    // The old implication (required=false, provided=true ⇒ true) is gone.
    assert!(!value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(false),
        &CapabilityValue::Bool(true)
    ));
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(false),
        &CapabilityValue::Bool(false)
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
    assert_eq!(
        contract.allowed_information_functions,
        vec![
            InformationFunction::CompressPositive,
            InformationFunction::Expand
        ]
    );
}

#[test]
fn test_refinement_rejects_capability_removal() {
    let base = {
        let mut b = base_agent();
        let reference = cref("sandbox.network_lock", 1);
        b.contract
            .required_capabilities
            .insert(reference.clone(), CapabilityValue::Bool(true));
        b.contract
            .capability_specs
            .insert(reference, spec(MatcherKind::Bool, SecurityClass::Sandbox));
        b
    };

    let derived = base_agent();
    assert!(matches!(
        is_valid_refinement(&base, &derived),
        Err(ContractError::InvalidRefinement { .. })
    ));
}

#[test]
fn test_refinement_rejects_security_class_downgrade() {
    let reference = cref("sandbox.network_lock", 1);
    let mut base = base_agent();
    base.contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    base.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Bool, SecurityClass::Sandbox),
    );

    let mut derived = base_agent();
    derived
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    derived.contract.capability_specs.insert(
        reference,
        spec(MatcherKind::Bool, SecurityClass::Functional),
    );

    assert!(is_valid_refinement(&base, &derived).is_err());
}

#[test]
fn test_refinement_rejects_matcher_downgrade() {
    let reference = cref("tools", 1);
    let mut base = base_agent();
    base.contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
    base.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Set, SecurityClass::Functional),
    );

    let mut derived = base_agent();
    derived
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
    derived.contract.capability_specs.insert(
        reference,
        spec(MatcherKind::Bool, SecurityClass::Functional),
    );

    assert!(is_valid_refinement(&base, &derived).is_err());
}

#[test]
fn test_more_specific_does_not_treat_weaker_capability_as_narrower() {
    let reference = cref("memory_mb", 1);
    let mut broad = base_agent();
    broad.type_ref = type_ref("broad", 1);
    broad.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(8192.0)),
    );
    broad.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Quantity, SecurityClass::Functional),
    );

    // A is narrower on budget but WEAKER on the memory capability.
    let mut weaker = base_agent();
    weaker.type_ref = type_ref("weaker", 1);
    weaker.contract.budget_ceiling = budget(10.0);
    weaker.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(1024.0)),
    );
    weaker.contract.capability_specs.insert(
        reference.clone(),
        spec(MatcherKind::Quantity, SecurityClass::Functional),
    );

    let mut req = base_task();
    req.budget = budget(5.0);
    req.required_capabilities
        .insert(reference, CapabilityValue::Quantity(quantity(1024.0)));

    assert!(can_execute(&weaker, &req).is_ok());
    assert!(can_execute(&broad, &req).is_ok());
    assert!(!more_specific_for(&weaker, &broad, &req));
}

#[test]
fn test_affinity_participates_in_specificity() {
    let req = base_task();
    let broad = base_agent();

    let mut narrow = base_agent();
    narrow.type_ref = type_ref("sqlite-reviewer", 1);
    narrow.contract.affinity = set(&["sqlite"]);

    assert!(more_specific_for(&narrow, &broad, &req));
    assert!(!more_specific_for(&broad, &narrow, &req));
}

#[test]
fn test_physical_safety_requires_validated_construction() {
    // The constructor canonicalizes the enforceable workspace set.
    let safety = PhysicalSafety::new(
        false,
        vec![
            WorkspaceMode::Write,
            WorkspaceMode::ReadOnly,
            WorkspaceMode::ReadOnly,
        ],
        [NetworkPolicy::Restricted].into_iter().collect(),
    )
    .unwrap();
    assert!(safety.enforces_workspace(WorkspaceMode::ReadOnly));
    assert!(safety.enforces_workspace(WorkspaceMode::Write));
    assert!(safety.enforces_network(NetworkPolicy::Restricted));
    assert!(!safety.enforces_network(NetworkPolicy::Enabled));
}

#[test]
fn test_invalid_ref_cannot_be_constructed_or_published() {
    assert!(AgentTypeRef::new("", 1).is_err());
    assert!(AgentTypeRef::new("x", 0).is_err());
    assert!(SpawnSourceRef::new("", 1).is_err());
    assert!(SourceConfigRef::new(SpawnSourceRef::new("s", 1).unwrap(), "", 1).is_err());
    assert!(AdapterPolicyRef::new("p", 0).is_err());
    assert!(CapabilityRef::new("", 1).is_err());
    assert!(CapabilityRef::new("x", 0).is_err());
    assert!(CredentialRef::new("").is_err());
    assert!(AdapterBindingKey::new("   ").is_err());
    assert!(AdapterBindingKey::new("domain").is_ok());
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
fn test_deprecation_is_monotonic_and_future_selection_only() {
    let mut catalog = PublishedCatalog::new();
    let r2 = type_ref("auditor", 2);
    catalog.publish(r2.clone());
    catalog.publish(type_ref("auditor", 3));

    assert!(catalog.deprecate(&r2));
    assert!(!catalog.is_published(&r2));
    assert!(catalog.is_deprecated(&r2));

    // A deprecated revision cannot be resurrected; publish a new revision.
    assert!(!catalog.publish(r2.clone()));

    let latest = AgentTypeSelector::Latest(AgentTypeId::from_string("auditor"));
    assert_eq!(
        resolve_selector(&latest, &catalog).unwrap(),
        type_ref("auditor", 3)
    );
}
