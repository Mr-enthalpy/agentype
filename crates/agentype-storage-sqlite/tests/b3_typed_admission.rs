//! M6-B.3 typed admission and matching conformance (schema v7): exact-revision
//! pinning, atomic Task + GenerationTaskBinding + TaskAgentRequirement creation,
//! the Generation policy fold, write-once agent bindings, and pure matching of
//! existing bound agents.

use agentype_agent_contract::{
    AffinityConstraint, AgentRequirementDraft, AgentType, AgentTypeContract, AgentTypeRef,
    AgentTypeSelector, Budget, CapabilityCatalog, CapabilityDefinition, CapabilityPolarity,
    CapabilityRef, ContinuityMode, GenerationPolicy, LifecycleMode, MatcherKind, NetworkPolicy,
    SecurityClass, SecurityContract,
};
use agentype_core::{
    Clock, Error, InformationFunction, LogicalAgentId, LogicalAgentState, ManualClock, PartitionId,
    PartitionSpec, ProposalStateKind, RawWorkIntent, Retention, SemanticInputSet, TaskSpec,
    TaskState, WorkspaceMode,
};
use agentype_storage_sqlite::{AgentTypeStatus, Kernel};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn sample_policy() -> GenerationPolicy {
    GenerationPolicy {
        allowed_information_functions: vec![InformationFunction::Expand],
        max_workspace: WorkspaceMode::Write,
        max_network: NetworkPolicy::Enabled,
        requires_attempt_isolation: false,
        min_continuity: ContinuityMode::Logical,
        sandbox_policy: None,
        budget_ceiling: Budget::new(10.0).unwrap(),
        allowed_affinity: None,
        anchor_constraint: None,
    }
}

fn memory_kernel() -> Kernel {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap()
}

fn partitioned_kernel() -> Kernel {
    let kernel = memory_kernel();
    kernel
        .upsert_partition(&PartitionSpec::new(
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

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn publish_agent_type(kernel: &Kernel, id: &str, revision: u64) -> AgentTypeRef {
    let agent = AgentType {
        type_ref: type_ref(id, revision),
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
    };
    kernel.publish_agent_type(&agent).unwrap();
    agent.type_ref
}

fn draft(pin: AgentTypeRef) -> AgentRequirementDraft {
    AgentRequirementDraft {
        required_type: AgentTypeSelector::Exact(pin),
        required_capabilities: BTreeMap::new(),
        required_network: NetworkPolicy::Restricted,
        required_attempt_isolation: false,
        sandbox_policy: None,
        required_anchor: None,
        budget: Budget::new(50.0).unwrap(),
    }
}

/// Compile one root intent whose suggested TaskSpec is `spec`.
fn compile_proposal(kernel: &Kernel, spec: TaskSpec) -> agentype_core::ProposalRecord {
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "typed_work".into(),
        objective: "typed work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec),
    };
    kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap()
}

fn latest_draft(type_id: &str) -> AgentRequirementDraft {
    AgentRequirementDraft {
        required_type: AgentTypeSelector::latest(type_id).unwrap(),
        required_capabilities: BTreeMap::new(),
        required_network: NetworkPolicy::Restricted,
        required_attempt_isolation: false,
        sandbox_policy: None,
        required_anchor: None,
        budget: Budget::new(50.0).unwrap(),
    }
}

fn file_path(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{tag}-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("scheduler.db")
}

#[test]
fn typed_admission_pins_exact_type_and_derives_spec_dimensions() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );

    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin.clone()))
        .unwrap();

    let requirement = kernel
        .get_task_agent_requirement(&task_id)
        .unwrap()
        .expect("typed requirement exists");
    assert_eq!(requirement.required_type, pin.clone());
    assert_eq!(
        requirement.hard.information_function,
        InformationFunction::Expand
    );
    // TaskSpec is the single authority for the derived dimensions.
    assert_eq!(requirement.hard.required_workspace, WorkspaceMode::ReadOnly);
    assert_eq!(requirement.hard.budget, Budget::new(50.0).unwrap());

    // Matching requires an explicit, exact binding.
    assert!(kernel
        .match_existing_agents_for_task(&task_id)
        .unwrap()
        .is_empty());

    let agent = kernel.ready_agent("general").unwrap();
    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    assert_eq!(
        kernel.get_logical_agent_type_binding(&agent).unwrap(),
        Some(pin.clone())
    );

    let matched = kernel.match_existing_agents_for_task(&task_id).unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].logical_agent_id, agent);
    assert_eq!(matched[0].agent_type.type_ref, pin);
    assert_eq!(matched[0].lifecycle, LifecycleMode::Resident);
}

