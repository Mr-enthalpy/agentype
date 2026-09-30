//! M6-A Semantic Frontier Kernel contract tests.
//!
//! Pinned invariants:
//! - INV-A1: Every M6 semantic Task belongs to exactly one Generation.
//! - INV-A5: Proposal admission atomically creates M5 Task and GenerationTaskBinding.
//! - INV-A6: A proposal can admit at most one Task.
//! - INV-A7: No EXPAND Task may be admitted after Generation enters FROZEN.
//! - INV-A8: No Task of any InformationFunction may be admitted after CLOSED.
//! - INV-A9: Generation close never manufactures Task terminality.
//! - INV-A10: Generation barrier depends on M5 Task authority (terminal disposition).
//! - INV-A11: Every compression Task has an immutable SemanticInputSet.

mod common;
use common::*;

use agentype_core::{
    BatchId, BatchState, Clock, GenerationState, InformationFunction, ManualClock, PartitionId,
    PartitionSpec, ProposalStateKind, RawWorkIntent, ResultId, Retention, SemanticInputSet,
    TaskSpec, TaskState,
};
use agentype_storage_sqlite::Kernel;
use serde_json::json;
use std::sync::Arc;

const CONTINUITY_MAX_BYTES: usize = 16_384;

fn test_kernel() -> Kernel {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    let kernel =
        Kernel::open_memory(clock, 30.0, CONTINUITY_MAX_BYTES).expect("open in-memory kernel");

    // Upsert partitions
    for name in ["default", "general"] {
        let part = PartitionSpec {
            name: PartitionId::new(name),
            desired_capacity: 5,
            retention: Retention::Resident,
            execution_target: "local".into(),
            execution_profile: "default".into(),
            tags: vec![],
        };
        kernel.upsert_partition(&part).expect("upsert partition");
    }
    kernel.reconcile_pool().expect("reconcile pool");
    kernel
}

fn create_test_result(kernel: &Kernel) -> ResultId {
    let (_batch, ids) = kernel
        .submit_batch(&[TaskSpec::new("producer", json!({}))])
        .unwrap();
    let _task_id = ids.values().next().unwrap();
    let claim = kernel.claim_next_available().unwrap().expect("claim");
    let safety = unisolated_launch_binding(&claim);
    let launch = kernel.create_execution(&claim, safety).unwrap();
    let exec_id = launch.execution_id().clone();
    kernel
        .confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, &exec_id, &json!({}))
        .unwrap();
    kernel
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&exec_id),
            &json!({"produced": true}),
            None,
            false,
            false,
        )
        .unwrap()
        .expect("must produce result")
}

