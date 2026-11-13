//! M6-B.5 B5.1 coarse-path gate conformance.
//!
//! B5.1 makes `SECURITY_V1` policies submittable and re-readable, but a Task
//! that needs the B.5 execution path MUST still be unable to gain authority
//! through the frozen coarse path. These tests prove every coarse stage refuses
//! it: the legacy M5 claim (typed Tasks are never claimable there), the
//! read-only acquisition preflight (before any source-private work), and the
//! authority-bearing acquisition transaction. `COARSE_V1` and legacy Tasks are
//! unchanged.

use agentype_agent_contract::{
    canonical_json_body_digest, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentRequirementDraft, AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget,
    ConfigDigest, ConfigStatus, ContinuityMode, EnforcementProfileRef, GenerationPolicy,
    LifecycleMode, MaterializationDigest, NetworkPolicy, NormalizedSandboxPolicy, PhysicalSafety,
    ResolvedProvisioningEvidence, SandboxPolicyRef, SecurityContract, SecurityContractMode,
    SourceConfig, SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef,
    SpawnSourceSecurityDeclaration,
};
use agentype_core::{
    Clock, Error, InformationFunction, ManualClock, PartitionSpec, RawWorkIntent, Retention,
    SemanticInputSet, TaskId, TaskSpec, TaskState, WorkspaceMode,
};
use agentype_execution_config::{ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig};
use agentype_storage_sqlite::{Kernel, ResolvedProvisioningSelection, SourceConfigBody};
use rusqlite::Connection;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

struct Env {
    kernel: Kernel,
    path: PathBuf,
    dir: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Self {
        let dir =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("b5-gate-{tag}-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                2,
                Retention::Ephemeral,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        Self { kernel, path, dir }
    }

    fn raw(&self) -> Connection {
        let conn = Connection::open(&self.path).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn type_ref(id: &str) -> AgentTypeRef {
    AgentTypeRef::new(id, 1).unwrap()
}

fn policy_ref(id: &str) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, 1).unwrap()
}

fn registry() -> ExecutionRegistry {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(ExecutionTargetConfig::new("local", "default", false))
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    registry
}

fn safety() -> PhysicalSafety {
    PhysicalSafety::new(
        false,
        vec![WorkspaceMode::ReadOnly],
        [NetworkPolicy::Disabled]
            .into_iter()
            .collect::<BTreeSet<_>>(),
    )
    .unwrap()
}

fn publish_agent_type(kernel: &Kernel, id: &str) -> AgentTypeRef {
    let agent = AgentType {
        type_ref: type_ref(id),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace: WorkspaceMode::ReadOnly,
                network: NetworkPolicy::Disabled,
                requires_attempt_isolation: false,
            },
            lifecycle: [LifecycleMode::Ephemeral].into_iter().collect(),
            continuity: ContinuityMode::None,
            sandbox_policy: None,
            anchor_constraint: None,
        },
    };
    kernel.publish_agent_type(&agent).unwrap();
    agent.type_ref
}

/// Publish one `SECURITY_V1` AgentType whose authored policy projects to its
/// `ReadOnly` / `Disabled` coarse contract.
fn publish_security_agent_type(kernel: &Kernel, id: &str) -> AgentTypeRef {
    let reference = policy_ref(&format!("agent-policy-{id}"));
    kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::deny_all())
        .unwrap();
    let agent = AgentType {
        type_ref: type_ref(id),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace: WorkspaceMode::ReadOnly,
                network: NetworkPolicy::Disabled,
                requires_attempt_isolation: false,
            },
            lifecycle: [LifecycleMode::Ephemeral].into_iter().collect(),
            continuity: ContinuityMode::None,
            sandbox_policy: Some(reference),
            anchor_constraint: None,
        },
    };
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

/// Publish one real `AdapterBindingPolicy` + `SpawnSource` + `SourceConfig` trio
/// and return the resolver-shaped selection that matches it exactly.
fn catalog(kernel: &Kernel, tag: &str) -> ResolvedProvisioningSelection {
    catalog_with_security(kernel, tag, None)
}

