//! B5.1 sandbox policy algebra: lattice laws, deny-all vs conflict, required
//! access, capability projection, and Task-local coarse/fine consistency.

use agentype_agent_contract::*;
use agentype_core::WorkspaceMode;
use std::collections::{BTreeMap, BTreeSet};

fn workspace(components: &[&str]) -> PathSelector {
    PathSelector::workspace(components.iter().copied()).unwrap()
}

fn endpoint(transport: NetworkTransport, host: &str, port: Option<u16>) -> NetworkEndpoint {
    NetworkEndpoint::new(transport, host, port).unwrap()
}

fn simple_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([workspace(&["src"])]),
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::any(),
        ),
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::allowed(["cargo".to_string()]).unwrap(),
        ),
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Tcp,
            "example.com",
            Some(443),
        )]),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn constrained_policy(attempt_isolation: bool) -> NormalizedSandboxPolicy {
    let credential_id = CredentialAuthorizationId::new("build", "service").unwrap();
    let grant = CredentialGrant::new(
        credential_id.clone(),
        ["read".to_string(), "write".to_string()],
        ["registry.example.com".to_string()],
        [CredentialDeliveryMode::BrokeredAccess],
    )
    .unwrap();
    let mut credentials = BTreeMap::new();
    credentials.insert(credential_id, grant);
    let mut resources = BTreeMap::new();
    resources.insert(
        ResourceLimitName::new("open_files").unwrap(),
        ResourceCeiling::new(ResourceScope::Process, ResourceUnit::Count, 64),
    );
    let mut required_values = BTreeMap::new();
    required_values.insert("HTTP_PROXY".to_string(), "http://proxy".to_string());
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([workspace(&["src", "lib"]), workspace(&["tests"])]),
            PathSet::allowed([workspace(&["tests"])]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::allowed([workspace(&["src"])]),
        ),
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::allowed(["cargo".to_string()]).unwrap(),
        ),
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Tcp,
            "registry.example.com",
            Some(443),
        )]),
        environment: EnvironmentPolicy::new(
            NameSet::allowed(["HTTP_PROXY".to_string()]).unwrap(),
            required_values,
        )
        .unwrap(),
        credentials,
        resources,
        working_directory: WorkingDirectory::Required(workspace(&["src"])),
        attempt_isolation,
    })
    .unwrap()
}

fn narrow_policy() -> NormalizedSandboxPolicy {
    let credential_id = CredentialAuthorizationId::new("build", "service").unwrap();
    let grant = CredentialGrant::new(
        credential_id.clone(),
        ["read".to_string()],
        ["registry.example.com".to_string()],
        [CredentialDeliveryMode::BrokeredAccess],
    )
    .unwrap();
    let mut credentials = BTreeMap::new();
    credentials.insert(credential_id, grant);
    let mut resources = BTreeMap::new();
    resources.insert(
        ResourceLimitName::new("open_files").unwrap(),
        ResourceCeiling::new(ResourceScope::Process, ResourceUnit::Count, 64),
    );
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([workspace(&["src"])]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::allowed([workspace(&["src"])]),
        ),
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::allowed(["cargo".to_string()]).unwrap(),
        ),
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Tcp,
            "example.com",
            Some(443),
        )]),
        environment: EnvironmentPolicy::top(),
        credentials,
        resources,
        working_directory: WorkingDirectory::Any,
        attempt_isolation: true,
    })
    .unwrap()
}

fn narrower_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([workspace(&["src", "lib"])]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::deny_all(),
        network: NetworkEndpointSet::deny_all(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

#[test]
fn meet_is_idempotent_commutative_and_associative() {
    let policies = [
        simple_policy(),
        narrow_policy(),
        narrower_policy(),
        constrained_policy(true),
        NormalizedSandboxPolicy::deny_all(),
        NormalizedSandboxPolicy::top(),
    ];
    for policy in &policies {
        assert_eq!(meet(policy, policy).unwrap(), *policy);
        assert_eq!(policy.normalize().unwrap(), *policy);
    }
    for left in &policies {
        for right in &policies {
            match (meet(left, right), meet(right, left)) {
                (Ok(one), Ok(two)) => assert_eq!(one, two),
                (Err(_), Err(_)) => {}
                (one, two) => panic!("meet is not commutative: {one:?} vs {two:?}"),
            }
        }
    }
    let p = &policies[0];
    let q = &policies[1];
    let r = &policies[2];
    assert_eq!(
        meet(&meet(p, q).unwrap(), r).unwrap(),
        meet(p, &meet(q, r).unwrap()).unwrap()
    );
}

#[test]
fn meet_is_a_lower_bound_and_keeps_guarantees() {
    let p = constrained_policy(false);
    let q = constrained_policy(true);
    let meet = meet(&p, &q).unwrap();
    assert!(is_within(&meet, &p));
    assert!(is_within(&meet, &q));
    assert!(is_within(&q, &p));
    // Isolation is a disjunction, not an authority dimension: the meet keeps it
    // but it never makes a policy "wider".
    assert!(meet.attempt_isolation());
    assert!(prove_no_wider(&meet, &p).is_ok());
    // The guarantee is not an authority dimension, so `p` and the meet have the
    // same authority ordering in both directions.
    assert!(is_within(&p, &meet));
    // A genuinely wider policy still fails the proof.
    assert!(prove_no_wider(&simple_policy(), &narrow_policy()).is_err());
    assert!(prove_no_wider(&p, &q).is_ok());
}

#[test]
fn empty_allowed_set_is_ok_deny_all_not_a_conflict() {
    let narrowed = meet(&simple_policy(), &NormalizedSandboxPolicy::deny_all()).unwrap();
    assert!(narrowed.files().read().is_empty());
    assert!(narrowed.files().write().is_empty());
    assert!(narrowed.tools().tools().is_empty());
    assert!(narrowed.network().is_empty());
    assert_eq!(narrowed, NormalizedSandboxPolicy::deny_all());
    assert!(meet(
        &NormalizedSandboxPolicy::top(),
        &NormalizedSandboxPolicy::top()
    )
    .is_ok());

    // `Any` and `Allowed(empty)` are opposite meanings.
    assert!(NormalizedSandboxPolicy::top().files().read().is_any());
    assert!(NormalizedSandboxPolicy::deny_all()
        .files()
        .read()
        .is_empty());
}

#[test]
fn network_meet_intersects_partial_boxes() {
    let left = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Tcp,
            "api.example.com",
            None,
        )]),
        ..Default::default()
    })
    .unwrap();
    let right = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Any,
            "api.example.com",
            Some(443),
        )]),
        ..Default::default()
    })
    .unwrap();
    let meet = meet(&left, &right).unwrap();
    let endpoints = meet.network().endpoints().unwrap();
    assert_eq!(
        endpoints.iter().cloned().collect::<Vec<_>>(),
        vec![endpoint(
            NetworkTransport::Tcp,
            "api.example.com",
            Some(443)
        )]
    );

    // `{TCP, UDP}` at one host/port is the same rule as `ANY` and MUST
    // normalize to the single canonical spelling.
    let merged = NetworkEndpointSet::allowed([
        endpoint(NetworkTransport::Tcp, "example.com", Some(80)),
        endpoint(NetworkTransport::Udp, "example.com", Some(80)),
    ]);
    assert_eq!(
        merged
            .endpoints()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec![endpoint(NetworkTransport::Any, "example.com", Some(80))]
    );
}