#[test]
fn typed_admission_fails_closed_and_leaves_no_task() {
    let kernel = partitioned_kernel();
    // No AgentType is published, so the exact selector cannot resolve.
    let missing = type_ref("missing", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );

    let err = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(missing))
        .unwrap_err();
    assert!(matches!(
        err,
        Error::InvariantViolation(_) | Error::NotFound(_)
    ));

    // The proposal is untouched and no Task was materialized.
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    let view = kernel.get_generation_view(&proposal.generation_id).unwrap();
    assert!(view.admitted_task_ids.is_empty());
}

#[test]
fn typed_admission_replay_is_stable_and_conflict_is_rejected() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );

    let first = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin.clone()))
        .unwrap();
    // Exact replay returns the same Task and does not create a second requirement.
    let replay = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin.clone()))
        .unwrap();
    assert_eq!(first, replay);

    // A conflicting requirement is a Conflict, not a second admission.
    let mut conflicting = draft(pin);
    conflicting.budget = Budget::new(999.0).unwrap();
    let err = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, conflicting)
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));
}

#[test]
fn generation_policy_folds_into_typed_requirement() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let policy = sample_policy();
    let gen = kernel
        .create_generation_with_policy(json!({}), Some(policy.clone()))
        .unwrap();
    assert_eq!(
        kernel.get_generation_policy(&gen.generation_id).unwrap(),
        Some(policy)
    );

    let intent = RawWorkIntent {
        raw_intent_key: "typed_work".into(),
        objective: "typed work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({})).partition("general")),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();

    let requirement = kernel
        .get_task_agent_requirement(&task_id)
        .unwrap()
        .unwrap();
    // The generation is an authority ceiling: the ReadOnly Task keeps ReadOnly,
    // and the ceiling still tightens the budget.
    assert_eq!(requirement.hard.required_workspace, WorkspaceMode::ReadOnly);
    assert_eq!(requirement.hard.budget, Budget::new(10.0).unwrap());
}

#[test]
fn policy_generation_rejects_a_task_that_exceeds_the_ceiling() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let mut policy = sample_policy();
    policy.max_workspace = WorkspaceMode::ReadOnly;
    let gen = kernel
        .create_generation_with_policy(json!({}), Some(policy))
        .unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "typed_work".into(),
        objective: "typed work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        // A Write Task exceeds the generation's ReadOnly ceiling.
        suggested_task_spec: Some(
            TaskSpec::new("audit", json!({}))
                .partition("general")
                .write(),
        ),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    let err = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)));
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
}

