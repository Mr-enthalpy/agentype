//! B5.1 golden vectors for normalization, `meet`, `RequiredAccess`, and
//! `SecurityDerivation`.
//!
//! Each vector is keyed by the exact `(canonical_version, algebra_version)` it
//! was produced under. Changing normalization or intersection semantics MUST
//! bump a version and add new vectors; it MUST NOT change these values, because
//! durable rows are read under the versions recorded with them (ADR-0013 §11).

use agentype_agent_contract::*;
use std::collections::BTreeMap;

fn workspace(components: &[&str]) -> PathSelector {
    PathSelector::workspace(components.iter().copied()).unwrap()
}

fn endpoint(transport: NetworkTransport, host: &str, port: Option<u16>) -> NetworkEndpoint {
    NetworkEndpoint::new(transport, host, port).unwrap()
}

fn credential_id() -> CredentialAuthorizationId {
    CredentialAuthorizationId::new("build", "service").unwrap()
}

/// The canonical document of the deny-all policy, pinned byte for byte. This is
/// the envelope every durable policy row must reproduce; a change here requires
/// a new `SANDBOX_POLICY_SCHEMA_VERSION`.
const DENY_ALL_CANONICAL_JSON: &str = r#"{"canonical":"agentype-contract/1","kind":"SANDBOX_POLICY","policy":{"attempt_isolation":false,"credentials":[],"environment":{"allowed_names":{"kind":"ALLOWED","names":[]},"required_values":{}},"files":{"delete":{"kind":"ALLOWED","selectors":[]},"execute":{"kind":"ALLOWED","selectors":[]},"read":{"kind":"ALLOWED","selectors":[]},"visible":{"kind":"ALLOWED","selectors":[]},"write":{"kind":"ALLOWED","selectors":[]}},"network":{"endpoints":[],"kind":"ALLOWED"},"resources":[],"tools":{"commands":{"kind":"ALLOWED","names":[]},"tools":{"kind":"ALLOWED","names":[]}},"working_directory":{"kind":"ANY"}},"sandbox_policy_schema_version":1}"#;

/// A policy that exercises every dimension with exactly one source of meaning.
fn base_policy() -> NormalizedSandboxPolicy {
    let grant = CredentialGrant::new(
        credential_id(),
        ["read".to_string(), "write".to_string()],
        ["registry.example.com".to_string()],
        [CredentialDeliveryMode::BrokeredAccess],
    )
    .unwrap();
    let mut credentials = BTreeMap::new();
    credentials.insert(credential_id(), grant);
    let mut resources = BTreeMap::new();
    resources.insert(
        ResourceLimitName::new("open_files").unwrap(),
        ResourceCeiling::new(ResourceScope::Process, ResourceUnit::Count, 64),
    );
    let mut required_values = BTreeMap::new();
    required_values.insert("HTTP_PROXY".to_string(), "http://proxy".to_string());
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([workspace(&["src"])]),
            PathSet::allowed([workspace(&["tests"])]),
            PathSet::allowed([workspace(&["tools"])]),
            PathSet::deny_all(),
            PathSet::allowed([workspace(&[])]),
        ),
        tools: ToolPolicy::new(
            NameSet::allowed(["git".to_string()]).unwrap(),
            NameSet::allowed(["cargo".to_string()]).unwrap(),
        ),
        network: NetworkEndpointSet::allowed([
            endpoint(NetworkTransport::Tcp, "example.com", None),
            endpoint(NetworkTransport::Any, "registry.example.com", Some(443)),
        ]),
        environment: EnvironmentPolicy::new(
            NameSet::allowed(["HTTP_PROXY".to_string()]).unwrap(),
            required_values,
        )
        .unwrap(),
        credentials,
        resources,
        working_directory: WorkingDirectory::Required(workspace(&["src"])),
        attempt_isolation: true,
    })
    .unwrap()
}

