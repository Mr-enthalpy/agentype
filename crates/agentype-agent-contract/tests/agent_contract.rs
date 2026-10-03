#![cfg(feature = "test-support")]
//! M6-B.1 conformance: AgentType purity, four independent relations,
//! refinement monotonicity, the shared capability-constraint order, catalog-
//! owned capability definitions, the source-envelope ceiling, imported (never
//! asserted) enforcement evidence, affinity, sandbox policy, and validated
//! construction.

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

fn sandbox_ref(id: &str, revision: u64) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, revision).unwrap()
}

fn budget(value: f64) -> Budget {
    Budget::new(value).unwrap()
}

fn quantity(value: f64) -> Quantity {
    Quantity::new(value).unwrap()
}

/// Convenience catalog definition using the conventional polarity for a
/// security class. Polarity is an independent field in the real API; tests that
/// exercise non-default pairings call `CapabilityCatalog::define` directly.
fn define(
    cat: &mut CapabilityCatalog,
    id: &str,
    revision: u64,
    matcher: MatcherKind,
    class: SecurityClass,
) {
    let polarity = match class {
        SecurityClass::Sandbox | SecurityClass::Continuity => CapabilityPolarity::Restriction,
        SecurityClass::Functional | SecurityClass::Authority => CapabilityPolarity::Ability,
    };
    cat.define(cref(id, revision), matcher, class, polarity)
        .unwrap();
}

fn claim(id: &str, revision: u64, value: CapabilityValue, assurance: Assurance) -> CapabilityClaim {
    CapabilityClaim {
        reference: cref(id, revision),
        value,
        assurance,
        declaration_provenance_ref: None,
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

/// Evidence bound to the exact `base_source()` / `base_config()` candidate.
fn evidence_for(
    policy: AdapterPolicyRef,
    safety: PhysicalSafety,
    sandbox: Vec<SandboxPolicyRef>,
    enforced: Vec<(CapabilityRef, CapabilityValue)>,
) -> ResolvedProvisioningEvidence {
    ResolvedProvisioningEvidence::for_tests(
        policy,
        "codex_cli",
        base_source().source_ref,
        base_config().config_ref,
        base_config().config_digest,
        safety,
        sandbox,
        enforced,
    )
    .unwrap()
}

fn base_evidence() -> ResolvedProvisioningEvidence {
    evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        Vec::new(),
    )
}

fn base_contract() -> AgentTypeContract {
    AgentTypeContract {
        allowed_information_functions: vec![InformationFunction::Expand],
        required_capabilities: BTreeMap::new(),
        affinity: AffinityConstraint::Any,
        budget_ceiling: budget(100.0),
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
        config_digest: ConfigDigest::new("sha256:abc").unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: vec![CredentialRef::new("vault://codex-prod").unwrap()],
        claims: Vec::new(),
        status: ConfigStatus::Active,
    }
}

#[test]
fn test_base_contract_satisfies_base_task() {
    let cat = CapabilityCatalog::new();
    assert!(can_execute(&base_agent(), &base_task(), &cat).is_ok());
    assert!(can_provision(
        &base_agent(),
        &base_source(),
        &base_config(),
        &base_evidence(),
        &cat
    )
    .is_ok());
}

#[test]
fn test_can_execute_gates() {
    let cat = CapabilityCatalog::new();
    let mut req = base_task();
    req.information_function = InformationFunction::CompressNegative;
    assert!(can_execute(&base_agent(), &req, &cat).is_err());

    let mut req = base_task();
    req.required_affinity = set(&["sqlite"]);
    // The general agent (Any) accepts it; a specialized type does not.
    let mut specialized = base_agent();
    specialized.contract.affinity = AffinityConstraint::Only(set(&["rust"]));
    assert!(can_execute(&base_agent(), &req, &cat).is_ok());
    assert!(can_execute(&specialized, &req, &cat).is_err());

    let mut req = base_task();
    req.required_workspace = WorkspaceMode::Write;
    assert!(matches!(
        can_execute(&base_agent(), &req, &cat),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    let mut req = base_task();
    req.required_network = NetworkPolicy::Enabled;
    assert!(can_execute(&base_agent(), &req, &cat).is_err());

    let mut req = base_task();
    req.budget = budget(200.0);
    assert!(can_execute(&base_agent(), &req, &cat).is_err());

    let mut req = base_task();
    req.required_anchor = Some("other".into());
    assert!(can_execute(&base_agent(), &req, &cat).is_ok());
    let mut constrained = base_agent();
    constrained.contract.anchor_constraint = Some("region-a".into());
    req.required_anchor = Some("region-b".into());
    assert!(can_execute(&constrained, &req, &cat).is_err());
    req.required_anchor = Some("region-a".into());
    assert!(can_execute(&constrained, &req, &cat).is_ok());
}

#[test]
fn test_can_execute_requires_capability_value() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "workspace.write",
        1,
        MatcherKind::Bool,
        SecurityClass::Authority,
    );
    let reference = cref("workspace.write", 1);

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));

    let mut req = base_task();
    req.required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    assert!(can_execute(&agent, &req, &cat).is_ok());

    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(false));
    assert!(can_execute(&agent, &req, &cat).is_err());

    req.required_capabilities
        .insert(cref("terminal.attach", 1), CapabilityValue::Bool(true));
    assert!(can_execute(&agent, &req, &cat).is_err());
}

