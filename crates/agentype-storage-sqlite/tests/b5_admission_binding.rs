//! M6-B.5 typed admission under an explicit `SecurityContractMode` and the
//! immutable `TaskSecurityBinding` (schema v9).
//!
//! These tests prove the B5.1 gate: a `SECURITY_V1` admission records exactly one
//! binding that re-derives from durable authority, a `COARSE_V1` admission
//! records none, and an explicit mode is never inferred from the authored
//! policy. They do not compile or execute a security plan.

use agentype_agent_contract::{
    sandbox_policy_content_digest, AffinityConstraint, AgentRequirementDraft, AgentType,
    AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget, CapabilityDefinition,
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
