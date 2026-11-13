//! M6-B.5 schema v9 effective-security catalog conformance:
//! authored sandbox policies, capability canonical versions, and the atomic
//! AgentType / SpawnSource `SECURITY_V1` publication.
//!
//! These tests exercise the durable boundary only. B5.1 does not compile or
//! execute a security plan; they prove that a policy can be submitted and
//! re-read as exactly the same canonical contract, that a revision's security
//! mode is part of its immutable identity, and that a corrupted or rewritten row
//! fails closed.

use agentype_agent_contract::{
    AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint, AgentType, AgentTypeContract,
    AgentTypeRef, Budget, CapabilityDefinition, CapabilityPolarity, CapabilityRef, CapabilityValue,
    ConfigStatus, ContinuityMode, EnforcementProfileRef, EnvironmentPolicy, FileOperationClass,
    FilePolicy, LifecycleMode, MatcherKind, NetworkEndpoint, NetworkEndpointSet, NetworkPolicy,
    NetworkTransport, NormalizedSandboxPolicy, PathSelector, PathSet, PhysicalSafety,
    PolicyAccessDimension, PolicyInput, PolicyOperation, PolicyProjection, SandboxPolicyParts,
    SandboxPolicyRef, SecurityClass, SecurityContract, SecurityContractMode, SourceStatus,
    SpawnSource, SpawnSourceRef, SpawnSourceSecurityDeclaration, ToolPolicy, WorkingDirectory,
};
use agentype_core::{Clock, InformationFunction, ManualClock, WorkspaceMode};
use agentype_storage_sqlite::Kernel;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

struct Db {
    dir: PathBuf,
    path: PathBuf,
}

impl Db {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("agentype-b5-{tag}-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            path: dir.join("scheduler.db"),
            dir,
        }
    }

    fn kernel(&self) -> Kernel {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        Kernel::open(&self.path, clock, 10.0, MAX_BYTES).unwrap()
    }

    fn raw(&self) -> Connection {
        let conn = Connection::open(&self.path).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn memory_kernel() -> Kernel {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap()
}

fn policy_ref(id: &str, revision: u64) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, revision).unwrap()
}