#[test]
fn test_refinement_accepts_narrowing() {
    let cat = CapabilityCatalog::new();
    let mut base = base_agent();
    base.contract.affinity = AffinityConstraint::Only(set(&["rust", "sqlite"]));
    let mut derived = base_agent();
    derived.type_ref = type_ref("readonly-rust-reviewer", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived.contract.budget_ceiling = budget(25.0);
    // Affinity may only shrink.
    derived.contract.affinity = AffinityConstraint::Only(set(&["rust"]));

    assert!(is_valid_refinement(&base, &derived, &cat).is_ok());
}

#[test]
fn test_refinement_rejects_each_widening() {
    let cat = CapabilityCatalog::new();
    let base = base_agent();

    let mut d = base_agent();
    d.contract.budget_ceiling = budget(500.0);
    assert!(is_valid_refinement(&base, &d, &cat).is_err());

    let mut d = base_agent();
    d.contract.lifecycle = [LifecycleMode::Resident, LifecycleMode::Revivable]
        .into_iter()
        .collect();
    assert!(is_valid_refinement(&base, &d, &cat).is_err());

    let mut d = base_agent();
    d.contract.security.workspace = WorkspaceMode::Write;
    assert!(is_valid_refinement(&base, &d, &cat).is_err());

    let mut d = base_agent();
    d.contract.security.network = NetworkPolicy::Enabled;
    assert!(is_valid_refinement(&base, &d, &cat).is_err());

    let mut d = base_agent();
    d.contract.allowed_information_functions = vec![
        InformationFunction::Expand,
        InformationFunction::CompressNegative,
    ];
    assert!(is_valid_refinement(&base, &d, &cat).is_err());

    let mut base_isolated = base_agent();
    base_isolated.contract.security.requires_attempt_isolation = true;
    let mut d = base_agent();
    d.contract.security.requires_attempt_isolation = false;
    assert!(is_valid_refinement(&base_isolated, &d, &cat).is_err());
}

#[test]
fn test_refinement_may_strengthen_but_not_weaken_continuity() {
    let cat = CapabilityCatalog::new();
    let mut base_none = base_agent();
    base_none.contract.continuity = ContinuityMode::None;
    let mut d_strengthened = base_agent();
    d_strengthened.contract.continuity = ContinuityMode::Logical;
    assert!(is_valid_refinement(&base_none, &d_strengthened, &cat).is_ok());

    let mut base_logical = base_agent();
    base_logical.contract.continuity = ContinuityMode::Logical;
    let mut d_weakened = base_agent();
    d_weakened.contract.continuity = ContinuityMode::None;
    assert!(is_valid_refinement(&base_logical, &d_weakened, &cat).is_err());
}

#[test]
fn test_refinement_anchor_must_satisfy_base() {
    let cat = CapabilityCatalog::new();
    let mut base = base_agent();
    base.contract.anchor_constraint = Some("region-a".into());

    let mut ok = base_agent();
    ok.contract.anchor_constraint = Some("region-a".into());
    assert!(is_valid_refinement(&base, &ok, &cat).is_ok());

    let mut bad = base_agent();
    bad.contract.anchor_constraint = Some("region-b".into());
    assert!(is_valid_refinement(&base, &bad, &cat).is_err());

    let mut missing = base_agent();
    missing.contract.anchor_constraint = None;
    assert!(is_valid_refinement(&base, &missing, &cat).is_err());
}

#[test]
fn test_can_provision_gates() {
    let cat = CapabilityCatalog::new();
    let agent = base_agent();
    let evidence = base_evidence();

    let mut inactive = base_source();
    inactive.status = SourceStatus::Draining;
    assert!(matches!(
        can_provision(&agent, &inactive, &base_config(), &evidence, &cat),
        Err(ContractError::SourceConfigInvalid { .. })
    ));

    let mut narrow_life = base_source();
    narrow_life.lifecycle_modes = [LifecycleMode::Ephemeral].into_iter().collect();
    assert!(can_provision(&agent, &narrow_life, &base_config(), &evidence, &cat).is_err());

    let mut narrow_cont = base_source();
    narrow_cont.continuity_modes = [ContinuityMode::None].into_iter().collect();
    assert!(can_provision(&agent, &narrow_cont, &base_config(), &evidence, &cat).is_err());
}

#[test]
fn test_functional_envelope_must_cover_agent_capabilities() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );
    let reference = cref("tools", 1);

    let mut agent = base_agent();
    agent.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Set(set(&["git", "ripgrep"])),
    );

    let mut source = base_source();
    source.functional_envelope.insert(
        reference.clone(),
        CapabilityValue::Set(set(&["git", "ripgrep", "jq"])),
    );
    assert!(can_provision(&agent, &source, &base_config(), &base_evidence(), &cat).is_ok());

    let mut weak = base_source();
    weak.functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    assert!(can_provision(&agent, &weak, &base_config(), &base_evidence(), &cat).is_err());
}

#[test]
fn test_source_config_declaration_cannot_exceed_source_envelope() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );
    let reference = cref("tools", 1);

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["shell"])));

    // The source ceiling only admits {git}; a config cannot gain {shell}.
    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    let mut config = base_config();
    config.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git", "shell"])),
        Assurance::Declared,
    ));

    assert!(matches!(
        can_provision(&agent, &source, &config, &base_evidence(), &cat),
        Err(ContractError::InvariantViolation(_))
    ));

    // A source declaration beyond its own envelope is also inconsistent.
    let mut bad_source = base_source();
    bad_source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));
    bad_source.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git", "shell"])),
        Assurance::Declared,
    ));
    assert!(can_provision(&agent, &bad_source, &base_config(), &base_evidence(), &cat).is_err());
}

#[test]
fn test_security_class_requires_imported_evidence() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );
    let reference = cref("sandbox.network_lock", 1);

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));

    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Bool(true));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence(), &cat),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    source.functional_envelope.clear();
    source.claims.push(claim(
        "sandbox.network_lock",
        1,
        CapabilityValue::Bool(true),
        Assurance::Enforced,
    ));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence(), &cat),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(reference.clone(), CapabilityValue::Bool(true))],
    );
    assert!(can_provision(&agent, &source, &base_config(), &evidence, &cat).is_ok());
}

#[test]
fn test_enforced_requires_imported_evidence_not_claim_label() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "authority.deploy",
        1,
        MatcherKind::Bool,
        SecurityClass::Authority,
    );
    let reference = cref("authority.deploy", 1);

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));

    let mut source = base_source();
    source.claims.push(claim(
        "authority.deploy",
        1,
        CapabilityValue::Bool(true),
        Assurance::Enforced,
    ));
    assert!(can_provision(&agent, &source, &base_config(), &base_evidence(), &cat).is_err());

    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(reference.clone(), CapabilityValue::Bool(true))],
    );
    assert!(can_provision(&agent, &source, &base_config(), &evidence, &cat).is_ok());
}

#[test]
fn test_evidence_must_match_source_adapter_policy() {
    let cat = CapabilityCatalog::new();
    let agent = base_agent();
    let evidence = evidence_for(
        policy_ref("other-adapter", 9),
        base_physical(),
        Vec::new(),
        Vec::new(),
    );
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &evidence, &cat),
        Err(ContractError::EvidencePolicyMismatch { .. })
    ));
}

#[test]
fn test_capability_claim_revision_must_match_spec_revision() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );
    define(
        &mut cat,
        "tools",
        2,
        MatcherKind::Set,
        SecurityClass::Functional,
    );

    let reference = cref("tools", 2);
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));

    let mut source = base_source();
    source
        .functional_envelope
        .insert(cref("tools", 1), CapabilityValue::Set(set(&["git", "jq"])));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence(), &cat),
        Err(ContractError::CapabilityMismatch { .. })
    ));

    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
    assert!(can_provision(&agent, &source, &base_config(), &base_evidence(), &cat).is_ok());
}

#[test]
fn test_source_config_must_belong_to_exact_source_revision() {
    let cat = CapabilityCatalog::new();
    let agent = base_agent();
    let mut other_source = base_source();
    other_source.source_ref = SpawnSourceRef::new("other-source", 1).unwrap();
    assert!(matches!(
        can_provision(
            &agent,
            &other_source,
            &base_config(),
            &base_evidence(),
            &cat
        ),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_config_specific_claims_affect_provisioning() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );
    let reference = cref("tools", 1);

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));

    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));

    // Config A narrows within the envelope and still covers the requirement.
    let mut config_a = base_config();
    config_a.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git", "jq"])),
        Assurance::Declared,
    ));
    // Config B narrows below the requirement.
    let mut config_b = base_config();
    config_b.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["jq"])),
        Assurance::Declared,
    ));

    assert!(can_provision(&agent, &source, &config_a, &base_evidence(), &cat).is_ok());
    assert!(can_provision(&agent, &source, &config_b, &base_evidence(), &cat).is_err());
}