#[test]
fn credential_absence_narrows_to_nothing() {
    let granted = constrained_policy(false);
    let without = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        credentials: BTreeMap::new(),
        ..granted.parts().clone()
    })
    .unwrap();
    let meet = meet(&granted, &without).unwrap();
    assert!(meet.credentials().is_empty());
    assert!(is_within(&meet, &granted));
    assert!(is_within(&meet, &without));
    assert!(!is_within(&granted, &meet));
}

#[test]
fn resource_meet_keeps_stricter_value_and_rejects_incompatible_scope() {
    let make = |scope, limit| {
        let mut resources = BTreeMap::new();
        resources.insert(
            ResourceLimitName::new("memory").unwrap(),
            ResourceCeiling::new(scope, ResourceUnit::Bytes, limit),
        );
        NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
            resources,
            ..Default::default()
        })
        .unwrap()
    };
    let merged = meet(
        &make(ResourceScope::Process, 4096),
        &make(ResourceScope::Process, 1024),
    )
    .unwrap();
    assert_eq!(
        merged
            .resources()
            .get(&ResourceLimitName::new("memory").unwrap())
            .unwrap()
            .limit(),
        1024
    );
    assert!(meet(
        &make(ResourceScope::Process, 4096),
        &make(ResourceScope::ExecutionDomain, 4096),
    )
    .is_err());
}

#[test]
fn structural_conflicts_are_rejected() {
    let working_a = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        working_directory: WorkingDirectory::Required(workspace(&["src"])),
        ..Default::default()
    })
    .unwrap();
    let working_b = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        working_directory: WorkingDirectory::Required(workspace(&["tests"])),
        ..Default::default()
    })
    .unwrap();
    assert!(meet(&working_a, &working_b).is_err());

    let environment = |value: &str| {
        NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
            environment: EnvironmentPolicy::new(
                NameSet::any(),
                [("PROXY".to_string(), value.to_string())]
                    .into_iter()
                    .collect(),
            )
            .unwrap(),
            ..Default::default()
        })
        .unwrap()
    };
    assert!(meet(&environment("a"), &environment("b")).is_err());
    assert_eq!(
        meet(&environment("a"), &environment("a")).unwrap(),
        environment("a")
    );
}

#[test]
fn required_access_checks_the_effective_projection() {
    let required = RequiredAccess::new(
        [FileOperationClass::Read],
        ["git".to_string()],
        [endpoint(NetworkTransport::Tcp, "example.com", Some(443))],
        ["HTTP_PROXY".to_string()],
        [],
    )
    .unwrap();
    let granting = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        environment: EnvironmentPolicy::new(
            NameSet::allowed(["HTTP_PROXY".to_string()]).unwrap(),
            BTreeMap::new(),
        )
        .unwrap(),
        ..simple_policy().parts().clone()
    })
    .unwrap();
    let access = PolicyAccess::from_policy(&granting);
    assert!(check_required_access(&required, &access).is_ok());

    let denied = PolicyAccess::from_policy(&NormalizedSandboxPolicy::deny_all());
    let deficit = check_required_access(&required, &denied).unwrap_err();
    assert_eq!(deficit.reasons().len(), 4);

    let mut denied_write = granting.clone();
    let mut parts = denied_write.parts().clone();
    parts.files = FilePolicy::new(
        parts.files.read().clone(),
        PathSet::deny_all(),
        parts.files.execute().clone(),
        parts.files.delete().clone(),
        parts.files.visible().clone(),
    );
    denied_write = NormalizedSandboxPolicy::from_parts(parts).unwrap();
    let write_required = RequiredAccess::new([FileOperationClass::Write], [], [], [], []).unwrap();
    assert!(
        check_required_access(&write_required, &PolicyAccess::from_policy(&denied_write)).is_err()
    );
}

fn projected_definition(security_class: SecurityClass) -> (CapabilityRef, CapabilityDefinition) {
    let members = [(
        "test_runner".to_string(),
        PolicyOperation::ToolCommand("test_runner".to_string()),
    )]
    .into_iter()
    .collect();
    let definition = CapabilityDefinition {
        matcher_kind: MatcherKind::Set,
        security_class,
        polarity: CapabilityPolarity::Ability,
        policy_projection: Some(
            PolicyProjection::new(PolicyAccessDimension::ToolCommandSet, members).unwrap(),
        ),
    };
    (CapabilityRef::new("sandbox.tools", 1).unwrap(), definition)
}

#[test]
fn capability_projection_fails_closed_on_unmapped_members() {
    let (_, definition) = projected_definition(SecurityClass::Sandbox);
    let projected = project_capability_access(
        &definition,
        &CapabilityValue::Set(["test_runner".to_string()].into_iter().collect()),
    )
    .unwrap();
    assert_eq!(
        projected.iter().cloned().collect::<Vec<_>>(),
        vec![PolicyOperation::ToolCommand("test_runner".to_string())]
    );

    let unmapped = project_capability_access(
        &definition,
        &CapabilityValue::Set(
            ["test_runner".to_string(), "unknown".to_string()]
                .into_iter()
                .collect(),
        ),
    );
    assert!(unmapped.is_err());

    let required = RequiredAccess::from_operations(projected).unwrap();
    let access = PolicyAccess::from_policy(&simple_policy());
    assert!(check_required_access(&required, &access).is_err());
    let mut parts = simple_policy().parts().clone();
    parts.tools = ToolPolicy::new(
        NameSet::allowed(["test_runner".to_string()]).unwrap(),
        NameSet::deny_all(),
    );
    let allowed = NormalizedSandboxPolicy::from_parts(parts).unwrap();
    assert!(check_required_access(&required, &PolicyAccess::from_policy(&allowed)).is_ok());
}

