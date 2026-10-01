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
    ArtifactRef, BatchState, Clock, FailureClass, GenerationState, InformationFunction,
    ManualClock, PartitionId, PartitionSpec, ProposalExpirationReason, ProposalStateKind,
    RawWorkIntent, ResultId, Retention, SemanticInputSet, TaskSpec, TaskState,
};
use agentype_storage_sqlite::frontier::{
    raw_work_intent_to_json, RESULT_INTENT_ENVELOPE_KEY, RESULT_INTENT_MAP_KEY,
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

/// ACK a worker Result whose payload is `payload`, returning its ResultId.
fn create_result_with_payload(kernel: &Kernel, payload: serde_json::Value) -> ResultId {
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
            &payload,
            None,
            false,
            false,
        )
        .unwrap()
        .expect("must produce result")
}

/// Build a Result payload carrying the given intents under the reserved envelope.
fn intent_envelope(intents: &[&RawWorkIntent]) -> serde_json::Value {
    let mut intents_map = serde_json::Map::new();
    for intent in intents {
        intents_map.insert(
            intent.raw_intent_key.clone(),
            raw_work_intent_to_json(intent).expect("encode intent"),
        );
    }
    let mut envelope = serde_json::Map::new();
    envelope.insert(
        RESULT_INTENT_MAP_KEY.into(),
        serde_json::Value::Object(intents_map),
    );
    let mut root = serde_json::Map::new();
    root.insert(
        RESULT_INTENT_ENVELOPE_KEY.into(),
        serde_json::Value::Object(envelope),
    );
    serde_json::Value::Object(root)
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
    let input_set = SemanticInputSet::new().with_artifact(
        ArtifactRef::new("heap_dump.bin", "sha256:durable_heap_dump_digest").unwrap(),
    );
    let intent = RawWorkIntent {
        raw_intent_key: "inspect_heap".into(),
        objective: "Inspect heap dump for allocations".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: input_set.clone(),
        rationale: Some("Suspected memory leak in buffer pool".into()),
        suggested_task_spec: Some(TaskSpec::new("inspect_heap", json!({}))),
    };

    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent.clone(), "root_session_1", 1)
        .expect("compile intent");
    assert_eq!(proposal.state, ProposalStateKind::Pending);
    assert_eq!(proposal.information_function, InformationFunction::Expand);

    // Idempotent compilation returns the existing proposal
    let proposal_dup = kernel
        .compile_root_intent(&gen.generation_id, intent, "root_session_1", 1)
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

    // INV-A6 & P1-3: crash-retry idempotent admission returns the exact same TaskId
    let dup_admit_res = kernel
        .admit_proposal(&proposal.proposal_id, 0, None)
        .expect("idempotent re-admission on crash retry");
    assert_eq!(dup_admit_res, task_id);
    let view_after_retry = kernel
        .get_generation_view(&gen.generation_id)
        .expect("generation view");
    assert_eq!(view_after_retry.admitted_task_ids.len(), 1);

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
    // A second EXPAND, compiled through the Root surface. This lifecycle test
    // is not about ingress provenance; result-carried compilation is covered by
    // the dedicated tests below.
    let proposal_expand_2 = kernel
        .compile_root_intent(&gen.generation_id, expand_intent_2, "worker_compilation", 1)
        .expect("compile second expand");

    // Produce a real durable Result for compression provenance.
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
        .compile_root_intent(&gen.generation_id, compress_intent, "root_session_1", 1)
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
    // INV/P1-2: a CLOSED generation is a terminal, settled frontier in the view.
    assert!(
        view_closed.is_settled,
        "CLOSED generation must project is_settled = true"
    );

    // INV-A8: No task can be admitted into CLOSED generation
    let late_compress_intent = RawWorkIntent {
        raw_intent_key: "late_compress".into(),
        objective: "Late compress attempt".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("late_compress", json!({}))),
    };
    let compile_closed_fail = kernel.compile_root_intent(
        &gen.generation_id,
        late_compress_intent,
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
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
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
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .expect("compile intent");

    let mut handles = Vec::new();
    for _ in 0..8 {
        let k = Arc::clone(&kernel);
        let pid = proposal.proposal_id.clone();
        handles.push(std::thread::spawn(move || k.admit_proposal(&pid, 0, None)));
    }

    let mut successes = 0;
    let mut admitted_tid: Option<agentype_core::TaskId> = None;

    for h in handles {
        match h.join().unwrap() {
            Ok(tid) => {
                successes += 1;
                if let Some(ref prev_tid) = admitted_tid {
                    assert_eq!(prev_tid, &tid, "All threads must observe the same TaskId");
                }
                admitted_tid = Some(tid);
            }
            Err(e) => {
                panic!("Admission race must resolve idempotently, got: {e:?}");
            }
        }
    }

    // All threads resolve successfully to the exact same TaskId, and exactly one task was created
    assert_eq!(
        successes, 8,
        "All threads must idempotently receive the admitted TaskId"
    );

    let view = kernel
        .get_generation_view(&gen.generation_id)
        .expect("view");
    assert_eq!(
        view.admitted_task_ids.len(),
        1,
        "Only one task must be created in the generation"
    );
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
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
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
        .compile_root_intent(&g1.generation_id, intent1, "session_root", 1)
        .unwrap();
    let p2 = kernel
        .compile_root_intent(&g2.generation_id, intent2, "session_root", 1)
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
        .compile_root_intent(&g.generation_id, intent1, "session_1", 1)
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
        .compile_root_intent(&g.generation_id, intent2, "session_1", 1)
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
        .compile_root_intent(&g.generation_id, intent1_dup, "session_1", 1)
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
        .compile_root_intent(&gen.generation_id, intent1, "session", 1)
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
    let t1_row = kernel.task(&t1).unwrap();
    let b1 = kernel.batch(&t1_row.batch_id).unwrap();
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
        .compile_root_intent(&gen.generation_id, intent2, "session", 1)
        .unwrap();
    let t2 = kernel.admit_proposal(&p2.proposal_id, 0, None).unwrap();

    // 4. Assert T2 has its own ACTIVE batch and is immediately claimable/executable!
    let t2_row = kernel.task(&t2).unwrap();
    assert_ne!(
        t1_row.batch_id, t2_row.batch_id,
        "T1 and T2 must have distinct dedicated batches"
    );
    let b2 = kernel.batch(&t2_row.batch_id).unwrap();
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
        .compile_root_intent(&gen.generation_id, intent1, "session", 1)
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
        .compile_root_intent(&gen.generation_id, intent2, "session", 1)
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
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
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
        .compile_root_intent(&gen.generation_id, clean_intent, "session", 1)
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
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
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
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
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
        .compile_root_intent(&gen.generation_id, intent_with_spec, "session", 1)
        .unwrap();

    let conflict_spec = TaskSpec::new("conflicting_spec", json!({}));
    let conflict_err = kernel
        .admit_proposal(&p2.proposal_id, 0, Some(conflict_spec))
        .unwrap_err();
    assert!(matches!(conflict_err, agentype_core::Error::Conflict(_)));
}

