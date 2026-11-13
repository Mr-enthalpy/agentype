//! M6-B.5 typed admission under an explicit `SecurityContractMode` and the
//! immutable `TaskSecurityBinding` (schema v9).
//!
//! These tests prove the B5.1 gate: a `SECURITY_V1` admission records exactly one
//! binding that re-derives from durable authority, a `COARSE_V1` admission
//! records none, and an explicit mode is never inferred from the authored
//! policy. They do not compile or execute a security plan.

use agentype_agent_contract::{
    project_workspace, sandbox_policy_content_digest, AffinityConstraint, AgentRequirementDraft,
    AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget, CapabilityDefinition,
    CapabilityPolarity, CapabilityRef, CapabilityValue, ContinuityMode, CredentialAuthorizationId,
    CredentialDeliveryMode, CredentialGrant, EnvironmentPolicy, FileOperationClass, FilePolicy,
    GenerationPolicy, LifecycleMode, MatcherKind, NetworkEndpoint, NetworkEndpointSet,
    NetworkPolicy, NetworkTransport, NormalizedSandboxPolicy, PathSelector, PathSet,
    PendingObligation, PolicyAccessDimension, PolicyOperation, PolicyProjection,
    SandboxPolicyParts, SandboxPolicyRef, SecurityClass, SecurityContract, SecurityContractMode,
    ToolPolicy, WorkingDirectory,
};
use agentype_core::{
    Clock, GenerationId, InformationFunction, ManualClock, ProposalRecord, ProposalStateKind,
    RawWorkIntent, Retention, SemanticInputSet, TaskSpec, WorkspaceMode,
};
use agentype_storage_sqlite::Kernel;
use rusqlite::Connection;
use serde_json::json;
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
        let dir = std::env::temp_dir().join(format!("agentype-b5-admission-{tag}-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            path: dir.join("scheduler.db"),
            dir,
        }
    }

    fn kernel(&self) -> Kernel {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&self.path, clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&agentype_core::PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        kernel
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
    let kernel = Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap();
    kernel
        .upsert_partition(&agentype_core::PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    kernel.reconcile_pool().unwrap();
    kernel
}

fn policy_ref(id: &str, revision: u64) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, revision).unwrap()
}

/// A policy that is consistent with a `ReadOnly` Generation ceiling: it must
/// not allow writing, and its finite network allowlist must project to
/// `Restricted` so the coarse and fine fields agree (ADR-0013 §3). It still
/// grants reads, tools and one endpoint so a positive `RequiredAccess` floor can
/// be satisfied.
fn read_only_workspace_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn restricted_network() -> NetworkEndpointSet {
    NetworkEndpointSet::allowed([NetworkEndpoint::new(
        NetworkTransport::Tcp,
        "example.com",
        Some(443),
    )
    .unwrap()])
}

/// A `ReadOnly` / `Restricted` policy whose two rules jointly cover
/// `ANY:*:443` but where no single rule covers it.
fn split_network_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: NetworkEndpointSet::allowed([
            NetworkEndpoint::new(NetworkTransport::Tcp, "*", Some(443)).unwrap(),
            NetworkEndpoint::new(NetworkTransport::Udp, "*", None).unwrap(),
        ]),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

/// A policy that still projects to `ReadOnly` / `Restricted` but denies every
/// file read, so a required read capability is unsatisfied.
fn read_only_restricted_but_file_denying_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::deny_all(),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn policy_with_working_directory(directory: &str) -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Required(
            PathSelector::workspace([directory]).unwrap(),
        ),
        attempt_isolation: false,
    })
    .unwrap()
}

fn credential_grant() -> (CredentialAuthorizationId, CredentialGrant) {
    let id = CredentialAuthorizationId::new("vault", "github-token").unwrap();
    let grant = CredentialGrant::new(
        id.clone(),
        ["repo.read".to_string()],
        ["github.com".to_string()],
        [CredentialDeliveryMode::BrokeredAccess],
    )
    .unwrap();
    (id, grant)
}

fn policy_with_credential() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::from([credential_grant()]),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn workspace_path(components: &[&str]) -> PathSelector {
    PathSelector::workspace(components.iter().copied()).unwrap()
}

fn endpoint(host: &str) -> NetworkEndpoint {
    NetworkEndpoint::new(NetworkTransport::Tcp, host, Some(443)).unwrap()
}

/// A policy with an explicit read-anything / write-narrow / restricted-network
/// shape, so the only dimension under test is the supplied one.
fn policy_with_files_and_network(
    write: PathSet,
    network: NetworkEndpointSet,
) -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            write,
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network,
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

/// A policy whose only workspace modification is deletion. It projects to
/// `Write`, because deleting content or a directory entry is modification.
fn policy_with_delete_and_network(
    delete: PathSet,
    network: NetworkEndpointSet,
) -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            delete,
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network,
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