#[test]
fn capability_without_projection_contributes_nothing() {
    let definition = CapabilityDefinition {
        matcher_kind: MatcherKind::Set,
        security_class: SecurityClass::Sandbox,
        polarity: CapabilityPolarity::Ability,
        policy_projection: None,
    };
    let projected = project_capability_access(
        &definition,
        &CapabilityValue::Set(["anything".to_string()].into_iter().collect()),
    )
    .unwrap();
    assert!(projected.is_empty());
}

#[test]
fn task_local_coarse_fine_rejects_contradictions() {
    let writable = simple_policy();
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        Some(&writable)
    )
    .is_err());
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
        Some(&writable)
    )
    .is_ok());
    assert!(
        validate_task_local_coarse_fine(WorkspaceMode::Write, NetworkPolicy::Restricted, None)
            .is_ok()
    );

    let no_write = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::any(),
        ),
        ..Default::default()
    })
    .unwrap();
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        Some(&no_write)
    )
    .is_ok());
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
        Some(&no_write)
    )
    .is_err());

    // `Delete` is a content-modifying operation: a delete-only policy does not
    // project to `ReadOnly`, so a `ReadOnly` Task cannot author it.
    let delete_only = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::any(),
            PathSet::any(),
            PathSet::any(),
        ),
        ..Default::default()
    })
    .unwrap();
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        Some(&delete_only)
    )
    .is_err());
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
        Some(&delete_only)
    )
    .is_ok());

    let networked = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::any(),
        ),
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Tcp,
            "example.com",
            Some(443),
        )]),
        ..Default::default()
    })
    .unwrap();
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Disabled,
        Some(&networked)
    )
    .is_err());
    assert!(validate_task_local_coarse_fine(
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        Some(&networked)
    )
    .is_ok());
}

#[test]
fn derivation_identity_is_order_insensitive() {
    let output = constrained_policy(false);
    let required = RequiredAccess::empty();
    let a = SecurityDerivation::new(
        [
            PolicyInput::of(None, &simple_policy()),
            PolicyInput::authored(
                SandboxPolicyRef::new("policy", 1).unwrap(),
                &constrained_policy(true),
            ),
        ],
        &output,
        &required,
    )
    .unwrap();
    let b = SecurityDerivation::new(
        [
            PolicyInput::authored(
                SandboxPolicyRef::new("policy", 1).unwrap(),
                &constrained_policy(true),
            ),
            PolicyInput::of(None, &simple_policy()),
        ],
        &output,
        &required,
    )
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(
        security_derivation_content_digest(&a),
        security_derivation_content_digest(&b)
    );
    assert_eq!(a.output_digest(), sandbox_policy_content_digest(&output));
    assert_eq!(
        a.required_access_digest(),
        required_access_content_digest(&required)
    );
    assert_eq!(
        a.normalization_version(),
        SANDBOX_POLICY_NORMALIZATION_VERSION
    );
    assert_eq!(
        a.meet_algorithm_version(),
        SANDBOX_POLICY_MEET_ALGORITHM_VERSION
    );
    assert!(SecurityDerivation::new([], &output, &required).is_err());
}

#[test]
fn security_contract_mode_spellings_are_stable() {
    assert_eq!(SecurityContractMode::CoarseV1.as_str(), "COARSE_V1");
    assert_eq!(SecurityContractMode::SecurityV1.as_str(), "SECURITY_V1");
    assert_eq!(
        SecurityContractMode::parse("SECURITY_V1"),
        Some(SecurityContractMode::SecurityV1)
    );
    assert_eq!(SecurityContractMode::parse("security_v1"), None);
}

#[test]
fn unusable_credential_grants_normalize_to_absence() {
    let id = CredentialAuthorizationId::new("build", "service").unwrap();
    let unusable = [
        CredentialGrant::new(
            id.clone(),
            Vec::<String>::new(),
            ["worker".to_string()],
            [CredentialDeliveryMode::BrokeredAccess],
        )
        .unwrap(),
        CredentialGrant::new(
            id.clone(),
            ["read".to_string()],
            Vec::<String>::new(),
            [CredentialDeliveryMode::BrokeredAccess],
        )
        .unwrap(),
        CredentialGrant::new(
            id.clone(),
            ["read".to_string()],
            ["worker".to_string()],
            Vec::<CredentialDeliveryMode>::new(),
        )
        .unwrap(),
    ];
    let bare = simple_policy();
    for grant in unusable {
        assert!(!grant.is_usable());
        let mut parts = bare.parts().clone();
        parts.credentials = BTreeMap::from([(id.clone(), grant)]);
        let with_grant = NormalizedSandboxPolicy::from_parts(parts).unwrap();
        // An unusable grant authorizes nothing, so it is not part of the policy's
        // authorization semantics: it must not change the canonical identity.
        assert!(with_grant.credentials().is_empty());
        assert_eq!(
            canonical_sandbox_policy_bytes(&with_grant),
            canonical_sandbox_policy_bytes(&bare)
        );
        assert_eq!(
            sandbox_policy_content_digest(&with_grant),
            sandbox_policy_content_digest(&bare)
        );
        assert!(is_within(&with_grant, &bare));
        assert!(is_within(&bare, &with_grant));
    }
}

#[test]
fn declared_collections_stay_canonical() {
    let mut tools = BTreeSet::new();
    tools.insert("git".to_string());
    let set = NameSet::allowed(tools.iter().cloned()).unwrap();
    assert_eq!(set.names().unwrap().len(), 1);
}

fn network_only<I>(endpoints: I) -> NormalizedSandboxPolicy
where
    I: IntoIterator<Item = NetworkEndpoint>,
{
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed(endpoints),
        ..Default::default()
    })
    .unwrap()
}

fn top_network_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::Any,
        ..Default::default()
    })
    .unwrap()
}

fn required_network<I>(endpoints: I) -> RequiredAccess
where
    I: IntoIterator<Item = NetworkEndpoint>,
{
    RequiredAccess::new(
        Vec::<FileOperationClass>::new(),
        Vec::<String>::new(),
        endpoints,
        Vec::<String>::new(),
        Vec::<CredentialAuthorizationId>::new(),
    )
    .unwrap()
}