#[test]
fn test_p1_1_rejected_proposal_deterministic_replay() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "replayable_key".into(),
        objective: "Test replay of rejected proposal".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task1", json!({}))),
    };

    let p1 = kernel
        .compile_root_intent(&gen.generation_id, intent.clone(), "session", 1)
        .unwrap();
    assert_eq!(p1.state, ProposalStateKind::Pending);
    assert!(p1.expiration_reason.is_none());
    assert!(p1.rejection_reason.is_none());

    let rejection_text = "Out of scope for current objective";
    kernel
        .reject_proposal(&p1.proposal_id, rejection_text)
        .unwrap();

    // Replay compilation of the EXACT same intent
    let p2 = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();

    assert_eq!(p2.proposal_id, p1.proposal_id);
    assert_eq!(p2.state, ProposalStateKind::Rejected);
    assert!(p2.expiration_reason.is_none());
    assert_eq!(p2.rejection_reason.as_deref(), Some(rejection_text));
}

#[test]
fn test_p1_1_expand_replay_after_freeze_returns_expired() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "expand_replay_freeze".into(),
        objective: "Committed EXPAND survives freeze replay".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("expand_replay", json!({}))),
    };

    let p1 = kernel
        .compile_root_intent(&gen.generation_id, intent.clone(), "session", 1)
        .unwrap();
    assert_eq!(p1.state, ProposalStateKind::Pending);

    kernel.freeze_generation(&gen.generation_id, 0).unwrap();

    let view = kernel.get_generation_view(&gen.generation_id).unwrap();
    assert!(view.expired_proposal_ids.contains(&p1.proposal_id));

    // Exact stable-identity replay observes the committed proposal even though
    // the generation frontier has advanced to FROZEN.
    let replay = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    assert_eq!(replay.proposal_id, p1.proposal_id);
    assert_eq!(replay.state, ProposalStateKind::Expired);
    assert_eq!(
        replay.expiration_reason,
        Some(ProposalExpirationReason::GenerationFrozen)
    );
}

#[test]
fn test_p1_1_admitted_replay_after_close_returns_admitted() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "admitted_replay_close".into(),
        objective: "Committed ADMITTED survives close replay".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("admitted_replay", json!({}))),
    };

    let p1 = kernel
        .compile_root_intent(&gen.generation_id, intent.clone(), "session", 1)
        .unwrap();
    let task_id = kernel.admit_proposal(&p1.proposal_id, 0, None).unwrap();

    kernel.freeze_generation(&gen.generation_id, 0).unwrap();
    kernel.cancel_task(&task_id, true).unwrap();
    kernel.close_generation(&gen.generation_id, 1).unwrap();

    let replay = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    assert_eq!(replay.proposal_id, p1.proposal_id);
    assert_eq!(replay.state, ProposalStateKind::Admitted);
    assert_eq!(replay.admitted_task_id, Some(task_id));
}

#[test]
fn test_p1_1_rejected_replay_after_close_returns_rejected() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "rejected_replay_close".into(),
        objective: "Committed REJECTED survives close replay".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("rejected_replay", json!({}))),
    };

    let p1 = kernel
        .compile_root_intent(&gen.generation_id, intent.clone(), "session", 1)
        .unwrap();
    kernel
        .reject_proposal(&p1.proposal_id, "out of scope")
        .unwrap();

    kernel.freeze_generation(&gen.generation_id, 0).unwrap();
    kernel.close_generation(&gen.generation_id, 1).unwrap();

    let replay = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    assert_eq!(replay.proposal_id, p1.proposal_id);
    assert_eq!(replay.state, ProposalStateKind::Rejected);
    assert_eq!(replay.rejection_reason.as_deref(), Some("out of scope"));
}