/// Publish a `SECURITY_V1` AgentType with the given authored policy and coarse
/// fields. Empty capabilities keep this focused on sandbox authority.
fn publish_secure_type(
    kernel: &Kernel,
    id: &str,
    reference: SandboxPolicyRef,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
) -> AgentTypeRef {
    let mut agent = agent_type(id, BTreeMap::new());
    agent.contract.security.workspace = workspace;
    agent.contract.security.network = network;
    agent.contract.sandbox_policy = Some(reference);
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

/// A rejected admission must not leave a half-written Task or binding behind:
/// the whole command is one transaction.
fn assert_no_durable_task_rows(db: &Db) {
    let conn = db.raw();
    for table in [
        "tasks",
        "generation_task_bindings",
        "task_security_bindings",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} must be empty after a rejected admission");
    }
}

fn read_capability() -> CapabilityRef {
    CapabilityRef::new("filesystem.read", 1).unwrap()
}

fn read_value() -> CapabilityValue {
    CapabilityValue::Set(["src".to_string()].into_iter().collect())
}

fn publish_read_capability(kernel: &Kernel) {
    let projection = PolicyProjection::new(
        PolicyAccessDimension::FileOperationClass,
        [(
            "src".to_string(),
            PolicyOperation::FileOperation(FileOperationClass::Read),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    kernel
        .publish_capability_definition(
            &read_capability(),
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();
}

fn network_capability() -> CapabilityRef {
    CapabilityRef::new("network.egress", 1).unwrap()
}

fn network_value() -> CapabilityValue {
    CapabilityValue::Set(["https".to_string()].into_iter().collect())
}

/// Publish a capability whose `https` member projects to the required endpoint
/// `ANY:*:443`. No single effective policy rule needs to cover that endpoint;
/// admission must compare against the union of the effective rules.
fn publish_network_capability(kernel: &Kernel) {
    let projection = PolicyProjection::new(
        PolicyAccessDimension::NetworkEndpoint,
        [(
            "https".to_string(),
            PolicyOperation::NetworkEndpoint(
                NetworkEndpoint::new(NetworkTransport::Any, "*", Some(443)).unwrap(),
            ),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    kernel
        .publish_capability_definition(
            &network_capability(),
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();
}

fn agent_type(id: &str, capabilities: BTreeMap<CapabilityRef, CapabilityValue>) -> AgentType {
    AgentType {
        type_ref: type_ref(id, 1),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: capabilities,
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

fn publish_plain_type(kernel: &Kernel) -> AgentTypeRef {
    let agent = agent_type("auditor", BTreeMap::new());
    kernel.publish_agent_type(&agent).unwrap();
    agent.type_ref
}

fn publish_reading_type(kernel: &Kernel) -> AgentTypeRef {
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let agent = agent_type("reader", capabilities);
    kernel.publish_agent_type(&agent).unwrap();
    agent.type_ref
}

/// Publish a `SECURITY_V1` AgentType whose authored policy is the given exact
/// revision, so admission can compare the Task's effective policy against it.
fn publish_secure_reading_type(kernel: &Kernel, reference: SandboxPolicyRef) -> AgentTypeRef {
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let mut agent = agent_type("secure-reader", capabilities);
    agent.contract.sandbox_policy = Some(reference);
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

fn publish_secure_network_type(kernel: &Kernel, reference: SandboxPolicyRef) -> AgentTypeRef {
    let mut capabilities = BTreeMap::new();
    capabilities.insert(network_capability(), network_value());
    let mut agent = agent_type("secure-network", capabilities);
    agent.contract.security.network = NetworkPolicy::Restricted;
    agent.contract.sandbox_policy = Some(reference);
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

fn write_capability() -> CapabilityRef {
    CapabilityRef::new("filesystem.write", 1).unwrap()
}

fn write_value() -> CapabilityValue {
    CapabilityValue::Set(["src".to_string()].into_iter().collect())
}

fn publish_write_capability(kernel: &Kernel) {
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
            &write_capability(),
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();
}

fn delete_capability() -> CapabilityRef {
    CapabilityRef::new("filesystem.delete", 1).unwrap()
}

fn delete_value() -> CapabilityValue {
    CapabilityValue::Set(["src".to_string()].into_iter().collect())
}

fn publish_delete_capability(kernel: &Kernel) {
    let projection = PolicyProjection::new(
        PolicyAccessDimension::FileOperationClass,
        [(
            "src".to_string(),
            PolicyOperation::FileOperation(FileOperationClass::Delete),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    kernel
        .publish_capability_definition(
            &delete_capability(),
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Sandbox,
                polarity: CapabilityPolarity::Ability,
                policy_projection: Some(projection),
            },
        )
        .unwrap();
}

/// Publish a `SECURITY_V1` AgentType with explicit capability claims and
/// sandbox fields.
fn publish_secure_type_with_capabilities(
    kernel: &Kernel,
    id: &str,
    reference: SandboxPolicyRef,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
    capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
) -> AgentTypeRef {
    let mut agent = agent_type(id, capabilities);
    agent.contract.security.workspace = workspace;
    agent.contract.security.network = network;
    agent.contract.sandbox_policy = Some(reference);
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

fn generation_policy(
    reference: Option<SandboxPolicyRef>,
    max_workspace: WorkspaceMode,
    max_network: NetworkPolicy,
) -> GenerationPolicy {
    GenerationPolicy {
        allowed_information_functions: vec![InformationFunction::Expand],
        max_workspace,
        max_network,
        requires_attempt_isolation: false,
        min_continuity: ContinuityMode::None,
        sandbox_policy: reference,
        budget_ceiling: Budget::new(100.0).unwrap(),
        allowed_affinity: None,
        anchor_constraint: None,
    }
}

fn draft(
    pin: AgentTypeRef,
    capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
) -> AgentRequirementDraft {
    AgentRequirementDraft {
        required_type: AgentTypeSelector::Exact(pin),
        required_capabilities: capabilities,
        required_network: NetworkPolicy::Restricted,
        required_attempt_isolation: false,
        sandbox_policy: None,
        required_anchor: None,
        budget: Budget::new(50.0).unwrap(),
    }
}

fn compile_proposal(
    kernel: &Kernel,
    generation_id: &GenerationId,
    spec: TaskSpec,
) -> ProposalRecord {
    compile_named_proposal(kernel, generation_id, spec, "security_work")
}

fn compile_named_proposal(
    kernel: &Kernel,
    generation_id: &GenerationId,
    spec: TaskSpec,
    key: &str,
) -> ProposalRecord {
    let intent = RawWorkIntent {
        raw_intent_key: key.into(),
        objective: "security work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec),
    };
    kernel
        .compile_root_intent(generation_id, intent, "session", 1)
        .unwrap()
}

fn task_spec() -> TaskSpec {
    TaskSpec::new("audit", json!({})).partition("general")
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

fn execute_without_guards(path: &Path, sql: &str) {
    let conn = Connection::open(path).unwrap();
    drop_schema_guards(&conn);
    conn.execute(sql, []).unwrap();
}

#[test]
fn security_v1_admission_binds_and_rereads_the_authored_policy() {
    let db = Db::new("security-round-trip");
    let kernel = db.kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    let policy = read_only_workspace_policy();
    kernel.publish_sandbox_policy(&reference, &policy).unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    assert_eq!(
        kernel.get_task_security_mode(&task_id).unwrap(),
        SecurityContractMode::SecurityV1
    );
    assert_eq!(
        kernel
            .get_generation_security_mode(&generation.generation_id)
            .unwrap(),
        SecurityContractMode::SecurityV1
    );
    let binding = kernel
        .get_task_security_binding(&task_id)
        .unwrap()
        .expect("a SECURITY_V1 task has a binding");
    assert_eq!(binding.policy(), &policy);
    assert_eq!(
        binding.derivation().output_digest(),
        sandbox_policy_content_digest(binding.policy())
    );
    assert!(binding
        .pending_obligations()
        .contains(&PendingObligation::RequiredCapabilities));

    // The binding is durable: a fresh handle re-derives and returns the exact
    // same canonical commitment.
    drop(kernel);
    let reopened = db.kernel();
    let reread = reopened
        .get_task_security_binding(&task_id)
        .unwrap()
        .expect("binding survives restart");
    assert_eq!(reread, binding);
}

#[test]
fn coarse_v1_admission_records_no_binding_and_is_not_inferred_from_a_policy() {
    let kernel = memory_kernel();
    let reference = policy_ref("coarse-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_plain_type(&kernel);

    // A COARSE_V1 Generation MAY carry an authored policy. The policy does not
    // select the full-security path.
    let generation = kernel
        .create_generation_with_policy(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
        )
        .unwrap();
    assert_eq!(
        kernel
            .get_generation_security_mode(&generation.generation_id)
            .unwrap(),
        SecurityContractMode::CoarseV1
    );
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin, BTreeMap::new()))
        .unwrap();

    assert_eq!(
        kernel.get_task_security_mode(&task_id).unwrap(),
        SecurityContractMode::CoarseV1
    );
    assert_eq!(kernel.get_task_security_binding(&task_id).unwrap(), None);

    // The legacy constructor is likewise COARSE_V1.
    let bare = kernel.create_generation(json!({})).unwrap();
    assert_eq!(
        kernel
            .get_generation_security_mode(&bare.generation_id)
            .unwrap(),
        SecurityContractMode::CoarseV1
    );
}

#[test]
fn security_mode_is_explicit_and_must_agree_with_the_generation() {
    let kernel = memory_kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);

    // SECURITY_V1 without an authored Generation policy is an invalid authority
    // claim, never a silent downgrade.
    let error = kernel
        .create_generation_with_security_mode(json!({}), None, SecurityContractMode::SecurityV1)
        .unwrap_err();
    assert!(error.to_string().contains("must author"), "{error}");

    // A SECURITY_V1 Generation must be admitted with the explicit mode; the
    // legacy typed wrapper is COARSE_V1 and must reject.
    let secure = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference.clone()),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &secure.generation_id, task_spec());
    let error = kernel
        .admit_typed_proposal(
            &proposal.proposal_id,
            0,
            None,
            draft(pin.clone(), BTreeMap::new()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("COARSE_V1"), "{error}");
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );

    // A COARSE_V1 Generation must reject an explicit SECURITY_V1 Task even when
    // it carries an authored policy.
    let coarse = kernel
        .create_generation_with_policy(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &coarse.generation_id, task_spec());
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, BTreeMap::new()),
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("COARSE_V1"), "{error}");
}

#[test]
fn unsatisfied_required_access_rejects_security_admission() {
    let kernel = memory_kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("deny-read-policy", 1);
    // The policy still projects to the Generation's `ReadOnly` / `Restricted`
    // ceiling but denies every file read, so the required read floor is not
    // satisfied.
    kernel
        .publish_sandbox_policy(&reference, &read_only_restricted_but_file_denying_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("required access"), "{error}");
    // A rejected admission leaves the proposal PENDING and writes no Task.
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
}

/// A capability-projected `ANY:*:443` requirement is satisfied by the union of
/// `TCP:*:443` and `UDP:*:*`. This is the admission-stage counterpart of the
/// policy-order union test; single-rule coverage would reject a valid Task.
#[test]
fn required_any_transport_endpoint_is_satisfied_by_a_split_network_union() {
    let kernel = memory_kernel();
    publish_network_capability(&kernel);
    let reference = policy_ref("split-network-policy", 1);
    let policy = split_network_policy();
    kernel.publish_sandbox_policy(&reference, &policy).unwrap();
    let pin = publish_secure_network_type(&kernel, reference.clone());
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut capabilities = BTreeMap::new();
    capabilities.insert(network_capability(), network_value());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let binding = kernel
        .get_task_security_binding(&task_id)
        .unwrap()
        .expect("a SECURITY_V1 task has a binding");
    assert_eq!(binding.policy(), &policy);
}

#[test]
fn a_task_policy_that_structurally_conflicts_with_its_generation_is_rejected() {
    let kernel = memory_kernel();
    let reference = policy_ref("generation-policy", 1);
    let other = policy_ref("task-policy", 1);
    // Two different required working directories cannot both hold: this is a
    // structural conflict, not a legal narrowing.
    kernel
        .publish_sandbox_policy(&reference, &policy_with_working_directory("generation"))
        .unwrap();
    kernel
        .publish_sandbox_policy(&other, &policy_with_working_directory("task"))
        .unwrap();
    let pin = publish_plain_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut conflicting = draft(pin, BTreeMap::new());
    conflicting.sandbox_policy = Some(other);
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            conflicting,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("task sandbox policy conflicts with its generation policy"),
        "{error}"
    );
}

#[test]
fn a_narrower_task_policy_is_a_legal_layering_not_a_conflict() {
    let kernel = memory_kernel();
    let reference = policy_ref("generation-policy", 1);
    let other = policy_ref("task-policy", 1);
    let generation_policy_value = read_only_workspace_policy();
    kernel
        .publish_sandbox_policy(&reference, &generation_policy_value)
        .unwrap();
    // A distinct authored revision that is strictly narrower: it restricts the
    // readable prefix while keeping the ReadOnly/Restricted projection.
    let narrower = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([PathSelector::workspace(["src"]).unwrap()]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap();
    kernel.publish_sandbox_policy(&other, &narrower).unwrap();
    let pin = publish_plain_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut layered = draft(pin, BTreeMap::new());
    layered.sandbox_policy = Some(other);
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            layered,
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let binding = kernel
        .get_task_security_binding(&task_id)
        .unwrap()
        .expect("a SECURITY_V1 task has a binding");
    // The effective policy is the real meet of both authored revisions.
    assert_eq!(binding.policy(), &narrower);
}

/// Build a policy with the same `ReadOnly`/`Restricted` projection as
/// [`read_only_workspace_policy`] but whose readable prefix is strictly narrower.
fn workspace_src_read_only_policy() -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::allowed([PathSelector::workspace(["src"]).unwrap()]),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

#[test]
fn a_security_v1_agent_type_admits_a_narrower_task_policy_in_another_revision() {
    let kernel = memory_kernel();
    publish_read_capability(&kernel);
    // AgentType, Generation and Task each pin a different authored revision. The
    // Task revision is strictly narrower, so the old exact-reference predicate
    // would reject a legal narrowing. The mode-aware predicate must accept it.
    let agent_reference = policy_ref("agent-policy", 1);
    let generation_reference = policy_ref("generation-policy", 1);
    let task_reference = policy_ref("task-policy", 1);
    kernel
        .publish_sandbox_policy(&agent_reference, &read_only_workspace_policy())
        .unwrap();
    kernel
        .publish_sandbox_policy(&generation_reference, &read_only_workspace_policy())
        .unwrap();
    let narrower = workspace_src_read_only_policy();
    kernel
        .publish_sandbox_policy(&task_reference, &narrower)
        .unwrap();

    let pin = publish_secure_reading_type(&kernel, agent_reference);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut layered = draft(pin, BTreeMap::new());
    layered.sandbox_policy = Some(task_reference);
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            layered,
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let binding = kernel
        .get_task_security_binding(&task_id)
        .unwrap()
        .expect("a SECURITY_V1 task has a binding");
    assert_eq!(binding.policy(), &narrower);
}

#[test]
fn a_task_policy_wider_than_the_security_v1_agent_type_is_rejected() {
    let kernel = memory_kernel();
    publish_read_capability(&kernel);
    // The AgentType authors the narrower ceiling. The Generation and Task author
    // a wider policy that still stays inside the Generation coarse fields, so
    // only the policy order between the effective Task policy and the pinned
    // AgentType policy can reject it.
    let agent_reference = policy_ref("agent-policy", 1);
    let generation_reference = policy_ref("generation-policy", 1);
    let task_reference = policy_ref("task-policy", 1);
    kernel
        .publish_sandbox_policy(&agent_reference, &workspace_src_read_only_policy())
        .unwrap();
    kernel
        .publish_sandbox_policy(&generation_reference, &read_only_workspace_policy())
        .unwrap();
    kernel
        .publish_sandbox_policy(&task_reference, &read_only_workspace_policy())
        .unwrap();

    let pin = publish_secure_reading_type(&kernel, agent_reference);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut layered = draft(pin, BTreeMap::new());
    layered.sandbox_policy = Some(task_reference);
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            layered,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("wider than the pinned agent type"),
        "{error}"
    );
}

#[test]
fn a_task_read_only_contradiction_with_its_authored_policy_is_rejected() {
    let kernel = memory_kernel();
    let reference = policy_ref("generation-policy", 1);
    let task_policy = policy_ref("task-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    // The Task authors a policy that allows writing while its coarse workspace
    // field is `ReadOnly`: the contradiction must be rejected even though the
    // Generation ceiling also forbids writing.
    let writing = NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            PathSet::any(),
            PathSet::any(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network: restricted_network(),
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap();
    kernel
        .publish_sandbox_policy(&task_policy, &writing)
        .unwrap();
    let pin = publish_plain_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    let mut contradictory = draft(pin, BTreeMap::new());
    contradictory.sandbox_policy = Some(task_policy);
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            contradictory,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("coarse fields contradict its authored sandbox policy"),
        "{error}"
    );
}

#[test]
fn an_empty_post_meet_write_region_rejects_a_write_task() {
    let db = Db::new("write-meet-empty");
    let kernel = db.kernel();
    let generation_reference = policy_ref("generation-policy", 1);
    let task_reference = policy_ref("task-policy", 1);
    let agent_reference = policy_ref("agent-policy", 1);
    let restricted = restricted_network();

    // Both authored layers individually allow writing, and both project to
    // `Write`. Their meet is a legal deny-all write set, so only a check on the
    // admitted policy can see that the Task's `Write` requirement is now
    // unsatisfiable.
    kernel
        .publish_sandbox_policy(
            &generation_reference,
            &policy_with_files_and_network(
                PathSet::allowed([workspace_path(&["src"])]),
                restricted.clone(),
            ),
        )
        .unwrap();
    kernel
        .publish_sandbox_policy(
            &task_reference,
            &policy_with_files_and_network(
                PathSet::allowed([workspace_path(&["tests"])]),
                restricted.clone(),
            ),
        )
        .unwrap();
    kernel
        .publish_sandbox_policy(
            &agent_reference,
            &policy_with_files_and_network(
                PathSet::allowed([workspace_path(&["src"]), workspace_path(&["tests"])]),
                restricted,
            ),
        )
        .unwrap();

    let pin = publish_secure_type(
        &kernel,
        "secure-writer",
        agent_reference,
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::Write,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec().write());

    let mut layered = draft(pin, BTreeMap::new());
    layered.sandbox_policy = Some(task_reference);
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            layered,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("coarse requirement is not satisfied"),
        "{error}"
    );
    assert!(error.to_string().contains("no write region"), "{error}");
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert_no_durable_task_rows(&db);
}

#[test]
fn an_empty_post_meet_network_rejects_a_restricted_task() {
    let db = Db::new("network-meet-empty");
    let kernel = db.kernel();
    let generation_reference = policy_ref("generation-policy", 1);
    let task_reference = policy_ref("task-policy", 1);
    let agent_reference = policy_ref("agent-policy", 1);
    let no_write = PathSet::deny_all();

    kernel
        .publish_sandbox_policy(
            &generation_reference,
            &policy_with_files_and_network(
                no_write.clone(),
                NetworkEndpointSet::allowed([endpoint("example.com")]),
            ),
        )
        .unwrap();
    kernel
        .publish_sandbox_policy(
            &task_reference,
            &policy_with_files_and_network(
                no_write.clone(),
                NetworkEndpointSet::allowed([endpoint("other.com")]),
            ),
        )
        .unwrap();
    kernel
        .publish_sandbox_policy(
            &agent_reference,
            &policy_with_files_and_network(
                no_write,
                NetworkEndpointSet::allowed([endpoint("example.com"), endpoint("other.com")]),
            ),
        )
        .unwrap();

    let pin = publish_secure_type(
        &kernel,
        "secure-reader",
        agent_reference,
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());

    // `draft` requires `Restricted` network, and both authored layers satisfy
    // it separately; the meet's empty allowlist projects to `Disabled`.
    let mut layered = draft(pin, BTreeMap::new());
    layered.sandbox_policy = Some(task_reference);
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            layered,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("coarse requirement is not satisfied"),
        "{error}"
    );
    assert!(error.to_string().contains("no network endpoint"), "{error}");
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert_no_durable_task_rows(&db);
}

#[test]
fn a_binding_keeps_generation_write_under_a_read_only_task() {
    let kernel = memory_kernel();
    let generation_reference = policy_ref("generation-policy", 1);
    let agent_reference = policy_ref("agent-policy", 1);
    let generation_policy_value = policy_with_files_and_network(
        PathSet::allowed([workspace_path(&["src"])]),
        restricted_network(),
    );
    kernel
        .publish_sandbox_policy(&generation_reference, &generation_policy_value)
        .unwrap();
    kernel
        .publish_sandbox_policy(&agent_reference, &generation_policy_value)
        .unwrap();

    let pin = publish_secure_type(
        &kernel,
        "secure-writer",
        agent_reference,
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::Write,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    // The Task is read-only and authors no fine layer, so its binding is the
    // Generation policy and still carries the Generation's write paths.
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, BTreeMap::new()),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    let requirement = kernel
        .get_task_agent_requirement(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(requirement.hard.required_workspace, WorkspaceMode::ReadOnly);
    assert_eq!(requirement.hard.sandbox_policy, None);

    let binding = kernel.get_task_security_binding(&task_id).unwrap().unwrap();
    assert_eq!(binding.policy(), &generation_policy_value);
    assert_eq!(project_workspace(binding.policy()), WorkspaceMode::Write);
    assert!(binding
        .policy()
        .files()
        .write()
        .contains(&workspace_path(&["src"])));
}

#[test]
fn an_absent_task_policy_is_not_an_explicit_top_layer() {
    let kernel = memory_kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    let generation_policy_value = policy_with_credential();
    let (credential_id, _grant) = credential_grant();
    kernel
        .publish_sandbox_policy(&reference, &generation_policy_value)
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    // The Generation policy carries a credential authorization. A Task that
    // authors no policy must keep it: `None` adds no restriction and is not an
    // explicit top that would erase the credential dimension.
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin.clone(), capabilities.clone()),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let binding = kernel.get_task_security_binding(&task_id).unwrap().unwrap();
    assert_eq!(binding.policy(), &generation_policy_value);
    assert!(binding.policy().credentials().contains_key(&credential_id));

    // An explicit Task layer with the same projection but no credential grant is
    // a real narrowing: the meet intersects the credential dimension and the
    // effective policy keeps no grant. This is observably different from `None`.
    let explicit = policy_ref("task-policy", 1);
    kernel
        .publish_sandbox_policy(&explicit, &read_only_workspace_policy())
        .unwrap();
    let proposal = compile_named_proposal(
        &kernel,
        &generation.generation_id,
        task_spec(),
        "explicit_layer_work",
    );
    let mut layered = draft(pin, capabilities);
    layered.sandbox_policy = Some(explicit);
    let layered_task = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            layered,
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let layered_binding = kernel
        .get_task_security_binding(&layered_task)
        .unwrap()
        .unwrap();
    assert!(layered_binding.policy().credentials().is_empty());
}

#[test]
fn replay_requires_the_same_security_mode() {
    let kernel = memory_kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin.clone(), capabilities.clone()),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin.clone(), capabilities.clone()),
            SecurityContractMode::CoarseV1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("replay"), "{error}");

    let replay = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    assert_eq!(replay, task_id);
}

#[test]
fn binding_rows_are_immutable_and_write_once() {
    let db = Db::new("binding-guards");
    let kernel = db.kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    let conn = db.raw();
    assert!(conn
        .execute(
            "UPDATE task_security_bindings SET pending_obligations_json='[]'",
            [],
        )
        .is_err());
    assert!(conn
        .execute("DELETE FROM task_security_bindings", [])
        .is_err());
    assert!(conn
        .execute(
            "INSERT OR REPLACE INTO task_security_bindings(
                 task_id, policy_json, policy_digest, derivation_json, derivation_digest,
                 normalization_version, meet_algorithm_version, required_access_digest,
                 pending_obligations_json, created_at)
             SELECT task_id, policy_json, policy_digest, derivation_json, derivation_digest,
                    normalization_version, meet_algorithm_version, required_access_digest,
                    '[]', created_at
             FROM task_security_bindings",
            [],
        )
        .is_err());
}

#[test]
fn binding_corruption_fails_closed() {
    let db = Db::new("binding-corruption");
    let kernel = db.kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    // A self-consistent row whose obligation set omits a mandatory pending
    // component must not be accepted as the admitted commitment.
    execute_without_guards(
        &db.path,
        "UPDATE task_security_bindings SET pending_obligations_json='[]'",
    );
    let error = kernel.get_task_security_binding(&task_id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not match its derived requirement"),
        "{error}"
    );
}

#[test]
fn binding_version_mirrors_are_checked() {
    let db = Db::new("binding-version");
    let kernel = db.kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    execute_without_guards(
        &db.path,
        "UPDATE task_security_bindings SET normalization_version=99",
    );
    let error = kernel.get_task_security_binding(&task_id).unwrap_err();
    assert!(error.to_string().contains("version mirror"), "{error}");
}

#[test]
fn requirement_mode_and_binding_presence_are_biconditional() {
    let db = Db::new("binding-presence");
    let kernel = db.kernel();
    publish_read_capability(&kernel);
    let reference = policy_ref("generation-policy", 1);
    kernel
        .publish_sandbox_policy(&reference, &read_only_workspace_policy())
        .unwrap();
    let pin = publish_reading_type(&kernel);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut capabilities = BTreeMap::new();
    capabilities.insert(read_capability(), read_value());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    // A SECURITY_V1 requirement without its binding is corruption, not a
    // COARSE_V1 downgrade.
    execute_without_guards(
        &db.path,
        "DELETE FROM task_security_bindings WHERE task_id IS NOT NULL",
    );
    let error = kernel.get_task_agent_requirement(&task_id).unwrap_err();
    assert!(
        error.to_string().contains("missing its security binding"),
        "{error}"
    );
}

#[test]
fn a_coarse_requirement_cannot_own_a_binding() {
    let db = Db::new("coarse-binding");
    let kernel = db.kernel();
    let pin = publish_plain_type(&kernel);
    let generation = kernel.create_generation(json!({})).unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin, BTreeMap::new()))
        .unwrap();

    // The database trigger refuses the smuggled child...
    let conn = db.raw();
    assert!(conn
        .execute(
            "INSERT INTO task_security_bindings(
                 task_id, policy_json, policy_digest, derivation_json, derivation_digest,
                 normalization_version, meet_algorithm_version, required_access_digest,
                 pending_obligations_json, created_at)
             VALUES(?1, '{}', 'sha256:x', '{}', 'sha256:y', 1, 1, 'sha256:z', '[]', 0.0)",
            [task_id.as_str()],
        )
        .is_err());

    // ...and even a bypassing writer cannot make the read accept it.
    execute_without_guards(
        &db.path,
        &format!(
            "INSERT INTO task_security_bindings(
                 task_id, policy_json, policy_digest, derivation_json, derivation_digest,
                 normalization_version, meet_algorithm_version, required_access_digest,
                 pending_obligations_json, created_at)
             VALUES('{}', '{{}}', 'sha256:x', '{{}}', 'sha256:y', 1, 1, 'sha256:z', '[]', 0.0)",
            task_id.as_str()
        ),
    );
    let error = kernel.get_task_agent_requirement(&task_id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unexpectedly has a security binding"),
        "{error}"
    );
}

