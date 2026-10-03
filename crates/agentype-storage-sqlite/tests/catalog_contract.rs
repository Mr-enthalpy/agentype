//! M6-B.2 catalog persistence conformance (schema v6):
//! immutable revisions, canonical content digests, `based_on` provenance and
//! refinement, disposition separation/monotonicity, and restart durability.

use agentype_agent_contract::{
    canonical_json_body_digest, content_digest, AdapterBindingPolicy, AdapterPolicyRef,
    AffinityConstraint, AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector, Assurance,
    Budget, CapabilityCatalog, CapabilityClaim, CapabilityDefinition, CapabilityPolarity,
    CapabilityRef, CapabilityValue, ConfigDigest, ConfigStatus, ContinuityMode, CredentialRef,
    LifecycleMode, MatcherKind, NetworkPolicy, PhysicalSafety, SecurityClass, SecurityContract,
    SourceConfig, SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef,
};
use agentype_core::{Clock, Error, InformationFunction, ManualClock, WorkspaceMode};
use agentype_storage_sqlite::{
    AgentTypeStatus, ConfigMode, Kernel, SourceConfigBody, SourceConfigRevision, SCHEMA_VERSION,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

/// A syntactically valid `sha256:<64 lowercase hex>` config digest.
const VALID_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

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

/// Define two evidence-class (no functional envelope needed) Bool capabilities.
fn publish_claims_catalog(kernel: &Kernel) {
    for id in ["terminal.attach", "terminal.network"] {
        kernel
            .publish_capability_definition(
                &cref(id, 1),
                &definition(
                    MatcherKind::Bool,
                    SecurityClass::Authority,
                    CapabilityPolarity::Ability,
                ),
            )
            .unwrap();
    }
}

fn claim(id: &str, value: bool) -> CapabilityClaim {
    CapabilityClaim {
        reference: cref(id, 1),
        value: CapabilityValue::Bool(value),
        assurance: Assurance::Enforced,
        declaration_provenance_ref: None,
    }
}

fn source_with_two_claims() -> SpawnSource {
    let mut source = base_source();
    source.claims = vec![
        claim("terminal.attach", true),
        claim("terminal.network", true),
    ];
    source
}

/// Rewrite `content_json` via `mutate` and recompute a matching `content_digest`,
/// so the row is self-consistent but (deliberately) not canonical.
fn tamper_json(
    path: &Path,
    table: &str,
    where_clause: &str,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let json: String = conn
        .query_row(
            &format!("SELECT content_json FROM {table} WHERE {where_clause}"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    mutate(&mut value);
    let tampered = serde_json::to_string(&value).unwrap();
    let digest = content_digest(tampered.as_bytes());
    conn.execute(
        &format!("UPDATE {table} SET content_json=?1, content_digest=?2 WHERE {where_clause}"),
        rusqlite::params![tampered, digest],
    )
    .unwrap();
}

/// Corrupt `content_json` without touching `content_digest`.
fn corrupt_json_only(path: &Path, table: &str, where_clause: &str, suffix: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        &format!(
            "UPDATE {table} SET content_json = content_json || '{suffix}' WHERE {where_clause}"
        ),
        [],
    )
    .unwrap();
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
fn external_ref_config_keeps_locator_and_digest_distinct() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    kernel.publish_spawn_source(&base_source()).unwrap();

    let locator = "file:///etc/codex/config.toml";
    let config = base_config(VALID_DIGEST, Vec::new());
    kernel
        .publish_source_config(
            &config,
            &SourceConfigBody::ExternalRef {
                locator: locator.into(),
            },
        )
        .unwrap();
    assert_eq!(
        kernel.get_source_config_mode(&config.config_ref).unwrap(),
        Some(ConfigMode::ExternalRef)
    );
    // The locator is stored verbatim and separately from the config digest.
    assert_eq!(
        kernel
            .get_source_config_locator(&config.config_ref)
            .unwrap(),
        Some(locator.to_string())
    );
    assert_ne!(config.config_digest.as_str(), locator);

    // An invalid content digest grammar is rejected.
    let invalid = base_config("sha256:abc", Vec::new());
    let err = kernel
        .publish_source_config(
            &invalid,
            &SourceConfigBody::ExternalRef {
                locator: locator.into(),
            },
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");

    // A locator *value* may legitimately equal the digest value (for example a
    // content-addressed locator that is both where the content lives and its
    // digest). They remain two distinct fields; Core must not reject equality.
    let shared = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut content_addressed = base_config(shared, Vec::new());
    content_addressed.config_ref =
        SourceConfigRef::new(base_source().source_ref, "content-addressed", 1).unwrap();
    kernel
        .publish_source_config(
            &content_addressed,
            &SourceConfigBody::ExternalRef {
                locator: shared.into(),
            },
        )
        .unwrap();
    assert_eq!(
        kernel
            .get_source_config_locator(&content_addressed.config_ref)
            .unwrap(),
        Some(shared.to_string())
    );
    assert_eq!(
        kernel
            .get_source_config(&content_addressed.config_ref)
            .unwrap()
            .unwrap()
            .config_digest
            .as_str(),
        shared
    );

    // An empty locator is rejected.
    let err = kernel
        .publish_source_config(
            &base_config(VALID_DIGEST, Vec::new()),
            &SourceConfigBody::ExternalRef {
                locator: "  ".into(),
            },
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");

    // A config whose source revision does not exist fails closed.
    let orphan =
        SourceConfigRef::new(SpawnSourceRef::new("codex-local", 99).unwrap(), "x", 1).unwrap();
    let orphan_config = SourceConfig {
        config_ref: orphan,
        config_digest: ConfigDigest::new(VALID_DIGEST).unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: Vec::new(),
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    let err = kernel
        .publish_source_config(
            &orphan_config,
            &SourceConfigBody::ExternalRef {
                locator: "file:///x".into(),
            },
        )
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
fn publication_persists_caller_initial_disposition() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);

    let policy = AdapterBindingPolicy::new(
        policy_ref("codex-local-adapter", 3),
        "codex_cli",
        "workstation-primary",
        full_safety(),
        ConfigStatus::Disabled,
    )
    .unwrap();
    kernel.publish_adapter_binding_policy(&policy).unwrap();
    assert_eq!(
        kernel
            .get_adapter_binding_policy(&policy.policy_ref)
            .unwrap()
            .unwrap()
            .status,
        ConfigStatus::Disabled
    );

    let mut source = base_source();
    source.status = SourceStatus::Disabled;
    kernel.publish_spawn_source(&source).unwrap();
    assert_eq!(
        kernel
            .get_spawn_source(&source.source_ref)
            .unwrap()
            .unwrap()
            .status,
        SourceStatus::Disabled
    );

    let mut config = base_config(VALID_DIGEST, Vec::new());
    config.status = ConfigStatus::Draining;
    kernel
        .publish_source_config(
            &config,
            &SourceConfigBody::ExternalRef {
                locator: "file:///x".into(),
            },
        )
        .unwrap();
    assert_eq!(
        kernel
            .get_source_config(&config.config_ref)
            .unwrap()
            .unwrap()
            .status,
        ConfigStatus::Draining
    );
}

#[test]
fn read_verifies_stored_content_integrity() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-i-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let agent = base_agent("general-reviewer", 1, None);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&agent).unwrap();
    }
    // Tamper with the stored document without updating its content digest.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE agent_types SET content_json = content_json || ' ' WHERE type_id='general-reviewer'",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    let err = kernel.get_agent_type(&agent.type_ref).unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got {err:?}");

    let _ = std::fs::remove_dir_all(&dir);
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

#[test]
fn external_ref_locator_is_stored_verbatim() {
    let kernel = memory_kernel();
    publish_catalog(&kernel);
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    kernel.publish_spawn_source(&base_source()).unwrap();

    // A locator may be byte-significant to its source; Core must not trim it.
    let locator = "  file:///etc/codex/config.toml  ";
    let config = base_config(VALID_DIGEST, Vec::new());
    kernel
        .publish_source_config(
            &config,
            &SourceConfigBody::ExternalRef {
                locator: locator.into(),
            },
        )
        .unwrap();
    assert_eq!(
        kernel
            .get_source_config_locator(&config.config_ref)
            .unwrap(),
        Some(locator.to_string())
    );
}

#[test]
fn source_config_column_drift_fails_closed() {
    // Locator column drift.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-l-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let config = base_config(VALID_DIGEST, Vec::new());
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel
            .publish_adapter_binding_policy(&adapter_policy())
            .unwrap();
        kernel.publish_spawn_source(&base_source()).unwrap();
        kernel
            .publish_source_config(
                &config,
                &SourceConfigBody::ExternalRef {
                    locator: "file:///etc/codex/config.toml".into(),
                },
            )
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE source_configs SET config_locator='file:///tampered' WHERE config_id='deep-reasoning'",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    assert!(matches!(
        kernel.get_source_config(&config.config_ref),
        Err(Error::InvariantViolation(_))
    ));
    assert!(matches!(
        kernel.get_source_config_locator(&config.config_ref),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);

    // Digest column drift (different but canonical digest).
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-d-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel
            .publish_adapter_binding_policy(&adapter_policy())
            .unwrap();
        kernel.publish_spawn_source(&base_source()).unwrap();
        kernel
            .publish_source_config(
                &config,
                &SourceConfigBody::ExternalRef {
                    locator: "file:///etc/codex/config.toml".into(),
                },
            )
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let other = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
        conn.execute(
            "UPDATE source_configs SET config_digest=?1 WHERE config_id='deep-reasoning'",
            rusqlite::params![other],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    assert!(matches!(
        kernel.get_source_config(&config.config_ref),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opaque_payload_drift_fails_closed() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-p-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let payload = serde_json::json!({"model": "x"});
    let config = base_config(&canonical_json_body_digest(&payload), Vec::new());
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel
            .publish_adapter_binding_policy(&adapter_policy())
            .unwrap();
        kernel.publish_spawn_source(&base_source()).unwrap();
        kernel
            .publish_source_config(&config, &SourceConfigBody::OpaqueJson(payload))
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE source_configs SET config_payload_json='{\"model\":\"tampered\"}' WHERE config_id='deep-reasoning'",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    assert!(matches!(
        kernel.get_source_config(&config.config_ref),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

fn seed_source(path: &Path) {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    let kernel = Kernel::open(path, clock, 10.0, MAX_BYTES).unwrap();
    publish_claims_catalog(&kernel);
    kernel
        .publish_adapter_binding_policy(&adapter_policy())
        .unwrap();
    kernel
        .publish_spawn_source(&source_with_two_claims())
        .unwrap();
}

fn assert_source_read_fails(tag: &str, mutate: impl FnOnce(&mut serde_json::Value)) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-c-{tag}-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    seed_source(&path);
    tamper_json(
        &path,
        "spawn_sources",
        "source_id='codex-local' AND revision=2",
        mutate,
    );
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    let reference = SpawnSourceRef::new("codex-local", 2).unwrap();
    assert!(
        matches!(
            kernel.get_spawn_source(&reference),
            Err(Error::InvariantViolation(_))
        ),
        "case {tag} must fail closed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_canonical_claim_rows_fail_closed() {
    // A self-consistent but non-canonical document (matching digest) must still
    // be rejected: the read authority re-canonicalizes, it does not merely
    // round-trip the decoder.
    assert_source_read_fails("duplicate", |value| {
        let first = value["claims"][0].clone();
        value["claims"].as_array_mut().unwrap().push(first);
    });
    assert_source_read_fails("permutation", |value| {
        value["claims"].as_array_mut().unwrap().reverse();
    });
    assert_source_read_fails("bool-false", |value| {
        value["claims"][0]["value"] = serde_json::json!({"kind": "BOOL", "value": false});
    });
}

#[test]
fn non_canonical_credential_ref_rows_fail_closed() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-r-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let a = CredentialRef::new("vault://a").unwrap();
    let b = CredentialRef::new("vault://b").unwrap();
    let config = base_config(VALID_DIGEST, vec![b, a]);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel
            .publish_adapter_binding_policy(&adapter_policy())
            .unwrap();
        kernel.publish_spawn_source(&base_source()).unwrap();
        kernel
            .publish_source_config(
                &config,
                &SourceConfigBody::ExternalRef {
                    locator: "file:///etc/codex/config.toml".into(),
                },
            )
            .unwrap();
    }
    tamper_json(
        &path,
        "source_configs",
        "config_id='deep-reasoning' AND config_revision=7",
        |value| {
            value["credential_refs"].as_array_mut().unwrap().reverse();
        },
    );
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    assert!(matches!(
        kernel.get_source_config(&config.config_ref),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn blank_adapter_binding_policy_is_rejected() {
    let kernel = memory_kernel();
    let valid_safety = full_safety();
    let blank_kind = AdapterBindingPolicy {
        policy_ref: policy_ref("p", 1),
        adapter_kind: "".into(),
        binding_ref: "workstation-primary".into(),
        required_safety: valid_safety.clone(),
        status: ConfigStatus::Active,
    };
    assert!(matches!(
        kernel.publish_adapter_binding_policy(&blank_kind),
        Err(Error::InvariantViolation(_))
    ));
    let blank_ref = AdapterBindingPolicy {
        policy_ref: policy_ref("p", 1),
        adapter_kind: "codex_cli".into(),
        binding_ref: "   ".into(),
        required_safety: valid_safety,
        status: ConfigStatus::Active,
    };
    assert!(matches!(
        kernel.publish_adapter_binding_policy(&blank_ref),
        Err(Error::InvariantViolation(_))
    ));
}

#[test]
fn idempotent_republish_over_corrupt_row_fails_closed() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-q-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let agent = base_agent("general-reviewer", 1, None);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&agent).unwrap();
    }
    // Corrupt the stored document without changing the recorded digest.
    corrupt_json_only(
        &path,
        "agent_types",
        "type_id='general-reviewer' AND revision=1",
        " ",
    );
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    // Idempotent republish of the same content must not report success over a
    // corrupted existing revision.
    assert!(matches!(
        kernel.publish_agent_type(&agent),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

fn assert_selector_lookup_fails(kernel: &Kernel, agent: &AgentType) {
    assert!(matches!(
        kernel.resolve_agent_type_selector(&AgentTypeSelector::Exact(agent.type_ref.clone())),
        Err(Error::InvariantViolation(_))
    ));
    assert!(matches!(
        kernel.resolve_agent_type_selector(
            &AgentTypeSelector::latest(agent.type_ref.id().as_str()).unwrap()
        ),
        Err(Error::InvariantViolation(_))
    ));
}

#[test]
fn selector_lookup_rejects_corrupt_digest_revision() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-sd-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let agent = base_agent("general-reviewer", 1, None);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&agent).unwrap();
    }
    corrupt_json_only(
        &path,
        "agent_types",
        "type_id='general-reviewer' AND revision=1",
        " ",
    );
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    // The selector path must not be a weaker authority than get_agent_type.
    assert!(matches!(
        kernel.get_agent_type(&agent.type_ref),
        Err(Error::InvariantViolation(_))
    ));
    assert_selector_lookup_fails(&kernel, &agent);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn selector_lookup_rejects_non_canonical_revision() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-sn-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let mut agent = base_agent("general-reviewer", 1, None);
    agent.contract.allowed_information_functions = vec![
        InformationFunction::CompressPositive,
        InformationFunction::Expand,
    ];
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&agent).unwrap();
    }
    // Self-consistent but non-canonical: reorder + recompute a matching digest.
    tamper_json(
        &path,
        "agent_types",
        "type_id='general-reviewer' AND revision=1",
        |value| {
            value["contract"]["allowed_information_functions"]
                .as_array_mut()
                .unwrap()
                .reverse();
        },
    );
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    assert_selector_lookup_fails(&kernel, &agent);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn selector_lookup_rejects_based_on_relational_drift() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-sb-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let base = base_agent("general-reviewer", 1, None);
    let base_two = base_agent("general-reviewer", 2, None);
    let mut derived = base_agent("derived-reviewer", 1, Some(type_ref("general-reviewer", 1)));
    derived.contract.budget_ceiling = Budget::new(50.0).unwrap();
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&base).unwrap();
        kernel.publish_agent_type(&base_two).unwrap();
        kernel.publish_agent_type(&derived).unwrap();
    }
    // Drift the duplicated based_on mirror column (to another existing revision)
    // without touching content_json.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE agent_types SET based_on_revision=2 WHERE type_id='derived-reviewer'",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    assert_selector_lookup_fails(&kernel, &derived);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_disposition_overlay_is_corruption() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-o-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let agent = base_agent("general-reviewer", 1, None);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&agent).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "DELETE FROM agent_type_dispositions WHERE type_id='general-reviewer' AND revision=1",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    // A present revision with a missing overlay is corruption, not "not found".
    assert!(matches!(
        kernel.get_agent_type(&agent.type_ref),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn selector_latest_does_not_fall_back_over_missing_overlay() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-lf-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let one = base_agent("general-reviewer", 1, None);
    let two = base_agent("general-reviewer", 2, None);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&one).unwrap();
        kernel.publish_agent_type(&two).unwrap();
    }
    // Corruption hides the highest revision's overlay.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "DELETE FROM agent_type_dispositions WHERE type_id='general-reviewer' AND revision=2",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    // Neither an exact pin nor Latest may silently drop @2 and resolve @1.
    assert!(matches!(
        kernel.resolve_agent_type_selector(&AgentTypeSelector::Exact(type_ref(
            "general-reviewer",
            2
        ))),
        Err(Error::InvariantViolation(_))
    ));
    assert!(matches!(
        kernel.resolve_agent_type_selector(&AgentTypeSelector::latest("general-reviewer").unwrap()),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn republish_does_not_repair_a_missing_overlay() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-rp-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let agent = base_agent("general-reviewer", 1, None);
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel.publish_agent_type(&agent).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "DELETE FROM agent_type_dispositions WHERE type_id='general-reviewer' AND revision=1",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    // An idempotent republish must not resurrect the overlay.
    assert!(matches!(
        kernel.publish_agent_type(&agent),
        Err(Error::InvariantViolation(_))
    ));
    let conn = rusqlite::Connection::open(&path).unwrap();
    let overlays: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_type_dispositions WHERE type_id='general-reviewer' AND revision=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(overlays, 0, "republish must not repair the overlay");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn status_setter_classifies_missing_overlay_as_corruption() {
    let kernel = memory_kernel();
    // Revision absent -> NotFound.
    assert!(matches!(
        kernel.set_spawn_source_status(
            &SpawnSourceRef::new("codex-local", 2).unwrap(),
            SourceStatus::Draining
        ),
        Err(Error::NotFound(_))
    ));

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b2-ss-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        publish_catalog(&kernel);
        kernel
            .publish_adapter_binding_policy(&adapter_policy())
            .unwrap();
        kernel.publish_spawn_source(&base_source()).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "DELETE FROM spawn_source_dispositions WHERE source_id='codex-local' AND revision=2",
            [],
        )
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    // Revision exists but overlay missing -> corruption (InvariantViolation).
    assert!(matches!(
        kernel.set_spawn_source_status(
            &SpawnSourceRef::new("codex-local", 2).unwrap(),
            SourceStatus::Draining
        ),
        Err(Error::InvariantViolation(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn source_config_revision_identity_includes_locator() {
    let a = SourceConfigRevision {
        config: base_config(VALID_DIGEST, Vec::new()),
        mode: ConfigMode::ExternalRef,
        locator: Some("file:///a.toml".into()),
    };
    let b = SourceConfigRevision {
        config: base_config(VALID_DIGEST, Vec::new()),
        mode: ConfigMode::ExternalRef,
        locator: Some("file:///b.toml".into()),
    };
    let same = SourceConfigRevision {
        config: base_config(VALID_DIGEST, Vec::new()),
        mode: ConfigMode::ExternalRef,
        locator: Some("file:///a.toml".into()),
    };
    // The metadata-only relation says equal...
    assert!(a.config.same_config_contract_content(&b.config));
    // ...but the complete durable revision identity does not.
    assert!(!a.same_revision_content(&b));
    assert!(a.same_revision_content(&same));
}