#[test]
fn test_p1_1_compress_replay_after_close_returns_expired() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // COMPRESS is admissible while FROZEN, so it survives freeze as PENDING.
    kernel.freeze_generation(&gen.generation_id, 0).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "compress_replay_close".into(),
        objective: "Committed COMPRESS survives close replay".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("compress_replay", json!({}))),
    };

    let p1 = kernel
        .compile_root_intent(&gen.generation_id, intent.clone(), "session", 1)
        .unwrap();
    assert_eq!(p1.state, ProposalStateKind::Pending);

    kernel.close_generation(&gen.generation_id, 1).unwrap();

    let replay = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    assert_eq!(replay.proposal_id, p1.proposal_id);
    assert_eq!(replay.state, ProposalStateKind::Expired);
    assert_eq!(
        replay.expiration_reason,
        Some(ProposalExpirationReason::GenerationClosed)
    );
}

#[test]
fn test_p1_1_canonical_task_spec_order_does_not_conflict_at_admission() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // Compiled proposal carries the sorted/canonical spec.
    let mut compiled_spec = TaskSpec::new("canonical_order", json!({}));
    compiled_spec.affinity_tags = vec!["a".into(), "z".into()];
    compiled_spec.retry_policy.retry_classes =
        vec![FailureClass::ExecutionLost, FailureClass::Timeout];
    let intent = RawWorkIntent {
        raw_intent_key: "canonical_order".into(),
        objective: "Canonical order must not conflict".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(compiled_spec),
    };
    let p = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();

    // Root expresses the SAME canonical spec with different vector order.
    let mut override_spec = TaskSpec::new("canonical_order", json!({}));
    override_spec.affinity_tags = vec!["z".into(), "a".into()];
    override_spec.retry_policy.retry_classes =
        vec![FailureClass::Timeout, FailureClass::ExecutionLost];

    let task_id = kernel
        .admit_proposal(&p.proposal_id, 0, Some(override_spec))
        .expect("canonically-equivalent override must admit");
    let task_row = kernel.task(&task_id).unwrap();
    assert_eq!(task_row.name, "canonical_order");
}

#[test]
fn test_p1_1_canonical_task_spec_retry_after_commit_returns_same_task_id() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // Proposal intentionally has no compiled spec; Root supplies the override.
    let intent = RawWorkIntent {
        raw_intent_key: "canonical_retry".into(),
        objective: "Retry with original override".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: None,
    };
    let p = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();

    let original_override = || {
        let mut spec = TaskSpec::new("canonical_retry", json!({}));
        spec.affinity_tags = vec!["z".into(), "a".into()];
        spec.retry_policy.retry_classes = vec![FailureClass::Timeout, FailureClass::ExecutionLost];
        spec
    };

    let first = kernel
        .admit_proposal(&p.proposal_id, 0, Some(original_override()))
        .expect("first admission");
    // Crash-replay the exact original (unsorted) override.
    let retry = kernel
        .admit_proposal(&p.proposal_id, 0, Some(original_override()))
        .expect("canonically-equivalent retry must return the same TaskId");
    assert_eq!(first, retry);
}

#[test]
fn test_p1_2_fingerprint_covers_all_task_spec_fields() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let base_spec = TaskSpec::new("task_base", json!({"x": 1}));
    let base_intent = RawWorkIntent {
        raw_intent_key: "spec_diff_key".into(),
        objective: "Base intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(base_spec.clone()),
    };

    let _p = kernel
        .compile_root_intent(&gen.generation_id, base_intent, "session", 1)
        .unwrap();

    // 1. Changing base_backoff_seconds produces different fingerprint & causes Conflict on compile
    let mut spec_backoff = base_spec.clone();
    spec_backoff.retry_policy.base_backoff_seconds = 42.0;
    let intent_backoff = RawWorkIntent {
        raw_intent_key: "spec_diff_key".into(),
        objective: "Base intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec_backoff),
    };
    let err = kernel
        .compile_root_intent(&gen.generation_id, intent_backoff, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));

    // 2. Changing max_backoff_seconds causes Conflict
    let mut spec_max_backoff = base_spec.clone();
    spec_max_backoff.retry_policy.max_backoff_seconds = 120.0;
    let intent_max_backoff = RawWorkIntent {
        raw_intent_key: "spec_diff_key".into(),
        objective: "Base intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec_max_backoff),
    };
    let err = kernel
        .compile_root_intent(&gen.generation_id, intent_max_backoff, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));

    // 3. Changing supersedes_task_id causes Conflict
    let mut spec_supersedes = base_spec.clone();
    spec_supersedes.supersedes_task_id = Some(agentype_core::TaskId::new());
    let intent_supersedes = RawWorkIntent {
        raw_intent_key: "spec_diff_key".into(),
        objective: "Base intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec_supersedes),
    };
    let err = kernel
        .compile_root_intent(&gen.generation_id, intent_supersedes, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));

    // 4. Changing task_id causes Conflict
    let mut spec_tid = base_spec.clone();
    spec_tid.task_id = Some(agentype_core::TaskId::new());
    let intent_tid = RawWorkIntent {
        raw_intent_key: "spec_diff_key".into(),
        objective: "Base intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec_tid),
    };
    let err = kernel
        .compile_root_intent(&gen.generation_id, intent_tid, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));

    // 5. Changing priority causes Conflict
    let mut spec_pri = base_spec;
    spec_pri.priority = 99;
    let intent_pri = RawWorkIntent {
        raw_intent_key: "spec_diff_key".into(),
        objective: "Base intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec_pri),
    };
    let err = kernel
        .compile_root_intent(&gen.generation_id, intent_pri, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));
}