/// P1-1: a `ReadOnly` Task whose required capability projects to a write cannot
/// be satisfied by its fine policy, even when the Generation's write paths and
/// the AgentType's policy both allow writing. Admission must judge the positive
/// floor against the coarse-constrained authority, not the fine layer alone.
#[test]
fn a_read_only_task_cannot_require_a_write_capability() {
    let db = Db::new("read-only-task-write-floor");
    let kernel = db.kernel();
    publish_write_capability(&kernel);

    let authored = policy_with_files_and_network(
        PathSet::allowed([workspace_path(&["src"])]),
        restricted_network(),
    );
    let generation_reference = policy_ref("write-generation", 1);
    let agent_reference = policy_ref("write-agent", 1);
    kernel
        .publish_sandbox_policy(&generation_reference, &authored)
        .unwrap();
    kernel
        .publish_sandbox_policy(&agent_reference, &authored)
        .unwrap();

    let mut capabilities = BTreeMap::new();
    capabilities.insert(write_capability(), write_value());
    let pin = publish_secure_type_with_capabilities(
        &kernel,
        "secure-writer",
        agent_reference,
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
        capabilities.clone(),
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::Write,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    // `task_spec()` is `ReadOnly`, so the coarse ceiling removes every write.
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("required access is not satisfied"),
        "{error}"
    );
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert_no_durable_task_rows(&db);
}

