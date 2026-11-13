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
            PathSet::any(),
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

    let networked = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::any(),
            PathSet::any(),
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