#[test]
fn test_p1_4_durable_task_spec_fail_closed() {
    use agentype_storage_sqlite::frontier::task_spec_from_json;

    let valid_spec = TaskSpec::new("task1", json!({"k": "v"}));
    let valid_json = valid_spec.canonical_json().expect("canonical_json");
    assert!(task_spec_from_json(&valid_json).is_ok());

    // 1. Missing name
    let mut bad = valid_json.clone();
    bad.as_object_mut().unwrap().remove("name");
    assert!(task_spec_from_json(&bad).is_err());

    // 2. Missing partition
    let mut bad = valid_json.clone();
    bad.as_object_mut().unwrap().remove("partition");
    assert!(task_spec_from_json(&bad).is_err());

    // 3. Empty partition
    let mut bad = valid_json.clone();
    bad["partition"] = json!("   ");
    assert!(task_spec_from_json(&bad).is_err());

    // 4. Unknown workspace_mode MUST NOT default to Write, must fail closed
    let mut bad = valid_json.clone();
    bad["workspace_mode"] = json!("SUPER_PRIVILEGED_WRITE");
    assert!(task_spec_from_json(&bad).is_err());

    // 5. Unknown continuity preference must fail closed
    let mut bad = valid_json.clone();
    bad["continuity"] = json!("MAGIC_CONTINUITY");
    assert!(task_spec_from_json(&bad).is_err());

    // 6. Unknown retry_class must fail closed
    let mut bad = valid_json.clone();
    bad["retry_classes"] = json!(["UNKNOWN_FAILURE_CLASS"]);
    assert!(task_spec_from_json(&bad).is_err());

    // 7. Negative or non-finite base_backoff_seconds; zero stays legal for M5 parity
    let mut bad = valid_json.clone();
    bad["base_backoff_seconds"] = json!(-5.0);
    assert!(task_spec_from_json(&bad).is_err());
    let mut zero_base = valid_json.clone();
    zero_base["base_backoff_seconds"] = json!(0.0);
    zero_base["max_backoff_seconds"] = json!(0.0);
    assert!(task_spec_from_json(&zero_base).is_ok());

    // 8. max_backoff_seconds < base_backoff_seconds
    let mut bad = valid_json.clone();
    bad["base_backoff_seconds"] = json!(10.0);
    bad["max_backoff_seconds"] = json!(5.0);
    assert!(task_spec_from_json(&bad).is_err());

    // 9. max_attempts == 0
    let mut bad = valid_json.clone();
    bad["max_attempts"] = json!(0);
    assert!(task_spec_from_json(&bad).is_err());
}

#[test]
fn test_p2_1_generation_view_order_determinism() {
    let clock = Arc::new(ManualClock::new(1_000.0));
    let kernel = Kernel::open_memory(clock.clone(), 30.0, CONTINUITY_MAX_BYTES)
        .expect("open in-memory kernel");

    for name in ["default", "general"] {
        let part = PartitionSpec {
            name: PartitionId::new(name),
            desired_capacity: 5,
            retention: Retention::Resident,
            execution_target: "local".into(),
            execution_profile: "default".into(),
            tags: vec![],
        };
        kernel.upsert_partition(&part).unwrap();
    }

    let gen = kernel.create_generation(json!({})).unwrap();

    // Compile multiple proposals with distinct timestamps by advancing clock
    let mut proposal_ids = Vec::new();
    for i in 0..5 {
        clock.advance(1.0);
        let intent = RawWorkIntent {
            raw_intent_key: format!("prop_{i}"),
            objective: format!("Objective {i}"),
            information_function: InformationFunction::Expand,
            semantic_input_set: SemanticInputSet::new(),
            rationale: None,
            suggested_task_spec: Some(TaskSpec::new(format!("task_{i}"), json!({}))),
        };
        let p = kernel
            .compile_root_intent(&gen.generation_id, intent, "session", 1)
            .unwrap();
        proposal_ids.push(p.proposal_id);
    }

    // Admit the proposals in a distinct sequence: 2, 0, 4
    let mut admitted_tasks = Vec::new();
    for idx in [2, 0, 4] {
        let rev = kernel
            .get_generation_view(&gen.generation_id)
            .unwrap()
            .generation
            .revision;
        let tid = kernel
            .admit_proposal(&proposal_ids[idx], rev, None)
            .unwrap();
        admitted_tasks.push(tid);
    }

    let view = kernel.get_generation_view(&gen.generation_id).unwrap();

    // Admitted tasks must be strictly ordered by admission_seq ASC
    assert_eq!(view.admitted_task_ids, admitted_tasks);

    // Pending proposals must be strictly ordered by created_at ASC, proposal_id ASC
    let remaining_pending: Vec<_> = view.pending_proposal_ids;
    assert_eq!(remaining_pending.len(), 2);
    // Proposal 1 and Proposal 3 were created in chronological order (created_at=1002.0 then 1004.0)
    assert_eq!(remaining_pending[0], proposal_ids[1]);
    assert_eq!(remaining_pending[1], proposal_ids[3]);
}