fn catalog_with_security(
    kernel: &Kernel,
    tag: &str,
    declaration: Option<&SpawnSourceSecurityDeclaration>,
) -> ResolvedProvisioningSelection {
    let policy = AdapterBindingPolicy {
        policy_ref: AdapterPolicyRef::new(format!("policy-{tag}"), 1).unwrap(),
        adapter_kind: "default".into(),
        binding_ref: "primary".into(),
        required_safety: safety(),
        status: ConfigStatus::Active,
    };
    kernel.publish_adapter_binding_policy(&policy).unwrap();

    let source = SpawnSource {
        source_ref: SpawnSourceRef::new(format!("source-{tag}"), 1).unwrap(),
        adapter_policy: policy.policy_ref.clone(),
        lifecycle_modes: [LifecycleMode::Ephemeral].into_iter().collect(),
        continuity_modes: [ContinuityMode::None].into_iter().collect(),
        functional_envelope: BTreeMap::new(),
        claims: Vec::new(),
        status: SourceStatus::Active,
    };
    kernel
        .publish_spawn_source_with_security(&source, declaration)
        .unwrap();

    let body = json!({"model": "opaque"});
    let config_digest = ConfigDigest::new(canonical_json_body_digest(&body)).unwrap();
    let config = SourceConfig {
        config_ref: SourceConfigRef::new(source.source_ref.clone(), format!("config-{tag}"), 1)
            .unwrap(),
        config_digest: config_digest.clone(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: Vec::new(),
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    kernel
        .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
        .unwrap();

    let evidence = ResolvedProvisioningEvidence::from_imported_binding(
        policy.policy_ref.clone(),
        "default",
        "k1",
        source.source_ref.clone(),
        config.config_ref.clone(),
        config_digest,
        safety(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    ResolvedProvisioningSelection {
        spawn_source: source.source_ref,
        source_config: config.config_ref,
        adapter_policy: policy.policy_ref,
        evidence,
        adapter_kind: "default".into(),
        adapter_binding_key: "k1".into(),
        provisioning_protocol: "agentype-test-source/1".into(),
        attested_materialization_digest: MaterializationDigest::new(format!(
            "sha256:{}",
            "1".repeat(64)
        ))
        .unwrap(),
        catalog_frontier_digest: kernel.catalog_frontier_digest().unwrap(),
    }
}

fn draft(pin: AgentTypeRef) -> AgentRequirementDraft {
    AgentRequirementDraft {
        required_type: AgentTypeSelector::Exact(pin),
        required_capabilities: BTreeMap::new(),
        required_network: NetworkPolicy::Disabled,
        required_attempt_isolation: false,
        sandbox_policy: None,
        required_anchor: None,
        budget: Budget::new(50.0).unwrap(),
    }
}

fn generation_policy(reference: SandboxPolicyRef) -> GenerationPolicy {
    GenerationPolicy {
        allowed_information_functions: vec![InformationFunction::Expand],
        max_workspace: WorkspaceMode::ReadOnly,
        max_network: NetworkPolicy::Disabled,
        requires_attempt_isolation: false,
        min_continuity: ContinuityMode::None,
        sandbox_policy: Some(reference),
        budget_ceiling: Budget::new(100.0).unwrap(),
        allowed_affinity: None,
        anchor_constraint: None,
    }
}

fn task_spec() -> TaskSpec {
    TaskSpec::new("audit", json!({})).partition("general")
}

fn compile_and_admit(
    kernel: &Kernel,
    generation_id: &agentype_core::GenerationId,
    pin: AgentTypeRef,
    mode: SecurityContractMode,
) -> TaskId {
    let intent = RawWorkIntent {
        raw_intent_key: "security_work".into(),
        objective: "security work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(task_spec()),
    };
    let proposal = kernel
        .compile_root_intent(generation_id, intent, "session", 1)
        .unwrap();
    if mode == SecurityContractMode::SecurityV1 {
        kernel
            .admit_typed_proposal_with_security_mode(
                &proposal.proposal_id,
                0,
                None,
                draft(pin),
                SecurityContractMode::SecurityV1,
            )
            .unwrap()
    } else {
        kernel
            .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
            .unwrap()
    }
}

/// Admit one `SECURITY_V1` Task whose authored policy is consistent with its
/// `ReadOnly` / `Disabled` ceiling. `deny_all` satisfies both and leaves an empty
/// `RequiredAccess`, so B5.1 admission itself succeeds.
fn admit_security_v1_task(kernel: &Kernel, type_id: &str) -> (TaskId, AgentTypeRef) {
    let reference = policy_ref(&format!("generation-policy-{type_id}"));
    kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::deny_all())
        .unwrap();
    let pin = publish_agent_type(kernel, type_id);
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(reference)),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let task = compile_and_admit(
        kernel,
        &generation.generation_id,
        pin.clone(),
        SecurityContractMode::SecurityV1,
    );
    (task, pin)
}

/// Admit one `COARSE_V1` typed Task with no authored policy; the frozen absence
/// default must stay untouched by the gate.
fn admit_coarse_v1_task(kernel: &Kernel, type_id: &str) -> (TaskId, AgentTypeRef) {
    let pin = publish_agent_type(kernel, type_id);
    let generation = kernel.create_generation(json!({})).unwrap();
    let task = compile_and_admit(
        kernel,
        &generation.generation_id,
        pin.clone(),
        SecurityContractMode::CoarseV1,
    );
    (task, pin)
}

/// Admit one `COARSE_V1` typed Task pinned to an already-published AgentType.
fn admit_coarse_task_for_pin(kernel: &Kernel, pin: AgentTypeRef) -> TaskId {
    let generation = kernel.create_generation(json!({})).unwrap();
    compile_and_admit(
        kernel,
        &generation.generation_id,
        pin,
        SecurityContractMode::CoarseV1,
    )
}

fn count(env: &Env, table: &str) -> i64 {
    env.raw()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn assert_no_acquisition_side_effects(env: &Env) {
    assert_eq!(count(env, "provisioning_bindings"), 0);
    assert_eq!(count(env, "attempts"), 0);
    assert_eq!(count(env, "leases"), 0);
}

fn assert_gate_rejection(error: &Error) {
    assert!(
        matches!(error, Error::InvalidAuthority(_)),
        "expected invalid authority, got {error:?}"
    );
    assert!(error.to_string().contains("SECURITY_V1"), "{error}");
}

#[test]
fn the_legacy_claim_path_never_claims_a_security_task() {
    let env = Env::new("legacy-claim");
    let kernel = &env.kernel;
    let (task, _pin) = admit_security_v1_task(kernel, "auditor");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);

    // The frozen M5 claim path excludes every typed Task, so a SECURITY_V1 Task
    // can never obtain Attempt/Lease authority there either.
    assert!(kernel.claim_next_available().unwrap().is_none());
    assert_no_acquisition_side_effects(&env);
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
}

#[test]
fn the_coarse_preflight_rejects_a_security_task_before_private_work() {
    let env = Env::new("preflight");
    let kernel = &env.kernel;
    let (task, _pin) = admit_security_v1_task(kernel, "auditor");

    // Read-only preflight: rejection happens before semantic matching,
    // source-private attestation, credential resolution, or any private I/O.
    let error = kernel.check_typed_acquisition_readiness(&task).unwrap_err();
    assert_gate_rejection(&error);

    // The refusal granted nothing.
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);

    // Guard the biconditional: the very same preflight accepts COARSE_V1.
    let (coarse, _pin) = admit_coarse_v1_task(kernel, "reviewer");
    kernel.check_typed_acquisition_readiness(&coarse).unwrap();
}

#[test]
fn the_coarse_acquisition_transaction_rejects_a_security_task() {
    let env = Env::new("acquisition");
    let kernel = &env.kernel;
    let (task, pin) = admit_security_v1_task(kernel, "auditor");
    // A real, published catalog and a READY, type-bound agent: the only reason
    // acquisition is refused is the security gate, not missing provisioning
    // inputs. The same fixture acquires a COARSE_V1 Task below.
    let selection = catalog(kernel, "security");
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let agents_before = count(&env, "logical_agents");

    let error = kernel
        .acquire_typed_task_existing(&task, &agent, &selection, &registry())
        .unwrap_err();
    assert_gate_rejection(&error);
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
    assert_eq!(count(&env, "logical_agents"), agents_before);

    // The new-agent path refuses before birthing a fresh LogicalAgent too.
    let error = kernel
        .acquire_typed_task_new_agent(&task, &selection, &registry())
        .unwrap_err();
    assert_gate_rejection(&error);
    assert_eq!(count(&env, "logical_agents"), agents_before);
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn coarse_and_legacy_tasks_are_not_gated() {
    let env = Env::new("coarse-unchanged");
    let kernel = &env.kernel;

    // The exact same catalog/selection/READY-agent fixture succeeds for a
    // COARSE_V1 Task: the gate is mode-specific, not a regression of B.4.
    let selection = catalog(kernel, "coarse");
    let (coarse, pin) = admit_coarse_v1_task(kernel, "reviewer");
    kernel.check_typed_acquisition_readiness(&coarse).unwrap();
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquired = kernel
        .acquire_typed_task_existing(&coarse, &agent, &selection, &registry())
        .unwrap();
    assert_eq!(acquired.claim.task_id, coarse);
    assert_eq!(kernel.task(&coarse).unwrap().state, TaskState::Leased);

    // A legacy Task has no security mode: readiness accepts it and the frozen M5
    // claim still grants it authority.
    let (_batch, ids) = kernel
        .submit_batch(&[TaskSpec::new("legacy", json!({})).partition("general")])
        .unwrap();
    let legacy = ids.values().next().unwrap().clone();
    kernel.check_typed_acquisition_readiness(&legacy).unwrap();
    let claim = kernel
        .claim_next_available()
        .unwrap()
        .expect("legacy claim is unchanged");
    assert_eq!(claim.task_id, legacy);
}

/// Drop every trigger so a test can simulate a bypassing direct-SQL writer.
fn execute_without_guards(env: &Env, sql: &str) {
    let conn = env.raw();
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
    conn.execute(sql, []).unwrap();
}

#[test]
fn a_typed_task_missing_its_requirement_row_fails_closed() {
    let env = Env::new("missing-requirement");
    let kernel = &env.kernel;
    let (task, _pin) = admit_coarse_v1_task(kernel, "reviewer");
    kernel.check_typed_acquisition_readiness(&task).unwrap();

    // The TYPED mirror survives but the requirement row (the sole owner of the
    // security mode) is gone: readiness must not silently treat this as LEGACY.
    execute_without_guards(
        &env,
        &format!(
            "DELETE FROM task_agent_requirements WHERE task_id='{}'",
            task.as_str()
        ),
    );
    let error = kernel.check_typed_acquisition_readiness(&task).unwrap_err();
    assert!(
        matches!(error, Error::InvariantViolation(_)),
        "expected an invariant violation, got {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("missing its agent requirement mode"),
        "{error}"
    );
}

#[test]
fn a_coarse_task_cannot_execute_a_security_v1_agent_type() {
    let env = Env::new("security-type");
    let kernel = &env.kernel;
    let pin = publish_security_agent_type(kernel, "secure-auditor");
    let task = admit_coarse_task_for_pin(kernel, pin.clone());
    let selection = catalog(kernel, "security-type");
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    let error = kernel
        .acquire_typed_task_existing(&task, &agent, &selection, &registry())
        .unwrap_err();
    assert_gate_rejection(&error);
    assert!(error.to_string().contains("agent type"), "{error}");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn a_coarse_task_cannot_select_a_security_v1_source_even_without_a_ceiling() {
    let env = Env::new("security-source");
    let kernel = &env.kernel;
    let (task, pin) = admit_coarse_v1_task(kernel, "reviewer");
    // `P_source = Top` adds no narrowing but the revision is still SECURITY_V1;
    // it must never fall back to the coarse path.
    let declaration = SpawnSourceSecurityDeclaration::without_policy(
        EnforcementProfileRef::new("agentype-enforcement", "policy-only", 1).unwrap(),
    );
    let selection = catalog_with_security(kernel, "security-source", Some(&declaration));
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    let error = kernel
        .acquire_typed_task_existing(&task, &agent, &selection, &registry())
        .unwrap_err();
    assert_gate_rejection(&error);
    assert!(error.to_string().contains("spawn source"), "{error}");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}