fn profile() -> EnforcementProfileRef {
    EnforcementProfileRef::new("agentype-enforcement", "policy-only", 1).unwrap()
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn adapter_policy() -> AdapterBindingPolicy {
    AdapterBindingPolicy::new(
        AdapterPolicyRef::new("codex-local-adapter", 3).unwrap(),
        "codex_cli",
        "workstation-primary",
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
        .unwrap(),
        ConfigStatus::Active,
    )
    .unwrap()
}

fn agent_type() -> AgentType {
    AgentType {
        type_ref: type_ref("coder", 1),
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
    }
}

fn spawn_source() -> SpawnSource {
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

fn source_at(revision: u64) -> SpawnSource {
    let mut source = spawn_source();
    source.source_ref = SpawnSourceRef::new("codex-local", revision).unwrap();
    source
}

/// A policy that projects to exactly `ReadOnly` workspace and `Restricted`
/// network: the coarse contract of [`agent_type`].
fn read_only_restricted_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: NetworkEndpointSet::allowed([NetworkEndpoint::new(
            NetworkTransport::Tcp,
            "example.com",
            Some(443),
        )
        .unwrap()]),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn publish_adapter_policy(kernel: &Kernel) {
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
}

/// Publish one `SECURITY_V1` AgentType revision and return its canonical value.
fn publish_secure_agent_type(kernel: &Kernel) -> AgentType {
    let reference = policy_ref("agent-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_restricted_policy())
        .unwrap();
    let mut agent = agent_type();
    agent.type_ref = type_ref("secure-coder", 1);
    agent.contract.sandbox_policy = Some(reference);
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent
}

/// Drop every trigger so a test can simulate a bypassing direct-SQL writer.
fn drop_schema_guards(conn: &Connection) {
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    for name in names {
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}"))
            .unwrap();
    }
}

fn update_without_guards(path: &Path, sql: &str) {
    let conn = Connection::open(path).unwrap();
    drop_schema_guards(&conn);
    conn.execute(sql, []).unwrap();
}

#[test]
fn sandbox_policy_round_trips_and_is_immutable() {
    let kernel = memory_kernel();
    let reference = policy_ref("workstation-policy", 1);
    let policy = NormalizedSandboxPolicy::top();

    let digest = kernel.publish_sandbox_policy(&reference, &policy).unwrap();
    assert_eq!(
        kernel.get_sandbox_policy(&reference).unwrap(),
        Some(policy.clone())
    );
    // Republishing the same exact revision is content-idempotent.
    assert_eq!(
        kernel.publish_sandbox_policy(&reference, &policy).unwrap(),
        digest
    );

    // A different document for an already-published revision fails closed.
    let error = kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::deny_all())
        .unwrap_err();
    assert!(error.to_string().contains("immutable"), "{error}");
    assert_eq!(kernel.get_sandbox_policy(&reference).unwrap(), Some(policy));
}

#[test]
fn sandbox_policy_content_corruption_fails_closed() {
    let db = Db::new("policy-corruption");
    let kernel = db.kernel();
    let reference = policy_ref("workstation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::top())
        .unwrap();

    update_without_guards(
        &db.path,
        "UPDATE sandbox_policy_revisions SET content_json = content_json || ' '",
    );

    let error = kernel.get_sandbox_policy(&reference).unwrap_err();
    assert!(error.to_string().contains("digest"), "{error}");
}

#[test]
fn sandbox_policy_version_drift_fails_closed() {
    let db = Db::new("policy-version");
    let kernel = db.kernel();
    let reference = policy_ref("workstation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::top())
        .unwrap();

    // A row re-labelled with a version this build does not implement must not
    // be decoded under current semantics, even if the bytes still parse.
    update_without_guards(
        &db.path,
        "UPDATE sandbox_policy_revisions SET canonical_version = 2",
    );

    let error = kernel.get_sandbox_policy(&reference).unwrap_err();
    assert!(error.to_string().contains("canonical_version"), "{error}");
}

#[test]
fn capability_definition_canonical_version_round_trips() {
    let kernel = memory_kernel();
    let v1 = CapabilityRef::new("terminal.attach", 1).unwrap();
    let v2 = CapabilityRef::new("filesystem.read", 1).unwrap();

    kernel
        .publish_capability_definition(
            &v1,
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Bool,
                security_class: SecurityClass::Functional,
                polarity: CapabilityPolarity::Ability,
                policy_projection: None,
            },
        )
        .unwrap();
    let projection = PolicyProjection::new(
        PolicyAccessDimension::FileOperationClass,
        [(
            "read".to_string(),
            PolicyOperation::FileOperation(FileOperationClass::Read),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    kernel
        .publish_capability_definition(
            &v2,
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();

    let catalog = kernel.load_capability_catalog().unwrap();
    assert_eq!(catalog.get(&v1).unwrap().canonical_version(), 1);
    assert_eq!(catalog.get(&v2).unwrap().canonical_version(), 2);
}

#[test]
fn capability_definition_version_column_corruption_fails_closed() {
    let db = Db::new("cap-version");
    let kernel = db.kernel();
    let reference = CapabilityRef::new("terminal.attach", 1).unwrap();
    kernel
        .publish_capability_definition(
            &reference,
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Bool,
                security_class: SecurityClass::Functional,
                polarity: CapabilityPolarity::Ability,
                policy_projection: None,
            },
        )
        .unwrap();

    update_without_guards(
        &db.path,
        "UPDATE capability_definitions SET canonical_version = 2",
    );

    let error = kernel.load_capability_catalog().unwrap_err();
    assert!(error.to_string().contains("canonical_version"), "{error}");
}

#[test]
fn capability_publication_rejects_an_invalid_projection_before_persisting() {
    let kernel = memory_kernel();
    let reference = CapabilityRef::new("filesystem.write", 1).unwrap();
    let projection = PolicyProjection::new(
        PolicyAccessDimension::FileOperationClass,
        [(
            "src".to_string(),
            PolicyOperation::FileOperation(FileOperationClass::Write),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();

    // A hand-built definition can bypass `PolicyProjection`'s constructor. The
    // publisher must re-run the whole-definition invariant before its digest and
    // insert: otherwise an authoritative read would fail closed forever on an
    // immutable row.
    let invalid = [
        CapabilityDefinition {
            matcher_kind: MatcherKind::Bool,
            security_class: SecurityClass::Sandbox,
            polarity: CapabilityPolarity::Ability,
            policy_projection: Some(projection.clone()),
        },
        CapabilityDefinition {
            matcher_kind: MatcherKind::Set,
            security_class: SecurityClass::Sandbox,
            polarity: CapabilityPolarity::Restriction,
            policy_projection: Some(projection.clone()),
        },
        CapabilityDefinition {
            matcher_kind: MatcherKind::Set,
            security_class: SecurityClass::Functional,
            polarity: CapabilityPolarity::Ability,
            policy_projection: Some(projection.clone()),
        },
    ];
    for definition in &invalid {
        let error = kernel
            .publish_capability_definition(&reference, definition)
            .unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("projection"),
            "{error}"
        );
    }

    // Reusing the same immutable identity proves no rejected definition became
    // durable, and the following catalog read must still succeed.
    kernel
        .publish_capability_definition(
            &reference,
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();
    let catalog = kernel.load_capability_catalog().unwrap();
    assert_eq!(catalog.get(&reference).unwrap().canonical_version(), 2);
}

#[test]
fn sandbox_policy_version_must_fit_u32() {
    let db = Db::new("policy-version-width");
    let kernel = db.kernel();
    let reference = policy_ref("workstation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::top())
        .unwrap();

    // `2^32 + 1` truncates to `1` under an `as u32` cast, which would make a
    // corrupted row compare equal to the real v1 document.
    update_without_guards(
        &db.path,
        "UPDATE sandbox_policy_revisions SET canonical_version = 4294967297",
    );

    let error = kernel.get_sandbox_policy(&reference).unwrap_err();
    assert!(error.to_string().contains("32-bit"), "{error}");
}

#[test]
fn agent_type_security_mode_is_published_atomically_and_write_once() {
    let db = Db::new("agent-declaration");
    let kernel = db.kernel();
    publish_adapter_policy(&kernel);

    // Absence is the frozen COARSE_V1 contract.
    let coarse = agent_type();
    kernel.publish_agent_type(&coarse).unwrap();
    assert_eq!(
        kernel.agent_type_security_mode(&coarse.type_ref).unwrap(),
        SecurityContractMode::CoarseV1
    );

    // A SECURITY_V1 revision is published together with its mode; there is no
    // post-publication declaration API.
    let secured = publish_secure_agent_type(&kernel);
    assert_eq!(
        kernel.agent_type_security_mode(&secured.type_ref).unwrap(),
        SecurityContractMode::SecurityV1
    );
    // Repeating the same mode is an idempotent republish.
    kernel
        .publish_agent_type_with_security(&secured, SecurityContractMode::SecurityV1)
        .unwrap();
    // Reinterpreting the same revision as COARSE_V1 requires a new revision.
    let error = kernel.publish_agent_type(&secured).unwrap_err();
    assert!(
        error.to_string().contains("requires a new revision"),
        "{error}"
    );

    let conn = db.raw();
    // The declaration is immutable at the storage boundary.
    assert!(conn
        .execute(
            "UPDATE agent_type_security_declarations SET security_contract_mode='SECURITY_V1'",
            []
        )
        .is_err());
    assert!(conn
        .execute("DELETE FROM agent_type_security_declarations", [])
        .is_err());
    // `INSERT OR REPLACE` cannot resurrect the same identity, and COARSE_V1 has
    // no durable spelling to smuggle back in.
    assert!(conn
        .execute(
            "INSERT OR REPLACE INTO agent_type_security_declarations(
                 type_id, revision, security_contract_mode, created_at)
             SELECT type_id, revision, 'COARSE_V1', created_at
             FROM agent_type_security_declarations",
            [],
        )
        .is_err());
}

#[test]
fn security_agent_type_requires_a_policy_exactly_projected_and_claim_consistent() {
    let kernel = memory_kernel();
    publish_adapter_policy(&kernel);

    // No authored policy: the coarse/fine relation cannot be proved.
    let mut bare = agent_type();
    bare.type_ref = type_ref("no-policy", 1);
    let error = kernel
        .publish_agent_type_with_security(&bare, SecurityContractMode::SecurityV1)
        .unwrap_err();
    assert!(error.to_string().contains("must author"), "{error}");

    // A policy whose workspace/network projection contradicts the coarse
    // contract is rejected rather than silently treated as a second grant.
    let reference = policy_ref("agent-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_restricted_policy())
        .unwrap();
    let mut mismatched = agent_type();
    mismatched.type_ref = type_ref("mismatched", 1);
    mismatched.contract.sandbox_policy = Some(reference.clone());
    mismatched.contract.security.workspace = WorkspaceMode::Write;
    let error = kernel
        .publish_agent_type_with_security(&mismatched, SecurityContractMode::SecurityV1)
        .unwrap_err();
    assert!(error.to_string().contains("contradicts"), "{error}");

    // A formally mapped capability claim the same policy denies is an invalid
    // SECURITY_V1 AgentType, not a later candidate-local mismatch.
    let write_capability = CapabilityRef::new("filesystem.write", 1).unwrap();
    let projection = PolicyProjection::new(
        PolicyAccessDimension::FileOperationClass,
        [(
            "src".to_string(),
            PolicyOperation::FileOperation(FileOperationClass::Write),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    kernel
        .publish_capability_definition(
            &write_capability,
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();
    let mut claiming = agent_type();
    claiming.type_ref = type_ref("claiming", 1);
    claiming.contract.sandbox_policy = Some(reference);
    claiming.contract.required_capabilities.insert(
        write_capability,
        CapabilityValue::Set(["src".to_string()].into_iter().collect()),
    );
    let error = kernel
        .publish_agent_type_with_security(&claiming, SecurityContractMode::SecurityV1)
        .unwrap_err();
    assert!(error.to_string().contains("capability claims"), "{error}");
}

#[test]
fn security_refinement_orders_authored_policies_and_forbids_mode_downgrade() {
    let kernel = memory_kernel();
    publish_adapter_policy(&kernel);
    let base = publish_secure_agent_type(&kernel);

    // A SECURITY_V1 derived revision with a distinct but provably narrower
    // authored policy is a legal refinement: the ordering uses the policy
    // algebra, not reference equality.
    let narrower_reference = policy_ref("agent-policy-narrow", 1);
    kernel
        .publish_sandbox_policy(
            &narrower_reference,
            &NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
                files: FilePolicy::new(
                    PathSet::allowed([PathSelector::workspace(["src"]).unwrap()]),
                    PathSet::deny_all(),
                    PathSet::deny_all(),
                    PathSet::deny_all(),
                    PathSet::deny_all(),
                ),
                tools: ToolPolicy::top(),
                network: NetworkEndpointSet::allowed([NetworkEndpoint::new(
                    NetworkTransport::Tcp,
                    "example.com",
                    Some(443),
                )
                .unwrap()]),
                environment: EnvironmentPolicy::top(),
                credentials: BTreeMap::new(),
                resources: BTreeMap::new(),
                working_directory: WorkingDirectory::Any,
                attempt_isolation: false,
            })
            .unwrap(),
        )
        .unwrap();
    let mut narrower = agent_type();
    narrower.type_ref = type_ref("secure-coder", 2);
    narrower.based_on = Some(base.type_ref.clone());
    narrower.contract.sandbox_policy = Some(narrower_reference);
    kernel
        .publish_agent_type_with_security(&narrower, SecurityContractMode::SecurityV1)
        .unwrap();

    // Refining the narrower revision with the wider base policy is rejected: the
    // derived policy must be provably no wider than its own base.
    let mut widened = agent_type();
    widened.type_ref = type_ref("secure-coder", 3);
    widened.based_on = Some(narrower.type_ref.clone());
    widened.contract.sandbox_policy = Some(policy_ref("agent-policy", 1));
    let error = kernel
        .publish_agent_type_with_security(&widened, SecurityContractMode::SecurityV1)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("derived sandbox policy is wider than base"),
        "{error}"
    );

    // Dropping the physical enforcement contract is never a legal refinement,
    // even when every coarse field and the policy reference are identical.
    let mut downgraded = narrower.clone();
    downgraded.type_ref = type_ref("secure-coder", 4);
    downgraded.based_on = Some(base.type_ref.clone());
    downgraded.contract.sandbox_policy = base.contract.sandbox_policy.clone();
    let error = kernel
        .publish_agent_type_with_security(&downgraded, SecurityContractMode::CoarseV1)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("must not drop the physical enforcement contract"),
        "{error}"
    );
}

#[test]
fn spawn_source_security_declaration_is_published_atomically_with_its_authored_ceiling() {
    let kernel = memory_kernel();
    publish_adapter_policy(&kernel);
    let policy_reference = policy_ref("source-policy", 1);
    let policy = NormalizedSandboxPolicy::deny_all();
    kernel
        .publish_sandbox_policy(&policy_reference, &policy)
        .unwrap();

    // A coarse revision has no declaration; it cannot be reinterpreted in place.
    let source = source_at(2);
    kernel.publish_spawn_source(&source).unwrap();
    assert_eq!(
        kernel
            .spawn_source_security_mode(&source.source_ref)
            .unwrap(),
        SecurityContractMode::CoarseV1
    );
    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(policy_reference, &policy),
        profile(),
    );
    let error = kernel
        .publish_spawn_source_with_security(&source, Some(&declaration))
        .unwrap_err();
    assert!(error.to_string().contains("immutable"), "{error}");

    // A fresh revision publishes content and declaration atomically.
    let secure = source_at(3);
    kernel
        .publish_spawn_source_with_security(&secure, Some(&declaration))
        .unwrap();
    assert_eq!(
        kernel
            .spawn_source_security_mode(&secure.source_ref)
            .unwrap(),
        SecurityContractMode::SecurityV1
    );
    let read = kernel
        .spawn_source_security_declaration(&secure.source_ref)
        .unwrap()
        .unwrap();
    assert_eq!(read, declaration);

    // Republishing identical content is idempotent; different content needs a
    // new source revision.
    kernel
        .publish_spawn_source_with_security(&secure, Some(&declaration))
        .unwrap();
    let changed = SpawnSourceSecurityDeclaration::new(
        None,
        EnforcementProfileRef::new("agentype-enforcement", "policy-only", 2).unwrap(),
    );
    let error = kernel
        .publish_spawn_source_with_security(&secure, Some(&changed))
        .unwrap_err();
    assert!(
        error.to_string().contains("requires a new revision"),
        "{error}"
    );
}

#[test]
fn spawn_source_declaration_without_ceiling_keeps_top_and_security_mode() {
    let kernel = memory_kernel();
    publish_adapter_policy(&kernel);
    let source = source_at(2);
    let declaration = SpawnSourceSecurityDeclaration::without_policy(profile());
    kernel
        .publish_spawn_source_with_security(&source, Some(&declaration))
        .unwrap();

    let read = kernel
        .spawn_source_security_declaration(&source.source_ref)
        .unwrap()
        .unwrap();
    // No authored ceiling is `P_source = Top`, not "skip full-path checks":
    // the revision is still SECURITY_V1.
    assert!(read.source_policy().is_none());
    assert_eq!(
        kernel
            .spawn_source_security_mode(&source.source_ref)
            .unwrap(),
        SecurityContractMode::SecurityV1
    );
}

#[test]
fn spawn_source_declaration_rejects_an_unresolvable_or_mismatched_policy() {
    let kernel = memory_kernel();
    publish_adapter_policy(&kernel);
    let source = source_at(2);

    // A derived input has no authored identity and cannot be a P_source.
    let derived = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::derived(&NormalizedSandboxPolicy::top()),
        profile(),
    );
    let error = kernel
        .publish_spawn_source_with_security(&source, Some(&derived))
        .unwrap_err();
    assert!(error.to_string().contains("authored"), "{error}");

    // A digest that does not match the resolved canonical content fails closed.
    let policy = NormalizedSandboxPolicy::top();
    kernel
        .publish_sandbox_policy(&policy_ref("missing-policy", 1), &policy)
        .unwrap();
    let wrong_digest = PolicyInput::from_parts(
        Some(policy_ref("missing-policy", 1)),
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    );
    let error = kernel
        .publish_spawn_source_with_security(
            &source,
            Some(&SpawnSourceSecurityDeclaration::new(
                Some(wrong_digest),
                profile(),
            )),
        )
        .unwrap_err();
    assert!(error.to_string().contains("digest"), "{error}");

    // An unresolvable reference is not a usable declaration.
    let missing = SpawnSourceSecurityDeclaration::new(
        Some(PolicyInput::from_parts(
            Some(policy_ref("absent-policy", 1)),
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )),
        profile(),
    );
    let error = kernel
        .publish_spawn_source_with_security(&source, Some(&missing))
        .unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");

    // None of the rejected publications left a source or declaration behind.
    assert_eq!(kernel.get_spawn_source(&source.source_ref).unwrap(), None);
}

#[test]
fn spawn_source_declaration_digest_corruption_fails_closed() {
    let db = Db::new("source-declaration");
    let kernel = db.kernel();
    publish_adapter_policy(&kernel);
    let reference = source_at(2).source_ref;
    let policy_reference = policy_ref("source-policy", 1);
    let policy = NormalizedSandboxPolicy::top();
    kernel
        .publish_sandbox_policy(&policy_reference, &policy)
        .unwrap();
    kernel
        .publish_spawn_source_with_security(
            &source_at(2),
            Some(&SpawnSourceSecurityDeclaration::with_policy(
                PolicyInput::authored(policy_reference, &policy),
                profile(),
            )),
        )
        .unwrap();

    update_without_guards(
        &db.path,
        "UPDATE spawn_source_security_declarations
         SET source_policy_digest='sha256:1111111111111111111111111111111111111111111111111111111111111111'",
    );

    let error = kernel
        .spawn_source_security_declaration(&reference)
        .unwrap_err();
    assert!(error.to_string().contains("does not match"), "{error}");
}

#[test]
fn spawn_source_declaration_enforcement_version_must_fit_u32() {
    let db = Db::new("source-version-width");
    let kernel = db.kernel();
    publish_adapter_policy(&kernel);
    let reference = source_at(2).source_ref;
    kernel
        .publish_spawn_source_with_security(
            &source_at(2),
            Some(&SpawnSourceSecurityDeclaration::without_policy(profile())),
        )
        .unwrap();

    update_without_guards(
        &db.path,
        "UPDATE spawn_source_security_declarations
         SET enforcement_profile_version = 4294967297",
    );

    let error = kernel
        .spawn_source_security_declaration(&reference)
        .unwrap_err();
    assert!(error.to_string().contains("32-bit"), "{error}");
}

#[test]
fn declarations_survive_a_restart() {
    let db = Db::new("restart");
    {
        let kernel = db.kernel();
        publish_adapter_policy(&kernel);
        kernel
            .publish_sandbox_policy(
                &policy_ref("agent-policy", 1),
                &read_only_restricted_policy(),
            )
            .unwrap();
        let mut agent = agent_type();
        agent.type_ref = type_ref("secure-coder", 1);
        agent.contract.sandbox_policy = Some(policy_ref("agent-policy", 1));
        kernel
            .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
            .unwrap();
        kernel
            .publish_spawn_source_with_security(
                &source_at(2),
                Some(&SpawnSourceSecurityDeclaration::without_policy(profile())),
            )
            .unwrap();
    }

    let kernel = db.kernel();
    assert_eq!(
        kernel
            .agent_type_security_mode(&type_ref("secure-coder", 1))
            .unwrap(),
        SecurityContractMode::SecurityV1
    );
    assert_eq!(
        kernel
            .spawn_source_security_mode(&SpawnSourceRef::new("codex-local", 2).unwrap())
            .unwrap(),
        SecurityContractMode::SecurityV1
    );
    assert_eq!(
        kernel
            .get_sandbox_policy(&policy_ref("agent-policy", 1))
            .unwrap(),
        Some(read_only_restricted_policy())
    );
}

/// A `SpawnSource` revision carries no isolation guarantee field, so its
/// `P_source` policy MUST NOT introduce an unattributed isolation requirement.
#[test]
fn spawn_source_policy_cannot_require_attempt_isolation() {
    let kernel = memory_kernel();
    publish_adapter_policy(&kernel);
    let reference = policy_ref("isolating-source-policy", 1);
    let mut parts = read_only_restricted_policy().parts().clone();
    parts.attempt_isolation = true;
    let policy = NormalizedSandboxPolicy::from_parts(parts).unwrap();
    kernel.publish_sandbox_policy(&reference, &policy).unwrap();

    let source = source_at(2);
    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(reference, &policy),
        profile(),
    );
    let error = kernel
        .publish_spawn_source_with_security(&source, Some(&declaration))
        .unwrap_err();
    assert!(error.to_string().contains("attempt isolation"), "{error}");
    assert!(kernel
        .get_spawn_source(&source.source_ref)
        .unwrap()
        .is_none());
}
