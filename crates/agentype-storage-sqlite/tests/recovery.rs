//! M4 authority recovery and physical-history refinement (spec 16 §A / 14).
//! Crash-leftover history is constructed below the API boundary with storage
//! fixtures; every operation under test goes through the Kernel public API.

mod common;

use agentype_core::*;
use agentype_storage_sqlite::Kernel;
use common::*;
use rusqlite::Connection;
use serde_json::json;
use std::sync::Arc;

#[test]
fn restart_recovery_prevents_blind_duplicate_execution() {
    let db = FixtureDb::new("restart");
    let env = file_env(&db);
    let task_id;
    let attempt_id;
    let original_expiry;
    {
        let k = &env.k;
        let (_b, ids) = k.submit_batch(&[retryable_write("restart")]).unwrap();
        task_id = ids["restart"].clone();
        let claim = k.claim_next_available().unwrap().unwrap();
        attempt_id = claim.attempt_id.clone();
        let _ = k
            .create_execution(&claim, FrozenExecutionSafety::UNISOLATED)
            .unwrap();
        original_expiry = k.lease_for_attempt(&attempt_id).unwrap().expires_at;
    }
    env.clock.advance(20.0);
    assert!(env.clock.now() > original_expiry);
    {
        let k = reopen(&env, &db);
        k.recover_authority().unwrap();
        let task = k.task(&task_id).unwrap();
        assert_eq!(task.state, TaskState::Suspended);
        assert!(task.current_attempt_id.is_none());
        assert!(k.claim_next_available().unwrap().is_none());
        let attempt = k.attempt(&attempt_id).unwrap();
        assert_eq!(attempt.state, AttemptState::Expired);
    }
}

#[test]
fn unknown_execution_can_reconcile_to_running() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("amb2")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    let launch = k
        .create_execution(&claim, FrozenExecutionSafety::UNISOLATED)
        .unwrap();
    let execution_id = launch.execution_id().clone();
    k.record_physical_outcome(
        &execution_id,
        ExecutionState::Unknown,
        None,
        None,
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        k.execution(&execution_id).unwrap().state,
        ExecutionState::Unknown
    );
    // UNKNOWN -> RUNNING re-establishes supervision without new proof bits.
    k.confirm_running_and_renew(
        &claim.attempt_id,
        claim.lease_epoch,
        &execution_id,
        &json!({"found": true}),
    )
    .unwrap();
    let row = k.execution(&execution_id).unwrap();
    assert_eq!(row.state, ExecutionState::Running);
    assert!(!row.terminal_confirmed);
}

#[test]
fn stale_lost_can_refine_to_terminated() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, retryable_read("lost"), false);
    k.nack(
        &claim.attempt_id,
        claim.lease_epoch,
        FailureClass::ExecutionLost,
        Some(&execution_id),
        false,
        false,
        false,
    )
    .unwrap();
    k.record_physical_outcome(
        &execution_id,
        ExecutionState::Lost,
        None,
        None,
        Some(FailureClass::ExecutionLost),
        false,
        false,
    )
    .unwrap();
    k.record_physical_outcome(
        &execution_id,
        ExecutionState::Terminated,
        None,
        None,
        None,
        true,
        true,
    )
    .unwrap();
    assert_eq!(
        k.execution(&execution_id).unwrap().state,
        ExecutionState::Terminated
    );
    assert_ne!(k.task(&task_id).unwrap().state, TaskState::Completed);
}

// --------------------------------------------------- retired-agent defences
// Invariant: RETIRED is terminal and a RETIRED LogicalAgent must never regain
// scheduler-authoritative live physical presence.

/// Crash leftover: an ACTIVE claim whose LogicalAgent is already semantically
/// retired. create_execution must refuse to mint a fresh Incarnation for it.
#[test]
fn retired_agent_claim_leftover_cannot_create_execution() {
    let db = FixtureDb::new("retired-exec");
    let env = file_env(&db);
    let (batch, _ids) = env.k.submit_batch(&[read_task("leftover")]).unwrap();
    let claim = env.k.claim_next_available().unwrap().unwrap();
    fixture_agent_state(&db, &claim.logical_agent_id, "RETIRED", 1_000_001.0);

    let stale_claim = Claim {
        task_id: claim.task_id.clone(),
        batch_id: batch,
        attempt_id: claim.attempt_id.clone(),
        attempt_number: claim.attempt_number,
        lease_id: claim.lease_id.clone(),
        lease_epoch: claim.lease_epoch,
        lease_expires_at: claim.lease_expires_at,
        logical_agent_id: claim.logical_agent_id.clone(),
        incarnation_id: None,
        execution_target: claim.execution_target.clone(),
        execution_profile: claim.execution_profile.clone(),
        workspace_mode: WorkspaceMode::ReadOnly,
        payload: json!({"objective": "leftover"}),
        acceptance: json!({}),
        workstream_id: None,
    };
    let err = env
        .k
        .create_execution(&stale_claim, FrozenExecutionSafety::UNISOLATED)
        .unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert!(err.to_string().contains("retired"));
}