#[test]
fn test_p2_3_generation_task_bindings_proposal_unique_constraint() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(agentype_storage_sqlite::SCHEMA_SQL)
        .unwrap();

    let now = 1000.0;
    // Insert generation
    conn.execute(
        "INSERT INTO generations(generation_id, state, revision, admission_seq, seed_payload_json, created_at)
         VALUES('gen_1', 'OPEN', 0, 0, '{}', ?1)",
        rusqlite::params![now],
    ).unwrap();

    // Insert pool_partitions for tasks FK
    conn.execute(
        "INSERT INTO pool_partitions(name, desired_capacity, retention, execution_target, execution_profile, created_at, updated_at)
         VALUES('default', 1, 'resident', 'local', 'default', ?1, ?1)",
        rusqlite::params![now],
    ).unwrap();

    // Insert batches for tasks
    conn.execute(
        "INSERT INTO batches(id, state, metadata_json, created_at, updated_at)
         VALUES('batch_1', 'ACTIVE', '{}', ?1, ?1)",
        rusqlite::params![now],
    )
    .unwrap();

    // Insert task 1 and task 2
    for tid in ["task_1", "task_2"] {
        conn.execute(
            "INSERT INTO tasks(id, batch_id, name, payload_json, acceptance_json, partition_name,
                               workstream_id, continuity, affinity_tags_json, workspace_mode, required, priority, state,
                               max_attempts, retry_classes_json, base_backoff_seconds, max_backoff_seconds, created_at, updated_at)
             VALUES(?1, 'batch_1', 't', '{}', '{}', 'default', NULL, 'none', '[]', 'write', 1, 0, 'QUEUED', 1, '[]', 1.0, 60.0, ?2, ?2)",
            rusqlite::params![tid, now],
        ).unwrap();
    }

    // Insert proposal
    conn.execute(
        "INSERT INTO compiled_work_proposals(proposal_id, generation_id, source_kind, source_ref, raw_intent_key,
                                             intent_fingerprint, objective, rationale, information_function, compiler_version, state, created_at, updated_at)
         VALUES('prop_1', 'gen_1', 'root', 'sess', 'k', 'fp', 'test objective', NULL, 'EXPAND', 1, 'ADMITTED', ?1, ?1)",
        rusqlite::params![now],
    ).unwrap();

    // First binding with prop_1 succeeds
    conn.execute(
        "INSERT INTO generation_task_bindings(generation_id, task_id, proposal_id, information_function,
                                              admission_seq, admitted_task_spec_json, created_at)
         VALUES('gen_1', 'task_1', 'prop_1', 'EXPAND', 1, '{}', ?1)",
        rusqlite::params![now],
    ).unwrap();

    // Second binding with SAME proposal_id ('prop_1') MUST fail the UNIQUE constraint
    let err = conn.execute(
        "INSERT INTO generation_task_bindings(generation_id, task_id, proposal_id, information_function,
                                              admission_seq, admitted_task_spec_json, created_at)
         VALUES('gen_1', 'task_2', 'prop_1', 'EXPAND', 2, '{}', ?1)",
        rusqlite::params![now],
    ).unwrap_err();

    let err_str = err.to_string();
    assert!(err_str.contains("UNIQUE constraint failed: generation_task_bindings.proposal_id"));
}

#[test]
fn test_p2_4_frozen_generation_rejects_expand_compilation() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // Freeze generation
    kernel.freeze_generation(&gen.generation_id, 0).unwrap();

    // 1. Attempt to compile EXPAND intent on FROZEN generation -> Rejected with InvalidTransition
    let expand_intent = RawWorkIntent {
        raw_intent_key: "expand_after_freeze".into(),
        objective: "Should fail".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task_exp", json!({}))),
    };

    let err = kernel
        .compile_root_intent(&gen.generation_id, expand_intent, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvalidTransition(_)));

    // 2. Attempt to compile COMPRESS_POSITIVE intent on FROZEN generation -> Succeeds
    let compress_intent = RawWorkIntent {
        raw_intent_key: "compress_after_freeze".into(),
        objective: "Should succeed".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("task_comp", json!({}))),
    };

    let prop = kernel
        .compile_root_intent(&gen.generation_id, compress_intent, "session", 1)
        .unwrap();
    assert_eq!(
        prop.information_function,
        InformationFunction::CompressPositive
    );
    assert_eq!(prop.state, ProposalStateKind::Pending);
}

#[test]
fn test_p1_1_proposal_retains_objective_and_reopen_read() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "audit_leak".into(),
        objective: "Audit cache leak on buffer pool".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: Some("Found 50MB spike during sustained writes".into()),
        suggested_task_spec: None,
    };

    let proposal = kernel
        .compile_root_intent(&gen.generation_id, intent, "session_1", 1)
        .unwrap();

    assert_eq!(proposal.objective, "Audit cache leak on buffer pool");
    assert_eq!(
        proposal.rationale.as_deref(),
        Some("Found 50MB spike during sustained writes")
    );
    assert_eq!(proposal.normalized_task_spec, None);

    // Root / runtime restarts: read proposal back by proposal_id
    let reloaded = kernel.get_proposal(&proposal.proposal_id).unwrap();
    assert_eq!(reloaded.proposal_id, proposal.proposal_id);
    assert_eq!(reloaded.objective, "Audit cache leak on buffer pool");
    assert_eq!(
        reloaded.rationale.as_deref(),
        Some("Found 50MB spike during sustained writes")
    );
    assert_eq!(reloaded.normalized_task_spec, None);

    // Root reconstructs the missing task spec and admits
    let override_spec = TaskSpec::new("audit_cache_task", json!({"target": "buffer_pool"}));
    let task_id = kernel
        .admit_proposal(&proposal.proposal_id, 0, Some(override_spec))
        .unwrap();

    let task_row = kernel.task(&task_id).unwrap();
    assert_eq!(task_row.name, "audit_cache_task");
}