/// A strictly narrower policy: subsumed path, deny-all tools, a port-narrowed
/// version of the same host, and no credential grant.
fn narrow_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([workspace(&["src", "lib"])]),
            PathSet::allowed([workspace(&["tests", "unit"])]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::allowed([workspace(&["src", "lib"])]),
        ),
        tools: ToolPolicy::deny_all(),
        network: NetworkEndpointSet::allowed([
            endpoint(NetworkTransport::Tcp, "example.com", Some(80)),
            endpoint(NetworkTransport::Any, "registry.example.com", Some(443)),
        ]),
        environment: EnvironmentPolicy::new(
            NameSet::allowed(["HTTP_PROXY".to_string()]).unwrap(),
            [("HTTP_PROXY".to_string(), "http://proxy".to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: true,
    })
    .unwrap()
}

fn required_access() -> RequiredAccess {
    RequiredAccess::new(
        [FileOperationClass::Read, FileOperationClass::Write],
        ["git".to_string(), "cargo".to_string()],
        [endpoint(NetworkTransport::Tcp, "example.com", Some(80))],
        ["HTTP_PROXY".to_string()],
        [credential_id()],
    )
    .unwrap()
}

#[test]
fn golden_vectors_are_keyed_by_the_current_versions() {
    assert_eq!(
        SandboxPolicyVersion::CURRENT,
        SandboxPolicyVersion::new(
            SANDBOX_POLICY_SCHEMA_VERSION,
            SANDBOX_POLICY_MEET_ALGORITHM_VERSION
        )
    );
    assert!(SandboxPolicyVersion::CURRENT.is_current());
}

#[test]
fn normalization_golden_vector() {
    assert_eq!(
        String::from_utf8(canonical_sandbox_policy_bytes(
            &NormalizedSandboxPolicy::deny_all()
        ))
        .unwrap(),
        DENY_ALL_CANONICAL_JSON
    );
    // Decoding the pinned document reproduces the same bytes: the golden vector
    // is a round-trip witness, not just a digest.
    let decoded = sandbox_policy_from_canonical_json(DENY_ALL_CANONICAL_JSON).unwrap();
    assert_eq!(decoded, NormalizedSandboxPolicy::deny_all());

    assert_eq!(
        sandbox_policy_content_digest(&base_policy()),
        "sha256:e0781d0a58fc48014f6f80bbeec3b239a9789b32d7628dea519597f7a1fa33df"
    );
    assert_eq!(
        sandbox_policy_content_digest(&narrow_policy()),
        "sha256:3dbe907df3f883b8bb2b3ada3893fbc7f7b2cb4d998698c98cae67115dbf5d97"
    );
    assert_eq!(
        sandbox_policy_content_digest(&NormalizedSandboxPolicy::deny_all()),
        "sha256:baf4ee91391f553c464cf556e3e5e9b529c3ff0d570aa8380551f9f70878ea7e"
    );
    assert_eq!(
        sandbox_policy_content_digest(&NormalizedSandboxPolicy::top()),
        "sha256:262727a2a171589de44c15671defe0e08f13810122ae9c949b48f359a27b2693"
    );
}

#[test]
fn meet_golden_vector() {
    let met = meet(&base_policy(), &narrow_policy()).unwrap();
    assert_eq!(
        sandbox_policy_content_digest(&met),
        "sha256:7a7a149e01e976aedd274bac5979dba8503285390bb0fa7d2779c9c28dad25f7"
    );
    assert!(is_within(&met, &base_policy()));
    assert!(is_within(&met, &narrow_policy()));

    // A partial endpoint box intersected with a host-wide rule keeps the exact
    // port, and the resulting content is pinned.
    let host_wide = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Any,
            "api.example.com",
            None,
        )]),
        ..Default::default()
    })
    .unwrap();
    let port_wide = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        network: NetworkEndpointSet::allowed([endpoint(
            NetworkTransport::Tcp,
            "api.example.com",
            Some(8443),
        )]),
        ..Default::default()
    })
    .unwrap();
    let boxed = meet(&host_wide, &port_wide).unwrap();
    assert_eq!(
        sandbox_policy_content_digest(&boxed),
        "sha256:d9a6d4d3cab4066fdd68ddb6583aad1d3521794e7d58871b13330ea88a35dc59"
    );
    assert_eq!(
        boxed
            .network()
            .endpoints()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec![endpoint(
            NetworkTransport::Tcp,
            "api.example.com",
            Some(8443)
        )]
    );
}