/// The union of several outer rules may cover one inner rule even when no single
/// outer rule does. Containment is over allowed *sets*, not rule-by-rule
/// subsumption.
#[test]
fn network_containment_compares_the_union_of_outer_rules() {
    let any_transport = network_only([endpoint(NetworkTransport::Any, "*", Some(443))]);
    let split = network_only([
        endpoint(NetworkTransport::Tcp, "*", Some(443)),
        endpoint(NetworkTransport::Udp, "*", None),
    ]);

    assert!(is_within(&any_transport, &split));
    assert!(!is_within(&split, &any_transport));

    // `meet` already normalizes the two transport slices back to the `ANY` rule,
    // so the failure of the old single-rule check also broke the GLB law.
    let met = meet(&any_transport, &split).unwrap();
    assert_eq!(
        met.network()
            .endpoints()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec![endpoint(NetworkTransport::Any, "*", Some(443))]
    );
    assert!(is_within(&met, &any_transport));
    assert!(is_within(&met, &split));

    // A union of exact hosts never covers the `*` host atom.
    let wildcard_host = network_only([endpoint(NetworkTransport::Tcp, "*", Some(443))]);
    let exact_hosts = network_only([
        endpoint(NetworkTransport::Tcp, "a.example.com", Some(443)),
        endpoint(NetworkTransport::Tcp, "b.example.com", Some(443)),
    ]);
    assert!(!is_within(&wildcard_host, &exact_hosts));

    // Neither does a union of exact ports cover the "any port" atom.
    let any_port = network_only([endpoint(NetworkTransport::Tcp, "a.example.com", None)]);
    let exact_ports = network_only([
        endpoint(NetworkTransport::Tcp, "a.example.com", Some(443)),
        endpoint(NetworkTransport::Tcp, "a.example.com", Some(8443)),
    ]);
    assert!(!is_within(&any_port, &exact_ports));
}

/// `is_within` is a preorder and `meet` is its greatest lower bound over the
/// finite transport/host/port grammar, including wildcard and mixed rules.
#[test]
fn network_order_is_a_preorder_with_a_greatest_lower_bound() {
    let mut policies = vec![
        network_only([]),
        top_network_policy(),
        // Universal endpoint spelled as an explicit rule (semantically top).
        network_only([endpoint(NetworkTransport::Any, "*", None)]),
        // A rule that only the union of several rules can cover.
        network_only([
            endpoint(NetworkTransport::Tcp, "*", Some(443)),
            endpoint(NetworkTransport::Udp, "*", None),
        ]),
        // `{TCP, UDP}` normalizes to `ANY` at one host/port.
        network_only([
            endpoint(NetworkTransport::Tcp, "api.example.com", Some(443)),
            endpoint(NetworkTransport::Udp, "api.example.com", Some(443)),
        ]),
    ];
    for rule in [
        endpoint(NetworkTransport::Any, "*", Some(443)),
        endpoint(NetworkTransport::Tcp, "*", Some(443)),
        endpoint(NetworkTransport::Udp, "*", None),
        endpoint(NetworkTransport::Any, "api.example.com", Some(443)),
        endpoint(NetworkTransport::Tcp, "api.example.com", None),
        endpoint(NetworkTransport::Udp, "api.example.com", Some(80)),
        endpoint(NetworkTransport::Any, "db.example.com", Some(5432)),
    ] {
        policies.push(network_only([rule]));
    }

    for policy in &policies {
        assert!(is_within(policy, policy), "reflexivity failed: {policy:?}");
    }

    for left in &policies {
        for right in &policies {
            let met = meet(left, right).unwrap();
            assert!(is_within(&met, left), "meet is not a lower bound: {met:?}");
            assert!(is_within(&met, right), "meet is not a lower bound: {met:?}");
        }
    }

    for a in &policies {
        for b in &policies {
            for c in &policies {
                if is_within(a, b) && is_within(b, c) {
                    assert!(is_within(a, c), "transitivity failed: {a:?} {b:?} {c:?}");
                }
            }
        }
    }

    for lower in &policies {
        for left in &policies {
            for right in &policies {
                if is_within(lower, left) && is_within(lower, right) {
                    assert!(
                        is_within(lower, &meet(left, right).unwrap()),
                        "meet is not the greatest lower bound: {lower:?} {left:?} {right:?}"
                    );
                }
            }
        }
    }
}

/// Requirement satisfaction MUST use the same union containment as the policy
/// order. A required `ANY:*:443` rule is covered by the union of a TCP rule and
/// a UDP rule, even though neither outer rule alone covers it.
#[test]
fn required_access_uses_network_union_containment() {
    let split = network_only([
        endpoint(NetworkTransport::Tcp, "*", Some(443)),
        endpoint(NetworkTransport::Udp, "*", None),
    ]);
    let access = PolicyAccess::from_policy(&split);
    let https_any_transport = endpoint(NetworkTransport::Any, "*", Some(443));

    assert!(access.allows_endpoint(&https_any_transport));
    let required = required_network([https_any_transport]);
    assert!(check_required_access(&required, &access).is_ok());

    // The split union does not cover every port on both transports.
    let every_port = required_network([endpoint(NetworkTransport::Any, "*", None)]);
    let deficit = check_required_access(&every_port, &access).unwrap_err();
    assert_eq!(deficit.reasons().len(), 1);
    assert!(deficit.reasons()[0].contains("network endpoint"));
}

/// `Any` and the explicit universal rule are one canonical value. Keeping both
/// spellings would make the coarse projection and the policy order disagree.
#[test]
fn explicit_universal_network_rule_normalizes_to_any() {
    let universal = endpoint(NetworkTransport::Any, "*", None);
    let explicit = network_only([universal.clone()]);
    let implicit = top_network_policy();

    assert_eq!(explicit, implicit);
    assert_eq!(explicit.network(), &NetworkEndpointSet::Any);
    assert!(explicit.network().is_any());
    assert_eq!(project_network(&explicit), NetworkPolicy::Enabled);
    assert_eq!(project_network(&implicit), NetworkPolicy::Enabled);
    assert!(is_within(&explicit, &implicit));
    assert!(is_within(&implicit, &explicit));

    // A universal rule subsumes every narrower rule, and the normalized bytes
    // and digest are exactly the `Any` ones.
    let mixed = network_only([
        universal,
        endpoint(NetworkTransport::Tcp, "registry.example.com", Some(443)),
    ]);
    assert_eq!(mixed, implicit);
    assert_eq!(
        canonical_sandbox_policy_bytes(&explicit),
        canonical_sandbox_policy_bytes(&implicit)
    );
    assert_eq!(
        sandbox_policy_content_digest(&explicit),
        sandbox_policy_content_digest(&implicit)
    );
    assert_eq!(
        sandbox_policy_content_digest(&explicit),
        "sha256:262727a2a171589de44c15671defe0e08f13810122ae9c949b48f359a27b2693"
    );
}

