//! M6-B.3 typed admission and matching conformance (schema v7): exact-revision
//! pinning, atomic Task + GenerationTaskBinding + TaskAgentRequirement creation,
//! the Generation policy fold, write-once agent bindings, and pure matching of
//! existing bound agents.

use agentype_agent_contract::{
    AffinityConstraint, AgentRequirementDraft, AgentRequirementPreferences, AgentType,
    AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget, CapabilityCatalog,
    CapabilityDefinition, CapabilityPolarity, CapabilityRef, ContinuityMode, GenerationPolicy,
    LifecycleMode, MatcherKind, NetworkPolicy, SecurityClass, SecurityContract,
};
use agentype_core::{
    Clock, Error, InformationFunction, ManualClock, PartitionSpec, ProposalStateKind,
    RawWorkIntent, Retention, SemanticInputSet, TaskSpec, WorkspaceMode,
};
use agentype_storage_sqlite::Kernel;
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
        required_capabilities: BTreeMap::new(),
        min_workspace: WorkspaceMode::Write,
        min_network: NetworkPolicy::Restricted,
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
        required_type: Some(AgentTypeSelector::Exact(pin)),
        required_capabilities: BTreeMap::new(),
        required_network: NetworkPolicy::Restricted,
        required_attempt_isolation: false,
        sandbox_policy: None,
        required_anchor: None,
        budget: Budget::new(50.0).unwrap(),
        preferred: AgentRequirementPreferences::default(),
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
    assert_eq!(requirement.required_type, Some(pin.clone()));
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
    assert!(matched[0].warm);
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
    // The generation-wide ceiling/floor strengthens the Task requirement.
    assert_eq!(requirement.hard.required_workspace, WorkspaceMode::Write);
    assert_eq!(requirement.hard.budget, Budget::new(10.0).unwrap());
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
