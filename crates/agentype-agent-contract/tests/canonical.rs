//! M6-B.2 canonical-encoding conformance: permutation invariance, `Bool(false)`
//! absence, conflict fail-closed behavior, and content-digest stability.

use agentype_agent_contract::*;
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn cref(id: &str, revision: u64) -> CapabilityRef {
    CapabilityRef::new(id, revision).unwrap()
}

fn policy_ref(id: &str, revision: u64) -> AdapterPolicyRef {
    AdapterPolicyRef::new(id, revision).unwrap()
}

fn catalog_with_bool() -> CapabilityCatalog {
    let mut catalog = CapabilityCatalog::new();
    // Authority class: proven by imported evidence, so a declaration does not
    // need to sit under a functional envelope. This exercises the claim
    // canonicalization path directly.
    catalog
        .define(
            cref("terminal.attach", 1),
            MatcherKind::Bool,
            SecurityClass::Authority,
            CapabilityPolarity::Ability,
        )
        .unwrap();
    catalog
}

fn claim(id: &str, revision: u64, value: CapabilityValue, assurance: Assurance) -> CapabilityClaim {
    CapabilityClaim {
        reference: cref(id, revision),
        value,
        assurance,
        declaration_provenance_ref: None,
    }
}

fn source_with_claims(claims: Vec<CapabilityClaim>) -> SpawnSource {
    SpawnSource {
        source_ref: SpawnSourceRef::new("codex-local", 2).unwrap(),
        adapter_policy: policy_ref("codex-local-adapter", 3),
        lifecycle_modes: [LifecycleMode::Resident].into_iter().collect(),
        continuity_modes: [ContinuityMode::None].into_iter().collect(),
        functional_envelope: BTreeMap::new(),
        claims,
        status: SourceStatus::Active,
    }
}

fn config_with_credential_refs(refs: Vec<CredentialRef>) -> SourceConfig {
    SourceConfig {
        config_ref: SourceConfigRef::new(
            SpawnSourceRef::new("codex-local", 2).unwrap(),
            "deep-reasoning",
            7,
        )
        .unwrap(),
        config_digest: ConfigDigest::new("sha256:abc").unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: refs,
        claims: Vec::new(),
        status: ConfigStatus::Active,
    }
}

#[test]
fn content_digest_is_prefixed_hex() {
    let digest = content_digest(b"hello");
    assert!(digest.starts_with("sha256:"));
    assert_eq!(digest.len(), "sha256:".len() + 64);
    assert_eq!(digest, content_digest(b"hello"));
    assert_ne!(digest, content_digest(b"hello!"));
}

#[test]
fn claim_permutation_and_duplicates_digest_identically() {
    let catalog = catalog_with_bool();
    let present = CapabilityValue::Bool(true);

    let mut a = source_with_claims(vec![
        claim("terminal.attach", 1, present.clone(), Assurance::Declared),
        claim("terminal.attach", 1, present.clone(), Assurance::Declared),
    ]);
    let mut b = source_with_claims(vec![claim(
        "terminal.attach",
        1,
        present.clone(),
        Assurance::Declared,
    )]);

    canonicalize_spawn_source(&mut a, &catalog).unwrap();
    canonicalize_spawn_source(&mut b, &catalog).unwrap();
    assert_eq!(a.claims.len(), 1);
    assert_eq!(
        spawn_source_content_digest(&a),
        spawn_source_content_digest(&b)
    );
}

#[test]
fn enforced_claim_is_preferred_over_declared() {
    let catalog = catalog_with_bool();
    let present = CapabilityValue::Bool(true);
    let mut source = source_with_claims(vec![
        claim("terminal.attach", 1, present.clone(), Assurance::Declared),
        claim("terminal.attach", 1, present.clone(), Assurance::Enforced),
    ]);
    canonicalize_spawn_source(&mut source, &catalog).unwrap();
    assert_eq!(source.claims.len(), 1);
    assert_eq!(source.claims[0].assurance, Assurance::Enforced);
}

#[test]
fn conflicting_claim_values_fail_closed() {
    let mut catalog = CapabilityCatalog::new();
    catalog
        .define(
            cref("tools", 1),
            MatcherKind::Set,
            SecurityClass::Authority,
            CapabilityPolarity::Ability,
        )
        .unwrap();
    let mut source = source_with_claims(vec![
        claim(
            "tools",
            1,
            CapabilityValue::Set(set(&["git"])),
            Assurance::Declared,
        ),
        claim(
            "tools",
            1,
            CapabilityValue::Set(set(&["ripgrep"])),
            Assurance::Declared,
        ),
    ]);
    assert!(canonicalize_spawn_source(&mut source, &catalog).is_err());
}