#[test]
fn required_access_golden_vector() {
    let required = required_access();
    assert_eq!(
        required_access_content_digest(&required),
        "sha256:04d326f86f5bf0f2ac261f176aead8726b3db21a6e6ccee74e1b0e17c4208b5b"
    );
    assert_eq!(
        required_access_content_digest(&RequiredAccess::empty()),
        "sha256:82c35a1175926e1d8f4d27d669cd490965ce80583cf3d1a580d91fb43363eca7"
    );
    assert_eq!(required.file_operations().len(), 2);
    assert_eq!(required.tools_commands().len(), 2);
    assert_eq!(required.network_endpoints().len(), 1);
    assert_eq!(required.environment_names().len(), 1);
    assert_eq!(required.credentials().len(), 1);
}

#[test]
fn required_access_checks_against_the_pinned_policy() {
    let access = PolicyAccess::from_policy(&base_policy());
    assert!(check_required_access(&required_access(), &access).is_ok());

    let with_uncovered_endpoint = RequiredAccess::new(
        [FileOperationClass::Read, FileOperationClass::Write],
        ["git".to_string(), "cargo".to_string()],
        [
            endpoint(NetworkTransport::Tcp, "example.com", Some(80)),
            endpoint(NetworkTransport::Tcp, "blocked.example.com", Some(443)),
        ],
        ["HTTP_PROXY".to_string()],
        [credential_id()],
    )
    .unwrap();
    let deficit = check_required_access(&with_uncovered_endpoint, &access).unwrap_err();
    assert_eq!(deficit.reasons().len(), 1);
    assert!(deficit.reasons()[0].contains("blocked.example.com"));

    let narrowed = meet(&base_policy(), &narrow_policy()).unwrap();
    assert!(
        check_required_access(&required_access(), &PolicyAccess::from_policy(&narrowed)).is_err()
    );
}

#[test]
fn derivation_golden_vector() {
    let derivation = SecurityDerivation::new(
        [
            PolicyInput::derived(&base_policy()),
            PolicyInput::authored(
                SandboxPolicyRef::new("policy", 1).unwrap(),
                &narrow_policy(),
            ),
        ],
        &narrow_policy(),
        &required_access(),
    )
    .unwrap();
    assert_eq!(
        security_derivation_content_digest(&derivation),
        "sha256:7ad27401835b1e364f2d8d3cafd61467ce4b84d43a98196ec53ec6ae4556937d"
    );
    assert_eq!(
        derivation.output_digest(),
        sandbox_policy_content_digest(&narrow_policy())
    );
    assert_eq!(
        derivation.required_access_digest(),
        required_access_content_digest(&required_access())
    );
    assert_eq!(
        derivation.normalization_version(),
        SANDBOX_POLICY_NORMALIZATION_VERSION
    );
    assert_eq!(
        derivation.meet_algorithm_version(),
        SANDBOX_POLICY_MEET_ALGORITHM_VERSION
    );
}

#[test]
fn unknown_version_pairs_fail_closed() {
    for pair in [
        SandboxPolicyVersion::new(0, 1),
        SandboxPolicyVersion::new(1, 0),
        SandboxPolicyVersion::new(1, 2),
        SandboxPolicyVersion::new(2, 1),
    ] {
        match pair.ensure_supported() {
            Err(ContractError::UnsupportedPolicyVersion {
                canonical_version,
                algebra_version,
            }) => {
                assert_eq!(canonical_version, pair.canonical_version());
                assert_eq!(algebra_version, pair.algebra_version());
            }
            other => panic!("expected a version rejection, got {other:?}"),
        }
    }
    assert_eq!(
        SandboxPolicyVersion::CURRENT.ensure_supported().unwrap(),
        SandboxPolicyVersion::CURRENT
    );
}