#[test]
fn test_generation_lifecycle_and_admit_contract() {
    let kernel = test_kernel();

    // 1. Create Generation
    let gen = kernel
        .create_generation(json!({"goal": "investigate memory leak"}))
        .expect("create generation");
    assert_eq!(gen.state, GenerationState::Open);
    assert_eq!(gen.revision, 0);
    assert_eq!(gen.admission_seq, 0);

    // 2. Compile an EXPAND intent
    let input_set = SemanticInputSet::new().with_artifact("heap_dump.bin");
    let intent = RawWorkIntent {
        raw_intent_key: "inspect_heap".into(),
        objective: "Inspect heap dump for allocations".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: input_set.clone(),
        rationale: Some("Suspected memory leak in buffer pool".into()),
        suggested_task_spec: Some(TaskSpec::new("inspect_heap", json!({}))),
    };

    let proposal = kernel
        .compile_intent(
            &gen.generation_id,
            intent.clone(),
            "root",
            "root_session_1",
            1,
        )
        .expect("compile intent");
    assert_eq!(proposal.state, ProposalStateKind::Pending);
    assert_eq!(proposal.information_function, InformationFunction::Expand);

    // Idempotent compilation returns the existing proposal
    let proposal_dup = kernel
        .compile_intent(&gen.generation_id, intent, "root", "root_session_1", 1)
        .expect("compile intent duplicate");
    assert_eq!(proposal_dup.proposal_id, proposal.proposal_id);

    // 3. Admit the proposal (INV-A5: atomically creates Task + Binding)
    let task_id = kernel
        .admit_proposal(&proposal.proposal_id, 0, None)
        .expect("admit proposal");

    // Check task in M5 kernel
    let task_row = kernel.task(&task_id).expect("fetch task");
    assert_eq!(task_row.id, task_id);
    assert_eq!(task_row.state, TaskState::Queued);

    // Check generation revision & seq
    let view = kernel
        .get_generation_view(&gen.generation_id)
        .expect("generation view");
    assert_eq!(view.generation.admission_seq, 1);
    assert_eq!(view.admitted_task_ids, vec![task_id.clone()]);
    assert!(!view.is_settled);

    // INV-A6: duplicate admission is rejected
    let dup_admit_res = kernel.admit_proposal(&proposal.proposal_id, 0, None);
    assert!(
        dup_admit_res.is_err(),
        "cannot admit already admitted proposal"
    );

    // 4. Freeze generation (INV-A7 barrier)
    // First compile another EXPAND intent that remains PENDING
    let expand_intent_2 = RawWorkIntent {
        raw_intent_key: "inspect_caches".into(),
        objective: "Inspect LRU caches".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("inspect_caches", json!({}))),
    };
    let proposal_expand_2 = kernel
        .compile_intent(
            &gen.generation_id,
            expand_intent_2,
            "worker",
            "worker_result_1",
            1,
        )
        .expect("compile second expand");

    // Produce a real durable Result for provenance validation
    let real_result = create_test_result(&kernel);

    // Compile a COMPRESS_POSITIVE intent
    let compress_intent = RawWorkIntent {
        raw_intent_key: "summarize_findings".into(),
        objective: "Summarize leak investigation findings".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new().with_result(real_result),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("summarize_findings", json!({}))),
    };
    let proposal_compress = kernel
        .compile_intent(
            &gen.generation_id,
            compress_intent,
            "root",
            "root_session_1",
            1,
        )
        .expect("compile compress proposal");

    // Now freeze generation
    kernel
        .freeze_generation(&gen.generation_id, 0)
        .expect("freeze generation");

    let view_frozen = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view after freeze");
    assert_eq!(view_frozen.generation.state, GenerationState::Frozen);
    assert_eq!(view_frozen.generation.revision, 1);
    // PENDING EXPAND proposal must be EXPIRED by freeze
    assert!(view_frozen
        .expired_proposal_ids
        .contains(&proposal_expand_2.proposal_id));

    // INV-A7: New EXPAND admission fails in FROZEN state
    let admit_expand_fail = kernel.admit_proposal(&proposal_expand_2.proposal_id, 1, None);
    assert!(admit_expand_fail.is_err());

    // But COMPRESS_POSITIVE is permitted in FROZEN state!
    let compress_task_id = kernel
        .admit_proposal(&proposal_compress.proposal_id, 1, None)
        .expect("admit compress in frozen generation");

    // 5. INV-A9/A10: Cannot close generation before tasks settle
    let close_fail = kernel.close_generation(&gen.generation_id, 1);
    assert!(
        close_fail.is_err(),
        "close must fail when tasks are not terminal"
    );

    // Cancel both tasks to make them terminal
    kernel.cancel_task(&task_id, true).expect("cancel task 1");
    kernel
        .cancel_task(&compress_task_id, true)
        .expect("cancel compress task");

    let view_terminal = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view when tasks terminal");
    assert!(view_terminal.is_settled);

    // 6. Close generation succeeds when settled
    kernel
        .close_generation(&gen.generation_id, 1)
        .expect("close generation");

    let view_closed = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view after close");
    assert_eq!(view_closed.generation.state, GenerationState::Closed);
    assert_eq!(view_closed.generation.revision, 2);

    // INV-A8: No task can be admitted into CLOSED generation
    let late_compress_intent = RawWorkIntent {
        raw_intent_key: "late_compress".into(),
        objective: "Late compress attempt".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("late_compress", json!({}))),
    };
    let compile_closed_fail = kernel.compile_intent(
        &gen.generation_id,
        late_compress_intent,
        "root",
        "root_session_1",
        1,
    );
    assert!(compile_closed_fail.is_err());
}

#[test]
fn test_proposal_rejection() {
    let kernel = test_kernel();
    let gen = kernel
        .create_generation(json!({}))
        .expect("create generation");

    let intent = RawWorkIntent {
        raw_intent_key: "unneeded_work".into(),
        objective: "Explore irrelevant directory".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("unneeded_work", json!({}))),
    };

    let proposal = kernel
        .compile_intent(&gen.generation_id, intent, "worker", "worker_1", 1)
        .expect("compile intent");

    kernel
        .reject_proposal(&proposal.proposal_id, "Out of scope for current objective")
        .expect("reject proposal");

    let view = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view");
    assert!(view.rejected_proposal_ids.contains(&proposal.proposal_id));

    // Cannot admit rejected proposal
    let admit_res = kernel.admit_proposal(&proposal.proposal_id, 0, None);
    assert!(admit_res.is_err());
}