#[test]
fn test_p1_2_dedicated_batch_never_aliases_existing_active_batch() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let custom_task_id = "target_task_99";
    let legacy_batch_name = format!("batch_{custom_task_id}");
    let (legacy_batch, _) = kernel
        .submit_batch(&[TaskSpec::new("legacy_task", json!({}))])
        .unwrap();

    // Now admit a proposal with task_id set to custom_task_id
    let mut spec = TaskSpec::new("spec_with_custom_id", json!({}));
    spec.task_id = Some(agentype_core::TaskId::from_string(custom_task_id));

    let intent = RawWorkIntent {
        raw_intent_key: "custom_id_intent".into(),
        objective: "Test dedicated batch allocation".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec),
    };

    let prop = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    let admitted_task_id = kernel.admit_proposal(&prop.proposal_id, 0, None).unwrap();
    assert_eq!(admitted_task_id.as_str(), custom_task_id);

    let task_row = kernel.task(&admitted_task_id).unwrap();
    // The admitted task's batch MUST NOT be the legacy batch name or alias any existing batch!
    assert_ne!(task_row.batch_id.as_str(), legacy_batch_name.as_str());
    assert_ne!(task_row.batch_id, legacy_batch);

    let batch_row = kernel.batch(&task_row.batch_id).unwrap();
    assert_eq!(batch_row.state, BatchState::Active);
}

#[test]
fn test_p1_2_dedicated_batch_never_strands_task_in_completed_batch() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    // Pre-populate a COMPLETED batch with a task
    let (completed_batch, ids) = kernel
        .submit_batch(&[TaskSpec::new("completed_worker", json!({}))])
        .unwrap();
    let _tid = ids.values().next().unwrap();
    let claim = kernel.claim_next_available().unwrap().unwrap();
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
            &json!({"ok": true}),
            None,
            false,
            false,
        )
        .unwrap();
    let b_status = kernel.batch(&completed_batch).unwrap();
    assert_eq!(b_status.state, BatchState::Completed);

    // Admit new M6 proposal
    let intent = RawWorkIntent {
        raw_intent_key: "task_after_completed_batch".into(),
        objective: "Must not strand in completed batch".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("fresh_task", json!({}))),
    };
    let prop = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();
    let tid = kernel.admit_proposal(&prop.proposal_id, 0, None).unwrap();

    let task_row = kernel.task(&tid).unwrap();
    assert_ne!(task_row.batch_id, completed_batch);

    let batch_row = kernel.batch(&task_row.batch_id).unwrap();
    assert_eq!(batch_row.state, BatchState::Active);

    // Task is immediately claimable because its batch is ACTIVE!
    let claim = kernel
        .claim_next_available()
        .unwrap()
        .expect("claim fresh task");
    assert_eq!(claim.task_id, tid);
}

#[test]
fn test_p1_3_admission_retry_returns_same_task_id() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "crash_retry_intent".into(),
        objective: "Test admission idempotent retry".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("retry_worker", json!({}))),
    };

    let prop = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .unwrap();

    let t1 = kernel.admit_proposal(&prop.proposal_id, 0, None).unwrap();

    // Retry call on the exact same proposal returns Ok(t1)
    let t2 = kernel
        .admit_proposal(&prop.proposal_id, 0, None)
        .expect("retry must succeed idempotently");
    assert_eq!(t1, t2);

    let view = kernel.get_generation_view(&gen.generation_id).unwrap();
    assert_eq!(view.admitted_task_ids, vec![t1.clone()]);

    // Retry call with conflicting override spec returns Conflict
    let conflict_spec = TaskSpec::new("conflicting_name", json!({}));
    let err = kernel
        .admit_proposal(&prop.proposal_id, 0, Some(conflict_spec))
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::Conflict(_)));
}

#[test]
fn test_p1_4_artifact_ref_requires_immutable_digest() {
    // Empty locator rejected
    assert!(ArtifactRef::new("", "sha256:1234").is_err());
    assert!(ArtifactRef::new("   ", "sha256:1234").is_err());

    // Empty digest rejected (provenance requires immutable content digest)
    assert!(ArtifactRef::new("file.bin", "").is_err());
    assert!(ArtifactRef::new("file.bin", "   ").is_err());

    // Valid locator and digest
    let art = ArtifactRef::new("s3://bucket/heap.bin", "sha256:abcdef").unwrap();
    assert_eq!(art.locator, "s3://bucket/heap.bin");
    assert_eq!(art.digest, "sha256:abcdef");
}

#[test]
fn test_p1_4_seed_ref_validated_against_generation_seed() {
    let kernel = test_kernel();
    let gen = kernel
        .create_generation(json!({
            "approved_corpus": "s3://corpus/v1",
            "benchmark_id": 42
        }))
        .unwrap();

    // Intent referencing an approved seed in seed_payload -> succeeds
    let valid_set = SemanticInputSet::new().with_seed("approved_corpus");
    let valid_intent = RawWorkIntent {
        raw_intent_key: "valid_seed_intent".into(),
        objective: "Process approved corpus".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: valid_set,
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("worker", json!({}))),
    };
    let prop = kernel
        .compile_root_intent(&gen.generation_id, valid_intent, "session", 1)
        .unwrap();
    assert_eq!(prop.raw_intent_key, "valid_seed_intent");

    // Intent referencing an unknown seed -> rejected with NotFound
    let invalid_set = SemanticInputSet::new().with_seed("unknown_external_data");
    let invalid_intent = RawWorkIntent {
        raw_intent_key: "invalid_seed_intent".into(),
        objective: "Process unapproved corpus".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: invalid_set,
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("worker", json!({}))),
    };
    let err = kernel
        .compile_root_intent(&gen.generation_id, invalid_intent, "session", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::NotFound(_)));
}