#[test]
fn bindings_are_write_once_and_require_a_valid_exact_revision() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let agent = kernel.ready_agent("general").unwrap();

    kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    let err = kernel.bind_logical_agent_type(&agent, &pin).unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));

    // Binding a missing revision fails closed.
    let other = kernel.ready_agent("general");
    if let Ok(other_agent) = other {
        let missing = type_ref("missing", 3);
        let err = kernel
            .bind_logical_agent_type(&other_agent, &missing)
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    // A missing LogicalAgent fails closed.
    let ghost = agentype_core::LogicalAgentId::from_string("agent-ghost");
    let err = kernel.bind_logical_agent_type(&ghost, &pin).unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[test]
fn capability_definitions_are_reusable_for_requirement_shape_checks() {
    // Sanity: the catalog used by requirement validation is the same validated
    // read the runtime exposes.
    let kernel = memory_kernel();
    let reference = CapabilityRef::new("tools", 1).unwrap();
    kernel
        .publish_capability_definition(
            &reference,
            &CapabilityDefinition {
                matcher_kind: MatcherKind::Set,
                security_class: SecurityClass::Functional,
                polarity: CapabilityPolarity::Ability,
            },
        )
        .unwrap();
    let catalog: CapabilityCatalog = kernel.load_capability_catalog().unwrap();
    assert!(catalog.contains(&reference));
}

#[test]
fn sqlite_mechanically_rejects_typed_table_mutation() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("m6b3-guards-{}", nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");

    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        let pin = publish_agent_type(&kernel, "reviewer", 1);

        let gen = kernel
            .create_generation_with_policy(json!({}), Some(sample_policy()))
            .unwrap();
        let intent = RawWorkIntent {
            raw_intent_key: "typed_work".into(),
            objective: "typed work".into(),
            information_function: InformationFunction::Expand,
            semantic_input_set: SemanticInputSet::new(),
            rationale: None,
            suggested_task_spec: Some(TaskSpec::new("audit", json!({})).partition("general")),
        };
        let proposal = kernel
            .compile_root_intent(&gen.generation_id, intent, "session", 1)
            .unwrap();
        kernel
            .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin.clone()))
            .unwrap();

        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    }

    let conn = rusqlite::Connection::open(&path).unwrap();
    // Every M6-B.3 durable table is immutable: content mutation and deletion are
    // rejected by SQLite itself, without the Kernel.
    assert!(conn
        .execute(
            "UPDATE task_agent_requirements SET requirement_digest='sha256:00'",
            [],
        )
        .is_err());
    assert!(conn
        .execute("DELETE FROM task_agent_requirements", [])
        .is_err());
    assert!(conn
        .execute(
            "UPDATE logical_agent_type_bindings SET type_revision=99",
            [],
        )
        .is_err());
    assert!(conn
        .execute("DELETE FROM logical_agent_type_bindings", [])
        .is_err());
    assert!(conn
        .execute(
            "UPDATE generation_policies SET policy_digest='sha256:00'",
            [],
        )
        .is_err());
    assert!(conn.execute("DELETE FROM generation_policies", []).is_err());
}

// =============================================================================
// P0-1: typed Tasks are invisible to the legacy untyped dispatch path.
// =============================================================================

#[test]
fn typed_task_is_quarantined_from_legacy_claim() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );
    let task = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();
    let agent = kernel.ready_agent("general").unwrap();

    // A READY, unbound legacy agent exists, but the typed Task acquires no M5
    // authority and the agent stays unassigned.
    assert!(kernel.claim_next_available().unwrap().is_none());
    let record = kernel.task(&task).unwrap();
    assert_eq!(record.state, TaskState::Queued);
    assert!(record.current_attempt_id.is_none());
    assert_eq!(
        kernel.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Ready
    );
}