#[test]
fn test_race_b_concurrent_admissions_same_proposal_single_winner() {
    let kernel = Arc::new(test_kernel());
    let gen = kernel
        .create_generation(json!({}))
        .expect("create generation");

    let intent = RawWorkIntent {
        raw_intent_key: "single_winner_test".into(),
        objective: "Perform task exactly once".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("single_winner_test", json!({}))),
    };

    let proposal = kernel
        .compile_intent(&gen.generation_id, intent, "root", "session", 1)
        .expect("compile intent");

    let mut handles = Vec::new();
    for _ in 0..8 {
        let k = Arc::clone(&kernel);
        let pid = proposal.proposal_id.clone();
        handles.push(std::thread::spawn(move || k.admit_proposal(&pid, 0, None)));
    }

    let mut successes = 0;
    let mut conflicts = 0;
    let mut admitted_tid = None;

    for h in handles {
        match h.join().unwrap() {
            Ok(tid) => {
                successes += 1;
                admitted_tid = Some(tid);
            }
            Err(_) => {
                conflicts += 1;
            }
        }
    }

    // Exactly one winner!
    assert_eq!(successes, 1, "Exactly one thread must admit the proposal");
    assert_eq!(conflicts, 7, "All other threads must fail admission");

    let view = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view");
    assert_eq!(view.admitted_task_ids.len(), 1);
    assert_eq!(view.admitted_task_ids[0], admitted_tid.unwrap());
}

#[test]
fn test_race_a_freeze_vs_admit_serializable() {
    let kernel = Arc::new(test_kernel());
    let gen = kernel
        .create_generation(json!({}))
        .expect("create generation");

    let intent = RawWorkIntent {
        raw_intent_key: "freeze_race_test".into(),
        objective: "Race with freeze".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("freeze_race_test", json!({}))),
    };

    let proposal = kernel
        .compile_intent(&gen.generation_id, intent, "worker", "worker_1", 1)
        .expect("compile intent");

    let k1 = Arc::clone(&kernel);
    let gid = gen.generation_id.clone();
    let freeze_handle = std::thread::spawn(move || k1.freeze_generation(&gid, 0));

    let k2 = Arc::clone(&kernel);
    let pid = proposal.proposal_id.clone();
    let admit_handle = std::thread::spawn(move || k2.admit_proposal(&pid, 0, None));

    let freeze_res = freeze_handle.join().unwrap();
    let admit_res = admit_handle.join().unwrap();

    // Freeze must always succeed if revision matched
    assert!(freeze_res.is_ok(), "freeze should succeed");

    let view = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view");
    assert_eq!(view.generation.state, GenerationState::Frozen);

    match admit_res {
        Ok(task_id) => {
            // Admit won the race: task must be included in generation
            assert!(view.admitted_task_ids.contains(&task_id));
        }
        Err(_) => {
            // Freeze won the race: proposal must be expired
            assert!(view.expired_proposal_ids.contains(&proposal.proposal_id));
        }
    }
}

#[test]
fn test_p0_1_proposal_isolation_across_generations() {
    let kernel = test_kernel();
    let g1 = kernel.create_generation(json!({"g": 1})).unwrap();
    let g2 = kernel.create_generation(json!({"g": 2})).unwrap();

    let intent1 = RawWorkIntent {
        raw_intent_key: "audit_code".into(),
        objective: "Audit repository code".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit_code", json!({}))),
    };
    let intent2 = intent1.clone();

    let p1 = kernel
        .compile_intent(&g1.generation_id, intent1, "root", "session_root", 1)
        .unwrap();
    let p2 = kernel
        .compile_intent(&g2.generation_id, intent2, "root", "session_root", 1)
        .unwrap();

    assert_ne!(p1.proposal_id, p2.proposal_id);
    assert_eq!(p1.generation_id, g1.generation_id);
    assert_eq!(p2.generation_id, g2.generation_id);

    let t2 = kernel.admit_proposal(&p2.proposal_id, 0, None).unwrap();
    let v1 = kernel.get_generation_view(&g1.generation_id).unwrap();
    let v2 = kernel.get_generation_view(&g2.generation_id).unwrap();

    assert!(v1.admitted_task_ids.is_empty());
    assert_eq!(v2.admitted_task_ids, vec![t2]);
}

