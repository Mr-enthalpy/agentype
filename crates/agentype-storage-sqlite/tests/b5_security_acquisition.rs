//! M6-B.5 B5.2 security-acquisition wiring (P2-4).
//!
//! `qualify_security_candidate` used to have no caller outside tests. This
//! target drives the B5 acquisition entry point against a real durable catalog
//! and proves the Gate acceptance: a qualified candidate is selectable, an
//! unqualified one is rejected before any Attempt/Lease, and the coarse gate
//! still refuses `SECURITY_V1`.
//!
//! The imported enforcement profile is the only caller-supplied input, because
//! it is an imported external fact with no durable row (ADR-0014 §5). Everything
//! else — the Task binding, requirement, actual AgentType revision and authored
//! policy, source revision, security declaration and source policy — is
//! re-loaded inside the acquisition transaction.

use agentype_agent_contract::{
    canonical_json_body_digest, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentRequirementDraft, AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget,
    CapabilityDefinition, CapabilityPolarity, CapabilityRef, CapabilityValue, ConfigDigest,
    ConfigStatus, ContinuityMode, CredentialRef, EnforcementFeature, EnforcementProfileRef,
    EnforcementSubject, EnforcementSupport, GenerationPolicy, ImportedEnforcementProfile,
    LifecycleMode, MatcherKind, MaterializationDigest, NetworkPolicy, NormalizedSandboxPolicy,
    PathSelector, PhysicalSafety, PolicyInput, ResolvedProvisioningEvidence, SandboxPolicyRef,
    SecurityClass, SecurityContract, SecurityContractMode, SourceConfig, SourceConfigRef,
    SourceStatus, SpawnSource, SpawnSourceRef, SpawnSourceSecurityDeclaration, WorkingDirectory,
    POLICY_ONLY_REFERENCE_PROFILE, POLICY_ONLY_REFERENCE_PROTOCOL, POLICY_ONLY_REFERENCE_VERSION,
};
use agentype_core::{
    Clock, Error, InformationFunction, ManualClock, PartitionSpec, RawWorkIntent, Retention,
    SemanticInputSet, TaskId, TaskSpec, TaskState, WorkspaceMode,
};
use agentype_execution_config::{ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig};
use agentype_storage_sqlite::{
    Kernel, ResolvedProvisioningSelection, SecurityProvisioningSelection, SourceConfigBody,
};
use rusqlite::Connection;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;
const ADAPTER_KIND: &str = "default";
/// The resolved physical `adapter_binding_key` of every fixture binding. In
/// these fixtures the B.4 alias and the physical key happen to be the same
/// string; the imported profile must name this physical key, not the alias.
const ADAPTER_BINDING_KEY: &str = "k1";

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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("b5-security-{tag}-{}", nanos()));
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

/// A coarse AgentType used only as the Task's admission pin, mirroring the
/// coarse-gate fixtures: admission does not require the pin's security mode.
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

/// `deny_all` plus one structural requirement. The authorization dimensions
/// stay deny-all, so the coarse projection is still `ReadOnly` / `Disabled`;
/// only the working-directory term makes the meet observable.
fn deny_all_requiring_workdir(component: &str) -> NormalizedSandboxPolicy {
    let mut parts = NormalizedSandboxPolicy::deny_all().parts().clone();
    parts.working_directory = WorkingDirectory::Required(
        PathSelector::workspace([component]).expect("a fixture path component is a legal selector"),
    );
    NormalizedSandboxPolicy::from_parts(parts)
        .expect("deny-all plus a working directory is a legal policy")
}

/// The bound agent's actual revision: `SECURITY_V1` with a deny-all authored
/// policy that projects to the `ReadOnly` / `Disabled` coarse contract.
fn publish_security_agent_type(kernel: &Kernel, id: &str) -> AgentTypeRef {
    publish_security_agent_type_with_policy(kernel, id, NormalizedSandboxPolicy::deny_all())
}

fn publish_security_agent_type_with_policy(
    kernel: &Kernel,
    id: &str,
    authored: NormalizedSandboxPolicy,
) -> AgentTypeRef {
    let reference = policy_ref(&format!("agent-policy-{id}"));
    kernel
        .publish_sandbox_policy(&reference, &authored)
        .unwrap();
    publish_security_agent_type_with_contract(kernel, id, security_contract(reference))
}

