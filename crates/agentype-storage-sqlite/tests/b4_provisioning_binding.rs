//! M6-B.4 provisioning binding and binding snapshot conformance (schema v8):
//! authority-bearing typed acquisition that re-proves every mandatory conjunct
//! over the current catalog and the agent's actual bound type, immutable
//! per-Incarnation provisioning bindings, atomic coherent Execution +
//! BindingSnapshot, the schema's mechanical guards including the
//! `INSERT OR REPLACE` identity paths, the marker/child coherence reads, the
//! validated-read enumeration, and the ADR-0009 typed-population capacity seam.

use agentype_agent_contract::{
    canonical_json_body_digest, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint,
    AgentRequirementDraft, AgentType, AgentTypeContract, AgentTypeRef, AgentTypeSelector,
    BindingSnapshot, Budget, ConfigDigest, ConfigStatus, ContinuityMode, CredentialRef,
    LifecycleMode, MaterializationDigest, NetworkPolicy, PhysicalSafety,
    ResolvedProvisioningEvidence, SecurityContract, SourceConfig, SourceConfigRef, SourceStatus,
    SpawnSource, SpawnSourceRef, RESOLVER_VERSION,
};
use agentype_core::{
    Clock, InformationFunction, LogicalAgentState, ManualClock, PartitionSpec, RawWorkIntent,
    Retention, SemanticInputSet, TaskSpec, TaskState, WorkspaceMode,
};
use agentype_execution_config::{
    resolve_execution_environment, AdapterBindingKey, ExecutionProfileConfig, ExecutionRegistry,
    ExecutionResolutionMode, ExecutionTargetConfig, FrozenPhysicalExecutionBinding,
    NetworkEnforcement,
};
use agentype_storage_sqlite::{Kernel, ResolvedProvisioningSelection, SourceConfigBody};
use rusqlite::Connection;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