#[test]
fn test_conflicting_claims_fail_closed() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );
    let reference = cref("tools", 1);

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Set(set(&["git"])));

    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Set(set(&["git", "jq"])));
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
        can_provision(&agent, &source, &base_config(), &base_evidence(), &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_relations_are_independent() {
    let cat = CapabilityCatalog::new();
    let agent = base_agent();
    assert!(can_execute(&agent, &base_task(), &cat).is_ok());

    let mut req = base_task();
    req.information_function = InformationFunction::CompressPositive;
    assert!(can_execute(&agent, &req, &cat).is_err());
    assert!(can_provision(
        &agent,
        &base_source(),
        &base_config(),
        &base_evidence(),
        &cat
    )
    .is_ok());
}

#[test]
fn test_equal_contracts_are_not_mutually_more_specific() {
    let cat = CapabilityCatalog::new();
    let req = base_task();
    let a = base_agent();
    let mut b = base_agent();
    b.type_ref = type_ref("identical-twin", 1);

    assert!(!more_specific_for(&a, &b, &req, &cat));
    assert!(!more_specific_for(&b, &a, &req, &cat));
}

#[test]
fn test_more_specific_rejects_wider_security_envelope() {
    let cat = CapabilityCatalog::new();
    let req = base_task();
    let broad = base_agent();

    let mut wider_security = base_agent();
    wider_security.type_ref = type_ref("wider-security", 1);
    wider_security.contract.budget_ceiling = budget(50.0);
    wider_security.contract.security.network = NetworkPolicy::Enabled;

    assert!(!more_specific_for(&wider_security, &broad, &req, &cat));
}

#[test]
fn test_more_specific_requires_executability_and_is_not_inheritance_depth() {
    let cat = CapabilityCatalog::new();
    let req = base_task();
    let broad = base_agent();

    let mut narrow = base_agent();
    narrow.type_ref = type_ref("readonly-rust-reviewer", 1);
    narrow.based_on = Some(broad.type_ref.clone());
    narrow.contract.budget_ceiling = budget(60.0);

    assert!(more_specific_for(&narrow, &broad, &req, &cat));
    assert!(!more_specific_for(&broad, &narrow, &req, &cat));

    let mut wider = base_agent();
    wider.type_ref = type_ref("deeper", 1);
    wider.based_on = Some(narrow.type_ref.clone());
    wider.contract.budget_ceiling = budget(500.0);
    assert!(!more_specific_for(&wider, &broad, &req, &cat));

    let mut detached = base_agent();
    detached.type_ref = type_ref("detached", 1);
    detached.contract.allowed_information_functions = vec![InformationFunction::CompressNegative];
    assert!(!more_specific_for(&detached, &broad, &req, &cat));
}

#[test]
fn test_lifecycle_participates_in_specificity() {
    let cat = CapabilityCatalog::new();
    let req = base_task();

    let mut broad = base_agent();
    broad.contract.lifecycle = [LifecycleMode::Resident, LifecycleMode::Revivable]
        .into_iter()
        .collect();
    broad.contract.budget_ceiling = budget(100.0);

    // Narrower budget but a WIDER lifecycle set must not be more specific.
    let mut wider_lifecycle = base_agent();
    wider_lifecycle.type_ref = type_ref("wider-lifecycle", 1);
    wider_lifecycle.contract.lifecycle = [
        LifecycleMode::Resident,
        LifecycleMode::Revivable,
        LifecycleMode::Ephemeral,
    ]
    .into_iter()
    .collect();
    wider_lifecycle.contract.budget_ceiling = budget(50.0);

    assert!(!more_specific_for(&wider_lifecycle, &broad, &req, &cat));
}

#[test]
fn test_sandbox_policy_participates_in_relations() {
    let cat = CapabilityCatalog::new();
    let req = base_task();

    let mut unconstrained = base_agent();
    unconstrained.contract.sandbox_policy = None;

    let pinned = {
        let mut a = base_agent();
        a.type_ref = type_ref("pinned", 1);
        a.contract.sandbox_policy = Some(sandbox_ref("strict-sandbox", 4));
        a
    };
    // Pinning a policy narrows; refinement accepts it, and it is more specific.
    assert!(is_valid_refinement(&unconstrained, &pinned, &cat).is_ok());
    assert!(more_specific_for(&pinned, &unconstrained, &req, &cat));

    let mut other_policy = base_agent();
    other_policy.type_ref = type_ref("other-policy", 1);
    other_policy.contract.sandbox_policy = Some(sandbox_ref("loose-sandbox", 1));
    // A different sandbox policy is not a valid narrowing of the pinned base.
    assert!(is_valid_refinement(&pinned, &other_policy, &cat).is_err());
}

#[test]
fn test_physical_enabled_network_does_not_prove_restricted_enforcement() {
    let cat = CapabilityCatalog::new();
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
    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        safety,
        Vec::new(),
        Vec::new(),
    );
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &evidence, &cat),
        Err(ContractError::SecurityUnenforceable { .. })
    ));
}

#[test]
fn test_filter_precedes_ranking() {
    let cat = CapabilityCatalog::new();
    let agent = base_agent();
    let evidence = base_evidence();

    let mut inactive = base_source();
    inactive.status = SourceStatus::Disabled;
    let eligible = base_source();

    let candidates = [inactive, eligible];
    let feasible: Vec<&SpawnSource> = candidates
        .iter()
        .filter(|s| can_provision(&agent, s, &base_config(), &evidence, &cat).is_ok())
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
}

#[test]
fn test_bool_matcher_presence_semantics() {
    // `false` denotes absence (bottom); `true` denotes presence (top).
    // A present value satisfies an absent requirement, but not the converse.
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(false),
        &CapabilityValue::Bool(true)
    ));
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(false),
        &CapabilityValue::Bool(false)
    ));
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
}

#[test]
fn test_bool_restriction_algebra_is_monotone() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );
    let reference = cref("sandbox.network_lock", 1);

    // Base requires no lock (false); derived requires the lock (true).
    let base = base_agent();
    let mut derived = base_agent();
    derived.type_ref = type_ref("locked", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived
        .contract
        .required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    assert!(is_valid_refinement(&base, &derived, &cat).is_ok());

    // Enforcement that provides the lock must satisfy the stricter derived
    // requirement *and* the weaker base requirement.
    let provided = CapabilityValue::Bool(true);
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(true),
        &provided
    ));
    assert!(value_satisfies(
        MatcherKind::Bool,
        &CapabilityValue::Bool(false),
        &provided
    ));

    // The joined effective restriction is proven by imported evidence.
    let mut task = base_task();
    task.required_capabilities
        .insert(reference.clone(), CapabilityValue::Bool(true));
    let mut source = base_source();
    source
        .functional_envelope
        .insert(reference.clone(), CapabilityValue::Bool(true));
    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(reference.clone(), CapabilityValue::Bool(true))],
    );
    assert!(can_provision_task(&derived, &source, &base_config(), &evidence, &cat, &task).is_ok());

    // Absence cannot satisfy the presence requirement.
    let absent = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(reference, CapabilityValue::Bool(false))],
    );
    assert!(can_provision_task(&derived, &source, &base_config(), &absent, &cat, &task).is_err());
}

#[test]
fn test_nan_budget_is_rejected() {
    assert!(Budget::new(f64::NAN).is_err());
    assert!(Budget::new(f64::INFINITY).is_err());
    assert!(Budget::new(-1.0).is_err());
    assert!(Budget::new(0.0).is_ok());
}

#[test]
fn test_nan_quantity_is_rejected() {
    assert!(Quantity::new(f64::NAN).is_err());
    assert!(Quantity::new(f64::INFINITY).is_err());
    assert!(Quantity::new(-1.0).is_err());
    assert!(Quantity::new(0.0).is_ok());
}