#[test]
fn typed_task_does_not_birth_a_legacy_consumer() {
    // A capacity-zero partition with no consumers: a legacy Task births one, a
    // typed Task must not.
    let legacy = memory_kernel();
    legacy
        .upsert_partition(&PartitionSpec::new(
            "general",
            0,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    legacy.reconcile_pool().unwrap();
    legacy
        .submit_batch(&[TaskSpec::new("legacy", json!({})).partition("general")])
        .unwrap();
    legacy.ensure_task_consumers().unwrap();
    assert!(legacy.ready_agent("general").is_ok());

    let typed = memory_kernel();
    typed
        .upsert_partition(&PartitionSpec::new(
            "general",
            0,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    typed.reconcile_pool().unwrap();
    let pin = publish_agent_type(&typed, "reviewer", 1);
    let proposal = compile_proposal(
        &typed,
        TaskSpec::new("audit", json!({})).partition("general"),
    );
    typed
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();
    typed.ensure_task_consumers().unwrap();
    assert!(typed.ready_agent("general").is_err());
}

#[test]
fn typed_mode_marker_quarantines_even_if_requirement_row_is_lost() {
    let path = file_path("b3-typed-marker");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    kernel
        .upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    kernel.reconcile_pool().unwrap();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );
    let task = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();

    // Simulate corruption: remove the requirement row.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("DROP TRIGGER task_agent_requirements_immutable_delete", [])
            .unwrap();
        conn.execute(
            "DELETE FROM task_agent_requirements WHERE task_id=?1",
            [task.as_str()],
        )
        .unwrap();
    }
    assert!(kernel.get_task_agent_requirement(&task).unwrap().is_none());

    // The positive parent marker keeps the Task out of the legacy path; it never
    // silently downgrades to a legacy Task.
    assert!(kernel.claim_next_available().unwrap().is_none());
    assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
}

#[test]
fn lost_generation_policy_still_blocks_legacy_admission() {
    let path = file_path("b3-policy-marker");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
    kernel
        .upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
    kernel.reconcile_pool().unwrap();
    let gen = kernel
        .create_generation_with_policy(json!({}), Some(sample_policy()))
        .unwrap();

    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("DROP TRIGGER generation_policies_immutable_delete", [])
            .unwrap();
        conn.execute(
            "DELETE FROM generation_policies WHERE generation_id=?1",
            [gen.generation_id.as_str()],
        )
        .unwrap();
    }
    assert!(kernel
        .get_generation_policy(&gen.generation_id)
        .unwrap()
        .is_none());

    let intent = RawWorkIntent {
        raw_intent_key: "legacy_after_policy_loss".into(),
        objective: "legacy admission".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({}))),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    // The `POLICY` marker forbids legacy admission even without the row.
    let err = kernel
        .admit_proposal(&proposal.proposal_id, 0, None)
        .unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));
}

#[test]
fn fresh_binding_requires_a_published_type() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    kernel
        .set_agent_type_status(&pin, AgentTypeStatus::Deprecated)
        .unwrap();
    let agent = kernel.ready_agent("general").unwrap();
    let err = kernel.bind_logical_agent_type(&agent, &pin).unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));
}

#[test]
fn matching_respects_task_partition() {
    let kernel = memory_kernel();
    for name in ["p1", "p2"] {
        kernel
            .upsert_partition(&PartitionSpec::new(
                name,
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
    }
    kernel.reconcile_pool().unwrap();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    for name in ["p1", "p2"] {
        let agent = kernel.ready_agent(name).unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
    }

    let proposal = compile_proposal(&kernel, TaskSpec::new("audit", json!({})).partition("p1"));
    let task = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();
    let matched = kernel.match_existing_agents_for_task(&task).unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].partition, PartitionId::new("p1"));
}

#[test]
fn mixed_queue_dispatches_legacy_work_past_typed_task() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );
    let typed = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();

    let (_batch, ids) = kernel
        .submit_batch(&[TaskSpec::new("legacy", json!({})).partition("general")])
        .unwrap();
    let legacy = ids.values().next().unwrap().clone();

    // The only eligible claim is the legacy Task.
    let claim = kernel
        .claim_next_available()
        .unwrap()
        .expect("legacy claim");
    assert_eq!(claim.task_id, legacy);
    assert_ne!(claim.task_id, typed);
    // The typed Task never becomes claimable.
    assert!(kernel.claim_next_available().unwrap().is_none());
    assert_eq!(kernel.task(&typed).unwrap().state, TaskState::Queued);
}

// =============================================================================
// P0-2: a policy-bearing Generation forbids legacy admission.
// =============================================================================

#[test]
fn policy_generation_rejects_legacy_admission() {
    let kernel = memory_kernel();
    let gen = kernel
        .create_generation_with_policy(json!({}), Some(sample_policy()))
        .unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "legacy_vs_policy".into(),
        objective: "legacy admission against a policy".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({}))),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();

    let err = kernel
        .admit_proposal(&proposal.proposal_id, 0, None)
        .unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));
    assert_eq!(
        kernel.get_proposal(&proposal.proposal_id).unwrap().state,
        ProposalStateKind::Pending
    );
    assert!(kernel
        .get_generation_view(&gen.generation_id)
        .unwrap()
        .admitted_task_ids
        .is_empty());
}