#[test]
fn bool_false_is_canonical_absence() {
    let catalog = catalog_with_bool();
    let mut with_false = source_with_claims(vec![claim(
        "terminal.attach",
        1,
        CapabilityValue::Bool(false),
        Assurance::Declared,
    )]);
    let mut omitted = source_with_claims(Vec::new());

    canonicalize_spawn_source(&mut with_false, &catalog).unwrap();
    canonicalize_spawn_source(&mut omitted, &catalog).unwrap();
    assert!(with_false.claims.is_empty());
    assert_eq!(
        spawn_source_content_digest(&with_false),
        spawn_source_content_digest(&omitted)
    );
}

#[test]
fn bool_false_for_non_bool_capability_fails_closed() {
    let mut catalog = CapabilityCatalog::new();
    catalog
        .define(
            cref("tools", 1),
            MatcherKind::Set,
            SecurityClass::Functional,
            CapabilityPolarity::Ability,
        )
        .unwrap();
    let mut source = source_with_claims(vec![claim(
        "tools",
        1,
        CapabilityValue::Bool(false),
        Assurance::Declared,
    )]);
    assert!(canonicalize_spawn_source(&mut source, &catalog).is_err());
}

#[test]
fn credential_ref_permutation_digests_identically() {
    let a = CredentialRef::new("vault://a").unwrap();
    let b = CredentialRef::new("vault://b").unwrap();

    let mut left = config_with_credential_refs(vec![b.clone(), a.clone(), a.clone()]);
    let mut right = config_with_credential_refs(vec![a.clone(), b.clone()]);

    canonicalize_credential_refs(&mut left.credential_refs);
    canonicalize_credential_refs(&mut right.credential_refs);
    assert_eq!(left.credential_refs, vec![a, b]);
    assert_eq!(
        source_config_content_digest(&left),
        source_config_content_digest(&right)
    );
}

#[test]
fn different_contract_content_digests_differently() {
    let catalog = CapabilityCatalog::new();
    let mut agent = AgentType {
        type_ref: AgentTypeRef::new("general-reviewer", 1).unwrap(),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace: WorkspaceMode::ReadOnly,
                network: NetworkPolicy::Restricted,
                requires_attempt_isolation: false,
            },
            lifecycle: [LifecycleMode::Resident].into_iter().collect(),
            continuity: ContinuityMode::Logical,
            sandbox_policy: None,
            anchor_constraint: None,
        },
    };
    canonicalize_agent_type(&mut agent, &catalog).unwrap();
    let first = agent_type_content_digest(&agent);

    agent.contract.budget_ceiling = Budget::new(101.0).unwrap();
    let second = agent_type_content_digest(&agent);
    assert_ne!(first, second);
}

#[test]
fn agent_type_digest_ignores_information_function_order() {
    let catalog = CapabilityCatalog::new();
    let make = |functions: Vec<InformationFunction>| {
        let mut agent = AgentType {
            type_ref: AgentTypeRef::new("general-reviewer", 1).unwrap(),
            based_on: None,
            contract: AgentTypeContract {
                allowed_information_functions: functions,
                required_capabilities: BTreeMap::new(),
                affinity: AffinityConstraint::Only(set(&["rust", "sqlite"])),
                budget_ceiling: Budget::new(100.0).unwrap(),
                security: SecurityContract {
                    workspace: WorkspaceMode::ReadOnly,
                    network: NetworkPolicy::Restricted,
                    requires_attempt_isolation: false,
                },
                lifecycle: [LifecycleMode::Resident].into_iter().collect(),
                continuity: ContinuityMode::Logical,
                sandbox_policy: None,
                anchor_constraint: None,
            },
        };
        canonicalize_agent_type(&mut agent, &catalog).unwrap();
        agent_type_content_digest(&agent)
    };
    assert_eq!(
        make(vec![
            InformationFunction::CompressNegative,
            InformationFunction::Expand,
        ]),
        make(vec![
            InformationFunction::Expand,
            InformationFunction::CompressNegative,
        ])
    );
}