#[test]
fn test_p0_1_proposal_content_mismatch_conflict() {
    let kernel = test_kernel();
    let g = kernel.create_generation(json!({})).unwrap();

    let intent1 = RawWorkIntent {
        raw_intent_key: "inspect_sql".into(),
        objective: "Inspect SQL queries for optimization".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("inspect_sql", json!({}))),
    };
    let p1 = kernel
        .compile_intent(&g.generation_id, intent1, "root", "session_1", 1)
        .unwrap();

    // Same identity (gen, kind, ref, key, version) but changed objective/payload
    let intent2 = RawWorkIntent {
        raw_intent_key: "inspect_sql".into(),
        objective: "DIFFERENT objective for the same key".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("inspect_sql", json!({}))),
    };
    let err = kernel
        .compile_intent(&g.generation_id, intent2, "root", "session_1", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));

    // Recompilation with exact same content returns original proposal
    let intent1_dup = RawWorkIntent {
        raw_intent_key: "inspect_sql".into(),
        objective: "Inspect SQL queries for optimization".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("inspect_sql", json!({}))),
    };
    let p1_dup = kernel
        .compile_intent(&g.generation_id, intent1_dup, "root", "session_1", 1)
        .unwrap();
    assert_eq!(p1.proposal_id, p1_dup.proposal_id);
}

#[test]
fn test_p0_2_generation_open_dynamic_admissions_with_completed_batches() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // 1. Admit T1
    let intent1 = RawWorkIntent {
        raw_intent_key: "task_1".into(),
        objective: "First dynamic task".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task_1", json!({}))),
    };
    let p1 = kernel
        .compile_intent(&gen.generation_id, intent1, "root", "session", 1)
        .unwrap();
    let t1 = kernel.admit_proposal(&p1.proposal_id, 0, None).unwrap();

    // 2. Complete T1 via M5 worker workflow
    let claim1 = kernel.claim_next_available().unwrap().expect("claim T1");
    assert_eq!(claim1.task_id, t1);
    let safety = unisolated_launch_binding(&claim1);
    let launch1 = kernel.create_execution(&claim1, safety).unwrap();
    let exec_id1 = launch1.execution_id().clone();
    kernel
        .confirm_running_and_renew(
            &claim1.attempt_id,
            claim1.lease_epoch,
            &exec_id1,
            &json!({}),
        )
        .unwrap();
    let _res1 = kernel
        .ack_success(
            &claim1.attempt_id,
            claim1.lease_epoch,
            Some(&exec_id1),
            &json!({"done": true}),
            None,
            false,
            false,
        )
        .unwrap()
        .expect("result for T1");

    // Verify T1's batch is completed
    let b1 = kernel
        .batch(&BatchId::from_string(format!("batch_{t1}")))
        .unwrap();
    assert_eq!(b1.state, BatchState::Completed);

    // 3. Generation is STILL Open! Now Root admits T2 dynamically into the same Generation
    let view_now = kernel.get_generation_view(&gen.generation_id).unwrap();
    assert_eq!(view_now.generation.state, GenerationState::Open);

    let intent2 = RawWorkIntent {
        raw_intent_key: "task_2".into(),
        objective: "Second dynamic task after T1 completion".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task_2", json!({}))),
    };
    let p2 = kernel
        .compile_intent(&gen.generation_id, intent2, "root", "session", 1)
        .unwrap();
    let t2 = kernel.admit_proposal(&p2.proposal_id, 0, None).unwrap();

    // 4. Assert T2 has its own ACTIVE batch and is immediately claimable/executable!
    let b2 = kernel
        .batch(&BatchId::from_string(format!("batch_{t2}")))
        .unwrap();
    assert_eq!(b2.state, BatchState::Active);

    let claim2 = kernel.claim_next_available().unwrap().expect("claim T2");
    assert_eq!(claim2.task_id, t2);
}

#[test]
fn test_p0_2_generation_open_dynamic_admissions_with_cancelled_batches() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // Admit T1
    let intent1 = RawWorkIntent {
        raw_intent_key: "task_cancel".into(),
        objective: "Task to be cancelled".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task_cancel", json!({}))),
    };
    let p1 = kernel
        .compile_intent(&gen.generation_id, intent1, "root", "session", 1)
        .unwrap();
    let t1 = kernel.admit_proposal(&p1.proposal_id, 0, None).unwrap();

    // Cancel T1
    kernel.cancel_task(&t1, true).unwrap();
    let t1_row = kernel.task(&t1).unwrap();
    assert_eq!(t1_row.state, TaskState::Cancelled);

    // Generation remains OPEN; admit T2
    let intent2 = RawWorkIntent {
        raw_intent_key: "task_after_cancel".into(),
        objective: "Task after cancellation".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task_after_cancel", json!({}))),
    };
    let p2 = kernel
        .compile_intent(&gen.generation_id, intent2, "root", "session", 1)
        .unwrap();
    let t2 = kernel.admit_proposal(&p2.proposal_id, 0, None).unwrap();

    // T2 is claimable and not blocked by T1's cancelled status
    let claim2 = kernel.claim_next_available().unwrap().expect("claim T2");
    assert_eq!(claim2.task_id, t2);
}

