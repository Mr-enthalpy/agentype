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

use agentype_core::{
    Clock, GenerationState, InformationFunction, ManualClock, PartitionId, PartitionSpec,
    ProposalStateKind, RawWorkIntent, ResultId, Retention, SemanticInputSet, TaskState,
};
use agentype_storage_sqlite::Kernel;
use serde_json::json;
use std::sync::Arc;

const CONTINUITY_MAX_BYTES: usize = 16_384;

fn test_kernel() -> Kernel {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
    let kernel =
        Kernel::open_memory(clock, 30.0, CONTINUITY_MAX_BYTES).expect("open in-memory kernel");

    // Upsert default partition
    let part = PartitionSpec {
        name: PartitionId::new("default"),
        desired_capacity: 5,
        retention: Retention::Resident,
        execution_target: "local".into(),
        execution_profile: "default".into(),
        tags: vec![],
    };
    kernel.upsert_partition(&part).expect("upsert partition");
    kernel
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
        suggested_task_spec: None,
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
        suggested_task_spec: None,
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

    // Compile a COMPRESS_POSITIVE intent
    let compress_intent = RawWorkIntent {
        raw_intent_key: "summarize_findings".into(),
        objective: "Summarize leak investigation findings".into(),
        information_function: InformationFunction::CompressPositive,
        semantic_input_set: SemanticInputSet::new().with_result(ResultId::new()),
        rationale: None,
        suggested_task_spec: None,
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
        suggested_task_spec: None,
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
        suggested_task_spec: None,
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
        suggested_task_spec: None,
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
        suggested_task_spec: None,
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