/// P1-2, delete side: `ReadOnly` is an upper bound on content and
/// directory-entry modification, so a Task that declares it cannot be satisfied
/// by a delete capability even when the Generation's fine policy allows delete.
#[test]
fn a_read_only_task_cannot_require_a_delete_capability() {
    let db = Db::new("read-only-task-delete-floor");
    let kernel = db.kernel();
    publish_delete_capability(&kernel);

    let authored = policy_with_delete_and_network(
        PathSet::allowed([workspace_path(&["src"])]),
        restricted_network(),
    );
    // Deleting modifies the workspace, so the policy projects to `Write` and can
    // legally back a `Write` AgentType and Generation.
    assert_eq!(project_workspace(&authored), WorkspaceMode::Write);
    let generation_reference = policy_ref("delete-generation", 1);
    let agent_reference = policy_ref("delete-agent", 1);
    kernel
        .publish_sandbox_policy(&generation_reference, &authored)
        .unwrap();
    kernel
        .publish_sandbox_policy(&agent_reference, &authored)
        .unwrap();

    let mut capabilities = BTreeMap::new();
    capabilities.insert(delete_capability(), delete_value());
    let pin = publish_secure_type_with_capabilities(
        &kernel,
        "secure-deleter",
        agent_reference,
        WorkspaceMode::Write,
        NetworkPolicy::Restricted,
        capabilities.clone(),
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::Write,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    // `task_spec()` is `ReadOnly`, so the coarse ceiling removes delete as well
    // as write before the required-access check.
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, capabilities),
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("required access is not satisfied"),
        "{error}"
    );
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert_no_durable_task_rows(&db);
}