/// A semantically retired agent has no legal transition back to READY: the
/// revival path must reject it outright.
#[test]
fn retired_agent_cannot_revive_via_public_api() {
    let db = FixtureDb::new("retired-revive");
    let env = file_env(&db);
    let agent = env.k.ready_agent("general").unwrap();
    fixture_agent_state(&db, &agent, "RETIRED", 1_000_001.0);

    let err = env.k.revive_agent(&agent, "local").unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert_eq!(
        env.k.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Retired
    );
}

/// The database itself rejects physically impossible history: two live
/// Incarnations for one agent violate the partial unique index.
#[test]
fn schema_rejects_second_live_incarnation_for_same_agent() {
    let db = FixtureDb::new("dup-inc");
    let env = file_env(&db);
    let agent = env.k.ready_agent("general").unwrap();
    fixture_incarnation(&db, &agent, 1, "local", "STARTING");

    let conn = Connection::open(&db.path).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    let err = conn.execute(
        "INSERT INTO incarnations(id,logical_agent_id,generation,execution_target,state,started_at)
         VALUES('second',?1,2,'local','WARM',1.0)",
        rusqlite::params![agent.as_str()],
    );
    assert!(err.is_err(), "second live incarnation must be rejected");
}

/// Excess unassigned INITIALIZING members retire without DRAINING; their
/// physical presence is fenced in the same retirement transaction.
#[test]
fn excess_initializing_retires_directly_and_fences_presence() {
    let db = FixtureDb::new("excess-init");
    let env = file_env(&db);
    let agent = env.k.ready_agent("general").unwrap();
    let inc = fixture_incarnation(&db, &agent, 1, "local", "STARTING");
    fixture_agent_state(&db, &agent, "INITIALIZING", 1_000_001.0);

    env.k.resize_partition("general", 0).unwrap();
    let report = env.k.reconcile_pool().unwrap();
    assert_eq!(report.retired, 1);
    assert_eq!(report.draining, 0);
    assert_eq!(
        env.k.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Retired
    );
    assert_eq!(
        env.k
            .incarnation(&IncarnationId::from_string(inc))
            .unwrap()
            .state,
        IncarnationState::Lost
    );
}

#[test]
fn excess_reviving_retires_directly() {
    let db = FixtureDb::new("excess-reviving");
    let env = file_env(&db);
    let agent = env.k.ready_agent("general").unwrap();
    fixture_agent_state(&db, &agent, "REVIVING", 1_000_001.0);

    env.k.resize_partition("general", 0).unwrap();
    let report = env.k.reconcile_pool().unwrap();
    assert_eq!(report.retired, 1);
    assert_eq!(report.draining, 0);
}

/// Crash boundary: the claim committed but the process died before
/// create_execution. Recovery must close the ACTIVE claim as an orphaned
/// attempt and mechanically recover the Task WITHOUT waiting for the original
/// lease deadline (spec 14: unstarted claims are authority-recoverable).
#[test]
fn restart_orphaned_claim_recovered_before_lease_deadline() {
    let db = FixtureDb::new("orphan-claim");
    let env = file_env(&db);
    let task_id;
    let attempt_id;
    let original_expiry;
    {
        let k = &env.k;
        let (_b, ids) = k.submit_batch(&[retryable_read("orphan")]).unwrap();
        task_id = ids["orphan"].clone();
        let claim = k.claim_next_available().unwrap().unwrap();
        attempt_id = claim.attempt_id.clone();
        original_expiry = k.lease_for_attempt(&attempt_id).unwrap().expires_at;
        // Crash before create_execution: no Execution row exists.
    }
    // Advance but stay inside the original lease window.
    env.clock.advance(2.0);
    assert!(env.clock.now() < original_expiry);

    let k = reopen(&env, &db);
    let report = k.recover_authority().unwrap();
    assert_eq!(
        report.retried, 1,
        "orphaned read claim recovers mechanically"
    );
    let attempt = k.attempt(&attempt_id).unwrap();
    assert_eq!(attempt.state, AttemptState::Expired);
    let task = k.task(&task_id).unwrap();
    assert!(task.current_attempt_id.is_none());
    assert_eq!(
        task.state,
        TaskState::RetryWait,
        "recovered under backoff, not waiting for the lease deadline"
    );
    // Backoff (1s) elapses inside the original lease window; promote and
    // re-claim with an advanced fencing epoch.
    env.clock.advance(2.0);
    assert!(env.clock.now() < original_expiry);
    k.promote_retry_wait().unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Queued);
    let claim2 = k.claim_next_available().unwrap().expect("re-claim");
    assert!(
        claim2.lease_epoch > attempt.lease_epoch,
        "fencing epoch advances"
    );
}