/// The default `SECURITY_V1` contract over an already-published authored policy.
/// Its coarse projection is `ReadOnly` / `Disabled`, so a deny-all effective
/// policy is a legal projection of the Task requirement.
fn security_contract(sandbox_policy: SandboxPolicyRef) -> AgentTypeContract {
    AgentTypeContract {
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
        sandbox_policy: Some(sandbox_policy),
        anchor_constraint: None,
    }
}

fn publish_security_agent_type_with_contract(
    kernel: &Kernel,
    id: &str,
    contract: AgentTypeContract,
) -> AgentTypeRef {
    let agent = AgentType {
        type_ref: type_ref(id),
        based_on: None,
        contract,
    };
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

/// Publish one real `AdapterBindingPolicy` + `SpawnSource` + config trio and
/// return the resolver-shaped selection that matches it exactly.
///
/// `declared_profile` decides the source's security mode: `Some` publishes a
/// `SECURITY_V1` declaration that names the profile and carries no `P_source`
/// ceiling (so the effective policy is exactly the Task/AgentType meet), `None`
/// publishes a `COARSE_V1` source.
fn security_catalog(
    kernel: &Kernel,
    tag: &str,
    declared_profile: Option<&EnforcementProfileRef>,
    credential_refs: Vec<CredentialRef>,
) -> ResolvedProvisioningSelection {
    security_catalog_with_binding(
        kernel,
        tag,
        declared_profile,
        credential_refs,
        ADAPTER_BINDING_KEY,
        ADAPTER_BINDING_KEY,
    )
}

/// The same catalog with an explicit B.4 alias / resolved physical key pair.
///
/// The alias and the key are distinct concepts: the policy stores the alias,
/// while the selection and the imported profile name the exact physical domain
/// the alias resolved to.
#[allow(clippy::too_many_arguments)]
fn security_catalog_with_binding(
    kernel: &Kernel,
    tag: &str,
    declared_profile: Option<&EnforcementProfileRef>,
    credential_refs: Vec<CredentialRef>,
    binding_alias: &str,
    adapter_binding_key: &str,
) -> ResolvedProvisioningSelection {
    let policy = AdapterBindingPolicy {
        policy_ref: AdapterPolicyRef::new(format!("policy-{tag}"), 1).unwrap(),
        adapter_kind: ADAPTER_KIND.into(),
        binding_ref: binding_alias.into(),
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
    let declaration = declared_profile.map(|profile| {
        SpawnSourceSecurityDeclaration::without_policy(source.source_ref.clone(), profile.clone())
    });
    kernel
        .publish_spawn_source_with_security(&source, declaration.as_ref())
        .unwrap();

    let body = json!({"model": "opaque"});
    let config_digest = ConfigDigest::new(canonical_json_body_digest(&body)).unwrap();
    let config = SourceConfig {
        config_ref: SourceConfigRef::new(source.source_ref.clone(), format!("config-{tag}"), 1)
            .unwrap(),
        config_digest: config_digest.clone(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs,
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    kernel
        .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
        .unwrap();

    let evidence = ResolvedProvisioningEvidence::from_imported_binding(
        policy.policy_ref.clone(),
        ADAPTER_KIND,
        adapter_binding_key,
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
        adapter_kind: ADAPTER_KIND.into(),
        adapter_binding_key: adapter_binding_key.into(),
        provisioning_protocol: "agentype-test-source/1".into(),
        attested_materialization_digest: MaterializationDigest::new(format!(
            "sha256:{}",
            "1".repeat(64)
        ))
        .unwrap(),
        catalog_frontier_digest: kernel.catalog_frontier_digest().unwrap(),
    }
}

fn reference_profile_ref() -> EnforcementProfileRef {
    EnforcementProfileRef::new(
        POLICY_ONLY_REFERENCE_PROTOCOL,
        POLICY_ONLY_REFERENCE_PROFILE,
        POLICY_ONLY_REFERENCE_VERSION,
    )
    .unwrap()
}

fn full_support_profile_ref() -> EnforcementProfileRef {
    EnforcementProfileRef::new("test-enforcement", "test-full-support", 1).unwrap()
}

/// Every current obligation, including credential delivery and isolation.
fn full_support() -> EnforcementSupport {
    EnforcementSupport::new([
        EnforcementFeature::WorkspaceWriteConfinement,
        EnforcementFeature::FileReadConfinement,
        EnforcementFeature::FileExecuteConfinement,
        EnforcementFeature::FileVisibilityConfinement,
        EnforcementFeature::ToolCommandAllowlist,
        EnforcementFeature::NetworkEgressRestriction,
        EnforcementFeature::EnvironmentAllowlist,
        EnforcementFeature::ResourceLimit,
        EnforcementFeature::AttemptIsolation,
        EnforcementFeature::CredentialDelivery,
    ])
}

/// The full matrix minus credential delivery: every obligation a `deny_all`
/// effective policy imposes is covered, but a config request cannot be served.
fn full_support_without_credentials() -> EnforcementSupport {
    EnforcementSupport::new(
        full_support()
            .features()
            .iter()
            .copied()
            .filter(|feature| *feature != EnforcementFeature::CredentialDelivery),
    )
}

fn profile(
    reference: EnforcementProfileRef,
    adapter_binding_key: &str,
    support: EnforcementSupport,
    subjects: impl IntoIterator<Item = EnforcementSubject>,
) -> ImportedEnforcementProfile {
    ImportedEnforcementProfile::import(
        reference,
        1,
        ADAPTER_KIND,
        adapter_binding_key,
        support,
        subjects,
    )
    .unwrap()
}

fn selected(
    provisioning: ResolvedProvisioningSelection,
    profile: ImportedEnforcementProfile,
    subject: EnforcementSubject,
) -> SecurityProvisioningSelection {
    SecurityProvisioningSelection {
        provisioning,
        profile,
        subject,
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

/// Admit one `SECURITY_V1` Task whose generation and effective policy are
/// `deny_all`. `deny_all` is a legal projection of the `ReadOnly` / `Disabled`
/// requirement and leaves an empty `RequiredAccess`.
fn admit_security_v1_task(kernel: &Kernel, tag: &str, pin: AgentTypeRef) -> TaskId {
    admit_security_v1_task_with_policy(kernel, tag, pin, None)
}

/// The same admission, optionally with an independent Task-authored fine policy
/// reference. The policy is published here with `deny_all` bytes, so it meets
/// the generation layer and `P_eff` stays `deny_all`: only the *reference*
/// differs.
fn admit_security_v1_task_with_policy(
    kernel: &Kernel,
    tag: &str,
    pin: AgentTypeRef,
    task_policy: Option<SandboxPolicyRef>,
) -> TaskId {
    let reference = policy_ref(&format!("generation-policy-{tag}"));
    kernel
        .publish_sandbox_policy(&reference, &NormalizedSandboxPolicy::deny_all())
        .unwrap();
    if let Some(policy) = &task_policy {
        kernel
            .publish_sandbox_policy(policy, &NormalizedSandboxPolicy::deny_all())
            .unwrap();
    }
    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(reference)),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "security_work".into(),
        objective: "security work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(task_spec()),
    };
    let proposal = kernel
        .compile_root_intent(&generation.generation_id, intent, "session", 1)
        .unwrap();
    let mut requirement = draft(pin);
    requirement.sandbox_policy = task_policy;
    kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            requirement,
            SecurityContractMode::SecurityV1,
        )
        .unwrap()
}

/// Admit one `COARSE_V1` typed Task, for the disjoint-chains check.
fn admit_coarse_task(kernel: &Kernel, pin: AgentTypeRef) -> TaskId {
    let generation = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "coarse_work".into(),
        objective: "coarse work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(task_spec()),
    };
    let proposal = kernel
        .compile_root_intent(&generation.generation_id, intent, "session", 1)
        .unwrap();
    kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap()
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

fn assert_candidate_rejection(error: &Error, needle: &str) {
    assert!(
        matches!(error, Error::InvalidAuthority(_)),
        "expected candidate-local invalid authority, got {error:?}"
    );
    assert!(error.to_string().contains(needle), "{error}");
}

/// Bind one READY agent to a `SECURITY_V1` type and pair it with the Task.
fn security_agent(kernel: &Kernel, secure_type: &AgentTypeRef) -> agentype_core::LogicalAgentId {
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, secure_type).unwrap();
    agent
}

#[test]
fn a_qualified_candidate_is_selectable() {
    let env = Env::new("qualified");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "qualified", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "qualified", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let acquired = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap();

    assert_eq!(acquired.claim.task_id, task);
    assert_eq!(acquired.provisioning_binding.agent_type, secure);
    assert_eq!(
        acquired.provisioning_binding.spawn_source,
        candidate.provisioning.spawn_source
    );
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Leased);
    assert_eq!(count(&env, "provisioning_bindings"), 1);
    assert_eq!(count(&env, "attempts"), 1);
    assert_eq!(count(&env, "leases"), 1);
}

#[test]
fn a_task_fine_policy_without_an_exact_evidence_reference_is_still_selectable() {
    let env = Env::new("task_fine");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    // The Task authors its own fine policy reference, distinct from the
    // Generation's. The durable `TaskSecurityBinding` meets both into `P_eff`,
    // but the imported evidence below claims no authored policy reference at
    // all. The exact-reference check belongs to `COARSE_V1`; a `SECURITY_V1`
    // candidate is proven by obligation coverage over `P_eff`.
    let task_policy = policy_ref("task-fine-policy");
    let task =
        admit_security_v1_task_with_policy(kernel, "task_fine", pin, Some(task_policy.clone()));
    let binding = kernel.get_task_security_binding(&task).unwrap().unwrap();
    assert!(
        binding
            .derivation()
            .inputs()
            .iter()
            .any(|input| input.reference() == Some(&task_policy)),
        "the Task authored layer must be part of the admitted derivation"
    );

    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "task_fine", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let acquired = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap();

    assert_eq!(acquired.claim.task_id, task);
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Leased);
    assert_eq!(count(&env, "attempts"), 1);
    assert_eq!(count(&env, "leases"), 1);
}