#[test]
fn required_continuity_has_the_same_gate_for_existing_and_new_acquisition() {
    for existing in [true, false] {
        for concrete_workstream in [false, true] {
            let env = Env::new("required-placement", 1);
            let kernel = &env.kernel;
            let pin = publish_agent_contract(
                kernel,
                "continuity",
                false,
                LifecycleMode::Ephemeral,
                ContinuityMode::Logical,
            );
            let catalog = publish_source_contract(
                kernel,
                "continuity",
                LifecycleMode::Ephemeral,
                ContinuityMode::Logical,
            );
            let workstream = concrete_workstream
                .then(|| kernel.create_workstream("project", None, None).unwrap());
            let mut spec = TaskSpec::new("continuous", json!({}))
                .continuity(agentype_core::ContinuityPreference::Required);
            spec.workstream_id = workstream.clone();
            let task = admit_typed_spec(kernel, pin.clone(), spec);
            let agent = kernel.ready_agent("general").unwrap();
            kernel.bind_logical_agent_type(&agent, &pin).unwrap();
            env.raw()
                .execute(
                    "UPDATE logical_agents SET workstream_id=?1 WHERE id=?2",
                    rusqlite::params![workstream.as_ref().map(|id| id.as_str()), agent.as_str()],
                )
                .unwrap();
            assert_eq!(
                !kernel
                    .match_existing_agents_for_task(&task)
                    .unwrap()
                    .is_empty(),
                concrete_workstream
            );
            let population: i64 = env
                .raw()
                .query_row("SELECT COUNT(*) FROM logical_agents", [], |r| r.get(0))
                .unwrap();
            let result = if existing {
                kernel.acquire_typed_task_existing(&task, &agent, &catalog.selection, &registry())
            } else {
                kernel.acquire_typed_task_new_agent(&task, &catalog.selection, &registry())
            };
            assert_eq!(
                result.is_ok(),
                concrete_workstream,
                "existing={existing}, concrete={concrete_workstream}"
            );
            if !concrete_workstream {
                assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
                assert_eq!(kernel.attempt_count_for_task(&task).unwrap(), 0);
                assert_eq!(count_provisioning_bindings(&env), 0);
                assert_eq!(
                    env.raw()
                        .query_row("SELECT COUNT(*) FROM logical_agents", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    population
                );
            }
        }
    }
}

#[test]
fn target_isolation_is_reproved_before_both_acquisition_paths() {
    // Task and AgentType require no isolation. The target independently promises
    // it: an incapable adapter must not consume an Attempt or create a binding.
    for existing in [true, false] {
        for target_isolation in [false, true] {
            for imported_isolation in [false, true] {
                let env = Env::new("target-import-intersection", 1);
                let kernel = &env.kernel;
                let pin = publish_agent_type(kernel, "reviewer");
                let mut catalog = publish_catalog(kernel, "intersection");
                let source = kernel
                    .get_spawn_source(&catalog.selection.spawn_source)
                    .unwrap()
                    .unwrap();
                let config = kernel
                    .get_source_config_revision(&catalog.selection.source_config)
                    .unwrap()
                    .unwrap();
                catalog.selection.evidence = ResolvedProvisioningEvidence::from_imported_binding(
                    source.adapter_policy.clone(),
                    "default",
                    "k1",
                    source.source_ref.clone(),
                    config.config().config_ref.clone(),
                    config.config().config_digest.clone(),
                    PhysicalSafety::new(
                        imported_isolation,
                        vec![WorkspaceMode::ReadOnly],
                        [NetworkPolicy::Disabled].into_iter().collect(),
                    )
                    .unwrap(),
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap();
                let task = admit_typed_task(kernel, pin.clone());
                let agent = kernel.ready_agent("general").unwrap();
                kernel.bind_logical_agent_type(&agent, &pin).unwrap();
                let execution_registry = if target_isolation {
                    registry_isolated()
                } else {
                    registry()
                };
                let result = if existing {
                    kernel.acquire_typed_task_existing(
                        &task,
                        &agent,
                        &catalog.selection,
                        &execution_registry,
                    )
                } else {
                    kernel.acquire_typed_task_new_agent(
                        &task,
                        &catalog.selection,
                        &execution_registry,
                    )
                };
                assert_eq!(result.is_ok(), !target_isolation || imported_isolation);
                if result.is_err() {
                    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
                    assert_eq!(kernel.attempt_count_for_task(&task).unwrap(), 0);
                    assert_eq!(count_provisioning_bindings(&env), 0);
                }
            }
        }
    }
}

#[test]
fn execution_cannot_freeze_target_isolation_beyond_the_committed_capability() {
    let env = Env::new("snapshot-target-intersection", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "snapshot-target");
    let task = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquired = kernel
        .acquire_typed_task_existing(&task, &agent, &catalog.selection, &registry())
        .unwrap();
    // A target changes to isolated after acquisition; neither semantic contract
    // requires isolation, but the PB never proved that physical capability.
    let physical = resolve_execution_environment(
        ExecutionResolutionMode::Authoritative(&registry_isolated()),
        &kernel.resolve_execution_binding(&acquired.claim).unwrap(),
    )
    .unwrap()
    .physical_binding(AdapterBindingKey::new("k1").unwrap())
    .unwrap();
    let mut record = snapshot(
        &agentype_core::ExecutionId::new(),
        &acquired.provisioning_binding,
        &catalog,
        &acquired.claim,
        &physical,
    );
    record.effective_isolation = true;
    assert!(kernel
        .create_execution_with_snapshot(&acquired.claim, physical, &record)
        .is_err());
    assert!(kernel
        .attempt(&acquired.claim.attempt_id)
        .unwrap()
        .incarnation_id
        .is_none());
    assert_eq!(
        env.raw()
            .query_row("SELECT COUNT(*) FROM executions", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// Build a genuinely reusable resident host via a committed Execution and a
/// valid success ACK, rather than manufacturing WARM with a fixture SQL update.
fn resident_host(
    env: &Env,
) -> (
    AgentTypeRef,
    Catalog,
    agentype_core::LogicalAgentId,
    agentype_core::IncarnationId,
) {
    let kernel = &env.kernel;
    let pin = publish_agent_contract(
        kernel,
        "resident",
        false,
        LifecycleMode::Resident,
        ContinuityMode::None,
    );
    let catalog = publish_source_contract(
        kernel,
        "resident",
        LifecycleMode::Resident,
        ContinuityMode::None,
    );
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let task = admit_typed_task(kernel, pin.clone());
    let acquired = kernel
        .acquire_typed_task_existing(&task, &agent, &catalog.selection, &registry())
        .unwrap();
    let physical = physical_binding(&acquired.claim);
    let record = snapshot(
        &agentype_core::ExecutionId::new(),
        &acquired.provisioning_binding,
        &catalog,
        &acquired.claim,
        &physical,
    );
    let launch = kernel
        .create_execution_with_snapshot(&acquired.claim, physical, &record)
        .unwrap();
    let incarnation = acquired.provisioning_binding.incarnation_id;
    assert_eq!(
        kernel
            .attempt(&acquired.claim.attempt_id)
            .unwrap()
            .incarnation_id,
        Some(incarnation.clone())
    );
    kernel
        .ack_success(
            &acquired.claim.attempt_id,
            acquired.claim.lease_epoch,
            Some(launch.execution_id()),
            &json!({"done": true}),
            None,
            true,
            true,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        kernel.incarnation(&incarnation).unwrap().state,
        agentype_core::IncarnationState::Warm
    );
    (pin, catalog, agent, incarnation)
}

#[test]
fn pre_execution_settlement_preserves_a_reused_resident_host_across_all_closures() {
    for action in [
        "configuration",
        "cancel-task",
        "cancel-batch",
        "expiry",
        "restart",
        "nack-history",
        "ack",
    ] {
        let env = Env::with_retention(action, 1, Retention::Resident);
        let (pin, catalog, agent, incarnation) = resident_host(&env);
        let task = admit_typed_task(&env.kernel, pin);
        let acquired = env
            .kernel
            .acquire_typed_task_existing(&task, &agent, &catalog.selection, &registry())
            .unwrap();
        assert_eq!(acquired.provisioning_binding.incarnation_id, incarnation);
        let claim = &acquired.claim;
        match action {
            "configuration" => {
                env.kernel
                    .report_configuration_unavailable(
                        &claim.attempt_id,
                        claim.lease_epoch,
                        "pure preparation unavailable",
                    )
                    .unwrap();
            }
            "cancel-task" => {
                env.kernel.cancel_task(&task, false).unwrap();
            }
            "cancel-batch" => {
                env.kernel.cancel_batch(&claim.batch_id).unwrap();
            }
            "expiry" | "restart" => {
                let now = if action == "expiry" {
                    claim.lease_expires_at + 1.0
                } else {
                    1_000.0
                };
                let reopened =
                    Kernel::open(&env.path, Arc::new(ManualClock::new(now)), 10.0, MAX_BYTES)
                        .unwrap();
                reopened.expire_leases(action == "restart").unwrap();
            }
            "nack-history" => {
                env.kernel
                    .nack_preserving_physical_history(
                        &claim.attempt_id,
                        claim.lease_epoch,
                        agentype_core::FailureClass::ResourceUnavailable,
                        None,
                    )
                    .unwrap();
            }
            "ack" => {
                env.kernel
                    .ack_success(
                        &claim.attempt_id,
                        claim.lease_epoch,
                        None,
                        &json!({}),
                        None,
                        true,
                        false,
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            env.kernel.incarnation(&incarnation).unwrap().state,
            agentype_core::IncarnationState::Warm,
            "{action}"
        );
        assert!(env
            .kernel
            .logical_agent(&agent)
            .unwrap()
            .current_task_id
            .is_none());
        assert_eq!(
            env.kernel.get_provisioning_binding(&incarnation).unwrap(),
            Some(acquired.provisioning_binding)
        );
        assert_eq!(
            env.raw()
                .query_row("SELECT COUNT(*) FROM executions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(env.raw().query_row("SELECT COUNT(*) FROM escalations WHERE failure_class='WRITER_QUIESCENCE_UNKNOWN'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
}

#[test]
fn fresh_pre_execution_reservation_remains_starting_and_can_be_reused() {
    let env = Env::with_retention("fresh-reservation", 1, Retention::Resident);
    let kernel = &env.kernel;
    let pin = publish_agent_contract(
        kernel,
        "resident",
        false,
        LifecycleMode::Resident,
        ContinuityMode::None,
    );
    let catalog = publish_source_contract(
        kernel,
        "resident",
        LifecycleMode::Resident,
        ContinuityMode::None,
    );
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let first_task = admit_typed_task(kernel, pin.clone());
    let first = kernel
        .acquire_typed_task_existing(&first_task, &agent, &catalog.selection, &registry())
        .unwrap();
    kernel.cancel_task(&first_task, false).unwrap();
    assert_eq!(
        kernel
            .incarnation(&first.provisioning_binding.incarnation_id)
            .unwrap()
            .state,
        agentype_core::IncarnationState::Starting
    );
    let second_task = admit_typed_task(kernel, pin);
    let second = kernel
        .acquire_typed_task_existing(&second_task, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_eq!(second.provisioning_binding, first.provisioning_binding);
    assert_eq!(count_provisioning_bindings(&env), 1);
}

#[test]
fn unstarted_reservation_rolls_over_after_closure_and_source_change() {
    for cancel in [false, true] {
        let env = Env::with_retention("reservation-rollover", 1, Retention::Resident);
        let kernel = &env.kernel;
        let pin = publish_agent_contract(
            kernel,
            "resident",
            false,
            LifecycleMode::Resident,
            ContinuityMode::None,
        );
        let catalog_a =
            publish_source_contract(kernel, "a", LifecycleMode::Resident, ContinuityMode::None);
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let first_task = admit_typed_task(kernel, pin.clone());
        let first = kernel
            .acquire_typed_task_existing(&first_task, &agent, &catalog_a.selection, &registry())
            .unwrap();
        if cancel {
            kernel.cancel_task(&first_task, false).unwrap();
        } else {
            kernel
                .report_configuration_unavailable(
                    &first.claim.attempt_id,
                    first.claim.lease_epoch,
                    "pure prepare unavailable",
                )
                .unwrap();
        }
        kernel
            .set_spawn_source_status(&catalog_a.selection.spawn_source, SourceStatus::Disabled)
            .unwrap();
        let catalog_b =
            publish_source_contract(kernel, "b", LifecycleMode::Resident, ContinuityMode::None);
        let next_task = admit_typed_task(kernel, pin);
        let second = kernel
            .acquire_typed_task_existing(&next_task, &agent, &catalog_b.selection, &registry())
            .unwrap();
        let old_id = &first.provisioning_binding.incarnation_id;
        let new_id = &second.provisioning_binding.incarnation_id;
        assert_ne!(old_id, new_id);
        assert_eq!(second.provisioning_binding.logical_agent_id, agent);
        assert_eq!(
            second.provisioning_binding.source_config,
            catalog_b.selection.source_config
        );
        assert_eq!(
            kernel.incarnation(old_id).unwrap().state,
            agentype_core::IncarnationState::Lost
        );
        assert_eq!(
            kernel.incarnation(new_id).unwrap().state,
            agentype_core::IncarnationState::Starting
        );
        assert_eq!(
            kernel.get_provisioning_binding(old_id).unwrap(),
            Some(first.provisioning_binding)
        );
        assert!(kernel
            .attempt(&second.claim.attempt_id)
            .unwrap()
            .incarnation_id
            .is_none());
        assert_eq!(
            env.raw()
                .query_row("SELECT COUNT(*) FROM executions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            env.raw()
                .query_row(
                    "SELECT COUNT(*) FROM incarnations WHERE state IN ('STARTING','WARM','COLD')",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn reservation_rollover_rejects_an_active_claim_even_without_incarnation_association() {
    let env = Env::with_retention("reservation-active-claim", 1, Retention::Resident);
    let kernel = &env.kernel;
    let pin = publish_agent_contract(
        kernel,
        "resident",
        false,
        LifecycleMode::Resident,
        ContinuityMode::None,
    );
    let mut catalog_a =
        publish_source_contract(kernel, "a", LifecycleMode::Resident, ContinuityMode::None);
    let catalog_b =
        publish_source_contract(kernel, "b", LifecycleMode::Resident, ContinuityMode::None);
    catalog_a.selection.catalog_frontier_digest = kernel.catalog_frontier_digest().unwrap();
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let first_task = admit_typed_task(kernel, pin.clone());
    let first = kernel
        .acquire_typed_task_existing(&first_task, &agent, &catalog_a.selection, &registry())
        .unwrap();
    // A damaged agent assignment must not hide an ACTIVE Attempt whose
    // incarnation_id is deliberately still NULL during pure preparation.
    env.raw()
        .execute(
            "UPDATE logical_agents SET state='READY',current_task_id=NULL WHERE id=?1",
            [agent.as_str()],
        )
        .unwrap();
    let next_task = admit_typed_task(kernel, pin);
    assert!(kernel
        .acquire_typed_task_existing(&next_task, &agent, &catalog_b.selection, &registry())
        .is_err());
    assert!(kernel.claim_authority_is_current(&first.claim).unwrap());
    assert_eq!(
        kernel
            .incarnation(&first.provisioning_binding.incarnation_id)
            .unwrap()
            .state,
        agentype_core::IncarnationState::Starting
    );
    assert_eq!(kernel.attempt_count_for_task(&next_task).unwrap(), 0);
    assert_eq!(count_provisioning_bindings(&env), 1);
}

#[test]
fn starting_incarnation_with_execution_history_cannot_roll_over() {
    let env = Env::with_retention("starting-history", 1, Retention::Resident);
    let (pin, _, agent, old_id) = resident_host(&env);
    // Even terminal physical history cannot be reinterpreted as an unstarted
    // reservation if the Incarnation is observed as STARTING again.
    env.raw()
        .execute(
            "UPDATE incarnations SET state='STARTING' WHERE id=?1",
            [old_id.as_str()],
        )
        .unwrap();
    let other = publish_source_contract(
        &env.kernel,
        "other",
        LifecycleMode::Resident,
        ContinuityMode::None,
    );
    let task = admit_typed_task(&env.kernel, pin);
    assert!(env
        .kernel
        .acquire_typed_task_existing(&task, &agent, &other.selection, &registry())
        .is_err());
    assert_eq!(env.kernel.attempt_count_for_task(&task).unwrap(), 0);
    assert_eq!(count_provisioning_bindings(&env), 1);
    assert_eq!(
        env.kernel.incarnation(&old_id).unwrap().state,
        agentype_core::IncarnationState::Starting
    );
}

/// A canonical `sha256:<64 hex>` materialization digest for fixtures.
fn mat_digest() -> MaterializationDigest {
    MaterializationDigest::new(
        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
    )
    .unwrap()
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

struct Env {
    kernel: Kernel,
    path: PathBuf,
}

impl Env {
    fn new(tag: &str, capacity: i64) -> Self {
        Self::with_retention(tag, capacity, Retention::Ephemeral)
    }

    fn with_retention(tag: &str, capacity: i64, retention: Retention) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("b4-{tag}-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general", capacity, retention, "local", "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        Self { kernel, path }
    }

    fn raw(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}

/// M5 execution registry whose `local` target is kind `default`, matching the
/// policy the catalog helper publishes.
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

fn registry_isolated() -> ExecutionRegistry {
    let mut registry = ExecutionRegistry::new();
    registry
        .register_target(ExecutionTargetConfig::new("local", "default", true))
        .unwrap();
    registry
        .register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    registry
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
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
    publish_agent_type_isolation(kernel, id, false)
}

fn publish_agent_type_isolation(
    kernel: &Kernel,
    id: &str,
    requires_attempt_isolation: bool,
) -> AgentTypeRef {
    publish_agent_contract(
        kernel,
        id,
        requires_attempt_isolation,
        LifecycleMode::Ephemeral,
        ContinuityMode::None,
    )
}

fn publish_agent_contract(
    kernel: &Kernel,
    id: &str,
    requires_attempt_isolation: bool,
    lifecycle: LifecycleMode,
    continuity: ContinuityMode,
) -> AgentTypeRef {
    let agent = AgentType {
        type_ref: type_ref(id, 1),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace: WorkspaceMode::ReadOnly,
                network: NetworkPolicy::Disabled,
                requires_attempt_isolation,
            },
            lifecycle: [lifecycle].into_iter().collect(),
            continuity,
            sandbox_policy: None,
            anchor_constraint: None,
        },
    };
    kernel.publish_agent_type(&agent).unwrap();
    agent.type_ref
}

struct Catalog {
    selection: ResolvedProvisioningSelection,
    config_digest: ConfigDigest,
}

fn publish_catalog(kernel: &Kernel, tag: &str) -> Catalog {
    publish_source_contract(kernel, tag, LifecycleMode::Ephemeral, ContinuityMode::None)
}

fn publish_source_contract(
    kernel: &Kernel,
    tag: &str,
    lifecycle: LifecycleMode,
    continuity: ContinuityMode,
) -> Catalog {
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
        lifecycle_modes: [lifecycle].into_iter().collect(),
        continuity_modes: [continuity].into_iter().collect(),
        functional_envelope: BTreeMap::new(),
        claims: Vec::new(),
        status: SourceStatus::Active,
    };
    kernel.publish_spawn_source(&source).unwrap();

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
        config_digest.clone(),
        safety(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    Catalog {
        selection: ResolvedProvisioningSelection {
            spawn_source: source.source_ref,
            source_config: config.config_ref,
            adapter_policy: policy.policy_ref,
            evidence,
            adapter_kind: "default".into(),
            adapter_binding_key: "k1".into(),
            provisioning_protocol: "agentype-test-source/1".into(),
            attested_materialization_digest: mat_digest(),
            catalog_frontier_digest: kernel.catalog_frontier_digest().unwrap(),
        },
        config_digest,
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

fn admit_typed_task(kernel: &Kernel, pin: AgentTypeRef) -> agentype_core::TaskId {
    admit_typed_spec(
        kernel,
        pin,
        TaskSpec::new("audit", json!({})).partition("general"),
    )
}

fn admit_typed_spec(kernel: &Kernel, pin: AgentTypeRef, spec: TaskSpec) -> agentype_core::TaskId {
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "typed_work".into(),
        objective: "typed work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap()
}

fn physical_binding(claim: &agentype_core::Claim) -> FrozenPhysicalExecutionBinding {
    let binding = agentype_core::AuthoritativeExecutionBinding {
        attempt_id: claim.attempt_id.clone(),
        lease_epoch: claim.lease_epoch,
        execution_target: claim.execution_target.clone(),
        execution_profile: claim.execution_profile.clone(),
    };
    resolve_execution_environment(ExecutionResolutionMode::DirectUnconfigured, &binding)
        .unwrap()
        .physical_binding(AdapterBindingKey::new("k1").unwrap())
        .unwrap()
}

fn snapshot(
    execution_id: &agentype_core::ExecutionId,
    binding: &agentype_agent_contract::ProvisioningBinding,
    catalog: &Catalog,
    claim: &agentype_core::Claim,
    physical: &FrozenPhysicalExecutionBinding,
) -> BindingSnapshot {
    BindingSnapshot {
        snapshot_id: format!("snap-{}", execution_id.as_str()),
        execution_id: execution_id.clone(),
        provisioning_binding_id: binding.provisioning_binding_id.clone(),
        adapter_kind: physical.adapter_kind().to_string(),
        adapter_binding_key: physical.adapter_binding_key().as_str().to_string(),
        spawn_source: catalog.selection.spawn_source.clone(),
        source_config: catalog.selection.source_config.clone(),
        source_config_digest: catalog.config_digest.clone(),
        attested_materialization_digest: mat_digest(),
        launch_descriptor: "env-handle".into(),
        execution_target: claim.execution_target.clone(),
        execution_profile: claim.execution_profile.clone(),
        required_capabilities: BTreeMap::new(),
        enforceable_security: safety(),
        effective_isolation: false,
        effective_workspace: WorkspaceMode::ReadOnly,
        effective_network: NetworkPolicy::Disabled,
        credential_refs_digest: None,
        resolver_version: RESOLVER_VERSION.to_string(),
    }
}

#[test]
fn typed_acquisition_and_atomic_binding_snapshot_round_trip() {
    let env = Env::new("roundtrip", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "a");
    let task_id = admit_typed_task(kernel, pin.clone());

    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_eq!(acquisition.provisioning_binding.logical_agent_id, agent);
    assert_eq!(acquisition.provisioning_binding.agent_type, pin);
    assert_eq!(
        acquisition.provisioning_binding.spawn_source,
        catalog.selection.spawn_source
    );
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Leased);

    let stored = kernel
        .get_provisioning_binding(&acquisition.provisioning_binding.incarnation_id)
        .unwrap()
        .expect("provisioning binding persisted");
    assert_eq!(stored, acquisition.provisioning_binding);

    let physical = physical_binding(&acquisition.claim);
    let execution_id = agentype_core::ExecutionId::new();
    let snap = snapshot(
        &execution_id,
        &stored,
        &catalog,
        &acquisition.claim,
        &physical,
    );
    let launch = kernel
        .create_execution_with_snapshot(&acquisition.claim, physical, &snap)
        .unwrap();
    assert_eq!(launch.execution_id(), &execution_id);
    // The effective network policy reaches the physical request.
    assert_eq!(launch.required_network(), NetworkEnforcement::Disabled);
    assert_eq!(
        kernel.get_binding_snapshot(&execution_id).unwrap(),
        Some(snap)
    );
}

#[test]
fn acquisition_fails_closed_on_credential_refs() {
    let env = Env::new("credential", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let mut catalog = publish_catalog(kernel, "r");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // Publish a config that declares a credential reference.
    let credential = CredentialRef::new("vault://codex-prod").unwrap();
    let body = json!({"model": "opaque"});
    let config = SourceConfig {
        config_ref: SourceConfigRef::new(catalog.selection.spawn_source.clone(), "cred-config", 1)
            .unwrap(),
        config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: vec![credential],
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    kernel
        .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
        .unwrap();
    catalog.selection.source_config = config.config_ref.clone();
    catalog.config_digest = config.config_digest.clone();
    catalog.selection.catalog_frontier_digest = kernel.catalog_frontier_digest().unwrap();
    catalog.selection.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k1",
        catalog.selection.spawn_source.clone(),
        config.config_ref.clone(),
        config.config_digest.clone(),
        safety(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    // M6-B.4 has no trusted availability authority, so any credential-bearing
    // config fails closed; no caller-supplied fact can grant authority.
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
}

#[test]
fn acquisition_rejects_isolation_the_target_cannot_enforce() {
    let env = Env::new("isolation", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "s");
    // A Task that requires attempt isolation.
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "iso".into(),
        objective: "iso".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({})).partition("general")),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    let mut draft = draft(pin.clone());
    draft.required_attempt_isolation = true;
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft)
        .unwrap();
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // The registry target is NOT isolated; adapter evidence claiming isolation
    // must not grant authority.
    let mut isolated_evidence = catalog.selection.clone();
    isolated_evidence.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k1",
        catalog.selection.spawn_source.clone(),
        catalog.selection.source_config.clone(),
        catalog.config_digest.clone(),
        PhysicalSafety::new(
            true,
            vec![WorkspaceMode::ReadOnly],
            [NetworkPolicy::Disabled].into_iter().collect(),
        )
        .unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &isolated_evidence, &registry())
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
}

#[test]
fn agent_type_isolation_requirement_is_enforced() {
    let env = Env::new("typeiso", 1);
    let kernel = &env.kernel;
    // The AgentType itself requires isolation; the Task does not.
    let pin = publish_agent_type_isolation(kernel, "reviewer", true);
    let catalog = publish_catalog(kernel, "t");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // Adapter evidence claims isolation, but the M4 target does not provide it.
    // The effective isolation is the OR of the AgentType and Task requirements.
    let mut isolated_evidence = catalog.selection.clone();
    isolated_evidence.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k1",
        catalog.selection.spawn_source.clone(),
        catalog.selection.source_config.clone(),
        catalog.config_digest.clone(),
        PhysicalSafety::new(
            true,
            vec![WorkspaceMode::ReadOnly],
            [NetworkPolicy::Disabled].into_iter().collect(),
        )
        .unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &isolated_evidence, &registry())
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
}

#[test]
fn capability_and_effective_isolation_are_separate() {
    let env = Env::new("capvs", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "u");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // The adapter can isolate, but neither the AgentType nor the Task requires
    // it and the target does not use it. Acquisition is legal (no effective
    // requirement); the binding records the capability, the execution records
    // the actual isolation, and both are representable.
    let mut isolated_evidence = catalog.selection.clone();
    isolated_evidence.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k1",
        catalog.selection.spawn_source.clone(),
        catalog.selection.source_config.clone(),
        catalog.config_digest.clone(),
        PhysicalSafety::new(
            true,
            vec![WorkspaceMode::ReadOnly],
            [NetworkPolicy::Disabled].into_iter().collect(),
        )
        .unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &isolated_evidence, &registry())
        .unwrap();
    assert!(acquisition
        .provisioning_binding
        .effective_security
        .attempt_isolation());

    let physical = physical_binding(&acquisition.claim);
    assert!(!physical.safety().attempt_isolation());
    let execution_id = agentype_core::ExecutionId::new();
    let mut snap = snapshot(
        &execution_id,
        &acquisition.provisioning_binding,
        &catalog,
        &acquisition.claim,
        &physical,
    );
    snap.enforceable_security = isolated_evidence.evidence.enforceable_safety().clone();
    snap.effective_isolation = false;
    kernel
        .create_execution_with_snapshot(&acquisition.claim, physical, &snap)
        .unwrap();
    let read_back = kernel.get_binding_snapshot(&execution_id).unwrap().unwrap();
    assert!(read_back.enforceable_security.attempt_isolation());
    assert!(!read_back.effective_isolation);
}

#[test]
fn typed_claim_cannot_use_legacy_create_execution() {
    let env = Env::new("typedlegacy", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "v");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();

    let physical = physical_binding(&acquisition.claim);
    assert!(kernel
        .create_execution(&acquisition.claim, physical)
        .is_err());
    // No Execution was produced for the typed Attempt.
    let executions: i64 = env
        .raw()
        .query_row("SELECT COUNT(*) FROM executions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(executions, 0);
}

#[test]
fn reconcile_pool_fails_closed_on_binding_corruption() {
    let env = Env::new("reconcorrupt", 1);
    let kernel = &env.kernel;
    let _pin = publish_agent_type(kernel, "reviewer");
    let agent = kernel.ready_agent("general").unwrap();

    // Manufacture an UNBOUND marker with an unexpected binding row.
    let conn = env.raw();
    conn.execute(
        "DROP TRIGGER logical_agent_type_bindings_require_bound_parent",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO logical_agent_type_bindings(logical_agent_id, type_id, type_revision, bound_at)
         VALUES(?1, 'reviewer', 1, 0)",
        rusqlite::params![agent.as_str()],
    )
    .unwrap();
    // A corrupt marker/child pair MUST abort the reconcile transaction, not be
    // silently excluded from capacity.
    assert!(kernel.reconcile_pool().is_err());
}

#[test]
fn execution_commitment_reproves_effective_isolation() {
    let env = Env::new("isocommit", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "x");
    // A Task that requires isolation.
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "iso2".into(),
        objective: "iso2".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({})).partition("general")),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    let mut draft = draft(pin.clone());
    draft.required_attempt_isolation = true;
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft)
        .unwrap();
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    let isolated_safety = PhysicalSafety::new(
        true,
        vec![WorkspaceMode::ReadOnly],
        [NetworkPolicy::Disabled].into_iter().collect(),
    )
    .unwrap();
    let mut isolated_evidence = catalog.selection.clone();
    isolated_evidence.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k1",
        catalog.selection.spawn_source.clone(),
        catalog.selection.source_config.clone(),
        catalog.config_digest.clone(),
        isolated_safety.clone(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    // Acquisition succeeds against the isolated target.
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &isolated_evidence, &registry_isolated())
        .unwrap();

    // At Execution commitment, an unisolated physical safety must be rejected
    // even though the Execution and the snapshot would be self-consistent.
    let unisolated = physical_binding(&acquisition.claim);
    assert!(!unisolated.safety().attempt_isolation());
    let execution_id = agentype_core::ExecutionId::new();
    let mut snap = snapshot(
        &execution_id,
        &acquisition.provisioning_binding,
        &catalog,
        &acquisition.claim,
        &unisolated,
    );
    snap.enforceable_security = isolated_safety;
    snap.effective_isolation = false;
    assert!(kernel
        .create_execution_with_snapshot(&acquisition.claim, unisolated, &snap)
        .is_err());
    let executions: i64 = env
        .raw()
        .query_row("SELECT COUNT(*) FROM executions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(executions, 0);
}

#[test]
fn recovery_rejects_missing_snapshot_child() {
    let env = Env::new("recovsnap", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "y");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let physical = physical_binding(&acquisition.claim);
    let execution_id = agentype_core::ExecutionId::new();
    let snap = snapshot(
        &execution_id,
        &acquisition.provisioning_binding,
        &catalog,
        &acquisition.claim,
        &physical,
    );
    kernel
        .create_execution_with_snapshot(&acquisition.claim, physical, &snap)
        .unwrap();

    // Simulate a lost SNAPSHOT child row; restart reconciliation must fail
    // closed rather than continuing physical reconciliation without provenance.
    let conn = env.raw();
    conn.execute("DROP TRIGGER binding_snapshots_immutable_delete", [])
        .unwrap();
    conn.execute(
        "DELETE FROM binding_snapshots WHERE execution_id=?1",
        rusqlite::params![execution_id.as_str()],
    )
    .unwrap();
    assert!(kernel.reconciliation_candidates().is_err());
}

#[test]
fn catalog_frontier_change_rejects_commit() {
    let env = Env::new("frontier", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "z");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // Publish a new active config after the selection was made: the captured
    // frontier is stale, so the authority transaction rejects the commit.
    let body = json!({"model": "new"});
    let config = SourceConfig {
        config_ref: SourceConfigRef::new(catalog.selection.spawn_source.clone(), "config-new", 1)
            .unwrap(),
        config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
        lifecycle_modes: None,
        continuity_modes: None,
        credential_refs: Vec::new(),
        claims: Vec::new(),
        status: ConfigStatus::Active,
    };
    kernel
        .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
        .unwrap();
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
}

#[test]
fn materialization_change_rolls_over_to_a_new_incarnation() {
    let env = Env::new("matroll", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "m1");
    let task_a = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let first = kernel
        .acquire_typed_task_existing(&task_a, &agent, &catalog.selection, &registry())
        .unwrap();
    let old_incarnation = first.provisioning_binding.incarnation_id.clone();
    finish_current_attempt(&env, &task_a, old_incarnation.as_str());

    // The same source/config/domain attests a different materialization digest:
    // the Scheduler must roll over to a fresh Incarnation, not reuse a binding
    // frozen with the old materialization.
    let mut selection = catalog.selection.clone();
    selection.attested_materialization_digest = MaterializationDigest::new(
        "sha256:2222222222222222222222222222222222222222222222222222222222222222",
    )
    .unwrap();
    selection.catalog_frontier_digest = kernel.catalog_frontier_digest().unwrap();
    let task_b = admit_typed_task(kernel, pin.clone());
    let second = kernel
        .acquire_typed_task_existing(&task_b, &agent, &selection, &registry())
        .unwrap();
    assert_ne!(second.provisioning_binding.incarnation_id, old_incarnation);
    assert_eq!(count_provisioning_bindings(&env), 2);
}

#[test]
fn frontier_digest_tracks_policy_disposition() {
    let env = Env::new("frontierpolicy", 1);
    let kernel = &env.kernel;
    let _pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "fp");
    let before = kernel.catalog_frontier_digest().unwrap();
    // Policy disposition affects durable eligibility, so it must be part of the
    // frontier the authority transaction fences against.
    kernel
        .set_adapter_binding_policy_status(
            &catalog.selection.adapter_policy,
            ConfigStatus::Disabled,
        )
        .unwrap();
    let after = kernel.catalog_frontier_digest().unwrap();
    assert_ne!(before, after);
}

#[test]
fn request_identity_replace_is_guarded() {
    let env = Env::new("requestreplace", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "w");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let physical = physical_binding(&acquisition.claim);
    let execution_id = agentype_core::ExecutionId::new();
    let snap = snapshot(
        &execution_id,
        &acquisition.provisioning_binding,
        &catalog,
        &acquisition.claim,
        &physical,
    );
    kernel
        .create_execution_with_snapshot(&acquisition.claim, physical, &snap)
        .unwrap();

    let conn = env.raw();
    let request_id: String = conn
        .query_row(
            "SELECT request_id FROM executions WHERE id=?1",
            rusqlite::params![execution_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    // A fresh primary key that reuses the durable request identity must be
    // rejected by a durable-identity guard, not silently replace the frozen row.
    assert!(conn
        .execute(
            "INSERT OR REPLACE INTO executions(id,request_id,task_id,attempt_id,incarnation_id,
                execution_target,execution_profile,adapter_kind,adapter_binding_key,state,
                started_at,updated_at)
             VALUES('exec-other',?1,'task-legacy','attempt-legacy','inc-legacy',
                'local','default','default','k1','STARTING',1,1)",
            rusqlite::params![request_id],
        )
        .is_err());
}

#[test]
fn acquisition_reproves_the_mandatory_conjuncts() {
    let env = Env::new("reprove", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let mut catalog = publish_catalog(kernel, "b");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // A selection whose imported evidence cannot enforce the task workspace is
    // rejected at the authority transaction, even though it is otherwise
    // well-formed. This is the resolver-proof the acquisition must re-check.
    let weak = PhysicalSafety::new(false, Vec::new(), BTreeSet::new()).unwrap();
    catalog.selection.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k1",
        catalog.selection.spawn_source.clone(),
        catalog.selection.source_config.clone(),
        catalog.config_digest.clone(),
        weak,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let err = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap_err();
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
    let _ = err;
}

#[test]
fn acquisition_uses_the_actual_bound_type_not_the_exact_pin() {
    let env = Env::new("actualtype", 1);
    let kernel = &env.kernel;
    // The Task pins `specialist`; the agent is bound to a different, compatible
    // `generalist` type. B.3 preselection admits it, so B.4 must prove
    // can_execute over the ACTUAL type, not narrow the relation to equality.
    let pin = publish_agent_type(kernel, "specialist");
    let actual = publish_agent_type(kernel, "generalist");
    let catalog = publish_catalog(kernel, "c");
    let task_id = admit_typed_task(kernel, pin);
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &actual).unwrap();

    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_eq!(acquisition.provisioning_binding.agent_type, actual);
}

#[test]
fn acquire_typed_task_new_agent_births_and_binds() {
    let env = Env::new("newagent", 0);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "d");
    let task_id = admit_typed_task(kernel, pin.clone());

    let acquisition = kernel
        .acquire_typed_task_new_agent(&task_id, &catalog.selection, &registry())
        .unwrap();
    let agent = acquisition.provisioning_binding.logical_agent_id.clone();
    assert_eq!(
        kernel.get_logical_agent_type_binding(&agent).unwrap(),
        Some(pin)
    );
    assert!(kernel
        .get_provisioning_binding(&acquisition.provisioning_binding.incarnation_id)
        .unwrap()
        .is_some());
}

#[test]
fn write_once_guards_cover_insert_or_replace_identities() {
    let env = Env::new("writeonce", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "e");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let incarnation = acquisition.provisioning_binding.incarnation_id.clone();
    let binding_id = acquisition
        .provisioning_binding
        .provisioning_binding_id
        .clone();

    let conn = env.raw();
    // Immutable UPDATE/DELETE.
    assert!(conn
        .execute("DELETE FROM provisioning_bindings", [])
        .unwrap_err()
        .to_string()
        .contains("immutable"));
    // INSERT OR REPLACE with a NEW primary key but the same incarnation must not
    // silently replace the frozen row.
    assert!(conn
        .execute(
            "INSERT OR REPLACE INTO provisioning_bindings(
                provisioning_binding_id,logical_agent_id,incarnation_id,agent_type_id,
                agent_type_revision,record_json,record_digest,created_at)
             SELECT 'pb-other', logical_agent_id, incarnation_id, agent_type_id,
                    agent_type_revision, record_json, record_digest, 0
             FROM provisioning_bindings WHERE provisioning_binding_id=?1",
            rusqlite::params![binding_id],
        )
        .is_err());
    // The incarnation identity itself cannot be replaced to inject a marker.
    assert!(conn
        .execute(
            "INSERT OR REPLACE INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
             SELECT id, logical_agent_id, generation, execution_target, 'LOST', started_at
             FROM incarnations WHERE id=?1",
            rusqlite::params![incarnation.as_str()],
        )
        .is_err());
}

#[test]
fn provisioning_binding_marker_and_row_are_one_invariant() {
    let env = Env::new("coherence", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "f");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let incarnation = acquisition.provisioning_binding.incarnation_id.clone();

    // Simulate a lost child row under a PROVISIONED marker; the read must fail
    // closed rather than return `None`.
    let conn = env.raw();
    conn.execute("DROP TRIGGER provisioning_bindings_immutable_delete", [])
        .unwrap();
    conn.execute(
        "DELETE FROM provisioning_bindings WHERE incarnation_id=?1",
        rusqlite::params![incarnation.as_str()],
    )
    .unwrap();
    assert!(kernel.get_provisioning_binding(&incarnation).is_err());
}

#[test]
fn binding_snapshot_must_be_coherent_with_the_execution() {
    let env = Env::new("coherentsnapshot", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "g");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let physical = physical_binding(&acquisition.claim);
    let execution_id = agentype_core::ExecutionId::new();

    // A snapshot that disagrees on the exact binding key is rejected.
    let mut snap = snapshot(
        &execution_id,
        &acquisition.provisioning_binding,
        &catalog,
        &acquisition.claim,
        &physical,
    );
    snap.adapter_binding_key = "k2".into();
    assert!(kernel
        .create_execution_with_snapshot(
            &acquisition.claim,
            physical_binding(&acquisition.claim),
            &snap
        )
        .is_err());

    // A snapshot that disagrees on the validated config digest is rejected.
    let mut snap = snapshot(
        &execution_id,
        &acquisition.provisioning_binding,
        &catalog,
        &acquisition.claim,
        &physical,
    );
    snap.source_config_digest = ConfigDigest::new(
        "sha256:9999999999999999999999999999999999999999999999999999999999999999",
    )
    .unwrap();
    assert!(kernel
        .create_execution_with_snapshot(
            &acquisition.claim,
            physical_binding(&acquisition.claim),
            &snap
        )
        .is_err());
}

#[test]
fn source_enumeration_fails_closed_on_missing_overlay() {
    let env = Env::new("enumeration", 1);
    let kernel = &env.kernel;
    publish_agent_type(kernel, "reviewer");
    let _ = publish_catalog(kernel, "h");

    let conn = env.raw();
    conn.execute("DROP TRIGGER spawn_source_dispositions_no_delete", [])
        .unwrap();
    conn.execute("DELETE FROM spawn_source_dispositions", [])
        .unwrap();
    assert!(kernel.list_active_spawn_source_refs().is_err());
    let _ = LogicalAgentState::Ready;
}

#[test]
fn reconcile_pool_excludes_typed_population() {
    let env = Env::new("capacity", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    let report = kernel.reconcile_pool().unwrap();
    assert_eq!(report.born, 1);
    assert_eq!(report.retired, 0);
    assert_eq!(
        kernel.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Ready
    );
    let report = kernel.reconcile_pool().unwrap();
    assert_eq!(report.born, 0);
    assert_eq!(report.retired, 0);
}

#[test]
fn bound_agent_is_not_legacy_claimable() {
    let env = Env::new("quarantine", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "i");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let _ = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    assert!(kernel.claim_next_available().unwrap().is_none());
}

/// Return an agent to READY with a reusable WARM Incarnation, as a completed
/// execution would, so a second typed Task can be acquired.
fn finish_current_attempt(env: &Env, task_id: &agentype_core::TaskId, incarnation: &str) {
    let conn = env.raw();
    conn.execute(
        "UPDATE attempts SET state='SUCCEEDED', ended_at=0 WHERE task_id=?1",
        rusqlite::params![task_id.as_str()],
    )
    .unwrap();
    conn.execute(
        "UPDATE leases SET state='RELEASED', ended_at=0 WHERE task_id=?1",
        rusqlite::params![task_id.as_str()],
    )
    .unwrap();
    conn.execute(
        "UPDATE tasks SET state='COMPLETED', current_attempt_id=NULL WHERE id=?1",
        rusqlite::params![task_id.as_str()],
    )
    .unwrap();
    conn.execute(
        "UPDATE incarnations SET state='WARM' WHERE id=?1",
        rusqlite::params![incarnation],
    )
    .unwrap();
    conn.execute(
        "UPDATE logical_agents SET state='READY', current_task_id=NULL
         WHERE id=(SELECT logical_agent_id FROM attempts WHERE task_id=?1)",
        rusqlite::params![task_id.as_str()],
    )
    .unwrap();
}

fn count_provisioning_bindings(env: &Env) -> i64 {
    env.raw()
        .query_row("SELECT COUNT(*) FROM provisioning_bindings", [], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn acquisition_rejects_a_policy_disabled_after_resolution() {
    let env = Env::new("policyrace", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "j");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // The policy was ACTIVE at resolution; disable it before the authority
    // transaction. The acquisition MUST re-load and reject it.
    kernel
        .set_adapter_binding_policy_status(
            &catalog.selection.adapter_policy,
            ConfigStatus::Disabled,
        )
        .unwrap();
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
    assert_eq!(count_provisioning_bindings(&env), 0);
}

#[test]
fn acquisition_rejects_partition_target_kind_mismatch() {
    let env = Env::new("kindmismatch", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "k");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // The task partition target is `local` whose authoritative kind is
    // `other_kind`, disagreeing with the policy's `default`.
    let mut bad = ExecutionRegistry::new();
    bad.register_target(ExecutionTargetConfig::new("local", "other_kind", false))
        .unwrap();
    bad.register_profile(ExecutionProfileConfig::new("default"))
        .unwrap();
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &bad)
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
    assert_eq!(count_provisioning_bindings(&env), 0);
}

#[test]
fn provisioning_binding_durably_qualifies_the_exact_binding() {
    let env = Env::new("recovery", 1);
    let pin;
    let incarnation;
    {
        let kernel = &env.kernel;
        pin = publish_agent_type(kernel, "reviewer");
        let catalog = publish_catalog(kernel, "l");
        let task_id = admit_typed_task(kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let acquisition = kernel
            .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
            .unwrap();
        assert_eq!(acquisition.provisioning_binding.adapter_kind, "default");
        assert_eq!(acquisition.provisioning_binding.adapter_binding_key, "k1");
        incarnation = acquisition.provisioning_binding.incarnation_id.clone();
    }
    // Reopen the store: the exact proven physical binding survives the
    // acquisition transaction and is reconstructable without the transient
    // resolver candidate.
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(2_000.0));
    let reopened = Kernel::open(&env.path, clock, 10.0, MAX_BYTES).unwrap();
    let stored = reopened
        .get_provisioning_binding(&incarnation)
        .unwrap()
        .expect("binding survives restart");
    assert_eq!(stored.adapter_kind, "default");
    assert_eq!(stored.adapter_binding_key, "k1");
    assert_eq!(stored.agent_type, pin);
}

#[test]
fn resident_agent_reuses_the_provisioning_binding() {
    let env = Env::new("reuse", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "m");
    let task_a = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let first = kernel
        .acquire_typed_task_existing(&task_a, &agent, &catalog.selection, &registry())
        .unwrap();
    let incarnation = first.provisioning_binding.incarnation_id.clone();
    finish_current_attempt(&env, &task_a, incarnation.as_str());

    let task_b = admit_typed_task(kernel, pin.clone());
    let second = kernel
        .acquire_typed_task_existing(&task_b, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_eq!(second.provisioning_binding.incarnation_id, incarnation);
    assert_eq!(
        second.provisioning_binding.provisioning_binding_id,
        first.provisioning_binding.provisioning_binding_id
    );
    assert_eq!(count_provisioning_bindings(&env), 1);
}

#[test]
fn source_change_rolls_over_to_a_new_incarnation() {
    let env = Env::new("rollover", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let mut catalog_a = publish_catalog(kernel, "n1");
    let mut catalog_b = publish_catalog(kernel, "n2");
    // Both configs are active, so refresh the frontier captured for each
    // selection to the now-current frontier.
    let frontier = kernel.catalog_frontier_digest().unwrap();
    catalog_a.selection.catalog_frontier_digest = frontier.clone();
    catalog_b.selection.catalog_frontier_digest = frontier;
    let task_a = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let first = kernel
        .acquire_typed_task_existing(&task_a, &agent, &catalog_a.selection, &registry())
        .unwrap();
    let old_incarnation = first.provisioning_binding.incarnation_id.clone();
    finish_current_attempt(&env, &task_a, old_incarnation.as_str());

    // A different source/config requires a fresh Incarnation, not an in-place
    // rebind.
    let task_b = admit_typed_task(kernel, pin.clone());
    let second = kernel
        .acquire_typed_task_existing(&task_b, &agent, &catalog_b.selection, &registry())
        .unwrap();
    assert_ne!(second.provisioning_binding.incarnation_id, old_incarnation);
    assert_eq!(count_provisioning_bindings(&env), 2);
    // The old binding remains immutable and readable.
    assert!(kernel
        .get_provisioning_binding(&old_incarnation)
        .unwrap()
        .is_some());
}

#[test]
fn evidence_from_another_physical_domain_is_rejected() {
    let env = Env::new("evidencekey", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let mut catalog = publish_catalog(kernel, "p");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // Evidence imported from a different installed domain K2, while the
    // selection commits to K1. The exact binding is now part of the evidence
    // subject, so the authority transaction rejects the substitution.
    catalog.selection.evidence = ResolvedProvisioningEvidence::from_imported_binding(
        catalog.selection.adapter_policy.clone(),
        "default",
        "k2",
        catalog.selection.spawn_source.clone(),
        catalog.selection.source_config.clone(),
        catalog.config_digest.clone(),
        safety(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    assert!(kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .is_err());
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Queued);
    assert_eq!(count_provisioning_bindings(&env), 0);
}

#[test]
fn legacy_incarnation_with_physical_history_is_not_requalified() {
    let env = Env::new("legacyhistory", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "q");
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // A legacy WARM Incarnation that already executed on the exact same physical
    // domain K1, but with no durable SourceConfig provenance. The domain match
    // does NOT prove the SourceConfig, so it must not be requalified in place.
    let legacy = "inc-legacy-history";
    let conn = env.raw();
    conn.execute(
        "INSERT INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
         VALUES(?1,?2,1,'local','WARM',1)",
        rusqlite::params![legacy, agent.as_str()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO batches(id,state,metadata_json,created_at,updated_at)
         VALUES('batch-legacy','ACTIVE','{}',1,1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks(id,batch_id,name,payload_json,acceptance_json,partition_name,
            continuity,affinity_tags_json,workspace_mode,required,priority,state,
            max_attempts,retry_classes_json,base_backoff_seconds,max_backoff_seconds,
            fencing_epoch,created_at,updated_at)
         VALUES('task-legacy','batch-legacy','legacy','{}','{}','general','none','[]',
            'read_only',1,0,'COMPLETED',1,'[]',0,1,1,1,1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attempts(id,task_id,logical_agent_id,incarnation_id,attempt_number,
            lease_epoch,state,execution_target,execution_profile,partition_name,created_at)
         VALUES('attempt-legacy','task-legacy',?1,?2,1,1,'SUCCEEDED','local','default','general',1)",
        rusqlite::params![agent.as_str(), legacy],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO executions(id,request_id,task_id,attempt_id,incarnation_id,
            execution_target,execution_profile,adapter_kind,adapter_binding_key,state,
            started_at,updated_at)
         VALUES('exec-legacy','request-legacy','task-legacy','attempt-legacy',?1,
            'local','default','default','k1','SUCCEEDED',1,1)",
        rusqlite::params![legacy],
    )
    .unwrap();

    let task_id = admit_typed_task(kernel, pin.clone());
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_ne!(
        acquisition.provisioning_binding.incarnation_id.as_str(),
        legacy
    );
    let legacy_state: String = env
        .raw()
        .query_row(
            "SELECT state FROM incarnations WHERE id=?1",
            rusqlite::params![legacy],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_state, "LOST");
}

#[test]
fn fresh_legacy_incarnation_is_requalified_in_place() {
    let env = Env::new("requalify", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "o");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();

    // A legacy Incarnation with no execution history: adoption is valid.
    let legacy = "inc-legacy-fresh";
    env.raw()
        .execute(
            "INSERT INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
             VALUES(?1,?2,1,'local','WARM',1)",
            rusqlite::params![legacy, agent.as_str()],
        )
        .unwrap();

    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_eq!(
        acquisition.provisioning_binding.incarnation_id.as_str(),
        legacy
    );
    assert_eq!(count_provisioning_bindings(&env), 1);
}

/// ADR-0009: V0.1 `MOVE_CAPACITY` counts only `UNBOUND` LogicalAgents. A typed
/// (`BOUND`) agent is governed by typed provisioning and MUST NOT be moved by a
/// legacy capacity command (which would silently relocate a typed agent across
/// the frozen B.3 partition placement gate).
#[test]
fn move_capacity_never_moves_bound_typed_agents() {
    let env = Env::new("move-bound", 1);
    let ids: Vec<String> = {
        let conn = env.raw();
        let mut stmt = conn
            .prepare("SELECT id FROM logical_agents WHERE partition_name='general' ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    assert_eq!(ids.len(), 1);
    let pin = publish_agent_type(&env.kernel, "reviewer");
    let bound = agentype_core::LogicalAgentId::from_string(ids[0].clone());
    env.kernel.bind_logical_agent_type(&bound, &pin).unwrap();

    env.kernel
        .upsert_partition(&PartitionSpec::new(
            "other",
            0,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    // The only capacity member is BOUND, so no agent may be moved.
    env.kernel.move_capacity("general", "other", 1).unwrap();

    let stayed = env.kernel.logical_agent(&bound).unwrap();
    assert_eq!(stayed.partition.as_str(), "general");
    assert!(
        stayed.pending_partition.is_none(),
        "a bound typed agent must not be staged for a legacy capacity move"
    );
}

/// `materialize` is pure/prepare-only, so a prepared-but-not-started typed claim
/// has NO physical side effect: cancelling it simply releases the agent (no
/// writer-quiescence obligation).
#[test]
fn cancel_task_releases_a_prepared_claim() {
    let env = Env::new("cancel-pending-task", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "ct");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    // Post-commit / pre-Execution: Attempt + Lease + ProvisioningBinding, no Execution.
    let _acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Leased);

    kernel.cancel_task(&task_id, false).unwrap();

    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Cancelled);
    assert!(matches!(
        kernel.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Ready | LogicalAgentState::Retired
    ));
    assert!(kernel.open_escalation_for_task(&task_id).is_err());
}

/// The batch-cancel path behaves the same.
#[test]
fn cancel_batch_releases_a_prepared_claim() {
    let env = Env::new("cancel-pending-batch", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "cb");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let _acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let batch = kernel.task(&task_id).unwrap().batch_id;

    kernel.cancel_batch(&batch).unwrap();

    assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Cancelled);
    assert!(matches!(
        kernel.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Ready | LogicalAgentState::Retired
    ));
    assert!(kernel.open_escalation_for_task(&task_id).is_err());
}

/// The committed-claim authority fence reuses the full AuthoritySnapshot
/// validation, so a partial durable inconsistency (Task.current_attempt_id or
/// Task.fencing_epoch) is rejected, not just Lease state.
#[test]
fn claim_authority_fence_rejects_tampered_task_fence() {
    let env = Env::new("fence-tamper", 1);
    let kernel = &env.kernel;
    let pin = publish_agent_type(kernel, "reviewer");
    let catalog = publish_catalog(kernel, "ft");
    let task_id = admit_typed_task(kernel, pin.clone());
    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let acquisition = kernel
        .acquire_typed_task_existing(&task_id, &agent, &catalog.selection, &registry())
        .unwrap();
    let claim = &acquisition.claim;
    assert!(kernel.claim_authority_is_current(claim).unwrap());

    env.raw()
        .execute(
            "UPDATE tasks SET current_attempt_id=NULL WHERE id=?1",
            rusqlite::params![task_id.as_str()],
        )
        .unwrap();
    assert!(!kernel.claim_authority_is_current(claim).unwrap());

    env.raw()
        .execute(
            "UPDATE tasks SET current_attempt_id=?1, fencing_epoch=fencing_epoch+1 WHERE id=?2",
            rusqlite::params![claim.attempt_id.as_str(), task_id.as_str()],
        )
        .unwrap();
    assert!(!kernel.claim_authority_is_current(claim).unwrap());
}