/// ADR-0013 §7 fixes visibility as directory observability, not an access
/// precondition. A deny-all visible set therefore does not erase a writable
/// operation region; visibility remains an independent ordering dimension.
#[test]
fn visibility_is_independent_of_file_operation_authority() {
    let hidden = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::deny_all(),
            PathSet::allowed([workspace(&["src"])]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        ..Default::default()
    })
    .unwrap();
    let visible = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::deny_all(),
            PathSet::allowed([workspace(&["src"])]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::any(),
        ),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(project_workspace(&hidden), WorkspaceMode::Write);
    assert_eq!(
        project_security_contract(&hidden).workspace,
        WorkspaceMode::Write
    );
    let access = PolicyAccess::from_policy(&hidden);
    assert!(access.allows_file_operation(FileOperationClass::Write));
    let required = RequiredAccess::new(
        [FileOperationClass::Write],
        Vec::<String>::new(),
        Vec::<NetworkEndpoint>::new(),
        Vec::<String>::new(),
        Vec::<CredentialAuthorizationId>::new(),
    )
    .unwrap();
    assert!(check_required_access(&required, &access).is_ok());

    // Visibility still orders independently: the hidden policy is narrower.
    assert!(is_within(&hidden, &visible));
    assert!(!is_within(&visible, &hidden));
}

/// The constructor enforces the bounded v1 exact-host grammar before any policy
/// reaches the containment algorithm.
#[test]
fn network_endpoint_hosts_are_bounded_tokens() {
    let too_long = "a".repeat(MAX_NETWORK_HOST_LEN + 1);
    for host in [
        too_long.as_str(),
        "bad\0host",
        "bad\nhost",
        "bad\thost",
        "bad host",
        "*.example.com",
        "example.*",
        "**",
        "*foo",
        "foo*",
        "é.example.com",
        "...",
        "example.com:443",
        "127.0.0.1:443",
        "https://example.com",
    ] {
        assert!(
            NetworkEndpoint::new(NetworkTransport::Tcp, host, Some(443)).is_err(),
            "host {host:?} must be rejected"
        );
    }
    for host in [
        "*",
        "example.com",
        "REGISTRY.example.com",
        "127.0.0.1",
        "2001:db8::1",
        "a_b.example.com",
    ] {
        assert!(
            NetworkEndpoint::new(NetworkTransport::Tcp, host, Some(443)).is_ok(),
            "host {host:?} must be accepted"
        );
    }
    assert!(NetworkEndpoint::new(NetworkTransport::Tcp, "example.com", Some(0)).is_err());
}

/// Policy and requirement collections have the same finite bound, so no
/// caller can make the host/port partition enumeration unbounded.
#[test]
fn network_endpoint_collections_are_bounded() {
    let build = |count: usize| {
        (0..count)
            .map(|index| {
                NetworkEndpoint::new(
                    NetworkTransport::Tcp,
                    format!("host{index}.example.com"),
                    Some(443),
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };

    let at_limit = build(MAX_NETWORK_ENDPOINTS);
    assert!(NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed(at_limit),
        ..Default::default()
    })
    .is_ok());

    let over_limit = build(MAX_NETWORK_ENDPOINTS + 1);
    assert!(NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed(over_limit.iter().cloned()),
        ..Default::default()
    })
    .is_err());
    assert!(RequiredAccess::new(
        Vec::<FileOperationClass>::new(),
        Vec::<String>::new(),
        over_limit,
        Vec::<String>::new(),
        Vec::<CredentialAuthorizationId>::new(),
    )
    .is_err());
}

/// The Task's coarse ceiling must constrain the authority used by admission
/// checks, but MUST NOT rewrite the fine-derived binding identity: only the
/// constrained dimensions are replaced.
#[test]
fn task_coarse_authority_narrows_only_the_coarse_dimensions() {
    let full = constrained_policy(false);

    // `Write` + `Restricted` is the identity on the authority view.
    assert_eq!(
        task_coarse_authority(&full, WorkspaceMode::Write, NetworkPolicy::Restricted).unwrap(),
        full
    );

    let read_only =
        task_coarse_authority(&full, WorkspaceMode::ReadOnly, NetworkPolicy::Restricted).unwrap();
    assert!(read_only.files().write().is_empty());
    assert_eq!(read_only.files().read(), full.files().read());
    assert_eq!(read_only.files().execute(), full.files().execute());
    assert_eq!(read_only.files().visible(), full.files().visible());
    assert_eq!(read_only.network(), full.network());
    assert_eq!(read_only.credentials(), full.credentials());
    assert_eq!(read_only.resources(), full.resources());
    assert_eq!(read_only.environment(), full.environment());
    assert!(is_within(&read_only, &full));
    assert_eq!(project_workspace(&read_only), WorkspaceMode::ReadOnly);

    let no_network =
        task_coarse_authority(&full, WorkspaceMode::Write, NetworkPolicy::Disabled).unwrap();
    assert_eq!(no_network.files().write(), full.files().write());
    assert!(no_network.network().endpoints().unwrap().is_empty());
    assert_eq!(project_network(&no_network), NetworkPolicy::Disabled);

    let both =
        task_coarse_authority(&full, WorkspaceMode::ReadOnly, NetworkPolicy::Disabled).unwrap();
    assert!(both.files().write().is_empty());
    assert!(both.network().endpoints().unwrap().is_empty());
    assert_eq!(
        both,
        task_coarse_authority(&both, WorkspaceMode::ReadOnly, NetworkPolicy::Disabled).unwrap()
    );

    // The narrowed view is what admission may use for positive floors, so a
    // required write or endpoint is unsatisfiable under the matching coarse
    // ceiling even though the fine layer still shows it.
    let write_required = RequiredAccess::new([FileOperationClass::Write], [], [], [], []).unwrap();
    assert!(
        check_required_access(&write_required, &PolicyAccess::from_policy(&read_only)).is_err()
    );
    let endpoint = endpoint(NetworkTransport::Tcp, "registry.example.com", Some(443));
    let endpoint_required = required_network([endpoint]);
    assert!(
        check_required_access(&endpoint_required, &PolicyAccess::from_policy(&read_only)).is_ok()
    );
    assert!(
        check_required_access(&endpoint_required, &PolicyAccess::from_policy(&no_network)).is_err()
    );

    // `Delete` is content modification, so it projects to `Write` and is removed
    // by a `ReadOnly` Task ceiling just like write.
    let files = full.files();
    let mut deleting_parts = full.parts().clone();
    deleting_parts.files = FilePolicy::new(
        files.read().clone(),
        PathSet::deny_all(),
        files.execute().clone(),
        PathSet::allowed([workspace(&["src", "lib"])]),
        files.visible().clone(),
    );
    let deleting = NormalizedSandboxPolicy::from_parts(deleting_parts).unwrap();
    assert_eq!(project_workspace(&deleting), WorkspaceMode::Write);
    let read_only_deleting = task_coarse_authority(
        &deleting,
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    )
    .unwrap();
    assert!(read_only_deleting.files().delete().is_empty());
    let delete_required =
        RequiredAccess::new([FileOperationClass::Delete], [], [], [], []).unwrap();
    assert!(check_required_access(
        &delete_required,
        &PolicyAccess::from_policy(&read_only_deleting)
    )
    .is_err());
    assert!(check_required_access(&delete_required, &PolicyAccess::from_policy(&deleting)).is_ok());
}

