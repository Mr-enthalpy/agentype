//! M6-B.2 catalog persistence conformance (schema v6):
//! immutable revisions, canonical content digests, `based_on` provenance and
//! refinement, disposition separation/monotonicity, and restart durability.

use agentype_agent_contract::{
    canonical_json_body_digest, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget, CapabilityCatalog,
    CapabilityDefinition, CapabilityPolarity, CapabilityRef, ConfigDigest, ConfigStatus,
    ContinuityMode, CredentialRef, LifecycleMode, MatcherKind, NetworkPolicy, PhysicalSafety,
    SecurityClass, SecurityContract, SourceConfig, SourceConfigRef, SourceStatus, SpawnSource,
    SpawnSourceRef,
};
use agentype_core::{Clock, Error, InformationFunction, ManualClock, WorkspaceMode};
use agentype_storage_sqlite::{
    AgentTypeStatus, ConfigMode, Kernel, SourceConfigBody, SCHEMA_VERSION,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

fn memory_kernel() -> Kernel {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap()
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn cref(id: &str, revision: u64) -> CapabilityRef {
    CapabilityRef::new(id, revision).unwrap()
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn policy_ref(id: &str, revision: u64) -> AdapterPolicyRef {
    AdapterPolicyRef::new(id, revision).unwrap()
}

fn definition(
    matcher: MatcherKind,
    class: SecurityClass,
    polarity: CapabilityPolarity,
) -> CapabilityDefinition {
    CapabilityDefinition {
        matcher_kind: matcher,
        security_class: class,
        polarity,
    }
}

fn full_safety() -> PhysicalSafety {
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
    .unwrap()
}

fn agent_contract() -> AgentTypeContract {
    AgentTypeContract {
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
    }
}

fn base_agent(id: &str, revision: u64, based_on: Option<AgentTypeRef>) -> AgentType {
    AgentType {
        type_ref: type_ref(id, revision),
        based_on,
        contract: agent_contract(),
    }
}

fn adapter_policy() -> AdapterBindingPolicy {
    AdapterBindingPolicy::new(
        policy_ref("codex-local-adapter", 3),
        "codex_cli",
        "workstation-primary",
        full_safety(),
        ConfigStatus::Active,
    )
    .unwrap()
}

fn base_source() -> SpawnSource {
    SpawnSource {
        source_ref: SpawnSourceRef::new("codex-local", 2).unwrap(),
        adapter_policy: policy_ref("codex-local-adapter", 3),
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

fn base_config(digest: &str, refs: Vec<CredentialRef>) -> SourceConfig {
    SourceConfig {
        config_ref: SourceConfigRef::new(base_source().source_ref, "deep-reasoning", 7).unwrap(),
        config_digest: ConfigDigest::new(digest).unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: refs,
        claims: Vec::new(),
        status: ConfigStatus::Active,
    }
}

fn publish_catalog(kernel: &Kernel) -> CapabilityCatalog {
    kernel
        .publish_capability_definition(
            &cref("terminal.attach", 1),
            &definition(
                MatcherKind::Bool,
                SecurityClass::Functional,
                CapabilityPolarity::Ability,
            ),
        )
        .unwrap();
    kernel.load_capability_catalog().unwrap()
}

#[test]
fn schema_version_is_six() {
    assert_eq!(SCHEMA_VERSION, 6);
    assert_eq!(memory_kernel().schema_version().unwrap(), 6);
}

#[test]
fn capability_definition_round_trips_and_is_immutable() {
    let kernel = memory_kernel();
    let reference = cref("tools", 1);
    let spec = definition(
        MatcherKind::Set,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    );
    let digest = kernel
        .publish_capability_definition(&reference, &spec)
        .unwrap();
    // Idempotent republish of identical content.
    assert_eq!(
        kernel
            .publish_capability_definition(&reference, &spec)
            .unwrap(),
        digest
    );
    let catalog = kernel.load_capability_catalog().unwrap();
    let loaded = catalog.get(&reference).unwrap();
    assert_eq!(loaded.matcher_kind, MatcherKind::Set);
    assert_eq!(loaded.security_class, SecurityClass::Sandbox);
    assert_eq!(loaded.polarity, CapabilityPolarity::Restriction);

    // Redefining the exact revision with different semantics fails closed.
    let conflicting = definition(
        MatcherKind::Bool,
        SecurityClass::Sandbox,
        CapabilityPolarity::Restriction,
    );
    let err = kernel
        .publish_capability_definition(&reference, &conflicting)
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");
}

#[test]
fn agent_type_publish_read_back_and_selector() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);
    let agent = base_agent("general-reviewer", 1, None);
    let digest = kernel.publish_agent_type(&agent).unwrap();
    assert!(digest.starts_with("sha256:"));

    let (loaded, status) = kernel
        .get_agent_type(&type_ref("general-reviewer", 1))
        .unwrap()
        .unwrap();
    assert_eq!(loaded, agent);
    assert_eq!(status, AgentTypeStatus::Published);

    // Exact and Latest selectors both resolve to the exact revision.
    assert_eq!(
        kernel
            .resolve_agent_type_selector(&AgentTypeSelector::Exact(type_ref("general-reviewer", 1)))
            .unwrap(),
        type_ref("general-reviewer", 1)
    );
    assert_eq!(
        kernel
            .resolve_agent_type_selector(&AgentTypeSelector::latest("general-reviewer").unwrap())
            .unwrap(),
        type_ref("general-reviewer", 1)
    );

    // Idempotent republish, then a conflicting content change on the same ref.
    assert_eq!(
        kernel.publish_agent_type(&agent).unwrap(),
        digest,
        "identical content is idempotent"
    );
    let mut changed = agent.clone();
    changed.contract.budget_ceiling = Budget::new(101.0).unwrap();
    let err = kernel.publish_agent_type(&changed).unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");

    // Deprecation is monotonic and removes the revision from Latest resolution.
    kernel
        .set_agent_type_status(
            &type_ref("general-reviewer", 1),
            AgentTypeStatus::Deprecated,
        )
        .unwrap();
    let (_, status) = kernel
        .get_agent_type(&type_ref("general-reviewer", 1))
        .unwrap()
        .unwrap();
    assert_eq!(status, AgentTypeStatus::Deprecated);
    assert!(kernel
        .resolve_agent_type_selector(&AgentTypeSelector::latest("general-reviewer").unwrap())
        .is_err());
    assert!(kernel
        .set_agent_type_status(&type_ref("general-reviewer", 1), AgentTypeStatus::Published)
        .is_err());
}

#[test]
fn agent_type_based_on_provenance_and_refinement() {
    let kernel = memory_kernel();
    let base = base_agent("general-reviewer", 1, None);
    kernel.publish_agent_type(&base).unwrap();

    // A narrowed derived type with matching provenance publishes.
    let mut derived = base_agent(
        "readonly-reviewer",
        1,
        Some(type_ref("general-reviewer", 1)),
    );
    derived.contract.budget_ceiling = Budget::new(50.0).unwrap();
    kernel.publish_agent_type(&derived).unwrap();

    // A widening refinement is rejected.
    let mut widening = base_agent("wide-reviewer", 1, Some(type_ref("general-reviewer", 1)));
    widening.contract.budget_ceiling = Budget::new(200.0).unwrap();
    let err = kernel.publish_agent_type(&widening).unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");

    // Missing base fails closed rather than trusting the field.
    let orphan = base_agent("orphan", 1, Some(type_ref("does-not-exist", 1)));
    let err = kernel.publish_agent_type(&orphan).unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
}

#[test]
fn spawn_source_and_opaque_config_round_trip() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    kernel.publish_spawn_source(&base_source()).unwrap();
    assert_eq!(
        kernel
            .get_spawn_source(&SpawnSourceRef::new("codex-local", 2).unwrap())
            .unwrap()
            .unwrap(),
        base_source()
    );

    let payload = serde_json::json!({"model": "x", "provider": "y"});
    let digest = canonical_json_body_digest(&payload);
    let config = base_config(
        &digest,
        vec![CredentialRef::new("vault://codex-prod").unwrap()],
    );
    kernel
        .publish_source_config(&config, &SourceConfigBody::OpaqueJson(payload.clone()))
        .unwrap();

    let loaded = kernel
        .get_source_config(&config.config_ref)
        .unwrap()
        .unwrap();
    assert_eq!(loaded, config);
    assert_eq!(
        kernel.get_source_config_mode(&config.config_ref).unwrap(),
        Some(ConfigMode::OpaqueJson)
    );

    // A mismatched declared digest fails closed: the body does not match the
    // digest the config was built with.
    let mut bad = config.clone();
    bad.config_ref = SourceConfigRef::new(base_source().source_ref, "fast-review", 1).unwrap();
    let err = kernel
        .publish_source_config(
            &bad,
            &SourceConfigBody::OpaqueJson(serde_json::json!({"different": true})),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");
}

#[test]
fn external_ref_config_and_unknown_source_fail_closed() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    kernel.publish_spawn_source(&base_source()).unwrap();

    let config = base_config("external://codex/config.toml", Vec::new());
    kernel
        .publish_source_config(&config, &SourceConfigBody::ExternalRef)
        .unwrap();
    assert_eq!(
        kernel.get_source_config_mode(&config.config_ref).unwrap(),
        Some(ConfigMode::ExternalRef)
    );

    // A config whose source revision does not exist fails closed.
    let orphan =
        SourceConfigRef::new(SpawnSourceRef::new("codex-local", 99).unwrap(), "x", 1).unwrap();
    let orphan_config = SourceConfig {
        config_ref: orphan,
        config_digest: ConfigDigest::new("sha256:abc").unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: Vec::new(),
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    let err = kernel
        .publish_source_config(&orphan_config, &SourceConfigBody::ExternalRef)
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
}

#[test]
fn disposition_is_separate_and_monotonic() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    let source = base_source();
    let digest = kernel.publish_spawn_source(&source).unwrap();

    // A disposition change does not alter revision content: republishing the
    // same content stays idempotent and returns the original digest.
    kernel
        .set_spawn_source_status(&source.source_ref, SourceStatus::Draining)
        .unwrap();
    let loaded = kernel
        .get_spawn_source(&source.source_ref)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, SourceStatus::Draining);
    assert_eq!(kernel.publish_spawn_source(&source).unwrap(), digest);

    // Monotonic ACTIVE -> DRAINING -> DISABLED; reverse fails closed.
    kernel
        .set_spawn_source_status(&source.source_ref, SourceStatus::Disabled)
        .unwrap();
    assert!(kernel
        .set_spawn_source_status(&source.source_ref, SourceStatus::Active)
        .is_err());
}

#[test]
fn catalog_is_durable_across_restart() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");

    let agent = base_agent("general-reviewer", 1, None);
    let source = base_source();
    let policy = adapter_policy();
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_adapter_binding_policy(&policy).unwrap();
        kernel.publish_spawn_source(&source).unwrap();
        kernel.publish_agent_type(&agent).unwrap();
    }

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let reopened = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    let (loaded, status) = reopened.get_agent_type(&agent.type_ref).unwrap().unwrap();
    assert_eq!(loaded, agent);
    assert_eq!(status, AgentTypeStatus::Published);
    assert_eq!(
        reopened
            .get_spawn_source(&source.source_ref)
            .unwrap()
            .unwrap(),
        source
    );
    // Recomputing and republishing the same content still matches the stored
    // digest, proving the persisted canonical digest survived the restart.
    reopened.publish_agent_type(&agent).unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}