#[test]
fn test_p1_1_task_spec_dependencies_rejected() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // Rejected at compile_intent if suggested_task_spec has dependencies
    let spec_with_dep = TaskSpec::new("child", json!({})).depends_on(["parent"]);
    let intent = RawWorkIntent {
        raw_intent_key: "dep_task".into(),
        objective: "Dependent task".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec_with_dep.clone()),
    };
    let err = kernel
        .compile_intent(&gen.generation_id, intent, "root", "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvalidAuthority(_)));

    // Also rejected at admit_proposal if override_task_spec has dependencies
    let clean_intent = RawWorkIntent {
        raw_intent_key: "clean_task".into(),
        objective: "Clean intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: None,
    };
    let p = kernel
        .compile_intent(&gen.generation_id, clean_intent, "root", "session", 1)
        .unwrap();

    let admit_err = kernel
        .admit_proposal(&p.proposal_id, 0, Some(spec_with_dep))
        .unwrap_err();
    assert!(matches!(
        admit_err,
        agentype_core::Error::InvalidAuthority(_)
    ));
}

#[test]
fn test_p1_2_provenance_validation_nonexistent_result_rejected() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let fake_id = ResultId::new();
    let intent = RawWorkIntent {
        raw_intent_key: "compression_fake".into(),
        objective: "Compress non-existent result".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new().with_result(fake_id),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("compression_fake", json!({}))),
    };

    let err = kernel
        .compile_intent(&gen.generation_id, intent, "root", "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::NotFound(_)));

    // Test fail-closed JSON decoding of semantic_input_set
    use agentype_storage_sqlite::frontier::semantic_input_set_from_json;
    // 1. Not an object
    let not_obj = json!(["not", "an", "object"]);
    assert!(semantic_input_set_from_json(&not_obj).is_err());

    // 2. Unknown field
    let unknown_field = json!({"result_ids": [], "unrecognized_field": 123});
    assert!(semantic_input_set_from_json(&unknown_field).is_err());

    // 3. Array elements not string
    let non_string_el = json!({"result_ids": [12345]});
    assert!(semantic_input_set_from_json(&non_string_el).is_err());
}

#[test]
fn test_p1_3_compiler_unspecified_spec_and_admission_enforcement() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // 1. Intent with suggested_task_spec: None
    let intent = RawWorkIntent {
        raw_intent_key: "unspecified_spec".into(),
        objective: "Unspecified spec intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: None,
    };
    let p = kernel
        .compile_intent(&gen.generation_id, intent, "root", "session", 1)
        .unwrap();
    assert!(p.normalized_task_spec.is_none());

    // 2. Admission fails if Root provides no spec
    let err = kernel.admit_proposal(&p.proposal_id, 0, None).unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvalidAuthority(_)));

    // 3. Admission succeeds when Root explicitly provides spec
    let root_spec = TaskSpec::new("admitted_spec", json!({"root_decided": true}));
    let tid = kernel
        .admit_proposal(&p.proposal_id, 0, Some(root_spec.clone()))
        .unwrap();
    let task = kernel.task(&tid).unwrap();
    assert_eq!(task.name, "admitted_spec");

    // 4. If proposal has Some(spec) and Root provides conflicting spec, reject with Conflict
    let intent_with_spec = RawWorkIntent {
        raw_intent_key: "specified_spec".into(),
        objective: "Specified spec intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("original_spec", json!({}))),
    };
    let p2 = kernel
        .compile_intent(&gen.generation_id, intent_with_spec, "root", "session", 1)
        .unwrap();

    let conflict_spec = TaskSpec::new("conflicting_spec", json!({}));
    let conflict_err = kernel
        .admit_proposal(&p2.proposal_id, 0, Some(conflict_spec))
        .unwrap_err();
    assert!(matches!(conflict_err, agentype_core::Error::Conflict(_)));
}