/// The builder enforces bounded path depth and component length before any
/// selector can enter a policy.
#[test]
fn path_selectors_are_bounded_in_depth_and_component_length() {
    let at_limit = (0..MAX_PATH_COMPONENTS).map(|_| "d").collect::<Vec<_>>();
    assert!(PathSelector::workspace(at_limit).is_ok());

    let too_deep = (0..=MAX_PATH_COMPONENTS).map(|_| "d").collect::<Vec<_>>();
    assert!(PathSelector::workspace(too_deep).is_err());

    let long = "a".repeat(MAX_PATH_COMPONENT_LEN);
    assert!(PathSelector::workspace([long.as_str()]).is_ok());

    let too_long = "a".repeat(MAX_PATH_COMPONENT_LEN + 1);
    assert!(PathSelector::workspace([too_long.as_str()]).is_err());
}

/// Path sets and whole policies are bounded before/around the pairwise cover
/// scan, so an authored policy cannot force unbounded normalization work.
#[test]
fn path_selector_collections_are_bounded() {
    let disjoint = |count: usize| {
        (0..count)
            .map(|index| {
                PathSelector::workspace([format!("p{index}")]).expect("a short component is legal")
            })
            .collect::<Vec<_>>()
    };

    let at_limit = disjoint(MAX_PATH_SELECTORS);
    assert!(PathSet::try_allowed(at_limit.iter().cloned()).is_ok());
    assert!(PathSet::try_allowed(disjoint(MAX_PATH_SELECTORS + 1)).is_err());

    // The per-set bound is also enforced by `from_parts` for an infallibly built
    // set, so a caller cannot smuggle one past the policy boundary.
    let oversized = PathSet::allowed(disjoint(MAX_PATH_SELECTORS + 1));
    assert!(NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            oversized,
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        ..Default::default()
    })
    .is_err());

    // Two full sets reach the aggregate bound exactly; a third exceeds it.
    let full_set = || PathSet::allowed(disjoint(MAX_PATH_SELECTORS));
    let at_policy_limit = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            full_set(),
            full_set(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        ..Default::default()
    });
    assert!(at_policy_limit.is_ok());
    let over_policy_limit = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            full_set(),
            full_set(),
            full_set(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        ..Default::default()
    });
    assert!(over_policy_limit.is_err());
}

/// A legal pair of 256-rule policies can propose 65,536 intersection candidates.
/// The result is unrepresentable (more than 256 distinct rules), so admission
/// must fail closed after bounded work rather than enter a global quadratic scan.
#[test]
fn a_bounded_network_meet_rejects_an_unrepresentable_product() {
    let left = network_only((0..MAX_NETWORK_ENDPOINTS).map(|index| {
        NetworkEndpoint::new(
            NetworkTransport::Any,
            format!("host{index}.example.com"),
            None,
        )
        .unwrap()
    }));
    let right = network_only((0..MAX_NETWORK_ENDPOINTS).map(|index| {
        NetworkEndpoint::new(
            NetworkTransport::Any,
            "*",
            Some(u16::try_from(index + 1).unwrap()),
        )
        .unwrap()
    }));

    assert!(meet(&left, &right).is_err());
}