#[test]
fn test_p2_1_canonical_json_rejects_nan_and_inf_backoff() {
    let mut spec = TaskSpec::new("test_backoff", json!({}));

    // 1. base_backoff is NaN
    spec.retry_policy.base_backoff_seconds = f64::NAN;
    assert!(matches!(
        spec.canonical_json(),
        Err(agentype_core::Error::InvariantViolation(_))
    ));

    // 2. base_backoff is INFINITY
    spec.retry_policy.base_backoff_seconds = f64::INFINITY;
    assert!(matches!(
        spec.canonical_json(),
        Err(agentype_core::Error::InvariantViolation(_))
    ));

    // 3. base_backoff 0.0 stays legal for M5 parity (max >= base holds)
    spec.retry_policy.base_backoff_seconds = 0.0;
    assert!(spec.canonical_json().is_ok());

    // 4. base_backoff is negative
    spec.retry_policy.base_backoff_seconds = -1.0;
    assert!(matches!(
        spec.canonical_json(),
        Err(agentype_core::Error::InvariantViolation(_))
    ));

    // 5. max_backoff < base_backoff
    spec.retry_policy.base_backoff_seconds = 10.0;
    spec.retry_policy.max_backoff_seconds = 5.0;
    assert!(matches!(
        spec.canonical_json(),
        Err(agentype_core::Error::InvariantViolation(_))
    ));

    // 6. max_attempts == 0
    spec.retry_policy.base_backoff_seconds = 1.0;
    spec.retry_policy.max_backoff_seconds = 60.0;
    spec.retry_policy.max_attempts = 0;
    assert!(matches!(
        spec.canonical_json(),
        Err(agentype_core::Error::InvariantViolation(_))
    ));

    // 7. Invalid spec bubbles through RawWorkIntent::fingerprint
    spec.retry_policy.base_backoff_seconds = f64::NAN;
    spec.retry_policy.max_attempts = 1;
    let intent = RawWorkIntent {
        raw_intent_key: "key".into(),
        objective: "obj".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec),
    };
    assert!(matches!(
        intent.fingerprint(),
        Err(agentype_core::Error::InvariantViolation(_))
    ));
}

#[test]
fn test_p2_1_zero_backoff_task_spec_roundtrips_and_can_be_admitted() {
    use agentype_storage_sqlite::frontier::task_spec_from_json;

    let mut spec = TaskSpec::new("zero_backoff", json!({}));
    spec.retry_policy.base_backoff_seconds = 0.0;
    spec.retry_policy.max_backoff_seconds = 0.0;

    // Zero backoff is legal M5 Task semantics and must survive canonical roundtrip.
    let json = spec.canonical_json().expect("zero backoff is legal");
    let parsed = task_spec_from_json(&json).expect("zero backoff parses");
    assert_eq!(parsed.retry_policy.base_backoff_seconds, 0.0);
    assert_eq!(parsed.retry_policy.max_backoff_seconds, 0.0);
    assert!(parsed.equivalent(&spec).unwrap());

    // And it is admissible through the M6-A semantic path.
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "zero_backoff".into(),
        objective: "Zero backoff must remain legal".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(spec),
    };
    let p = kernel
        .compile_root_intent(&gen.generation_id, intent, "session", 1)
        .expect("zero backoff compiles");
    let task_id = kernel
        .admit_proposal(&p.proposal_id, 0, None)
        .expect("zero backoff admits");
    let task_row = kernel.task(&task_id).unwrap();
    assert_eq!(task_row.name, "zero_backoff");
}

#[test]
fn test_result_backed_intent_source_must_exist() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let missing = ResultId::new();
    let err = kernel
        .compile_result_intent(&gen.generation_id, &missing, "any_key", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::NotFound(_)));
}

#[test]
fn test_result_carried_intent_is_loaded_from_result_payload() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "carried_probe".into(),
        objective: "Audit session ownership race".into(),
        information_function: InformationFunction::CompressNegative,
        semantic_input_set: SemanticInputSet::new(),
        rationale: Some("Observed inconsistent ownership".into()),
        suggested_task_spec: Some(TaskSpec::new("carried_task", json!({"x": 1}))),
    };
    let result_id = create_result_with_payload(&kernel, intent_envelope(&[&intent]));

    let prop = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "carried_probe", 1)
        .unwrap();

    assert_eq!(prop.raw_intent_key, "carried_probe");
    assert_eq!(prop.objective, "Audit session ownership race");
    assert_eq!(
        prop.information_function,
        InformationFunction::CompressNegative
    );
    assert_eq!(
        prop.rationale.as_deref(),
        Some("Observed inconsistent ownership")
    );
    assert_eq!(prop.source_kind, "result");
    assert_eq!(prop.source_ref, result_id.as_str());
    assert_eq!(prop.state, ProposalStateKind::Pending);
    assert_eq!(
        prop.normalized_task_spec.as_ref().map(|s| s.name.as_str()),
        Some("carried_task")
    );
}

#[test]
fn test_result_without_requested_intent_fails_closed() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "present_key".into(),
        objective: "Present intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("present", json!({}))),
    };

    // A Result with an ordinary payload carries no intents.
    let plain = create_result_with_payload(&kernel, json!({"produced": true}));
    let err = kernel
        .compile_result_intent(&gen.generation_id, &plain, "anything", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::NotFound(_)));

    // A Result carrying some intent cannot satisfy a different key.
    let carried = create_result_with_payload(&kernel, intent_envelope(&[&intent]));
    let err = kernel
        .compile_result_intent(&gen.generation_id, &carried, "wrong_key", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::NotFound(_)));
}

