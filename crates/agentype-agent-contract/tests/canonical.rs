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

fn claim_with_provenance(
    id: &str,
    revision: u64,
    value: CapabilityValue,
    assurance: Assurance,
    provenance: &str,
) -> CapabilityClaim {
    CapabilityClaim {
        reference: cref(id, revision),
        value,
        assurance,
        declaration_provenance_ref: Some(provenance.to_string()),
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
        source_config_content_digest(&left, None),
        source_config_content_digest(&right, None)
    );
}

#[test]
fn ambiguous_claim_provenance_fails_closed() {
    let catalog = catalog_with_bool();
    let value = CapabilityValue::Bool(true);
    let mut source = source_with_claims(vec![
        claim_with_provenance(
            "terminal.attach",
            1,
            value.clone(),
            Assurance::Declared,
            "a",
        ),
        claim_with_provenance(
            "terminal.attach",
            1,
            value.clone(),
            Assurance::Declared,
            "b",
        ),
    ]);
    assert!(canonicalize_spawn_source(&mut source, &catalog).is_err());

    // Two ENFORCED declarations with different provenance are equally ambiguous.
    let mut enforced = source_with_claims(vec![
        claim_with_provenance(
            "terminal.attach",
            1,
            value.clone(),
            Assurance::Enforced,
            "a",
        ),
        claim_with_provenance(
            "terminal.attach",
            1,
            value.clone(),
            Assurance::Enforced,
            "b",
        ),
    ]);
    assert!(canonicalize_spawn_source(&mut enforced, &catalog).is_err());
}

#[test]
fn claim_provenance_permutation_is_deterministic() {
    let catalog = catalog_with_bool();
    let value = CapabilityValue::Bool(true);
    let provenance = "codex-driver@4";
    let mut left = source_with_claims(vec![
        claim_with_provenance(
            "terminal.attach",
            1,
            value.clone(),
            Assurance::Declared,
            provenance,
        ),
        claim_with_provenance(
            "terminal.attach",
            1,
            value.clone(),
            Assurance::Declared,
            provenance,
        ),
    ]);
    let mut right = source_with_claims(vec![claim_with_provenance(
        "terminal.attach",
        1,
        value.clone(),
        Assurance::Declared,
        provenance,
    )]);
    canonicalize_spawn_source(&mut left, &catalog).unwrap();
    canonicalize_spawn_source(&mut right, &catalog).unwrap();
    assert_eq!(left.claims, right.claims);
    assert_eq!(
        spawn_source_content_digest(&left),
        spawn_source_content_digest(&right)
    );
}

#[test]
fn golden_digest_vectors_are_stable() {
    // Frozen vectors: if a serde_json feature or the canonical encoder changes
    // the byte format, these break instead of silently rotating every digest.
    let catalog = catalog_with_bool();
    let mut source = source_with_claims(vec![claim(
        "terminal.attach",
        1,
        CapabilityValue::Bool(true),
        Assurance::Enforced,
    )]);
    canonicalize_spawn_source(&mut source, &catalog).unwrap();
    let mut config = config_with_credential_refs(vec![CredentialRef::new("vault://a").unwrap()]);
    canonicalize_credential_refs(&mut config.credential_refs);

    let definition = CapabilityDefinition {
        matcher_kind: MatcherKind::Set,
        security_class: SecurityClass::Sandbox,
        polarity: CapabilityPolarity::Restriction,
    };
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
    canonicalize_agent_type(&mut agent, &CapabilityCatalog::new()).unwrap();
    let policy = AdapterBindingPolicy::new(
        policy_ref("codex-local-adapter", 3),
        "codex_cli",
        "workstation-primary",
        PhysicalSafety::new(
            false,
            vec![WorkspaceMode::ReadOnly],
            [NetworkPolicy::Disabled].into_iter().collect(),
        )
        .unwrap(),
        ConfigStatus::Active,
    )
    .unwrap();
    // Every canonical document kind has at least one pinned vector, so a
    // future encoder change cannot silently drift the durable protocol without
    // bumping `CANONICAL_FORMAT_VERSION`.
    assert_eq!(
        capability_definition_content_digest(&cref("tools", 1), &definition),
        "sha256:81de3142a1a2265ddcfee9331e5a443d774b75bbec1e9cf1cfea62e34f338df1"
    );
    assert_eq!(
        agent_type_content_digest(&agent),
        "sha256:963a2a17b9910abeb98e376c57af2202c05d625e1ffbf67e114fe93675e4af51"
    );
    assert_eq!(
        spawn_source_content_digest(&source),
        "sha256:49e88707e0a3d8517c64a7007996a0ba8876380c5d666f2a20906564a594096e"
    );
    assert_eq!(
        source_config_content_digest(&config, None),
        "sha256:d62e85b99b73d78c77fe233db692c4308d0ed30e6e865911ad200424beff8bf9"
    );
    assert_eq!(
        source_config_content_digest(&config, Some("file:///etc/codex.toml")),
        "sha256:4f47dbd3a52d8c08fec6e1cc792802cc39d4bec76b0b19885480825b8fa58330"
    );
    assert_eq!(
        adapter_binding_policy_content_digest(&policy),
        "sha256:2e5b9369c5de2e1b5a730b25d14220f2a631b005ed24dcddea3ca8036876e48a"
    );
    assert_eq!(
        canonical_json_body_digest(&serde_json::json!({"model": "x", "provider": "y"})),
        "sha256:5b7bbbd084793f932fb7916a5f6c1d2adc4d0a49b19c48060eda1d979392568e"
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

#[test]
fn adapter_binding_policy_blank_fields_fail_closed() {
    let safety =
        PhysicalSafety::new(false, vec![WorkspaceMode::ReadOnly], BTreeSet::new()).unwrap();
    let blank_kind = AdapterBindingPolicy {
        policy_ref: policy_ref("p", 1),
        adapter_kind: "".into(),
        binding_ref: "workstation-primary".into(),
        required_safety: safety.clone(),
        status: ConfigStatus::Active,
    };
    assert!(blank_kind.validate().is_err());
    // A document the encoder happily produces for an invalid record MUST still be
    // rejected by the canonical decoder.
    let json = String::from_utf8(canonical_adapter_binding_policy_bytes(&blank_kind)).unwrap();
    assert!(adapter_binding_policy_from_canonical_json(&json, ConfigStatus::Active).is_err());

    let blank_ref = AdapterBindingPolicy {
        policy_ref: policy_ref("p", 1),
        adapter_kind: "codex_cli".into(),
        binding_ref: "   ".into(),
        required_safety: safety,
        status: ConfigStatus::Active,
    };
    assert!(blank_ref.validate().is_err());
    let json = String::from_utf8(canonical_adapter_binding_policy_bytes(&blank_ref)).unwrap();
    assert!(adapter_binding_policy_from_canonical_json(&json, ConfigStatus::Active).is_err());
}