/// P1-1, network side: a `Disabled` Task whose required capability projects to
/// an endpoint is unsatisfiable even when the Generation's fine policy allows
/// the endpoint.
#[test]
fn a_disabled_network_task_cannot_require_an_endpoint() {
    let db = Db::new("disabled-network-task-endpoint-floor");
    let kernel = db.kernel();
    publish_network_capability(&kernel);

    let policy = split_network_policy();
    let generation_reference = policy_ref("split-network-generation", 1);
    let agent_reference = policy_ref("split-network-agent", 1);
    kernel
        .publish_sandbox_policy(&generation_reference, &policy)
        .unwrap();
    kernel
        .publish_sandbox_policy(&agent_reference, &policy)
        .unwrap();

    let mut capabilities = BTreeMap::new();
    capabilities.insert(network_capability(), network_value());
    let pin = publish_secure_type_with_capabilities(
        &kernel,
        "secure-network",
        agent_reference,
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        capabilities.clone(),
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut requirement = draft(pin, capabilities);
    requirement.required_network = NetworkPolicy::Disabled;
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            requirement,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("required access is not satisfied"),
        "{error}"
    );
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert_no_durable_task_rows(&db);
}

/// P1-2: a legal `ReadOnly` Task under a `Write` Generation must be admitted
/// against a `ReadOnly` AgentType. The Generation's write paths are removed by
/// the Task ceiling before the pinned-type compatibility check, so the fine
/// layer looking wider than the type is not a rejection.
#[test]
fn a_read_only_task_admits_against_a_read_only_agent_type() {
    let kernel = memory_kernel();
    let generation_reference = policy_ref("write-generation", 1);
    let agent_reference = policy_ref("read-only-agent", 1);
    let generation_policy_value = policy_with_files_and_network(
        PathSet::allowed([workspace_path(&["src"])]),
        restricted_network(),
    );
    kernel
        .publish_sandbox_policy(&generation_reference, &generation_policy_value)
        .unwrap();
    kernel
        .publish_sandbox_policy(&agent_reference, &read_only_workspace_policy())
        .unwrap();

    let pin = publish_secure_type(
        &kernel,
        "secure-read-only",
        agent_reference,
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::Write,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pin, BTreeMap::new()),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    let binding = kernel.get_task_security_binding(&task_id).unwrap().unwrap();
    assert_eq!(binding.policy(), &generation_policy_value);
    assert_eq!(project_workspace(binding.policy()), WorkspaceMode::Write);
}