/// The same 256 × 256 product can normalize back to the 256-rule bound. This
/// must remain admissible: the bound applies to the represented result, not to
/// the intermediate candidate count.
#[test]
fn a_large_network_product_that_simplifies_is_admissible() {
    let left = network_only((0..MAX_NETWORK_ENDPOINTS).map(|index| {
        NetworkEndpoint::new(
            NetworkTransport::Tcp,
            format!("host{index}.example.com"),
            None,
        )
        .unwrap()
    }));
    let mut rules = (0..MAX_NETWORK_ENDPOINTS - 1)
        .map(|index| {
            NetworkEndpoint::new(
                NetworkTransport::Any,
                "*",
                Some(u16::try_from(index + 1).unwrap()),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    rules.push(NetworkEndpoint::new(NetworkTransport::Tcp, "*", None).unwrap());
    let right = network_only(rules);

    let met = meet(&left, &right).unwrap();
    let endpoints = met.network().endpoints().unwrap();
    assert_eq!(endpoints.len(), MAX_NETWORK_ENDPOINTS);
    assert!(endpoints.iter().all(|endpoint| endpoint.port().is_none()));
}

/// Exhaustive equivalence check for the grouped endpoint normalizer against the
/// straightforward pairwise definition over a small domain. This keeps the
/// `O(n log n)` implementation from drifting from the rule-subsumption semantics.
#[test]
fn grouped_network_normalization_matches_pairwise_subsumption() {
    fn naive_allowed(endpoints: impl IntoIterator<Item = NetworkEndpoint>) -> NetworkEndpointSet {
        let mut all: BTreeSet<NetworkEndpoint> = endpoints.into_iter().collect();
        let keys: BTreeSet<(String, Option<u16>)> = all
            .iter()
            .map(|endpoint| (endpoint.host().to_string(), endpoint.port()))
            .collect();
        for (host, port) in keys {
            let tcp = NetworkEndpoint::new(NetworkTransport::Tcp, host.clone(), port).unwrap();
            let udp = NetworkEndpoint::new(NetworkTransport::Udp, host.clone(), port).unwrap();
            if all.contains(&tcp) && all.contains(&udp) {
                all.remove(&tcp);
                all.remove(&udp);
                all.insert(NetworkEndpoint::new(NetworkTransport::Any, host, port).unwrap());
            }
        }
        if all.iter().any(|endpoint| {
            endpoint.transport() == NetworkTransport::Any
                && endpoint.host() == "*"
                && endpoint.port().is_none()
        }) {
            return NetworkEndpointSet::Any;
        }
        let all = all.into_iter().collect::<Vec<_>>();
        let kept = all
            .iter()
            .enumerate()
            .filter(|(index, endpoint)| {
                !all.iter()
                    .enumerate()
                    .any(|(other_index, other)| other_index != *index && other.covers(endpoint))
            })
            .map(|(_, endpoint)| endpoint.clone())
            .collect::<BTreeSet<_>>();
        NetworkEndpointSet::Allowed(kept)
    }

    let domain = [
        NetworkTransport::Tcp,
        NetworkTransport::Udp,
        NetworkTransport::Any,
    ]
    .into_iter()
    .flat_map(|transport| {
        ["*", "a.example.com"].into_iter().flat_map(move |host| {
            [None, Some(1)]
                .into_iter()
                .map(move |port| NetworkEndpoint::new(transport, host, port).unwrap())
        })
    })
    .collect::<Vec<_>>();
    assert_eq!(domain.len(), 12);

    for subset in 0u16..(1 << domain.len()) {
        let selected = domain
            .iter()
            .enumerate()
            .filter(|(index, _)| subset & (1 << index) != 0)
            .map(|(_, endpoint)| endpoint.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            NetworkEndpointSet::allowed(selected.clone()),
            naive_allowed(selected),
            "subset {subset:#x}"
        );
    }
}

/// P2-1: the RequiredAccess network floor is one set, so it is decided by one
/// union-containment pass. The per-endpoint scan is only the rejecting path's
/// diagnostic walk, not 256 independent partition enumerations.
#[test]
fn required_access_checks_the_whole_network_floor_once() {
    let allowed = std::iter::once(endpoint(NetworkTransport::Tcp, "*", None))
        .chain((1..=255u16).map(|port| endpoint(NetworkTransport::Udp, "*", Some(port))));
    let access = PolicyAccess::from_policy(&network_only(allowed));

    // No single allowed rule covers `ANY:*:p`, but the union does.
    let covered =
        required_network((1..=255u16).map(|port| endpoint(NetworkTransport::Any, "*", Some(port))));
    assert!(check_required_access(&covered, &access).is_ok());

    // A 256-rule floor with exactly one uncovered member reports only that
    // member after the aggregate check fails.
    let missing =
        required_network((1..=256u16).map(|port| endpoint(NetworkTransport::Any, "*", Some(port))));
    let error = check_required_access(&missing, &access).unwrap_err();
    assert_eq!(error.reasons().len(), 1, "{error}");
    assert!(error.to_string().contains("*:256"), "{error}");
}

/// P2-2: the name, map, facet and whole-document dimensions are bounded, so a
/// policy cannot be enlarged without limit by legal individual members.
#[test]
fn policy_sets_maps_and_documents_are_bounded() {
    let over_names = (0..=MAX_NAME_SET_MEMBERS)
        .map(|index| format!("tool{index}"))
        .collect::<Vec<_>>();
    assert!(NameSet::allowed(over_names.clone()).is_err());

    let required = over_names
        .iter()
        .map(|name| (name.clone(), "value".to_string()))
        .collect::<BTreeMap<_, _>>();
    assert!(EnvironmentPolicy::new(NameSet::any(), required).is_err());

    let over_facet = (0..=MAX_CREDENTIAL_FACET_MEMBERS)
        .map(|index| format!("use{index}"))
        .collect::<Vec<_>>();
    let id = CredentialAuthorizationId::new("build", "service").unwrap();
    assert!(CredentialGrant::new(
        id,
        over_facet,
        ["registry.example.com".to_string()],
        [CredentialDeliveryMode::BrokeredAccess],
    )
    .is_err());

    let mut credentials = BTreeMap::new();
    for index in 0..=MAX_POLICY_MAP_ENTRIES {
        let id = CredentialAuthorizationId::new("build", format!("service{index}")).unwrap();
        credentials.insert(
            id.clone(),
            CredentialGrant::new(
                id,
                ["read".to_string()],
                ["registry.example.com".to_string()],
                [CredentialDeliveryMode::BrokeredAccess],
            )
            .unwrap(),
        );
    }
    assert!(NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        credentials,
        ..Default::default()
    })
    .is_err());

    let mut resources = BTreeMap::new();
    for index in 0..=MAX_POLICY_MAP_ENTRIES {
        resources.insert(
            ResourceLimitName::new(format!("limit{index}")).unwrap(),
            ResourceCeiling::new(ResourceScope::Process, ResourceUnit::Count, 1),
        );
    }
    assert!(NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        resources,
        ..Default::default()
    })
    .is_err());

    let over_required = (0..=MAX_NAME_SET_MEMBERS)
        .map(|index| format!("name{index}"))
        .collect::<Vec<_>>();
    assert!(RequiredAccess::new(
        Vec::<FileOperationClass>::new(),
        over_required,
        Vec::<NetworkEndpoint>::new(),
        Vec::<String>::new(),
        Vec::<CredentialAuthorizationId>::new(),
    )
    .is_err());

    // The decoder enforces the whole-document budget before parsing it.
    let oversized = format!("{}{}", " ".repeat(MAX_SANDBOX_POLICY_BYTES), "{}");
    let error = sandbox_policy_from_canonical_json(&oversized).unwrap_err();
    assert!(error.to_string().contains("bytes"), "{error}");
}