#[test]
fn test_negative_zero_is_canonicalized() {
    assert!(Budget::new(-0.0).unwrap().get().is_sign_positive());
    assert_eq!(Budget::new(-0.0).unwrap(), Budget::new(0.0).unwrap());
    assert!(Quantity::new(-0.0).unwrap().get().is_sign_positive());
    assert_eq!(Quantity::new(-0.0).unwrap(), Quantity::new(0.0).unwrap());
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
fn test_capability_definition_is_global_authority() {
    let mut cat = CapabilityCatalog::new();
    cat.define(
        cref("network.lock", 1),
        MatcherKind::Bool,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    )
    .unwrap();

    // Idempotent with identical semantics.
    assert!(cat
        .define(
            cref("network.lock", 1),
            MatcherKind::Bool,
            SecurityClass::Sandbox,
            CapabilityPolarity::Restriction
        )
        .is_ok());

    // A per-AgentType downgrade to Functional is impossible: the definition is
    // global, so redefining it fails closed.
    assert!(matches!(
        cat.define(
            cref("network.lock", 1),
            MatcherKind::Bool,
            SecurityClass::Functional,
            CapabilityPolarity::Ability
        ),
        Err(ContractError::CapabilityDefinitionConflict { .. })
    ));
}

#[test]
fn test_capability_value_shape_must_match_definition() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(cref("tools", 1), CapabilityValue::Bool(true));

    assert!(can_execute(&agent, &base_task(), &cat).is_err());
}

#[test]
fn test_refinement_capability_polarity() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );
    define(
        &mut cat,
        "tools.ripgrep",
        1,
        MatcherKind::Bool,
        SecurityClass::Functional,
    );

    // A Sandbox capability is a restriction: dropping it weakens the guarantee.
    let mut restricted = base_agent();
    restricted
        .contract
        .required_capabilities
        .insert(cref("sandbox.network_lock", 1), CapabilityValue::Bool(true));
    let mut dropped = base_agent();
    dropped
        .contract
        .required_capabilities
        .insert(cref("tools.ripgrep", 1), CapabilityValue::Bool(true));
    assert!(matches!(
        is_valid_refinement(&restricted, &dropped, &cat),
        Err(ContractError::InvalidRefinement { .. })
    ));

    // A Functional capability is an ability: dropping it narrows (valid).
    let mut with_tool = base_agent();
    with_tool
        .contract
        .required_capabilities
        .insert(cref("tools.ripgrep", 1), CapabilityValue::Bool(true));
    assert!(is_valid_refinement(&with_tool, &base_agent(), &cat).is_ok());

    // Adding a restriction narrows (valid); adding an ability widens (invalid).
    assert!(is_valid_refinement(&base_agent(), &restricted, &cat).is_ok());
    assert!(matches!(
        is_valid_refinement(&base_agent(), &with_tool, &cat),
        Err(ContractError::InvalidRefinement { .. })
    ));
}

#[test]
fn test_derived_adds_authority_capability_is_not_valid_refinement() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "authority.deploy",
        1,
        MatcherKind::Bool,
        SecurityClass::Authority,
    );

    let base = base_agent();
    let mut derived = base_agent();
    derived.type_ref = type_ref("deployer", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived
        .contract
        .required_capabilities
        .insert(cref("authority.deploy", 1), CapabilityValue::Bool(true));

    // The derived type can execute (and be provisioned for) Tasks the base
    // cannot: authority expansion.
    assert!(matches!(
        is_valid_refinement(&base, &derived, &cat),
        Err(ContractError::InvalidRefinement { .. })
    ));

    let mut req = base_task();
    req.required_capabilities
        .insert(cref("authority.deploy", 1), CapabilityValue::Bool(true));
    assert!(can_execute(&base, &req, &cat).is_err());
    assert!(can_execute(&derived, &req, &cat).is_ok());
}

#[test]
fn test_lower_provided_capability_is_more_specific() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "memory_mb",
        1,
        MatcherKind::Quantity,
        SecurityClass::Functional,
    );
    let reference = cref("memory_mb", 1);

    let mut broad = base_agent();
    broad.type_ref = type_ref("broad", 1);
    broad.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(8192.0)),
    );

    // Advertises less: executes a subset of the broad agent's Tasks.
    let mut narrow = base_agent();
    narrow.type_ref = type_ref("narrow", 1);
    narrow.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(1024.0)),
    );

    let mut low = base_task();
    low.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(1024.0)),
    );
    let mut high = base_task();
    high.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(4096.0)),
    );

    assert!(can_execute(&narrow, &low, &cat).is_ok());
    assert!(can_execute(&broad, &low, &cat).is_ok());
    assert!(can_execute(&narrow, &high, &cat).is_err());
    assert!(can_execute(&broad, &high, &cat).is_ok());

    assert!(more_specific_for(&narrow, &broad, &low, &cat));
    assert!(!more_specific_for(&broad, &narrow, &low, &cat));
}

#[test]
fn test_derived_capability_must_not_expand_executable_task_set() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "memory_mb",
        1,
        MatcherKind::Quantity,
        SecurityClass::Functional,
    );
    let reference = cref("memory_mb", 1);

    let mut base = base_agent();
    base.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(4096.0)),
    );

    // A "stronger provisioning requirement" (higher advertised value) can
    // execute more Tasks, so it is NOT a valid refinement and NOT more specific.
    let mut stronger = base_agent();
    stronger.type_ref = type_ref("stronger", 1);
    stronger.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Quantity(quantity(8192.0)),
    );
    assert!(is_valid_refinement(&base, &stronger, &cat).is_err());

    let mut req = base_task();
    req.required_capabilities
        .insert(reference, CapabilityValue::Quantity(quantity(8192.0)));
    assert!(!more_specific_for(&stronger, &base, &req, &cat));
}

#[test]
fn test_empty_lifecycle_is_rejected() {
    let cat = CapabilityCatalog::new();
    let mut agent = base_agent();
    agent.contract.lifecycle = BTreeSet::new();
    assert!(can_execute(&agent, &base_task(), &cat).is_err());
}