/// Authoritative durable state decodes fail-closed: a corrupted
/// retry_classes_json must surface as an invariant error, not silently become
/// an empty retry-class list that changes scheduling semantics.
#[test]
fn corrupted_durable_json_fails_closed() {
    let db = FixtureDb::new("corrupt-json");
    let env = file_env(&db);
    let (_b, task_id, _claim, _exec) = run_claim(&env.k, retryable_write("corrupt"), false);
    fixture_corrupt_json(&db, "tasks", "retry_classes_json", task_id.as_str());

    env.clock.advance(20.0);
    let err = env.k.expire_leases(false).unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
    assert!(err.to_string().contains("corrupted durable JSON"));
}

/// Spec 16 §B/13 conformance gate: crash/restart during Result AVAILABLE.
/// Task COMPLETED + Result AVAILABLE + Batch COMPLETED + exactly one
/// BATCH_RESULTS_READY must survive a full close/reopen cycle untouched, and
/// authority recovery must be a no-op over terminal history (no duplicate
/// Result, no re-enqueued wakeup).
#[test]
fn restart_during_result_available_preserves_durable_outcome() {
    let db = FixtureDb::new("result-available");
    let clock = Arc::new(ManualClock::new(2_000_000.0));
    let batch;
    let task_id;
    let result_id;
    {
        // Separate scope = full connection lifecycle before the crash point.
        let k = Kernel::open(
            &db.path,
            clock.clone() as Arc<dyn Clock>,
            10.0,
            CONTINUITY_MAX_BYTES,
        )
        .unwrap();
        k.upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
        k.reconcile_pool().unwrap();
        let (b, ids) = k.submit_batch(&[read_task("durable")]).unwrap();
        batch = b;
        task_id = ids["durable"].clone();
        let claim = k.claim_next_available().unwrap().unwrap();
        let launch = k
            .create_execution(&claim, FrozenExecutionSafety::UNISOLATED)
            .unwrap();
        let execution_id = launch.execution_id().clone();
        k.confirm_running_and_renew(
            &claim.attempt_id,
            claim.lease_epoch,
            &execution_id,
            &json!({"live": true}),
        )
        .unwrap();
        result_id = k
            .ack_success(
                &claim.attempt_id,
                claim.lease_epoch,
                Some(&execution_id),
                &json!({"answer": 7}),
                None,
                true,
                false,
            )
            .unwrap()
            .unwrap();

        assert_eq!(
            k.result_for_task(&task_id).unwrap().state,
            ResultState::Available
        );
        assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
        assert_eq!(
            k.outbox_for_batch(&batch, BATCH_RESULTS_READY)
                .unwrap()
                .len(),
            1
        );
    }
    clock.advance(30.0);

    {
        let k = Kernel::open(
            &db.path,
            clock.clone() as Arc<dyn Clock>,
            10.0,
            CONTINUITY_MAX_BYTES,
        )
        .unwrap();
        let report = k.recover_authority().unwrap();
        assert_eq!(report.retried, 0);
        assert_eq!(report.suspended, 0);

        // The durable outcome is exactly what it was before the restart.
        let stored = k.result_for_task(&task_id).unwrap();
        assert_eq!(stored.id, result_id);
        assert_eq!(stored.state, ResultState::Available);
        assert_eq!(stored.payload["answer"], 7);
        assert_eq!(k.task(&task_id).unwrap().state, TaskState::Completed);
        assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);

        // No duplicate wakeup, no resurrected work.
        let events = k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].state, OutboxState::Pending);
        assert!(k.claim_next_available().unwrap().is_none());
    }
}