/// P2-3: v1 has one `ToolCommandSet` namespace. A name allowed as a tool
/// satisfies a required tool-or-command operation and vice versa; the physical
/// compiler cannot later split the same string into two meanings.
#[test]
fn tool_and_command_allowlists_share_one_operation_namespace() {
    let tool_only = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::deny_all(),
        ),
        ..Default::default()
    })
    .unwrap();
    let command_only = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        tools: ToolPolicy::new(
            NameSet::deny_all(),
            NameSet::allowed(["git".to_string()]).unwrap(),
        ),
        ..Default::default()
    })
    .unwrap();
    let required = RequiredAccess::new(
        Vec::<FileOperationClass>::new(),
        ["git".to_string()],
        Vec::<NetworkEndpoint>::new(),
        Vec::<String>::new(),
        Vec::<CredentialAuthorizationId>::new(),
    )
    .unwrap();

    assert!(check_required_access(&required, &PolicyAccess::from_policy(&tool_only)).is_ok());
    assert!(check_required_access(&required, &PolicyAccess::from_policy(&command_only)).is_ok());
}

/// P1 regression: `PolicyAccess`, `meet` and `is_within` agree on the one v1
/// `ToolCommandSet`. Because normalization stores the effective union in
/// `tools` and deny-all in `commands`, the two authored spellings are mutually
/// comparable, have one canonical identity, and their GLB still allows the
/// shared operation.
#[test]
fn tool_and_command_allowlists_have_one_glb() {
    let tool_spelling = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::deny_all(),
        ),
        ..Default::default()
    })
    .unwrap();
    let command_spelling = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        tools: ToolPolicy::new(
            NameSet::deny_all(),
            NameSet::allowed(["git".to_string()]).unwrap(),
        ),
        ..Default::default()
    })
    .unwrap();

    // The two spellings are the same canonical policy: equal ordering in both
    // directions and identical bytes/digest.
    assert!(is_within(&tool_spelling, &command_spelling));
    assert!(is_within(&command_spelling, &tool_spelling));
    assert_eq!(tool_spelling, command_spelling);
    assert_eq!(
        canonical_sandbox_policy_bytes(&tool_spelling),
        canonical_sandbox_policy_bytes(&command_spelling)
    );
    assert_eq!(
        sandbox_policy_content_digest(&tool_spelling),
        sandbox_policy_content_digest(&command_spelling)
    );

    let met = meet(&tool_spelling, &command_spelling).unwrap();
    assert!(met.tools().allows("git"));
    assert!(is_within(&met, &tool_spelling));
    assert!(is_within(&met, &command_spelling));
    // The meet keeps the operation instead of intersecting two empty fields.
    assert!(PolicyAccess::from_policy(&met).allows_tool_or_command("git"));

    // The GLB is over the effective union, not over the two storage fields:
    // `{git, cargo}` against `{git} ∪ {cargo}` intersects to `{git, cargo}`.
    let both_in_tools = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string(), "cargo".to_string()]).unwrap(),
            NameSet::deny_all(),
        ),
        ..Default::default()
    })
    .unwrap();
    let split_fields = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::allowed(["cargo".to_string()]).unwrap(),
        ),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(both_in_tools, split_fields);
    let met = meet(&both_in_tools, &split_fields).unwrap();
    assert!(met.tools().allows("git"));
    assert!(met.tools().allows("cargo"));
    assert_eq!(met, both_in_tools);
}

/// P2 regression: the v1 network canonical form is deterministic for one rule
/// syntax, but it is not union-minimal. Two rule sets may permit exactly the
/// same operations while producing different bytes and digests. Permission
/// equivalence therefore MUST be decided by `is_within`/`meet`, never by digest
/// equality.
#[test]
fn network_canonical_form_is_deterministic_but_not_union_minimal() {
    let split_udp = network_only([
        endpoint(NetworkTransport::Tcp, "*", None),
        endpoint(NetworkTransport::Udp, "*", Some(443)),
    ]);
    let any_transport = network_only([
        endpoint(NetworkTransport::Tcp, "*", None),
        endpoint(NetworkTransport::Any, "*", Some(443)),
    ]);

    // `TCP:*:*` already covers the TCP half of `ANY:*:443`, so the two sets
    // permit exactly the same network operations.
    assert!(is_within(&split_udp, &any_transport));
    assert!(is_within(&any_transport, &split_udp));

    // They are not collapsed to one canonical spelling, so their identities
    // differ even though their authority does not.
    assert_ne!(split_udp, any_transport);
    assert_ne!(
        canonical_sandbox_policy_bytes(&split_udp),
        canonical_sandbox_policy_bytes(&any_transport)
    );
    assert_ne!(
        sandbox_policy_content_digest(&split_udp),
        sandbox_policy_content_digest(&any_transport)
    );

    // The same rule syntax always produces the same canonical identity.
    let rebuilt = network_only([
        endpoint(NetworkTransport::Tcp, "*", None),
        endpoint(NetworkTransport::Udp, "*", Some(443)),
    ]);
    assert_eq!(
        sandbox_policy_content_digest(&split_udp),
        sandbox_policy_content_digest(&rebuilt)
    );

    // A meet of the two equivalent spellings stays below both.
    let met = meet(&split_udp, &any_transport).unwrap();
    assert!(is_within(&met, &split_udp));
    assert!(is_within(&met, &any_transport));
}

/// P2 performance regression: a legal 256×256 containment query with many
/// wildcard-host rules and distinct ports must finish under a bounded budget.
/// The exact rule of thumb is pinned in ADR-0013: the finite partition has at
/// most `2 · 513 · 513` atoms, and each rule projects once to the host/port
/// atoms it covers instead of rescanning the whole partition per rule.
#[test]
fn adversarial_network_containment_stays_bounded() {
    use std::time::{Duration, Instant};

    // The outer set allows one distinct port on every host, so no two rules
    // cover each other and the host partition is genuinely enumerated.
    let allowed =
        network_only((1..=256u16).map(|port| endpoint(NetworkTransport::Any, "*", Some(port))));
    // The inner set adds 256 distinct exact hosts, enlarging the host partition
    // while every rule remains covered by the corresponding wildcard-host rule.
    let inner = network_only((1..=256u16).map(|port| {
        endpoint(
            NetworkTransport::Any,
            &format!("host{port}.example.com"),
            Some(port),
        )
    }));

    let started = Instant::now();
    assert!(is_within(&inner, &allowed));
    let elapsed = started.elapsed();
    eprintln!("adversarial 256x256 network containment: {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(10),
        "network containment exceeded its 10s budget: {elapsed:?}"
    );

    // The reverse direction is not contained, and it must reject promptly too.
    assert!(!is_within(&allowed, &inner));
}