#[test]
fn test_unreferenced_config_claim_cannot_exceed_source_envelope() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );

    // The AgentType does not require "tools" at all.
    let agent = base_agent();
    let mut source = base_source();
    source
        .functional_envelope
        .insert(cref("tools", 1), CapabilityValue::Set(set(&["git"])));
    let mut config = base_config();
    config.claims.push(claim(
        "tools",
        1,
        CapabilityValue::Set(set(&["git", "shell"])),
        Assurance::Declared,
    ));

    assert!(matches!(
        can_provision(&agent, &source, &config, &base_evidence(), &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_unreferenced_conflicting_claims_fail_validation() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "tools",
        1,
        MatcherKind::Set,
        SecurityClass::Functional,
    );

    let agent = base_agent();
    let mut source = base_source();
    source.functional_envelope.insert(
        cref("tools", 1),
        CapabilityValue::Set(set(&["git", "jq", "shell"])),
    );
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
        can_provision(&agent, &source, &base_config(), &base_evidence(), &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_unknown_claim_capability_revision_fails_validation() {
    let cat = CapabilityCatalog::new(); // "tools@9" is undefined
    let agent = base_agent();
    let mut source = base_source();
    source
        .functional_envelope
        .insert(cref("tools", 9), CapabilityValue::Set(set(&["git"])));
    source.claims.push(claim(
        "tools",
        9,
        CapabilityValue::Set(set(&["git"])),
        Assurance::Declared,
    ));
    assert!(matches!(
        can_provision(&agent, &source, &base_config(), &base_evidence(), &cat),
        Err(ContractError::CapabilityMismatch { .. })
    ));
}

#[test]
fn test_affinity_participates_in_specificity() {
    let cat = CapabilityCatalog::new();
    let req = base_task();

    let mut broad = base_agent();
    broad.contract.affinity = AffinityConstraint::Only(set(&["rust", "sqlite"]));

    let mut narrow = base_agent();
    narrow.type_ref = type_ref("rust-reviewer", 1);
    narrow.contract.affinity = AffinityConstraint::Only(set(&["rust"]));

    assert!(more_specific_for(&narrow, &broad, &req, &cat));
    assert!(!more_specific_for(&broad, &narrow, &req, &cat));
}

#[test]
fn test_affinity_has_unconstrained_top() {
    let cat = CapabilityCatalog::new();

    let general = base_agent(); // affinity = Any
    let mut rust = base_agent();
    rust.type_ref = type_ref("rust-reviewer", 1);
    rust.contract.affinity = AffinityConstraint::Only(set(&["rust"]));

    // A general (Any) agent accepts a previously unseen affinity tag.
    let mut unseen = base_task();
    unseen.required_affinity = set(&["postgres"]);
    assert!(can_execute(&general, &unseen, &cat).is_ok());
    assert!(can_execute(&rust, &unseen, &cat).is_err());

    // Any -> Only is a valid narrowing; Only -> Any is not.
    assert!(is_valid_refinement(&general, &rust, &cat).is_ok());
    assert!(is_valid_refinement(&rust, &general, &cat).is_err());

    // And the narrowing is more specific for a task both can execute.
    assert!(more_specific_for(&rust, &general, &base_task(), &cat));
}

#[test]
fn test_refinement_must_not_expand_executable_task_set_via_affinity() {
    let cat = CapabilityCatalog::new();

    let mut base = base_agent();
    base.contract.affinity = AffinityConstraint::Only(set(&["rust"]));

    // Derived adds {sqlite}: it could execute Tasks the base cannot, so it is
    // authority expansion and must be rejected.
    let mut derived = base_agent();
    derived.type_ref = type_ref("broader", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived.contract.affinity = AffinityConstraint::Only(set(&["rust", "sqlite"]));
    assert!(is_valid_refinement(&base, &derived, &cat).is_err());

    // Algebra invariant on a valid narrowing: Derived executable tasks ⊆ Base.
    let mut narrowed = base_agent();
    narrowed.type_ref = type_ref("narrower", 1);
    narrowed.based_on = Some(base.type_ref.clone());
    narrowed.contract.affinity = AffinityConstraint::Only(BTreeSet::new());
    assert!(is_valid_refinement(&base, &narrowed, &cat).is_ok());

    let mut req = base_task();
    req.required_affinity = set(&["rust"]);
    if can_execute(&narrowed, &req, &cat).is_ok() {
        assert!(can_execute(&base, &req, &cat).is_ok());
    }
    // And the narrowing genuinely cannot execute the {sqlite} task the base can.
    let mut sqlite_req = base_task();
    sqlite_req.required_affinity = set(&["sqlite"]);
    assert!(can_execute(&base, &sqlite_req, &cat).is_err());
    assert!(can_execute(&narrowed, &sqlite_req, &cat).is_err());
}

#[test]
fn test_sandbox_policy_requirement_compatible_with_unconstrained_agent() {
    let cat = CapabilityCatalog::new();
    let policy = sandbox_ref("strict-sandbox", 4);

    // Unconstrained agent (None) accepts any Task sandbox requirement.
    let unconstrained = base_agent();
    let mut req = base_task();
    req.sandbox_policy = Some(policy.clone());
    assert!(can_execute(&unconstrained, &req, &cat).is_ok());

    // A pinned agent accepts only its own policy.
    let mut pinned = base_agent();
    pinned.contract.sandbox_policy = Some(policy.clone());
    assert!(can_execute(&pinned, &req, &cat).is_ok());

    let mut other = base_task();
    other.sandbox_policy = Some(sandbox_ref("other", 1));
    assert!(can_execute(&pinned, &other, &cat).is_err());
}

#[test]
fn test_sandbox_policy_must_have_provisioning_enforcement_proof() {
    let cat = CapabilityCatalog::new();
    let policy = sandbox_ref("strict-sandbox", 4);
    let mut agent = base_agent();
    agent.contract.sandbox_policy = Some(policy.clone());

    // Evidence without the sandbox policy does not enforce it.
    assert!(matches!(
        can_provision(
            &agent,
            &base_source(),
            &base_config(),
            &base_evidence(),
            &cat
        ),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        vec![policy],
        Vec::new(),
    );
    assert!(can_provision(&agent, &base_source(), &base_config(), &evidence, &cat).is_ok());
}

#[test]
fn test_provisioning_evidence_for_config_a_must_not_authorize_config_b() {
    let cat = CapabilityCatalog::new();
    let agent = base_agent();

    // Evidence resolved for config B.
    let mut config_b = base_config();
    config_b.config_ref = SourceConfigRef::new(base_source().source_ref, "fast-review", 4).unwrap();
    config_b.config_digest = ConfigDigest::new("sha256:def").unwrap();
    let evidence_b = ResolvedProvisioningEvidence::for_tests(
        policy_ref("codex-local-adapter", 3),
        "codex_cli",
        base_source().source_ref,
        config_b.config_ref.clone(),
        config_b.config_digest.clone(),
        base_physical(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    // Evidence bound to B must NOT authorize config A.
    assert!(matches!(
        can_provision(&agent, &base_source(), &base_config(), &evidence_b, &cat),
        Err(ContractError::EvidenceSubjectMismatch { .. })
    ));
    // It authorizes its own config B.
    assert!(can_provision(&agent, &base_source(), &config_b, &evidence_b, &cat).is_ok());
}

#[test]
fn test_physical_safety_requires_validated_construction() {
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
    assert!(SandboxPolicyRef::new("", 1).is_err());
    assert!(CapabilityRef::new("", 1).is_err());
    assert!(CapabilityRef::new("x", 0).is_err());
    assert!(CredentialRef::new("").is_err());
}

#[test]
fn test_latest_selector_empty_id_is_invalid() {
    assert!(AgentTypeSelector::latest("").is_err());
    assert!(AgentTypeSelector::latest("   ").is_err());
}

#[test]
fn test_selector_resolution() {
    let mut catalog = InMemorySelectorCatalog::new();
    catalog.publish(type_ref("auditor", 2));
    catalog.publish(type_ref("auditor", 3));

    let exact = AgentTypeSelector::Exact(type_ref("auditor", 2));
    assert_eq!(
        resolve_selector(&exact, &catalog).unwrap(),
        type_ref("auditor", 2)
    );

    let latest = AgentTypeSelector::latest("auditor").unwrap();
    assert_eq!(
        resolve_selector(&latest, &catalog).unwrap(),
        type_ref("auditor", 3)
    );

    let missing = AgentTypeSelector::Exact(type_ref("auditor", 9));
    assert!(matches!(
        resolve_selector(&missing, &catalog),
        Err(ContractError::AgentTypeNotFound { .. })
    ));

    let unknown = AgentTypeSelector::latest("ghost").unwrap();
    assert!(resolve_selector(&unknown, &catalog).is_err());
}

#[test]
fn test_deprecation_is_monotonic_and_future_selection_only() {
    let mut catalog = InMemorySelectorCatalog::new();
    let r2 = type_ref("auditor", 2);
    catalog.publish(r2.clone());
    catalog.publish(type_ref("auditor", 3));

    assert!(catalog.deprecate(&r2));
    assert!(!catalog.is_published(&r2));
    assert!(catalog.is_deprecated(&r2));
    assert!(!catalog.publish(r2.clone()));

    let latest = AgentTypeSelector::latest("auditor").unwrap();
    assert_eq!(
        resolve_selector(&latest, &catalog).unwrap(),
        type_ref("auditor", 3)
    );
}

#[test]
fn test_continuity_strengthening_is_guarantee_not_executable_subset() {
    let cat = CapabilityCatalog::new();

    let mut base = base_agent();
    base.contract.continuity = ContinuityMode::None;
    let mut derived = base_agent();
    derived.type_ref = type_ref("logical-guarantee", 1);
    derived.based_on = Some(base.type_ref.clone());
    derived.contract.continuity = ContinuityMode::Logical;

    // Strengthening continuity is a valid refinement (a guarantee upgrade) ...
    assert!(is_valid_refinement(&base, &derived, &cat).is_ok());

    // ... but it deliberately enlarges the executable Task set, so the
    // executable-subset invariant is scoped to the authority/scope dimensions.
    let mut req = base_task();
    req.required_continuity = ContinuityMode::Logical;
    assert!(can_execute(&base, &req, &cat).is_err());
    assert!(can_execute(&derived, &req, &cat).is_ok());
}

#[test]
fn test_effective_task_policy_must_be_enforceable() {
    let cat = CapabilityCatalog::new();

    // AgentType ceiling is wide (write/enabled); Task is stricter.
    let mut agent = base_agent();
    agent.contract.security.workspace = WorkspaceMode::Write;
    agent.contract.security.network = NetworkPolicy::Enabled;
    let policy = sandbox_ref("strict-sandbox", 4);

    let mut task = base_task();
    task.required_workspace = WorkspaceMode::ReadOnly;
    task.required_network = NetworkPolicy::Disabled;
    task.sandbox_policy = Some(policy);

    // Environment can enforce write/enabled but NOT read-only/disabled or P.
    let safety = PhysicalSafety::new(
        false,
        vec![WorkspaceMode::Write],
        [NetworkPolicy::Enabled].into_iter().collect(),
    )
    .unwrap();
    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        safety,
        Vec::new(),
        Vec::new(),
    );

    // The two per-predicate checks both pass ...
    assert!(can_execute(&agent, &task, &cat).is_ok());
    assert!(can_provision(&agent, &base_source(), &base_config(), &evidence, &cat).is_ok());

    // ... but the conjunction is not sufficient: the Task's effective policy is
    // not enforceable by the imported environment.
    assert!(matches!(
        can_provision_task(
            &agent,
            &base_source(),
            &base_config(),
            &evidence,
            &cat,
            &task
        ),
        Err(ContractError::SecurityUnenforceable { .. })
    ));
}

#[test]
fn test_validate_source_config_rejects_wrong_exact_source_revision() {
    let cat = CapabilityCatalog::new();
    let config_a = base_config();
    let mut source_b = base_source();
    source_b.source_ref = SpawnSourceRef::new("other-source", 1).unwrap();

    assert!(matches!(
        validate_source_config(&config_a, &source_b, &cat),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_security_class_claim_value_shape_must_match_definition() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "network.lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );

    let mut source = base_source();
    // Wrong shape (Set) for a Bool definition: rejected regardless of class.
    source.claims.push(claim(
        "network.lock",
        1,
        CapabilityValue::Set(set(&["nonsense"])),
        Assurance::Declared,
    ));
    assert!(matches!(
        validate_spawn_source(&source, &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_losing_security_guarantee_is_not_more_specific() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );

    let mut guaranteed = base_agent();
    guaranteed.type_ref = type_ref("guarded", 1);
    guaranteed
        .contract
        .required_capabilities
        .insert(cref("sandbox.network_lock", 1), CapabilityValue::Bool(true));

    // Drops the guarantee: physically less constrained, so it must not be
    // ranked as more specific.
    let unguarded = base_agent();

    let req = base_task();
    assert!(can_execute(&guaranteed, &req, &cat).is_ok());
    assert!(can_execute(&unguarded, &req, &cat).is_ok());
    assert!(!more_specific_for(&unguarded, &guaranteed, &req, &cat));
}

#[test]
fn test_continuity_is_ordered_minimum_guarantee() {
    let cat = CapabilityCatalog::new();

    // Source advertises {Logical}, which is at least None.
    let mut source = base_source();
    source.continuity_modes = [ContinuityMode::Logical].into_iter().collect();

    let mut agent_none = base_agent();
    agent_none.contract.continuity = ContinuityMode::None;
    assert!(can_provision(&agent_none, &source, &base_config(), &base_evidence(), &cat).is_ok());

    let mut agent_logical = base_agent();
    agent_logical.contract.continuity = ContinuityMode::Logical;
    assert!(can_provision(
        &agent_logical,
        &source,
        &base_config(),
        &base_evidence(),
        &cat
    )
    .is_ok());

    // Source advertises only {None}; a Logical requirement fails.
    let mut weak_source = base_source();
    weak_source.continuity_modes = [ContinuityMode::None].into_iter().collect();
    assert!(can_provision(
        &agent_logical,
        &weak_source,
        &base_config(),
        &base_evidence(),
        &cat
    )
    .is_err());

    // Task minimum also uses the ordered semantics.
    let task = base_task(); // required_continuity = None
    assert!(can_provision_task(
        &agent_logical,
        &source,
        &base_config(),
        &base_evidence(),
        &cat,
        &task
    )
    .is_ok());
}

#[test]
fn test_continuity_is_not_semantic_specificity() {
    let cat = CapabilityCatalog::new();
    let req = base_task();

    let mut logical = base_agent();
    logical.type_ref = type_ref("logical", 1);
    logical.contract.continuity = ContinuityMode::Logical;

    let mut none = base_agent();
    none.type_ref = type_ref("none", 1);
    none.contract.continuity = ContinuityMode::None;

    // Differing only in continuity: incomparable, not mutually more specific.
    assert!(!more_specific_for(&logical, &none, &req, &cat));
    assert!(!more_specific_for(&none, &logical, &req, &cat));
}

#[test]
fn test_capability_polarity_is_independent_of_security_class() {
    let mut cat = CapabilityCatalog::new();
    // Sandbox classification, but ability-shaped value: growing widens.
    cat.define(
        cref("allowed_commands", 1),
        MatcherKind::Set,
        SecurityClass::Sandbox,
        CapabilityPolarity::Ability,
    )
    .unwrap();
    // Sandbox classification with restriction-shaped value: growing narrows.
    cat.define(
        cref("locked_paths", 1),
        MatcherKind::Set,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    )
    .unwrap();

    let mut base = base_agent();
    base.contract.required_capabilities.insert(
        cref("allowed_commands", 1),
        CapabilityValue::Set(set(&["git"])),
    );
    base.contract
        .required_capabilities
        .insert(cref("locked_paths", 1), CapabilityValue::Set(set(&["/a"])));

    // Growing an ability-shaped capability widens authority: rejected.
    let mut grew_ability = base_agent();
    grew_ability.contract.required_capabilities.insert(
        cref("allowed_commands", 1),
        CapabilityValue::Set(set(&["git", "shell"])),
    );
    grew_ability
        .contract
        .required_capabilities
        .insert(cref("locked_paths", 1), CapabilityValue::Set(set(&["/a"])));
    assert!(matches!(
        is_valid_refinement(&base, &grew_ability, &cat),
        Err(ContractError::InvalidRefinement { .. })
    ));

    // Growing a restriction-shaped capability narrows: accepted.
    let mut grew_restriction = base_agent();
    grew_restriction.contract.required_capabilities.insert(
        cref("allowed_commands", 1),
        CapabilityValue::Set(set(&["git"])),
    );
    grew_restriction.contract.required_capabilities.insert(
        cref("locked_paths", 1),
        CapabilityValue::Set(set(&["/a", "/b"])),
    );
    assert!(is_valid_refinement(&base, &grew_restriction, &cat).is_ok());
}

#[test]
fn test_config_narrows_source_continuity() {
    let cat = CapabilityCatalog::new();
    let mut source = base_source();
    source.continuity_modes = [ContinuityMode::None, ContinuityMode::Logical]
        .into_iter()
        .collect();

    let mut agent = base_agent();
    agent.contract.continuity = ContinuityMode::Logical;

    // Config A keeps the source envelope: eligible.
    let config_a = base_config();
    assert!(can_provision(&agent, &source, &config_a, &base_evidence(), &cat).is_ok());

    // Config B narrows to {None}: not eligible for a Logical requirement.
    let mut config_b = base_config();
    config_b.continuity_modes = Some([ContinuityMode::None].into_iter().collect());
    assert!(can_provision(&agent, &source, &config_b, &base_evidence(), &cat).is_err());

    // A config cannot widen beyond the source envelope.
    let mut narrow_source = base_source();
    narrow_source.continuity_modes = [ContinuityMode::None].into_iter().collect();
    let mut widening = base_config();
    widening.continuity_modes = Some([ContinuityMode::Logical].into_iter().collect());
    assert!(matches!(
        validate_source_config(&widening, &narrow_source, &cat),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_config_cannot_widen_source_lifecycle() {
    let cat = CapabilityCatalog::new();
    let source = base_source(); // lifecycle {Resident, Ephemeral}
    let mut config = base_config();
    config.lifecycle_modes = Some(
        [LifecycleMode::Resident, LifecycleMode::Revivable]
            .into_iter()
            .collect(),
    );
    assert!(matches!(
        validate_source_config(&config, &source, &cat),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_task_adds_restriction_to_broad_agent() {
    let mut cat = CapabilityCatalog::new();
    cat.define(
        cref("locked_paths", 1),
        MatcherKind::Set,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    )
    .unwrap();

    // Broad AgentType: no locked_paths.
    let agent = base_agent();

    let mut task = base_task();
    task.required_capabilities.insert(
        cref("locked_paths", 1),
        CapabilityValue::Set(set(&["/secret"])),
    );

    // The Task adds a restriction the AgentType did not pre-advertise.
    assert!(can_execute(&agent, &task, &cat).is_ok());

    let enforced = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(
            cref("locked_paths", 1),
            CapabilityValue::Set(set(&["/secret"])),
        )],
    );
    assert!(can_provision_task(
        &agent,
        &base_source(),
        &base_config(),
        &enforced,
        &cat,
        &task
    )
    .is_ok());

    // No evidence for the restriction: ineligible.
    assert!(matches!(
        can_provision_task(
            &agent,
            &base_source(),
            &base_config(),
            &base_evidence(),
            &cat,
            &task
        ),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // Evidence that does not actually enforce the joined restriction: ineligible.
    let weak = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(cref("locked_paths", 1), CapabilityValue::Set(set(&["/a"])))],
    );
    assert!(
        can_provision_task(&agent, &base_source(), &base_config(), &weak, &cat, &task).is_err()
    );
}

#[test]
fn test_task_strengthens_restriction() {
    let mut cat = CapabilityCatalog::new();
    cat.define(
        cref("locked_paths", 1),
        MatcherKind::Set,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    )
    .unwrap();

    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(cref("locked_paths", 1), CapabilityValue::Set(set(&["/a"])));

    let mut task = base_task();
    task.required_capabilities.insert(
        cref("locked_paths", 1),
        CapabilityValue::Set(set(&["/a", "/b"])),
    );

    assert!(can_execute(&agent, &task, &cat).is_ok());

    let strong = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(
            cref("locked_paths", 1),
            CapabilityValue::Set(set(&["/a", "/b"])),
        )],
    );
    assert!(
        can_provision_task(&agent, &base_source(), &base_config(), &strong, &cat, &task).is_ok()
    );

    let weak = evidence_for(
        policy_ref("codex-local-adapter", 3),
        base_physical(),
        Vec::new(),
        vec![(cref("locked_paths", 1), CapabilityValue::Set(set(&["/a"])))],
    );
    assert!(
        can_provision_task(&agent, &base_source(), &base_config(), &weak, &cat, &task).is_err()
    );
}

#[test]
fn test_config_continuity_override_is_ordered() {
    let cat = CapabilityCatalog::new();

    let mut source = base_source();
    source.continuity_modes = [ContinuityMode::Logical].into_iter().collect();

    // A source that provides Logical permits a config that only commits to None.
    let mut config = base_config();
    config.continuity_modes = Some([ContinuityMode::None].into_iter().collect());
    assert!(validate_source_config(&config, &source, &cat).is_ok());

    // But a config cannot demand Logical from a source that only has None.
    let mut weak_source = base_source();
    weak_source.continuity_modes = [ContinuityMode::None].into_iter().collect();
    let mut too_strong = base_config();
    too_strong.continuity_modes = Some([ContinuityMode::Logical].into_iter().collect());
    assert!(matches!(
        validate_source_config(&too_strong, &weak_source, &cat),
        Err(ContractError::SourceConfigInvalid { .. })
    ));
}

#[test]
fn test_functional_restriction_proven_by_declaration_not_evidence() {
    let mut cat = CapabilityCatalog::new();
    cat.define(
        cref("locked_paths", 1),
        MatcherKind::Set,
        SecurityClass::Functional,
        CapabilityPolarity::Restriction,
    )
    .unwrap();

    let agent = base_agent();
    let mut task = base_task();
    task.required_capabilities.insert(
        cref("locked_paths", 1),
        CapabilityValue::Set(set(&["/secret"])),
    );

    // Functional class: proven by the source envelope, not imported evidence.
    let mut source = base_source();
    source.functional_envelope.insert(
        cref("locked_paths", 1),
        CapabilityValue::Set(set(&["/secret", "/a"])),
    );
    assert!(can_provision_task(
        &agent,
        &source,
        &base_config(),
        &base_evidence(),
        &cat,
        &task
    )
    .is_ok());

    // A weaker envelope cannot satisfy the joined restriction.
    let mut weak = base_source();
    weak.functional_envelope
        .insert(cref("locked_paths", 1), CapabilityValue::Set(set(&["/a"])));
    assert!(
        can_provision_task(&agent, &weak, &base_config(), &base_evidence(), &cat, &task).is_err()
    );
}

#[test]
fn test_malformed_task_restriction_shape_fails_closed() {
    let mut cat = CapabilityCatalog::new();
    cat.define(
        cref("locked_paths", 1),
        MatcherKind::Set,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    )
    .unwrap();

    let mut task = base_task();
    // Wrong shape (Bool) for a Set definition, even though the polarity is
    // Restriction and the capability is not pre-advertised.
    task.required_capabilities
        .insert(cref("locked_paths", 1), CapabilityValue::Bool(true));

    assert!(matches!(
        can_execute(&base_agent(), &task, &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_bool_false_is_absence_in_can_execute() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "workspace.write",
        1,
        MatcherKind::Bool,
        SecurityClass::Authority,
    );
    let reference = cref("workspace.write", 1);
    let agent = base_agent();

    let mut explicit = base_task();
    explicit
        .required_capabilities
        .insert(reference, CapabilityValue::Bool(false));
    let omitted = base_task();

    // `Bool(false)` and an omitted entry are the same absence state.
    assert_eq!(
        can_execute(&agent, &explicit, &cat).is_ok(),
        can_execute(&agent, &omitted, &cat).is_ok()
    );
    assert!(can_execute(&agent, &explicit, &cat).is_ok());
}

#[test]
fn test_bool_false_is_absence_in_specificity() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );
    let reference = cref("sandbox.network_lock", 1);

    let mut omitted = base_agent();
    omitted.type_ref = type_ref("omitted", 1);
    let mut explicit = base_agent();
    explicit.type_ref = type_ref("explicit", 1);
    explicit
        .contract
        .required_capabilities
        .insert(reference, CapabilityValue::Bool(false));

    let req = base_task();
    assert!(can_execute(&omitted, &req, &cat).is_ok());
    assert!(can_execute(&explicit, &req, &cat).is_ok());
    // Semantically equivalent agents must not be strictly ranked.
    assert!(!more_specific_for(&omitted, &explicit, &req, &cat));
    assert!(!more_specific_for(&explicit, &omitted, &req, &cat));
}

#[test]
fn test_task_bool_false_restriction_needs_no_proof() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );
    let reference = cref("sandbox.network_lock", 1);

    let agent = base_agent();
    let mut task = base_task();
    task.required_capabilities
        .insert(reference, CapabilityValue::Bool(false));

    // `false` means "no restriction": no envelope entry and no imported
    // evidence may be required to prove it.
    assert!(can_provision_task(
        &agent,
        &base_source(),
        &base_config(),
        &base_evidence(),
        &cat,
        &task
    )
    .is_ok());
}

#[test]
fn test_task_attempt_isolation_intersection() {
    let cat = CapabilityCatalog::new();
    let source = base_source();
    let config = base_config();
    let agent = base_agent();

    // agent false + task false -> isolation not required.
    let plain = base_task();
    assert!(can_provision_task(&agent, &source, &config, &base_evidence(), &cat, &plain).is_ok());

    // agent false + task true -> physical isolation required but absent.
    let mut isolated = base_task();
    isolated.required_attempt_isolation = true;
    assert!(matches!(
        can_provision_task(&agent, &source, &config, &base_evidence(), &cat, &isolated),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // agent true + task false -> still required for every task.
    let mut strict_agent = base_agent();
    strict_agent.contract.security.requires_attempt_isolation = true;
    assert!(matches!(
        can_provision_task(
            &strict_agent,
            &source,
            &config,
            &base_evidence(),
            &cat,
            &plain
        ),
        Err(ContractError::SecurityUnenforceable { .. })
    ));

    // When the environment can isolate, both are satisfied.
    let isolatable = PhysicalSafety::new(
        true,
        vec![WorkspaceMode::ReadOnly, WorkspaceMode::Write],
        [
            NetworkPolicy::Disabled,
            NetworkPolicy::Restricted,
            NetworkPolicy::Enabled,
        ]
        .into_iter()
        .collect(),
    )
    .unwrap();
    let evidence = evidence_for(
        policy_ref("codex-local-adapter", 3),
        isolatable,
        Vec::new(),
        Vec::new(),
    );
    assert!(can_provision_task(&agent, &source, &config, &evidence, &cat, &isolated).is_ok());
    assert!(can_provision_task(&strict_agent, &source, &config, &evidence, &cat, &plain).is_ok());
}

#[test]
fn test_incompatible_restriction_join_fails_can_execute() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "region",
        1,
        MatcherKind::Ordered,
        SecurityClass::Sandbox,
    );
    let reference = cref("region", 1);

    let mut agent = base_agent();
    agent.contract.required_capabilities.insert(
        reference.clone(),
        CapabilityValue::Ordered {
            class: "A".into(),
            rank: 1,
        },
    );
    let mut task = base_task();
    task.required_capabilities.insert(
        reference,
        CapabilityValue::Ordered {
            class: "B".into(),
            rank: 1,
        },
    );

    // The pure restriction join cannot exist, so `can_execute` already fails.
    assert!(matches!(
        can_execute(&agent, &task, &cat),
        Err(ContractError::CapabilityMismatch { .. })
    ));
}

#[test]
fn test_disposition_is_separate_from_revision_content() {
    let source = base_source();
    let mut draining_source = base_source();
    draining_source.status = SourceStatus::Draining;
    // Ordinary equality sees the operational change...
    assert_ne!(source, draining_source);
    // ...while revision-content identity deliberately ignores it.
    assert!(source.same_revision_content(&draining_source));

    let config = base_config();
    let mut draining_config = base_config();
    draining_config.status = ConfigStatus::Draining;
    assert_ne!(config, draining_config);
    // `SourceConfig` only carries config-selection metadata; the complete
    // durable revision identity lives in the B.2 `SourceConfigRevision`.
    assert!(config.same_config_contract_content(&draining_config));
}

#[test]
fn test_bool_false_does_not_bypass_shape_validation_task() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "locked_paths",
        1,
        MatcherKind::Set,
        SecurityClass::Sandbox,
    );
    let mut task = base_task();
    task.required_capabilities
        .insert(cref("locked_paths", 1), CapabilityValue::Bool(false));

    // Bool(false) is absence only for a Bool capability; a Set definition must
    // still fail closed on the wrong shape.
    assert!(matches!(
        can_execute(&base_agent(), &task, &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_bool_false_does_not_bypass_shape_validation_agent() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "locked_paths",
        1,
        MatcherKind::Set,
        SecurityClass::Sandbox,
    );
    let mut agent = base_agent();
    agent
        .contract
        .required_capabilities
        .insert(cref("locked_paths", 1), CapabilityValue::Bool(false));

    assert!(matches!(
        can_execute(&agent, &base_task(), &cat),
        Err(ContractError::InvariantViolation(_))
    ));
}

#[test]
fn test_bool_false_does_not_bypass_catalog_lookup() {
    let cat = CapabilityCatalog::new();
    let mut task = base_task();
    task.required_capabilities.insert(
        cref("unknown_capability", 999),
        CapabilityValue::Bool(false),
    );

    // An unknown capability must not be silently treated as absence.
    assert!(matches!(
        can_execute(&base_agent(), &task, &cat),
        Err(ContractError::CapabilityMismatch { .. })
    ));
}

#[test]
fn test_bool_false_absence_for_real_bool_capability() {
    let mut cat = CapabilityCatalog::new();
    define(
        &mut cat,
        "sandbox.network_lock",
        1,
        MatcherKind::Bool,
        SecurityClass::Sandbox,
    );
    let reference = cref("sandbox.network_lock", 1);
    let mut explicit = base_task();
    explicit
        .required_capabilities
        .insert(reference, CapabilityValue::Bool(false));

    assert!(can_execute(&base_agent(), &explicit, &cat).is_ok());
    assert!(can_execute(&base_agent(), &base_task(), &cat).is_ok());
}