#[test]
fn the_binding_alias_resolves_to_the_selections_physical_key() {
    let env = Env::new("alias_key");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "alias_key", pin);
    let profile_ref = full_support_profile_ref();
    // The policy stores the stable alias `primary`; the resolver selection and
    // the imported profile name the exact physical key it resolved to, `k2`.
    let provisioning = security_catalog_with_binding(
        kernel,
        "alias_key",
        Some(&profile_ref),
        Vec::new(),
        "primary",
        "k2",
    );
    assert_eq!(provisioning.adapter_binding_key, "k2");
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            "k2",
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let acquired = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap();
    // The durable binding freezes the physical key, never the alias.
    assert_eq!(acquired.provisioning_binding.adapter_binding_key, "k2");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Leased);
}

#[test]
fn an_uncovered_profile_is_rejected_before_attempt_or_lease() {
    let env = Env::new("uncovered");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "uncovered", pin);
    // The policy-only reference profile claims workspace write and environment
    // only; a deny-all effective policy imposes read/execute/visibility/tool and
    // network obligations it does not cover.
    let profile_ref = reference_profile_ref();
    let provisioning = security_catalog(kernel, "uncovered", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            EnforcementSupport::policy_only_reference(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "does not cover");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn an_uncovered_subject_is_rejected_before_attempt_or_lease() {
    let env = Env::new("subject");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "subject", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "subject", Some(&profile_ref), Vec::new());
    // The profile covers every obligation but does not claim the executing
    // subject. The audit's P2-1 fix must turn that into ineligibility.
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::UnprovenExternalProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "does not claim the executing subject");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn a_profile_imported_for_another_physical_domain_is_rejected_before_attempt_or_lease() {
    let env = Env::new("binding");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "binding", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "binding", Some(&profile_ref), Vec::new());
    // Same reference, support matrix and subject, but imported for a different
    // physical adapter domain than the selection resolved: the validated tuple
    // must reject it.
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            "some-other-binding",
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "different physical adapter binding");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn the_durable_declaration_wins_over_the_callers_profile() {
    let env = Env::new("declaration");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "declaration", pin);
    // The source declares the policy-only reference profile; the caller presents
    // a different (stronger) profile for the same binding. The durable authority
    // names the declared profile, so the compiled plan cannot accept the impostor.
    let declared = reference_profile_ref();
    let provisioning = security_catalog(kernel, "declaration", Some(&declared), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            full_support_profile_ref(),
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "not the one the source declared");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// P1-2: a profile's `CredentialDelivery` claim is a necessary condition inside
/// the pure compile, never an authorization. The config requests
/// `vault://codex-prod`, `P_eff.credentials` authorizes nothing, and B5.3 owns
/// the canonical `CredentialRequest`, `CredentialEligibilityEvidence` and broker
/// qualification that do not exist yet, so the authority-bearing entry MUST
/// refuse before Attempt/Lease (ADR-0015 §5/§9).
#[test]
fn a_credential_bearing_config_is_fail_closed_even_when_the_profile_can_deliver() {
    let env = Env::new("credential-capable");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "credential-capable", pin);
    let profile_ref = full_support_profile_ref();
    // The full-support matrix includes `CredentialDelivery`, so the request gate
    // is the only thing that can refuse this candidate.
    let provisioning = security_catalog(
        kernel,
        "credential-capable",
        Some(&profile_ref),
        vec![CredentialRef::new("vault://codex-prod").unwrap()],
    );
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "credential");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// The same request is refused when the profile cannot deliver either: the
/// fail-closed rule is unconditional, not a capability negotiation.
#[test]
fn a_requested_credential_the_profile_cannot_deliver_is_rejected_before_attempt_or_lease() {
    let env = Env::new("credential");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "credential", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(
        kernel,
        "credential",
        Some(&profile_ref),
        vec![CredentialRef::new("vault://codex-prod").unwrap()],
    );
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support_without_credentials(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "credential");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// P1-1: the security path must intersect the frozen B.4 eligibility. Evidence
/// resolved for a *different* `SourceConfig` is a real B.4 subject, just not
/// this config's, and must never authorize the selected config. The frontier is
/// refreshed so the stale-frontier fence is not what rejects the candidate.
#[test]
fn evidence_resolved_for_another_config_cannot_authorize_this_config() {
    let env = Env::new("evidence-config");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "evidence-config", pin);
    let profile_ref = full_support_profile_ref();
    let mut provisioning =
        security_catalog(kernel, "evidence-config", Some(&profile_ref), Vec::new());

    let body = json!({"model": "another"});
    let other_digest = ConfigDigest::new(canonical_json_body_digest(&body)).unwrap();
    let other = SourceConfig {
        config_ref: SourceConfigRef::new(provisioning.spawn_source.clone(), "config-other", 1)
            .unwrap(),
        config_digest: other_digest.clone(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: Vec::new(),
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    kernel
        .publish_source_config(&other, &SourceConfigBody::OpaqueJson(body))
        .unwrap();
    provisioning.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        provisioning.adapter_policy.clone(),
        ADAPTER_KIND,
        ADAPTER_BINDING_KEY,
        provisioning.spawn_source.clone(),
        other.config_ref.clone(),
        other_digest,
        safety(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    provisioning.catalog_frontier_digest = kernel.catalog_frontier_digest().unwrap();

    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "exact source config digest");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

fn publish_functional_capability(kernel: &Kernel) -> CapabilityRef {
    let reference = CapabilityRef::new("process.spawn", 1).unwrap();
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
    reference
}

/// P1-1: an AgentType functional capability the source/config envelope cannot
/// provide is B.4 ineligibility. The Task itself does not require it, so only
/// `can_provision` can reject it.
#[test]
fn an_agent_type_capability_the_source_cannot_provide_is_rejected() {
    let env = Env::new("functional");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let reference = publish_functional_capability(kernel);
    let authored = policy_ref("agent-policy-functional");
    kernel
        .publish_sandbox_policy(&authored, &NormalizedSandboxPolicy::deny_all())
        .unwrap();
    let mut contract = security_contract(authored);
    contract
        .required_capabilities
        .insert(reference, CapabilityValue::Bool(true));
    let secure = publish_security_agent_type_with_contract(kernel, "functional", contract);
    let task = admit_security_v1_task(kernel, "functional", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "functional", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "process.spawn");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// P1-1: the config/source continuity envelope is a B.4 conjunct. The AgentType
/// advertises `Logical` and the Task requires nothing, so the security policy
/// qualification passes; only the physical continuity check can reject it.
#[test]
fn a_config_that_cannot_realize_the_agent_type_continuity_is_rejected() {
    let env = Env::new("continuity");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let authored = policy_ref("agent-policy-continuity");
    kernel
        .publish_sandbox_policy(&authored, &NormalizedSandboxPolicy::deny_all())
        .unwrap();
    let mut contract = security_contract(authored);
    contract.continuity = ContinuityMode::Logical;
    let secure = publish_security_agent_type_with_contract(kernel, "continuity", contract);
    let task = admit_security_v1_task(kernel, "continuity", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "continuity", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "continuity");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// P1-1: the source/config lifecycle must realize the AgentType lifecycle.
#[test]
fn a_config_that_cannot_realize_the_agent_type_lifecycle_is_rejected() {
    let env = Env::new("lifecycle");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let authored = policy_ref("agent-policy-lifecycle");
    kernel
        .publish_sandbox_policy(&authored, &NormalizedSandboxPolicy::deny_all())
        .unwrap();
    let mut contract = security_contract(authored);
    contract.lifecycle = [LifecycleMode::Resident].into_iter().collect();
    let secure = publish_security_agent_type_with_contract(kernel, "lifecycle", contract);
    let task = admit_security_v1_task(kernel, "lifecycle", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "lifecycle", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "lifecycle");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// P1-1: a `DISABLED` SourceConfig is not eligible even when the caller refreshes
/// the frontier fence, so the ACTIVE disposition is a per-candidate conjunct and
/// not merely an artifact of the selection snapshot.
#[test]
fn a_disabled_source_config_is_rejected_even_with_a_refreshed_frontier() {
    let env = Env::new("disabled-config");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "disabled-config", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "disabled-config", Some(&profile_ref), Vec::new());
    let mut candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    kernel
        .set_source_config_status(
            &candidate.provisioning.source_config,
            ConfigStatus::Disabled,
        )
        .unwrap();
    candidate.provisioning.catalog_frontier_digest = kernel.catalog_frontier_digest().unwrap();
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "not active");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn the_two_acquisition_chains_are_disjoint() {
    let env = Env::new("disjoint");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");

    // The coarse entry point still refuses a SECURITY_V1 Task (B5.1 gate).
    let security_task = admit_security_v1_task(kernel, "disjoint", pin.clone());
    let coarse_selection = security_catalog(
        kernel,
        "disjoint-coarse",
        Some(&full_support_profile_ref()),
        Vec::new(),
    );
    let agent = security_agent(kernel, &secure);
    let error = kernel
        .acquire_typed_task_existing(&security_task, &agent, &coarse_selection, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "SECURITY_V1");

    // The security entry point refuses a COARSE_V1 Task rather than silently
    // treating it as a weaker security candidate.
    let coarse_task = admit_coarse_task(kernel, pin);
    let profile_ref = full_support_profile_ref();
    let provisioning =
        security_catalog(kernel, "disjoint-security", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let error = kernel
        .acquire_security_v1_task_existing(&coarse_task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "only acquires a SECURITY_V1 task");

    assert_eq!(
        kernel.task(&security_task).unwrap().state,
        TaskState::Queued
    );
    assert_eq!(kernel.task(&coarse_task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

#[test]
fn a_coarse_agent_type_is_not_a_security_candidate() {
    let env = Env::new("coarse-agent");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let task = admit_security_v1_task(kernel, "coarse-agent", pin.clone());
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "coarse-agent", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &pin);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    // The acquisition-level gate's own message, not merely the later authority
    // constructor's: the candidate must be refused before the composite is built.
    assert_candidate_rejection(
        &error,
        "a COARSE_V1 agent type cannot execute a SECURITY_V1 task",
    );
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// The requested subject is the one the proof is about: the same profile and
/// support matrix is acquired successfully when the caller asks for the subject
/// the profile actually claims, so the subject is not silently replaced by a
/// default.
#[test]
fn the_callers_subject_is_the_one_proved() {
    let env = Env::new("requested-subject");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "requested-subject", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning =
        security_catalog(kernel, "requested-subject", Some(&profile_ref), Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ExternallyDelegatedExecution],
        ),
        EnforcementSubject::ExternallyDelegatedExecution,
    );
    let agent = security_agent(kernel, &secure);

    let acquired = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap();
    assert_eq!(acquired.claim.task_id, task);
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Leased);
}

/// The security chain must refuse a `COARSE_V1` source. Without its own
/// declaration there is no imported profile, no `P_source` identity and no
/// enforcement subject, so the source cannot carry `SECURITY_V1` authority even
/// when the rest of the tuple qualifies.
#[test]
fn a_coarse_source_cannot_provision_a_security_task() {
    let env = Env::new("coarse-source");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let secure = publish_security_agent_type(kernel, "secure");
    let task = admit_security_v1_task(kernel, "coarse-source", pin);
    let profile_ref = full_support_profile_ref();
    let provisioning = security_catalog(kernel, "coarse-source", None, Vec::new());
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(
        &error,
        "a COARSE_V1 spawn source cannot provision a SECURITY_V1 task",
    );
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}

/// A declared `P_source` ceiling must reach the composite authority. The two
/// terms require different working directories, so `meet` has no representable
/// common policy: if the acquisition skipped the source layer (or loaded `None`
/// for a declaration that carries one) this candidate would be acquired.
#[test]
fn an_impossible_source_ceiling_is_candidate_local_ineligibility() {
    let env = Env::new("source-ceiling");
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "pinned");
    let task = admit_security_v1_task(kernel, "source-ceiling", pin);

    // A SECURITY_V1 source whose authored `P_source` is `deny_all` plus a
    // required working directory. Attempt isolation stays false, so publication
    // accepts it.
    let source_policy = deny_all_requiring_workdir("source");
    let policy = AdapterBindingPolicy {
        policy_ref: AdapterPolicyRef::new("policy-source-ceiling", 1).unwrap(),
        adapter_kind: ADAPTER_KIND.into(),
        binding_ref: ADAPTER_BINDING_KEY.into(),
        required_safety: safety(),
        status: ConfigStatus::Active,
    };
    kernel.publish_adapter_binding_policy(&policy).unwrap();
    let source_reference = policy_ref("source-ceiling-policy");
    kernel
        .publish_sandbox_policy(&source_reference, &source_policy)
        .unwrap();
    let source = SpawnSource {
        source_ref: SpawnSourceRef::new("source-source-ceiling", 1).unwrap(),
        adapter_policy: policy.policy_ref.clone(),
        lifecycle_modes: [LifecycleMode::Ephemeral].into_iter().collect(),
        continuity_modes: [ContinuityMode::None].into_iter().collect(),
        functional_envelope: BTreeMap::new(),
        claims: Vec::new(),
        status: SourceStatus::Active,
    };
    let profile_ref = full_support_profile_ref();
    let declaration = SpawnSourceSecurityDeclaration::with_policy(
        source.source_ref.clone(),
        PolicyInput::authored(source_reference, &source_policy),
        profile_ref.clone(),
    );
    kernel
        .publish_spawn_source_with_security(&source, Some(&declaration))
        .unwrap();
    let body = json!({"model": "opaque"});
    let config_digest = ConfigDigest::new(canonical_json_body_digest(&body)).unwrap();
    let config = SourceConfig {
        config_ref: SourceConfigRef::new(source.source_ref.clone(), "config-source-ceiling", 1)
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
        ADAPTER_KIND,
        ADAPTER_BINDING_KEY,
        source.source_ref.clone(),
        config.config_ref.clone(),
        config_digest,
        safety(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let provisioning = ResolvedProvisioningSelection {
        spawn_source: source.source_ref,
        source_config: config.config_ref,
        adapter_policy: policy.policy_ref,
        evidence,
        adapter_kind: ADAPTER_KIND.into(),
        adapter_binding_key: ADAPTER_BINDING_KEY.into(),
        provisioning_protocol: "agentype-test-source/1".into(),
        attested_materialization_digest: MaterializationDigest::new(format!(
            "sha256:{}",
            "2".repeat(64)
        ))
        .unwrap(),
        catalog_frontier_digest: kernel.catalog_frontier_digest().unwrap(),
    };
    let candidate = selected(
        provisioning,
        profile(
            profile_ref,
            ADAPTER_BINDING_KEY,
            full_support(),
            [EnforcementSubject::ControlledPrimaryProcess],
        ),
        EnforcementSubject::ControlledPrimaryProcess,
    );
    // The bound AgentType's authored policy requires a *different* directory, so
    // only the source term can make the meet unsatisfiable.
    let secure = publish_security_agent_type_with_policy(
        kernel,
        "secure-ceiling",
        deny_all_requiring_workdir("agent"),
    );
    let agent = security_agent(kernel, &secure);

    let error = kernel
        .acquire_security_v1_task_existing(&task, &agent, &candidate, &registry())
        .unwrap_err();
    assert_candidate_rejection(&error, "source ceiling and candidate policies do not meet");
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
    assert_no_acquisition_side_effects(&env);
}
