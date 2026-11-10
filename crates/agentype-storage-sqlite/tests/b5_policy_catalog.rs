//! M6-B.5 schema v9 effective-security catalog conformance:
//! authored sandbox policies, capability canonical versions, and the additive
//! AgentType / SpawnSource `SECURITY_V1` declarations.
//!
//! These tests exercise the durable boundary only. B5.1 does not compile or
//! execute a security plan; they prove that a policy can be submitted and
//! re-read as exactly the same canonical contract, and that a corrupted or
//! rewritten row fails closed.

use agentype_agent_contract::{
    AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint, AgentType, AgentTypeContract,
    AgentTypeRef, Budget, CapabilityDefinition, CapabilityPolarity, CapabilityRef, ConfigStatus,
    ContinuityMode, EnforcementProfileRef, FileOperationClass, LifecycleMode, MatcherKind,
    NetworkPolicy, NormalizedSandboxPolicy, PhysicalSafety, PolicyAccessDimension, PolicyInput,
    PolicyOperation, PolicyProjection, SandboxPolicyRef, SecurityClass, SecurityContract,
    SourceStatus, SpawnSource, SpawnSourceRef, SpawnSourceSecurityDeclaration,
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

fn publish_parents(kernel: &Kernel) {
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    kernel.publish_agent_type(&agent_type()).unwrap();
    kernel.publish_spawn_source(&spawn_source()).unwrap();
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
fn agent_type_security_declaration_is_additive_and_write_once() {
    let db = Db::new("agent-declaration");
    let kernel = db.kernel();
    publish_parents(&kernel);
    let reference = type_ref("coder", 1);

    // Absence is the frozen COARSE_V1 contract.
    assert_eq!(
        kernel.agent_type_security_mode(&reference).unwrap(),
        agentype_agent_contract::SecurityContractMode::CoarseV1
    );

    kernel
        .publish_agent_type_security_declaration(&reference)
        .unwrap();
    assert_eq!(
        kernel.agent_type_security_mode(&reference).unwrap(),
        agentype_agent_contract::SecurityContractMode::SecurityV1
    );
    // Repeating the one legal declaration is an idempotent no-op.
    kernel
        .publish_agent_type_security_declaration(&reference)
        .unwrap();

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
fn spawn_source_security_declaration_round_trips_with_authored_ceiling() {
    let kernel = memory_kernel();
    publish_parents(&kernel);
    let reference = SpawnSourceRef::new("codex-local", 2).unwrap();
    let policy_reference = policy_ref("source-policy", 1);
    let policy = NormalizedSandboxPolicy::deny_all();
    kernel
        .publish_sandbox_policy(&policy_reference, &policy)
        .unwrap();

    assert_eq!(
        kernel.spawn_source_security_mode(&reference).unwrap(),
        agentype_agent_contract::SecurityContractMode::CoarseV1
    );
    assert_eq!(
        kernel
            .spawn_source_security_declaration(&reference)
            .unwrap(),
        None
    );

    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::authored(policy_reference.clone(), &policy),
        profile(),
    );
    kernel
        .publish_spawn_source_security_declaration(&reference, &declaration)
        .unwrap();
    assert_eq!(
        kernel.spawn_source_security_mode(&reference).unwrap(),
        agentype_agent_contract::SecurityContractMode::SecurityV1
    );
    let read = kernel
        .spawn_source_security_declaration(&reference)
        .unwrap()
        .unwrap();
    assert_eq!(read, declaration);
    assert_eq!(
        read.mode(),
        agentype_agent_contract::SecurityContractMode::SecurityV1
    );

    // Republishing identical content is idempotent; different content needs a
    // new source revision.
    kernel
        .publish_spawn_source_security_declaration(&reference, &declaration)
        .unwrap();
    let changed = SpawnSourceSecurityDeclaration::new(
        None,
        EnforcementProfileRef::new("agentype-enforcement", "policy-only", 2).unwrap(),
    );
    let error = kernel
        .publish_spawn_source_security_declaration(&reference, &changed)
        .unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
}

#[test]
fn spawn_source_declaration_without_ceiling_keeps_top_and_security_mode() {
    let kernel = memory_kernel();
    publish_parents(&kernel);
    let reference = SpawnSourceRef::new("codex-local", 2).unwrap();
    let declaration = SpawnSourceSecurityDeclaration::without_policy(profile());
    kernel
        .publish_spawn_source_security_declaration(&reference, &declaration)
        .unwrap();

    let read = kernel
        .spawn_source_security_declaration(&reference)
        .unwrap()
        .unwrap();
    // No authored ceiling is `P_source = Top`, not "skip full-path checks":
    // the revision is still SECURITY_V1.
    assert!(read.source_policy().is_none());
    assert_eq!(
        kernel.spawn_source_security_mode(&reference).unwrap(),
        agentype_agent_contract::SecurityContractMode::SecurityV1
    );
}

#[test]
fn spawn_source_declaration_rejects_an_unresolvable_or_mismatched_policy() {
    let kernel = memory_kernel();
    publish_parents(&kernel);
    let reference = SpawnSourceRef::new("codex-local", 2).unwrap();

    // A derived input has no authored identity and cannot be a P_source.
    let derived = SpawnSourceSecurityDeclaration::with_policy(
        PolicyInput::derived(&NormalizedSandboxPolicy::top()),
        profile(),
    );
    let error = kernel
        .publish_spawn_source_security_declaration(&reference, &derived)
        .unwrap_err();
    assert!(error.to_string().contains("authored"), "{error}");

    // A digest that does not match the resolved canonical content fails closed.
    let policy = NormalizedSandboxPolicy::top();
    // Publish the referenced revision, then declare a different digest for it.
    kernel
        .publish_sandbox_policy(&policy_ref("missing-policy", 1), &policy)
        .unwrap();
    let wrong_digest = PolicyInput::from_parts(
        Some(policy_ref("missing-policy", 1)),
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    );
    let error = kernel
        .publish_spawn_source_security_declaration(
            &reference,
            &SpawnSourceSecurityDeclaration::new(Some(wrong_digest), profile()),
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
        .publish_spawn_source_security_declaration(&reference, &missing)
        .unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
}

#[test]
fn spawn_source_declaration_digest_corruption_fails_closed() {
    let db = Db::new("source-declaration");
    let kernel = db.kernel();
    publish_parents(&kernel);
    let reference = SpawnSourceRef::new("codex-local", 2).unwrap();
    let policy_reference = policy_ref("source-policy", 1);
    let policy = NormalizedSandboxPolicy::top();
    kernel
        .publish_sandbox_policy(&policy_reference, &policy)
        .unwrap();
    kernel
        .publish_spawn_source_security_declaration(
            &reference,
            &SpawnSourceSecurityDeclaration::with_policy(
                PolicyInput::authored(policy_reference, &policy),
                profile(),
            ),
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
fn declarations_survive_a_restart() {
    let db = Db::new("restart");
    {
        let kernel = db.kernel();
        publish_parents(&kernel);
        kernel
            .publish_sandbox_policy(
                &policy_ref("source-policy", 1),
                &NormalizedSandboxPolicy::top(),
            )
            .unwrap();
        kernel
            .publish_agent_type_security_declaration(&type_ref("coder", 1))
            .unwrap();
        kernel
            .publish_spawn_source_security_declaration(
                &SpawnSourceRef::new("codex-local", 2).unwrap(),
                &SpawnSourceSecurityDeclaration::without_policy(profile()),
            )
            .unwrap();
    }

    let kernel = db.kernel();
    assert_eq!(
        kernel
            .agent_type_security_mode(&type_ref("coder", 1))
            .unwrap(),
        agentype_agent_contract::SecurityContractMode::SecurityV1
    );
    assert_eq!(
        kernel
            .spawn_source_security_mode(&SpawnSourceRef::new("codex-local", 2).unwrap())
            .unwrap(),
        agentype_agent_contract::SecurityContractMode::SecurityV1
    );
    assert_eq!(
        kernel
            .get_sandbox_policy(&policy_ref("source-policy", 1))
            .unwrap(),
        Some(NormalizedSandboxPolicy::top())
    );
}