#[test]
fn test_malformed_result_intent_envelope_fails_closed() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "k".into(),
        objective: "Malformed envelope probe".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("probe", json!({}))),
    };

    // 1. _agentype present but not an object
    let r1 = create_result_with_payload(&kernel, json!({"_agentype": "nope"}));
    let err = kernel
        .compile_result_intent(&gen.generation_id, &r1, "k", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvariantViolation(_)));

    // 2. raw_work_intents present but not an object
    let r2 = create_result_with_payload(&kernel, json!({"_agentype": {"raw_work_intents": []}}));
    let err = kernel
        .compile_result_intent(&gen.generation_id, &r2, "k", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvariantViolation(_)));

    // 3. entry carries an unknown field
    let mut entry = raw_work_intent_to_json(&intent).unwrap();
    entry
        .as_object_mut()
        .unwrap()
        .insert("surprise".into(), json!(1));
    let mut map = serde_json::Map::new();
    map.insert("k".into(), entry);
    let r3 = create_result_with_payload(
        &kernel,
        json!({"_agentype": {"raw_work_intents": serde_json::Value::Object(map)}}),
    );
    let err = kernel
        .compile_result_intent(&gen.generation_id, &r3, "k", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvariantViolation(_)));
}

#[test]
fn test_result_carried_intent_replay_is_idempotent() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "result_replay".into(),
        objective: "Replay a result-backed intent".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("result_replay", json!({}))),
    };
    let result_id = create_result_with_payload(&kernel, intent_envelope(&[&intent]));

    let p1 = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "result_replay", 1)
        .unwrap();
    let p2 = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "result_replay", 1)
        .unwrap();

    assert_eq!(p1.proposal_id, p2.proposal_id);
    assert_eq!(p1.source_kind, "result");
    assert_eq!(p1.source_ref, result_id.as_str());
}

#[test]
fn test_result_carried_intent_replay_after_close_returns_existing() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "carried_close_replay".into(),
        objective: "Carried intent survives close replay".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("carried_close_replay", json!({}))),
    };
    let result_id = create_result_with_payload(&kernel, intent_envelope(&[&intent]));

    let p1 = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "carried_close_replay", 1)
        .unwrap();
    let task_id = kernel.admit_proposal(&p1.proposal_id, 0, None).unwrap();
    kernel.freeze_generation(&gen.generation_id, 0).unwrap();
    kernel.cancel_task(&task_id, true).unwrap();
    kernel.close_generation(&gen.generation_id, 1).unwrap();

    // Replay observes the durable commitment, not the now-CLOSED frontier.
    let replay = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "carried_close_replay", 1)
        .unwrap();
    assert_eq!(replay.proposal_id, p1.proposal_id);
    assert_eq!(replay.state, ProposalStateKind::Admitted);
}

#[test]
fn test_result_carried_expand_after_generation_freeze_is_not_compiled() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "expand_after_freeze".into(),
        objective: "Expand after freeze".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("expand_after_freeze", json!({}))),
    };
    let result_id = create_result_with_payload(&kernel, intent_envelope(&[&intent]));

    kernel.freeze_generation(&gen.generation_id, 0).unwrap();

    let err = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "expand_after_freeze", 1)
        .unwrap_err();
    assert!(matches!(err, agentype_core::Error::InvalidTransition(_)));
}

#[test]
fn test_result_carried_intent_survives_file_backed_restart_before_compile() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let _ = std::fs::create_dir_all(&base);
    let path = base.join(format!(
        "frontier_result_intent_restart_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let clock = Arc::new(ManualClock::new(1_000.0)) as Arc<dyn Clock>;
    let kernel = Kernel::open(&path, clock.clone(), 30.0, CONTINUITY_MAX_BYTES)
        .expect("open file-backed kernel");
    for name in ["default", "general"] {
        kernel
            .upsert_partition(&PartitionSpec {
                name: PartitionId::new(name),
                desired_capacity: 5,
                retention: Retention::Resident,
                execution_target: "local".into(),
                execution_profile: "default".into(),
                tags: vec![],
            })
            .expect("upsert partition");
    }
    kernel.reconcile_pool().unwrap();

    let gen = kernel.create_generation(json!({})).unwrap();
    let intent = RawWorkIntent {
        raw_intent_key: "restart_intent".into(),
        objective: "Survive a restart before compilation".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("restart_intent", json!({}))),
    };
    let result_id = create_result_with_payload(&kernel, intent_envelope(&[&intent]));

    // Simulate a process crash after the Result committed but before compile.
    drop(kernel);

    let kernel = Kernel::open(&path, clock.clone(), 30.0, CONTINUITY_MAX_BYTES)
        .expect("reopen file-backed kernel");
    let prop = kernel
        .compile_result_intent(&gen.generation_id, &result_id, "restart_intent", 1)
        .expect("result-carried intent is recoverable after restart");
    assert_eq!(prop.raw_intent_key, "restart_intent");
    assert_eq!(prop.source_ref, result_id.as_str());

    // The proposal itself is durable across a second reopen.
    drop(kernel);
    let kernel = Kernel::open(&path, clock, 30.0, CONTINUITY_MAX_BYTES).unwrap();
    let reloaded = kernel.get_proposal(&prop.proposal_id).unwrap();
    assert_eq!(reloaded.proposal_id, prop.proposal_id);
    assert_eq!(reloaded.objective, "Survive a restart before compilation");

    drop(kernel);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_root_intent_uses_root_source_identity() {
    let kernel = test_kernel();
    let gen = kernel.create_generation(json!({})).unwrap();

    let intent = RawWorkIntent {
        raw_intent_key: "root_identity".into(),
        objective: "Root source identity".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("root_identity", json!({}))),
    };

    let p = kernel
        .compile_root_intent(&gen.generation_id, intent, "cli_cmd_7", 1)
        .unwrap();
    assert_eq!(p.source_kind, "root");
    assert_eq!(p.source_ref, "cli_cmd_7");
}