/// P2: the Generation's authored isolation bit and its coarse guarantee are a
/// single authority. Either direction of disagreement is rejected at publication.
#[test]
fn generation_isolation_policy_and_guarantee_must_agree() {
    let kernel = memory_kernel();
    let isolated_reference = policy_ref("isolated-generation-policy", 1);
    let mut parts = read_only_workspace_policy().parts().clone();
    parts.attempt_isolation = true;
    let isolated = NormalizedSandboxPolicy::from_parts(parts).unwrap();
    kernel
        .publish_sandbox_policy(&isolated_reference, &isolated)
        .unwrap();

    // Policy authors isolation, guarantee does not declare it.
    let error = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(isolated_reference.clone()),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("attempt isolation"), "{error}");

    // Guarantee declares isolation, policy does not author it.
    let plain_reference = policy_ref("plain-generation-policy", 1);
    kernel
        .publish_sandbox_policy(&plain_reference, &read_only_workspace_policy())
        .unwrap();
    let mut declaring = generation_policy(
        Some(plain_reference),
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );
    declaring.requires_attempt_isolation = true;
    let error = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(declaring),
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("attempt isolation"), "{error}");
}

/// P2: a Task's authored policy may repeat the Task's own isolation requirement
/// but MUST NOT introduce one that no requirement asks an execution to honor.
#[test]
fn a_task_policy_cannot_introduce_an_unrequested_isolation_guarantee() {
    let db = Db::new("task-policy-unrequested-isolation");
    let kernel = db.kernel();
    let generation_reference = policy_ref("plain-generation", 1);
    let task_reference = policy_ref("isolated-task", 1);
    kernel
        .publish_sandbox_policy(&generation_reference, &read_only_workspace_policy())
        .unwrap();
    let mut parts = read_only_workspace_policy().parts().clone();
    parts.attempt_isolation = true;
    let isolated_task = NormalizedSandboxPolicy::from_parts(parts).unwrap();
    kernel
        .publish_sandbox_policy(&task_reference, &isolated_task)
        .unwrap();

    let pin = publish_secure_type(
        &kernel,
        "secure-read-only",
        generation_reference.clone(),
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(
                Some(generation_reference),
                WorkspaceMode::ReadOnly,
                NetworkPolicy::Restricted,
            )),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = compile_proposal(&kernel, &generation.generation_id, task_spec());
    let mut requirement = draft(pin, BTreeMap::new());
    requirement.sandbox_policy = Some(task_reference);
    let error = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            requirement,
            SecurityContractMode::SecurityV1,
        )
        .unwrap_err();

    assert!(error.to_string().contains("attempt isolation"), "{error}");
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert_no_durable_task_rows(&db);
}