#[test]
fn no_policy_generation_still_accepts_legacy_admission() {
    let kernel = partitioned_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "legacy_ok".into(),
        objective: "legacy admission without a policy".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({}))),
    };
    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    assert!(kernel
        .admit_proposal(&proposal.proposal_id, 0, None)
        .is_ok());
}

// =============================================================================
// P1-1: only READY, unassigned bound agents are candidates.
// =============================================================================

#[test]
fn matching_returns_only_ready_unassigned_agents() {
    let path = file_path("b3-ready-only");
    {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&path, clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                4,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        let pin = publish_agent_type(&kernel, "reviewer", 1);

        let ids: Vec<String> = {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let mut statement = conn
                .prepare("SELECT id FROM logical_agents ORDER BY id")
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(ids.len(), 4);
        for id in &ids {
            kernel
                .bind_logical_agent_type(&LogicalAgentId::from_string(id.clone()), &pin)
                .unwrap();
        }

        let proposal = compile_proposal(
            &kernel,
            TaskSpec::new("audit", json!({})).partition("general"),
        );
        let task = kernel
            .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin.clone()))
            .unwrap();

        // Keep ids[0] READY and drive the others into non-ready M5 states.
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE logical_agents SET state='ASSIGNED' WHERE id=?1",
                [&ids[1]],
            )
            .unwrap();
            conn.execute(
                "UPDATE logical_agents SET state='RETIRED' WHERE id=?1",
                [&ids[2]],
            )
            .unwrap();
            conn.execute(
                "UPDATE logical_agents SET state='SUSPENDED' WHERE id=?1",
                [&ids[3]],
            )
            .unwrap();
        }

        let matched = kernel.match_existing_agents_for_task(&task).unwrap();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].logical_agent_id.as_str(), ids[0]);
    }
}

// =============================================================================
// P2-1: loose-selector replay semantics.
// =============================================================================

#[test]
fn latest_selector_replay_is_stable_until_catalog_drifts() {
    let kernel = partitioned_kernel();
    publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );

    let task = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, latest_draft("reviewer"))
        .unwrap();
    // Replay before any catalog drift is idempotent.
    let replay = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, latest_draft("reviewer"))
        .unwrap();
    assert_eq!(task, replay);

    // A newer revision is published; a `Latest` retry now resolves to it and is a
    // fail-closed Conflict, never a silent re-pin.
    publish_agent_type(&kernel, "reviewer", 2);
    let err = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, latest_draft("reviewer"))
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));

    // An exact replay of the committed revision stays idempotent.
    let exact = kernel
        .admit_typed_proposal(
            &proposal.proposal_id,
            0,
            None,
            draft(type_ref("reviewer", 1)),
        )
        .unwrap();
    assert_eq!(task, exact);
}

#[test]
fn exact_replay_survives_deprecation() {
    let kernel = partitioned_kernel();
    let pin = publish_agent_type(&kernel, "reviewer", 1);
    let proposal = compile_proposal(
        &kernel,
        TaskSpec::new("audit", json!({})).partition("general"),
    );
    let task = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin.clone()))
        .unwrap();

    kernel
        .set_agent_type_status(&pin, AgentTypeStatus::Deprecated)
        .unwrap();

    // A fresh pre-commit selection of the deprecated exact revision fails closed.
    let fresh = compile_proposal(
        &kernel,
        TaskSpec::new("audit2", json!({})).partition("general"),
    );
    let err = kernel
        .admit_typed_proposal(&fresh.proposal_id, 0, None, draft(pin.clone()))
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)));

    // Replaying the already-committed exact admission still returns the same Task:
    // deprecation is disposition drift, not loss of committed identity.
    let replay = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pin))
        .unwrap();
    assert_eq!(task, replay);
}
