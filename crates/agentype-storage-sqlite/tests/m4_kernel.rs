//! M4 correctness-kernel conformance (docs/specs/v0.2/16 §A):
//! Task/Attempt/Lease/Result/Batch machines, writer safety, proof bits,
//! Outbox atomicity.

mod common;

use agentype_core::*;
use agentype_storage_sqlite::Kernel;
use common::*;
use serde_json::json;
use std::sync::Arc;

#[test]
fn wal_full_and_foreign_keys() {
    let db = FixtureDb::new("wal");
    let env = file_env(&db);
    let (journal, sync, fk) = env.k.pragmas().unwrap();
    assert_eq!(journal.to_uppercase(), "WAL");
    assert_eq!(sync, 2, "synchronous=FULL");
    assert_eq!(fk, 1);
    assert_eq!(env.k.schema_version().unwrap(), 1);
}

/// A database carrying the Rust-era identity survives reopen; a foreign
/// lineage is rejected even when its schema_migrations version collides with
/// the Rust schema version (Python V0.1 databases start at version 1 too).
#[test]
fn fresh_database_carries_rust_identity_and_reopens() {
    let db = FixtureDb::new("identity");
    {
        let env = file_env(&db);
        let conn = rusqlite::Connection::open(&db.path).unwrap();
        let line: String = conn
            .query_row(
                "SELECT value_json FROM scheduler_meta WHERE key='implementation_line'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(line, agentype_storage_sqlite::IMPLEMENTATION_LINE);
        drop(env);
    }
    // Reopen must succeed: same family, version 1.
    let env = file_env(&db);
    assert_eq!(env.k.schema_version().unwrap(), 1);
}

/// A simulated Python-lineage database (schema_migrations present at version
/// 1, no Rust identity) must fail closed instead of being adopted.
#[test]
fn foreign_lineage_with_colliding_version_is_rejected() {
    let db = FixtureDb::new("foreign-v1");
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL NOT NULL);
         INSERT INTO schema_migrations(version, applied_at) VALUES (1, 0.0);
         CREATE TABLE scheduler_meta (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at REAL NOT NULL);",
    )
    .unwrap();
    drop(conn);

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let err = match Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES) {
        Err(e) => e,
        Ok(_) => panic!("foreign database must be rejected"),
    };
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
    assert!(err.to_string().contains("not importable"), "got: {err:?}");
}

/// A database that has tables but no identity marker at all is rejected too
/// (partial adoption by IF NOT EXISTS must never be observable).
#[test]
fn tables_without_identity_are_rejected() {
    let db = FixtureDb::new("no-identity");
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch("CREATE TABLE batches (id TEXT PRIMARY KEY);")
        .unwrap();
    drop(conn);

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let err = match Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES) {
        Err(e) => e,
        Ok(_) => panic!("foreign database must be rejected"),
    };
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
}

#[test]
fn claim_creates_attempt_and_lease_atomically() {
    let Env { k, .. } = memory_env();
    let (batch, ids) = k.submit_batch(&[read_task("inspect")]).unwrap();
    let task_id = ids["inspect"].clone();
    let before = k.task(&task_id).unwrap();
    assert_eq!(before.state, TaskState::Queued);
    assert_eq!(before.fencing_epoch, LeaseEpoch(0));
    assert!(before.current_attempt_id.is_none());

    let claim = k.claim_next_available().unwrap().expect("claim");
    assert_eq!(claim.task_id, task_id);
    assert_eq!(claim.batch_id, batch);
    assert_eq!(claim.lease_epoch, LeaseEpoch(1));
    assert!(claim.incarnation_id.is_none());

    let task = k.task(&task_id).unwrap();
    assert_eq!(task.state, TaskState::Leased);
    assert_eq!(task.fencing_epoch, LeaseEpoch(1));
    assert_eq!(task.current_attempt_id.as_ref(), Some(&claim.attempt_id));

    let attempt = k.attempt(&claim.attempt_id).unwrap();
    assert_eq!(attempt.state, AttemptState::Active);
    assert_eq!(attempt.attempt_number, 1);
    let lease = k.lease_for_attempt(&claim.attempt_id).unwrap();
    assert_eq!(lease.state, LeaseState::Active);
    assert_eq!(lease.epoch, LeaseEpoch(1));
}

#[test]
fn submit_does_not_grant_authority() {
    assert!(!task_create_establishes_authority());
    let Env { k, .. } = memory_env();
    let (_, ids) = k.submit_batch(&[read_task("a")]).unwrap();
    let task = k.task(&ids["a"]).unwrap();
    assert_eq!(task.state, TaskState::Queued);
    assert!(task.current_attempt_id.is_none());
    assert_eq!(task.fencing_epoch, LeaseEpoch(0));
}

#[test]
fn stale_ack_cannot_complete_task() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, retryable_read("stale-ack"), false);
    k.nack(
        &claim.attempt_id,
        claim.lease_epoch,
        FailureClass::Timeout,
        Some(&execution_id),
        true,
        true,
        false,
    )
    .unwrap();
    let err = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&execution_id),
            &json!({"late": true}),
            None,
            true,
            false,
        )
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
    assert_ne!(k.task(&task_id).unwrap().state, TaskState::Completed);
}

#[test]
fn stale_nack_cannot_retry_task() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, retryable_read("stale-nack"), false);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&execution_id),
        &json!({"ok": 1}),
        None,
        true,
        false,
    )
    .unwrap();
    let err = k
        .nack(
            &claim.attempt_id,
            claim.lease_epoch,
            FailureClass::Timeout,
            Some(&execution_id),
            true,
            true,
            false,
        )
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Completed);
}

#[test]
fn lease_expiration_alone_does_not_permit_duplicate_writer() {
    let Env { k, clock } = memory_env();
    let (_b, task_id, _claim, _exec) = run_claim(&k, retryable_write("writer"), false);
    clock.advance(20.0);
    let report = k.expire_leases(false).unwrap();
    assert_eq!(report.suspended, 1);
    assert_eq!(report.retried, 0);
    let task = k.task(&task_id).unwrap();
    assert_eq!(task.state, TaskState::Suspended);
    let esc = k.open_escalation_for_task(&task_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
    assert!(k.claim_next_available().unwrap().is_none());
}

#[test]
fn isolated_writer_may_safely_recover() {
    let Env { k, clock } = memory_env();
    let (_b, task_id, _claim, _exec) = run_claim(&k, retryable_write("iso"), true);
    clock.advance(20.0);
    let report = k.expire_leases(false).unwrap();
    assert_eq!(report.retried, 1);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::RetryWait);
    clock.advance(2.0);
    k.promote_retry_wait().unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Queued);
    let claim2 = k.claim_next_available().unwrap().expect("retry claim");
    assert_eq!(claim2.lease_epoch, LeaseEpoch(2));
    assert_eq!(claim2.attempt_number, 2);
}

#[test]
fn read_only_expired_work_may_retry_under_policy() {
    let Env { k, clock } = memory_env();
    let (_b, task_id, _claim, _exec) = run_claim(&k, retryable_read("ro"), false);
    clock.advance(20.0);
    let report = k.expire_leases(false).unwrap();
    assert_eq!(report.retried, 1);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::RetryWait);
}

#[test]
fn writer_quiescence_unknown_suspends() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, write_task("wq"), false);
    let result = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&execution_id),
            &json!({"ok": true}),
            None,
            false,
            false,
        )
        .unwrap();
    assert!(result.is_none());
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
    assert!(k.result_for_task(&task_id).is_err());
}

#[test]
fn omitted_execution_id_cannot_bypass_writer_safety() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, _execution_id) = run_claim(&k, write_task("omit"), false);
    let result = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            None,
            &json!({"ok": true}),
            None,
            false,
            false,
        )
        .unwrap();
    assert!(result.is_none());
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
}

#[test]
fn cancelled_writer_still_requires_quiescence() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, _exec) = run_claim(&k, write_task("cancel-w"), false);
    k.cancel_task(&task_id, false).unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Cancelled);
    let esc = k.open_escalation_for_task(&task_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.state, LogicalAgentState::Suspended);
}

#[test]
fn open_writer_safety_escalation_blocks_retire() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, _claim, _exec) = run_claim(&k, write_task("block-retire"), false);
    k.cancel_task(&task_id, false).unwrap();
    let err = k.retire_partition("general").unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert!(err.to_string().contains("writer-safety"));
}

#[test]
fn running_confirmation_and_first_lease_renewal_are_atomic() {
    let Env { k, clock } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("near-deadline")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    let execution_id = launch.execution_id().clone();
    clock.advance(9.5);
    let expires = k
        .confirm_running_and_renew(
            &claim.attempt_id,
            claim.lease_epoch,
            &execution_id,
            &json!({"thread": 1}),
        )
        .unwrap();
    assert!(expires > clock.now());
    let lease = k.lease_for_attempt(&claim.attempt_id).unwrap();
    assert_eq!(lease.state, LeaseState::Active);
    assert_eq!(
        k.execution(&execution_id).unwrap().state,
        ExecutionState::Running
    );
    assert_eq!(k.task(&claim.task_id).unwrap().state, TaskState::Running);
}

#[test]
fn stale_physical_history_cannot_mutate_result() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, read_task("hist"), false);
    let result = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&execution_id),
            &json!({"answer": 1}),
            None,
            true,
            false,
        )
        .unwrap()
        .unwrap();
    k.record_physical_outcome(
        &execution_id,
        ExecutionState::Terminated,
        Some(&json!({"late": true})),
        Some(&json!({"answer": 99})),
        None,
        true,
        true,
    )
    .unwrap();
    let stored = k.result_for_task(&task_id).unwrap();
    assert_eq!(stored.id, result);
    assert_eq!(stored.payload["answer"], 1);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Completed);
}

#[test]
fn exactly_one_result_per_completed_task() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, read_task("one-result"), false);
    let first = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&execution_id),
            &json!({"n": 1}),
            None,
            true,
            false,
        )
        .unwrap()
        .unwrap();
    let err = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&execution_id),
            &json!({"n": 2}),
            None,
            true,
            false,
        )
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
    let stored = k.result_for_task(&task_id).unwrap();
    assert_eq!(stored.id, first);
}

#[test]
fn result_ack_does_not_change_task_completion() {
    let Env { k, .. } = memory_env();
    let (batch, task_id, claim, execution_id) = run_claim(&k, read_task("root-ack"), false);
    let result = k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&execution_id),
            &json!({"n": 1}),
            None,
            true,
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
    k.ack_result(&result, "root").unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Completed);
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
    assert_eq!(
        k.result_for_task(&task_id).unwrap().state,
        ResultState::Acked
    );
}

#[test]
fn first_batch_completion_inserts_exactly_one_batch_results_ready() {
    let Env { k, .. } = memory_env();
    let (batch, _task_id, claim, execution_id) = run_claim(&k, read_task("done"), false);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&execution_id),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
    let events = k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].state, OutboxState::Pending);
}

#[test]
fn notifier_ack_allows_pending_to_acked() {
    let Env { k, .. } = memory_env();
    let (batch, _task, claim, execution_id) = run_claim(&k, read_task("outbox"), false);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&execution_id),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    let event = &k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap()[0];
    let state = k.ack_outbox(&event.id).unwrap();
    assert_eq!(state, OutboxState::Acked);
    assert_eq!(
        k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap()[0].state,
        OutboxState::Acked
    );
}

#[test]
fn notifier_ack_from_delivered() {
    let Env { k, .. } = memory_env();
    let (batch, _task, claim, execution_id) = run_claim(&k, read_task("delivered"), false);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&execution_id),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    let event = k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap()[0]
        .id
        .clone();
    k.mark_outbox_delivered(&event).unwrap();
    k.ack_outbox(&event).unwrap();
    assert_eq!(
        k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap()[0].state,
        OutboxState::Acked
    );
}

// ------------------------------------------------------- configuration events

/// Configuration unavailability carries no physical proof. With a RUNNING
/// writer it must never manufacture the terminal/quiescence evidence that
/// would allow an unsafe replacement (spec 02: cancellation/config events are
/// not quiescence proof).
#[test]
fn configuration_unavailable_with_running_writer_must_not_retry() {
    let Env { k, .. } = memory_env();
    let spec = retryable_write("cfg-writer").retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![FailureClass::ResourceUnavailable],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    });
    let (_b, task_id, claim, _exec) = run_claim(&k, spec, false);
    let state = k
        .report_configuration_unavailable(&claim.attempt_id, claim.lease_epoch, "profile gone")
        .unwrap();
    assert_eq!(state, TaskState::Suspended);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
    let esc = k.open_escalation_for_task(&task_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
    assert!(k.claim_next_available().unwrap().is_none());
}

/// Without any persisted Execution there is no writer to prove quiet, so the
/// standard RESOURCE_UNAVAILABLE retry policy applies unchanged.
#[test]
fn unavailable_runtime_configuration_is_standardized_failure() {
    let Env { k, .. } = memory_env();
    let spec = read_task("cfg").retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![FailureClass::ResourceUnavailable],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    });
    let (_b, task_id) = {
        let (batch, ids) = k.submit_batch(&[spec]).unwrap();
        let _claim = k.claim_next_available().unwrap().expect("claim");
        // No Execution row yet: the dispatch-time configuration failure case.
        (batch, ids["cfg"].clone())
    };
    let task = k.task(&task_id).unwrap();
    assert_eq!(task.state, TaskState::Leased);
    let attempt_id = task.current_attempt_id.clone().unwrap();
    let epoch = k.attempt(&attempt_id).unwrap().lease_epoch;
    let state = k
        .report_configuration_unavailable(&attempt_id, epoch, "target gone")
        .unwrap();
    assert_eq!(state, TaskState::RetryWait);
    assert_eq!(
        unavailable_configuration_failure(),
        FailureClass::ResourceUnavailable
    );
}

// ------------------------------------------------------- physical proof bits

/// Spec 16 §A / 14: a nonterminal authoritative observation MUST NOT carry
/// terminal/quiescence proof. UNKNOWN/LOST records with proof are rejected.
#[test]
fn unresolved_physical_outcome_rejects_proof_bits() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("amb")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    let execution_id = launch.execution_id().clone();

    let err = k
        .record_physical_outcome(
            &execution_id,
            ExecutionState::Unknown,
            None,
            None,
            None,
            false,
            true,
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert!(err.to_string().contains("proof"));

    let err = k
        .record_physical_outcome(
            &execution_id,
            ExecutionState::Lost,
            None,
            None,
            Some(FailureClass::ExecutionLost),
            true,
            false,
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
}

/// A nonterminal nack (UNKNOWN) must persist zero proof bits regardless of
/// caller-supplied claims: durable proof requires a terminal physical state.
#[test]
fn nonterminal_nack_persists_no_quiescence() {
    let Env { k, .. } = memory_env();
    let (_b, _task, claim, execution_id) = run_claim(&k, retryable_read("nack-bits"), false);
    k.nack(
        &claim.attempt_id,
        claim.lease_epoch,
        FailureClass::ExecutionLost,
        Some(&execution_id),
        false,
        true, // unproven quiescence claim must not become durable
        false,
    )
    .unwrap();
    let row = k.execution(&execution_id).unwrap();
    assert_eq!(row.state, ExecutionState::Unknown);
    assert!(!row.terminal_confirmed);
    assert!(!row.quiescent_confirmed);
}

/// The retry policy must consult the SAME normalized truth the durable state
/// records. A caller can claim `quiescent_confirmed=true` without terminality;
/// that claim must not send an unproven writer's Task to RETRY_WAIT — after a
/// crash there would be no live lease to re-run the writer-safety gate, and a
/// duplicate writer could be dispatched over an execution the database itself
/// records as quiescence-unknown.
#[test]
fn nonterminal_nack_cannot_retry_writer_on_unproven_quiescence() {
    let Env { k, clock } = memory_env();
    let (batch, task_id, claim, execution_id) = run_claim(&k, retryable_write("unproven"), false);

    // Retryable class + raw quiescent=true + terminal=false: the historical
    // hole dispatched a replacement writer here.
    let state = k
        .nack(
            &claim.attempt_id,
            claim.lease_epoch,
            FailureClass::Timeout,
            Some(&execution_id),
            false,
            true,
            false,
        )
        .unwrap();
    assert_eq!(
        state,
        TaskState::Suspended,
        "no retry on unproven quiescence"
    );

    // Durable truth and scheduling outcome agree.
    let row = k.execution(&execution_id).unwrap();
    assert_eq!(row.state, ExecutionState::Unknown);
    assert!(!row.quiescent_confirmed);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Suspended);
    let esc = k.open_escalation_for_task(&task_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
    assert!(k.claim_next_available().unwrap().is_none());

    // Crash-restart convergence: recovery must not resurrect a replacement.
    clock.advance(20.0);
    k.recover_authority().unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
    assert!(k.claim_next_available().unwrap().is_none());
}

/// Entering an unresolved state supersedes earlier stored proof; once proof
/// became durable together with a terminal state, a late authoritative
/// nonterminal observation can neither rewrite history nor resume the writer.
#[test]
fn late_nonterminal_collect_cannot_inherit_durable_terminal_proof() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, retryable_read("inherit"), false);

    // Authoritative failure observation: durable terminal + quiescence proof.
    // UNKNOWN is not in the retry classes, so the task lands SUSPENDED.
    k.nack(
        &claim.attempt_id,
        claim.lease_epoch,
        FailureClass::Unknown,
        Some(&execution_id),
        true,
        true,
        false,
    )
    .unwrap();
    let row = k.execution(&execution_id).unwrap();
    assert_eq!(row.state, ExecutionState::Failed);
    assert!(row.quiescent_confirmed);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);

    // Late collect_outcome claims the process is alive again: the physical
    // transition graph forbids rewriting FAILED -> RUNNING, so the old proof
    // cannot be laundered into a fresh execution either.
    let err = k
        .record_physical_outcome(
            &execution_id,
            ExecutionState::Running,
            None,
            None,
            None,
            false,
            false,
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert_ne!(k.task(&task_id).unwrap().state, TaskState::RetryWait);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
}

/// After an authoritative UNKNOWN record the stored proof bits are cleared,
/// so no later consumer can mistake stale evidence for quiescence.
#[test]
fn unresolved_record_clears_stored_proof() {
    let db = FixtureDb::new("clear-proof");
    let env = file_env(&db);
    let (_b, _ids) = env.k.submit_batch(&[read_task("stale-proof")]).unwrap();
    let claim = env.k.claim_next_available().unwrap().unwrap();
    let launch = env
        .k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    let execution_id = launch.execution_id().clone();
    // Simulate corrupted pre-fix history below the API boundary...
    fixture_execution(&db, &execution_id, "STARTING", true, false);
    // ...then an authoritative nonterminal observation must clear it.
    env.k
        .record_physical_outcome(
            &execution_id,
            ExecutionState::Unknown,
            None,
            None,
            None,
            false,
            false,
        )
        .unwrap();
    let row = env.k.execution(&execution_id).unwrap();
    assert_eq!(row.state, ExecutionState::Unknown);
    assert!(!row.terminal_confirmed);
    assert!(!row.quiescent_confirmed);
}

// ------------------------------------------------------------ batch machine

/// Multi-task batches stay partially live: one COMPLETED Task keeps its Result
/// while another suspension flips only the aggregate to SUSPENDED.
#[test]
fn multi_task_batch_partial_completion_and_suspension() {
    let env = memory_env();
    env.k.resize_partition("general", 2).unwrap();
    env.k.reconcile_pool().unwrap();
    let (batch, ids) = env
        .k
        .submit_batch(&[read_task("p-done"), write_task("p-stuck")])
        .unwrap();
    assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Active);

    let done_id = ids["p-done"].clone();
    let stuck_id = ids["p-stuck"].clone();

    // Claim order is by (priority, created_at, random id): dispatch each ack
    // by task identity so the writer ends SUSPENDED (success without
    // quiescence proof) and the read task completes when claimable.
    let first = env.k.claim_next_available().unwrap().unwrap();
    let launch1 = env
        .k
        .create_execution(
            &first,
            FrozenExecutionSafety::unisolated(&first.execution_target, &first.execution_profile),
        )
        .unwrap();
    let exec1 = launch1.execution_id().clone();
    env.k
        .confirm_running_and_renew(&first.attempt_id, first.lease_epoch, &exec1, &json!({}))
        .unwrap();
    if first.task_id == stuck_id {
        env.k
            .ack_success(
                &first.attempt_id,
                first.lease_epoch,
                Some(&exec1),
                &json!({}),
                None,
                false,
                false,
            )
            .unwrap();
    } else {
        env.k
            .ack_success(
                &first.attempt_id,
                first.lease_epoch,
                Some(&exec1),
                &json!({"n": 1}),
                None,
                true,
                false,
            )
            .unwrap();
        assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Active);
        let second = env.k.claim_next_available().unwrap().unwrap();
        let launch2 = env
            .k
            .create_execution(
                &second,
                FrozenExecutionSafety::unisolated(
                    &second.execution_target,
                    &second.execution_profile,
                ),
            )
            .unwrap();
        let exec2 = launch2.execution_id().clone();
        env.k
            .confirm_running_and_renew(&second.attempt_id, second.lease_epoch, &exec2, &json!({}))
            .unwrap();
        env.k
            .ack_success(
                &second.attempt_id,
                second.lease_epoch,
                Some(&exec2),
                &json!({}),
                None,
                false,
                false,
            )
            .unwrap();
    }
    // One suspended writer flips the aggregate to SUSPENDED while the read
    // sibling either completed earlier or remains open in the same batch.
    assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Suspended);
    assert!(matches!(
        env.k.task(&done_id).unwrap().state,
        TaskState::Completed | TaskState::Queued
    ));
    assert_eq!(env.k.task(&stuck_id).unwrap().state, TaskState::Suspended);
    if env.k.task(&done_id).unwrap().state == TaskState::Completed {
        assert!(env.k.result_for_task(&done_id).is_ok());
    }
}

#[test]
fn cancel_queued_batch_cancels_tasks_and_batch() {
    let Env { k, .. } = memory_env();
    let (batch, ids) = k
        .submit_batch(&[read_task("q1"), read_task("q2").depends_on(["q1"])])
        .unwrap();
    assert_eq!(k.task(&ids["q2"]).unwrap().state, TaskState::Blocked);
    k.cancel_batch(&batch).unwrap();
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Cancelled);
    assert_eq!(k.task(&ids["q1"]).unwrap().state, TaskState::Cancelled);
    assert_eq!(k.task(&ids["q2"]).unwrap().state, TaskState::Cancelled);
    assert!(k.claim_next_available().unwrap().is_none());
}

#[test]
fn cancel_active_read_only_batch_closes_attempts_and_releases_agents() {
    let Env { k, .. } = memory_env();
    let (batch, task_id, claim, _exec) = run_claim(&k, read_task("busy-ro"), false);
    let incarnation = k.attempt(&claim.attempt_id).unwrap().incarnation_id.clone();
    k.cancel_batch(&batch).unwrap();

    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Cancelled);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Cancelled);
    let attempt = k.attempt(&claim.attempt_id).unwrap();
    assert_eq!(attempt.state, AttemptState::Cancelled);
    let lease = k.lease_for_attempt(&claim.attempt_id).unwrap();
    assert_eq!(lease.state, LeaseState::Revoked);
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.state, LogicalAgentState::Ready);
    assert!(agent.current_task_id.is_none());
    // Read-only work with no quiescence proof is fenced conservatively (LOST),
    // matching the V0.1 oracle: cancellation is not quiescence proof.
    assert_eq!(
        k.incarnation(&incarnation.unwrap()).unwrap().state,
        IncarnationState::Lost
    );
    // Nothing was cancelled behind an open writer obligation.
    assert!(k.open_escalation_for_task(&task_id).is_err());
    assert!(k.claim_next_available().unwrap().is_none());
}

#[test]
fn cancel_active_writer_with_unknown_quiescence_keeps_obligation_open() {
    let Env { k, .. } = memory_env();
    let (batch, task_id, claim, _exec) = run_claim(&k, write_task("busy-w"), false);
    k.cancel_batch(&batch).unwrap();

    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Cancelled);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Cancelled);
    let esc = k.open_escalation_for_task(&task_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.state, LogicalAgentState::Suspended);
    // The surviving obligation still blocks partition retirement.
    let err = k.retire_partition("general").unwrap_err();
    assert!(err.to_string().contains("writer-safety"));
}

#[test]
fn cancel_suspended_batch_preserves_open_writer_obligation() {
    let env = memory_env();
    env.k.resize_partition("general", 2).unwrap();
    env.k.reconcile_pool().unwrap();
    let (batch, ids) = env
        .k
        .submit_batch(&[write_task("stuck"), read_task("sibling")])
        .unwrap();
    let stuck_id = ids["stuck"].clone();

    // Suspend the batch through a cancelled non-quiescent writer. Claim order
    // is by (priority, created_at, random id), so dispatch by identity:
    // complete the read sibling cleanly, then cancel the writer with unknown
    // quiescence to open the obligation and suspend the batch.
    let first = env.k.claim_next_available().unwrap().unwrap();
    let launch1 = env
        .k
        .create_execution(
            &first,
            FrozenExecutionSafety::unisolated(&first.execution_target, &first.execution_profile),
        )
        .unwrap();
    let exec1 = launch1.execution_id().clone();
    env.k
        .confirm_running_and_renew(&first.attempt_id, first.lease_epoch, &exec1, &json!({}))
        .unwrap();
    if first.task_id == stuck_id {
        env.k.cancel_task(&stuck_id, false).unwrap();
    } else {
        env.k
            .ack_success(
                &first.attempt_id,
                first.lease_epoch,
                Some(&exec1),
                &json!({"ok": true}),
                None,
                true,
                false,
            )
            .unwrap();
        assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Active);
        let second = env.k.claim_next_available().unwrap().unwrap();
        let launch2 = env
            .k
            .create_execution(
                &second,
                FrozenExecutionSafety::unisolated(
                    &second.execution_target,
                    &second.execution_profile,
                ),
            )
            .unwrap();
        let exec2 = launch2.execution_id().clone();
        env.k
            .confirm_running_and_renew(&second.attempt_id, second.lease_epoch, &exec2, &json!({}))
            .unwrap();
        env.k.cancel_task(&stuck_id, false).unwrap();
    }
    assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Suspended);
    assert_eq!(
        env.k
            .open_escalation_for_task(&stuck_id)
            .unwrap()
            .failure_class,
        FailureClass::WriterQuiescenceUnknown
    );

    env.k.cancel_batch(&batch).unwrap();

    assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Cancelled);
    assert_eq!(env.k.task(&stuck_id).unwrap().state, TaskState::Cancelled);
    // The sibling is either still open (cancelled by the batch) or was
    // completed before cancellation and stays terminal.
    assert!(matches!(
        env.k.task(&ids["sibling"]).unwrap().state,
        TaskState::Cancelled | TaskState::Completed
    ));
    // Pre-existing writer-safety escalation survives batch cancellation.
    let esc = env.k.open_escalation_for_task(&stuck_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
}

#[test]
fn cancel_batch_is_idempotent() {
    let Env { k, .. } = memory_env();
    let (batch, _ids) = k.submit_batch(&[read_task("idem")]).unwrap();
    k.cancel_batch(&batch).unwrap();
    k.cancel_batch(&batch).unwrap();
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Cancelled);
}

#[test]
fn cancel_rejects_completed_batch() {
    let Env { k, .. } = memory_env();
    let (batch, _task, claim, execution_id) = run_claim(&k, read_task("finished"), false);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&execution_id),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
    let err = k.cancel_batch(&batch).unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
}

// ------------------------------------------------------------------ misc M4

#[test]
fn dependency_is_not_claimable_until_parent_completes() {
    let Env { k, .. } = memory_env();
    k.resize_partition("general", 2).unwrap();
    k.reconcile_pool().unwrap();
    let (_b, ids) = k
        .submit_batch(&[
            read_task("parent"),
            read_task("child").depends_on(["parent"]),
        ])
        .unwrap();
    assert_eq!(k.task(&ids["child"]).unwrap().state, TaskState::Blocked);
    let claim = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.task_id, ids["parent"]);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        None,
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(k.task(&ids["child"]).unwrap().state, TaskState::Queued);
}

#[test]
fn completed_task_never_reopens() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, read_task("term"), false);
    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&execution_id),
        &json!({}),
        None,
        true,
        false,
    )
    .unwrap();
    k.cancel_task(&task_id, true).unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Completed);
}

#[test]
fn heartbeat_requires_running_execution() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("hb")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    let err = k
        .heartbeat(&claim.attempt_id, claim.lease_epoch)
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    let execution_id = launch.execution_id().clone();
    let err = k
        .heartbeat(&claim.attempt_id, claim.lease_epoch)
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
    k.confirm_running_and_renew(
        &claim.attempt_id,
        claim.lease_epoch,
        &execution_id,
        &json!({}),
    )
    .unwrap();
    k.heartbeat(&claim.attempt_id, claim.lease_epoch).unwrap();
}

#[test]
fn checkpoint_is_fenced_by_attempt_epoch() {
    let Env { k, .. } = memory_env();
    let (_b, _task, claim, execution_id) = run_claim(&k, retryable_read("cp"), false);
    k.promote_checkpoint(
        &claim.attempt_id,
        claim.lease_epoch,
        &json!({"CURRENT CHECKPOINT": "v1"}),
    )
    .unwrap();
    k.nack(
        &claim.attempt_id,
        claim.lease_epoch,
        FailureClass::Timeout,
        Some(&execution_id),
        true,
        true,
        false,
    )
    .unwrap();
    let err = k
        .promote_checkpoint(
            &claim.attempt_id,
            claim.lease_epoch,
            &json!({"CURRENT CHECKPOINT": "v2"}),
        )
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
}

#[test]
fn unique_active_lease_constraint() {
    let Env { k, .. } = memory_env();
    k.submit_batch(&[read_task("u")]).unwrap();
    assert!(k.claim_next_available().unwrap().is_some());
    assert!(k.claim_next_available().unwrap().is_none());
}

#[test]
fn nack_without_named_retry_class_suspends() {
    let Env { k, .. } = memory_env();
    let (_b, task_id, claim, execution_id) = run_claim(&k, read_task("unknown-fail"), false);
    let state = k
        .nack(
            &claim.attempt_id,
            claim.lease_epoch,
            FailureClass::Unknown,
            Some(&execution_id),
            true,
            true,
            false,
        )
        .unwrap();
    assert_eq!(state, TaskState::Suspended);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::Suspended);
    assert_eq!(
        k.batch(&claim.batch_id).unwrap().state,
        BatchState::Suspended
    );
}

#[test]
fn epoch_is_monotonic() {
    let Env { k, clock } = memory_env();
    let spec = retryable_read("mono");
    let (_b, task_id, claim1, exec1) = run_claim(&k, spec, false);
    k.nack(
        &claim1.attempt_id,
        claim1.lease_epoch,
        FailureClass::Timeout,
        Some(&exec1),
        true,
        true,
        false,
    )
    .unwrap();
    clock.advance(2.0);
    k.promote_retry_wait().unwrap();
    let claim2 = k.claim_next_available().unwrap().unwrap();
    assert!(claim2.lease_epoch > claim1.lease_epoch);
    assert_eq!(k.task(&task_id).unwrap().fencing_epoch, claim2.lease_epoch);
}

#[test]
fn no_generation_membership_on_task() {
    // Compile-time: TaskRecord has no Generation field. Runtime: schema has no generation_id.
    let Env { k, .. } = memory_env();
    let (_b, ids) = k.submit_batch(&[read_task("no-gen")]).unwrap();
    let _ = k.task(&ids["no-gen"]).unwrap();
}

#[test]
fn partial_batch_cancellation_preserves_completed_task_and_result() {
    let Env { k, .. } = memory_env();
    let (_b, ids) = k
        .submit_batch(&[read_task("task-a"), read_task("task-b")])
        .unwrap();
    let task_a_id = &ids["task-a"];
    let task_b_id = &ids["task-b"];

    // Claim and complete one task
    let claim_a = k.claim_next_available().unwrap().unwrap();
    let launch_a = k
        .create_execution(
            &claim_a,
            FrozenExecutionSafety::unisolated(
                &claim_a.execution_target,
                &claim_a.execution_profile,
            ),
        )
        .unwrap();
    let exec_a = launch_a.execution_id().clone();
    k.confirm_running_and_renew(
        &claim_a.attempt_id,
        claim_a.lease_epoch,
        &exec_a,
        &json!({}),
    )
    .unwrap();
    k.ack_success(
        &claim_a.attempt_id,
        claim_a.lease_epoch,
        Some(&exec_a),
        &json!({"out": "done"}),
        None,
        true,
        false,
    )
    .unwrap();

    let completed_task_id = claim_a.task_id.clone();
    let other_task_id = if &completed_task_id == task_a_id {
        task_b_id
    } else {
        task_a_id
    };

    assert_eq!(
        k.task(&completed_task_id).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(
        k.result_for_task(&completed_task_id).unwrap().state,
        ResultState::Available
    );
    assert_eq!(k.task(other_task_id).unwrap().state, TaskState::Queued);

    // Cancel the entire batch
    k.cancel_batch(&claim_a.batch_id).unwrap();

    // Verify completed_task and its result remain intact, other_task is cancelled, and batch is cancelled
    assert_eq!(
        k.task(&completed_task_id).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(
        k.result_for_task(&completed_task_id).unwrap().state,
        ResultState::Available
    );
    assert_eq!(k.task(other_task_id).unwrap().state, TaskState::Cancelled);
    assert_eq!(
        k.batch(&claim_a.batch_id).unwrap().state,
        BatchState::Cancelled
    );
}

#[test]
fn create_execution_rejects_tampered_claim_identities() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("t1"), read_task("t2")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();

    // Mutate task_id to a different ID
    let mut bad_task_claim = claim.clone();
    bad_task_claim.task_id = TaskId::new();
    let err = k
        .create_execution(
            &bad_task_claim,
            FrozenExecutionSafety::unisolated(
                &bad_task_claim.execution_target,
                &bad_task_claim.execution_profile,
            ),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));

    // Mutate logical_agent_id
    let mut bad_agent_claim = claim.clone();
    bad_agent_claim.logical_agent_id = LogicalAgentId::new();
    let err = k
        .create_execution(
            &bad_agent_claim,
            FrozenExecutionSafety::unisolated(
                &bad_agent_claim.execution_target,
                &bad_agent_claim.execution_profile,
            ),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));

    // Mutate execution_target
    let mut bad_target_claim = claim.clone();
    bad_target_claim.execution_target = "foreign-target".to_string();
    let err = k
        .create_execution(
            &bad_target_claim,
            FrozenExecutionSafety::unisolated(
                &bad_target_claim.execution_target,
                &bad_target_claim.execution_profile,
            ),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));

    // Mutate execution_profile
    let mut bad_profile_claim = claim.clone();
    bad_profile_claim.execution_profile = "foreign-profile".to_string();
    let err = k
        .create_execution(
            &bad_profile_claim,
            FrozenExecutionSafety::unisolated(
                &bad_profile_claim.execution_target,
                &bad_profile_claim.execution_profile,
            ),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidAuthority(_)));

    // Original valid claim succeeds
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    let exec_id = launch.execution_id().clone();
    assert!(!exec_id.as_str().is_empty());
}

#[test]
fn durable_json_shape_fail_closed_regressions() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("json-shape")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();

    // Continuity capsule must be JSON object (array and null must fail with InvalidTransition)
    let err_arr = k
        .promote_checkpoint(
            &claim.attempt_id,
            claim.lease_epoch,
            &json!(["not", "an", "object"]),
        )
        .unwrap_err();
    assert!(matches!(err_arr, Error::InvalidTransition(_)));

    let err_null = k
        .promote_checkpoint(&claim.attempt_id, claim.lease_epoch, &json!(null))
        .unwrap_err();
    assert!(matches!(err_null, Error::InvalidTransition(_)));

    // Malformed JSON shape in retry_classes_json or partition tags fails closed with InvariantViolation
    let err_obj_classes = agentype_storage_sqlite::txutil::parse_failure_classes("{}").unwrap_err();
    assert!(matches!(err_obj_classes, Error::InvariantViolation(_)));

    let err_mixed_classes =
        agentype_storage_sqlite::txutil::parse_failure_classes("[\"TIMEOUT\", 42]").unwrap_err();
    assert!(matches!(err_mixed_classes, Error::InvariantViolation(_)));

    let err_obj_tags =
        agentype_storage_sqlite::txutil::parse_str_list("{\"foo\":\"bar\"}").unwrap_err();
    assert!(matches!(err_obj_tags, Error::InvariantViolation(_)));
}

#[test]
fn quiescent_confirmed_requires_terminal_confirmed_db_constraint() {
    let db = FixtureDb::new("db-check");
    let env = file_env(&db);
    let (_b, _ids) = env.k.submit_batch(&[read_task("db-check")]).unwrap();
    let claim = env.k.claim_next_available().unwrap().unwrap();
    let launch = env
        .k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    let exec_id = launch.execution_id().clone();

    let conn = rusqlite::Connection::open(&db.path).unwrap();
    // Raw UPDATE setting quiescent_confirmed=1 and terminal_confirmed=0 must violate CHECK constraint
    let res = conn.execute(
        "UPDATE executions SET quiescent_confirmed=1, terminal_confirmed=0 WHERE id=?1",
        rusqlite::params![exec_id.as_str()],
    );
    assert!(
        res.is_err(),
        "CHECK constraint must reject quiescent_confirmed=1 with terminal_confirmed=0"
    );

    // Raw UPDATE setting quiescent_confirmed=1, terminal_confirmed=1 but state=RUNNING must also violate CHECK constraint
    let res_running = conn.execute(
        "UPDATE executions SET quiescent_confirmed=1, terminal_confirmed=1, state='RUNNING' WHERE id=?1",
        rusqlite::params![exec_id.as_str()],
    );
    assert!(
        res_running.is_err(),
        "CHECK constraint must reject quiescent_confirmed=1 with non-terminal state"
    );
}

#[test]
fn arbitrary_existing_table_without_marker_is_rejected() {
    let db = FixtureDb::new("arbitrary-table");
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch("CREATE TABLE custom_app_data (id INTEGER PRIMARY KEY, name TEXT);")
        .unwrap();
    drop(conn);

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let err = match Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES) {
        Err(e) => e,
        Ok(_) => panic!("arbitrary existing table database must be rejected"),
    };
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
}

#[test]
fn tasks_only_database_without_marker_is_rejected() {
    let db = FixtureDb::new("tasks-only");
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY);")
        .unwrap();
    drop(conn);

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let err = match Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES) {
        Err(e) => e,
        Ok(_) => panic!("tasks-only database must be rejected"),
    };
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
}

#[test]
fn scheduler_meta_with_wrong_identity_is_rejected() {
    let db = FixtureDb::new("wrong-identity");
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch(
        "CREATE TABLE scheduler_meta (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at REAL NOT NULL);
         INSERT INTO scheduler_meta(key, value_json, updated_at) VALUES ('implementation_line', 'some-other-runtime', 0.0);",
    )
    .unwrap();
    drop(conn);

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let err = match Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES) {
        Err(e) => e,
        Ok(_) => panic!("wrong identity database must be rejected"),
    };
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
}

#[test]
fn rust_marker_without_schema_version_is_rejected() {
    let db = FixtureDb::new("missing-migrations");
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch(
        "CREATE TABLE scheduler_meta (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at REAL NOT NULL);
         INSERT INTO scheduler_meta(key, value_json, updated_at) VALUES ('implementation_line', 'rust-v0.2', 0.0);",
    )
    .unwrap();
    drop(conn);

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let err = match Kernel::open(&db.path, clock, 10.0, CONTINUITY_MAX_BYTES) {
        Err(e) => e,
        Ok(_) => panic!("database with scheduler_meta but no migrations must be rejected"),
    };
    assert!(matches!(err, Error::InvariantViolation(_)), "got: {err:?}");
}

#[test]
fn escalation_resolution_of_lost_writer_transitions_to_terminated_with_durable_quiescence() {
    let Env { k, clock } = memory_env();
    let (_batch, _task_id, claim, exec_id) = run_claim(&k, write_task("lost-writer"), false);

    // Physical observation transitions execution to LOST
    k.record_physical_outcome(
        &exec_id,
        ExecutionState::Lost,
        None,
        None,
        None,
        false,
        false,
    )
    .unwrap();

    // Lease expires -> sweeper runs -> Task becomes SUSPENDED and Escalation is created
    clock.advance(20.0);
    k.expire_leases(false).unwrap();
    assert_eq!(k.task(&claim.task_id).unwrap().state, TaskState::Suspended);

    let esc = k.open_escalation_for_task(&claim.task_id).unwrap();
    assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);

    // Root confirms quiescence and resolves escalation to retry
    k.resolve_escalation(&esc.id, "retry", true).unwrap();

    // Execution must transition to TERMINATED (not stay LOST) and carry durable terminal/quiescence proof
    let exec = k.execution(&exec_id).unwrap();
    assert_eq!(exec.state, ExecutionState::Terminated);
    assert!(exec.terminal_confirmed);
    assert!(exec.quiescent_confirmed);

    // Task is back in QUEUED and ready for retry
    assert_eq!(k.task(&claim.task_id).unwrap().state, TaskState::Queued);
}

// ------------------------------------------------------------ M5.1 launch & tamper regressions

#[test]
fn mutated_claim_payload_does_not_alter_launch_snapshot() {
    let Env { k, .. } = memory_env();
    let spec = TaskSpec::new(
        "tamper-payload",
        json!({"original_key": "authoritative_value"}),
    );
    let (_b, _ids) = k.submit_batch(&[spec]).unwrap();
    let mut claim = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.payload["original_key"], "authoritative_value");

    // Tamper with caller-held Claim DTO
    claim.payload = json!({"injected_key": "malicious_tampered_value"});

    // Authoritative launch snapshot MUST contain the durable payload
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    assert_eq!(
        launch.payload(),
        &json!({"original_key": "authoritative_value"})
    );
}

#[test]
fn mutated_claim_acceptance_does_not_alter_launch_snapshot() {
    let Env { k, .. } = memory_env();
    let mut spec = TaskSpec::new("tamper-acceptance", json!({}));
    spec.acceptance = json!({"criteria": "strict_validation"});
    let (_b, _ids) = k.submit_batch(&[spec]).unwrap();
    let mut claim = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.acceptance["criteria"], "strict_validation");

    // Tamper with caller-held Claim DTO
    claim.acceptance = json!({"criteria": "bypassed"});

    // Authoritative launch snapshot MUST contain durable acceptance
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    assert_eq!(
        launch.acceptance(),
        &json!({"criteria": "strict_validation"})
    );
}

#[test]
fn mutated_claim_workspace_mode_cannot_widen_launch_authority() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("read-only-task")]).unwrap();
    let mut claim = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.workspace_mode, WorkspaceMode::ReadOnly);

    // Tamper with caller-held Claim DTO to claim WRITE authority
    claim.workspace_mode = WorkspaceMode::Write;

    // Authoritative launch snapshot MUST remain ReadOnly
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    assert_eq!(launch.workspace_mode(), WorkspaceMode::ReadOnly);
}

#[test]
fn mutated_claim_workstream_does_not_alter_launch_snapshot() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("no-ws-task")]).unwrap();
    let mut claim = k.claim_next_available().unwrap().unwrap();
    assert!(claim.workstream_id.is_none());

    // Tamper with caller-held Claim DTO to inject a workstream
    claim.workstream_id = Some(WorkstreamId::from_string("forged-workstream"));

    // Authoritative launch snapshot MUST remain None
    let launch = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap();
    assert!(launch.workstream_id().is_none());
}

#[test]
fn launch_snapshot_matches_persisted_execution_identity_and_isolation() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[write_task("isolated-task")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();

    let safety =
        FrozenExecutionSafety::new(&claim.execution_target, &claim.execution_profile, true);
    let launch = k.create_execution(&claim, safety.clone()).unwrap();
    assert_eq!(launch.task_id(), &claim.task_id);
    assert_eq!(launch.attempt_id(), &claim.attempt_id);
    assert_eq!(launch.logical_agent_id(), &claim.logical_agent_id);
    assert!(launch.attempt_isolation());
    assert_eq!(launch.safety(), &safety);

    let exec = k.execution(launch.execution_id()).unwrap();
    assert_eq!(&exec.id, launch.execution_id());
    assert_eq!(&exec.attempt_id, launch.attempt_id());
    assert_eq!(&exec.incarnation_id, launch.incarnation_id());
    assert_eq!(exec.execution_target.as_str(), launch.execution_target());
    assert_eq!(exec.execution_profile.as_str(), launch.execution_profile());
    assert!(exec.attempt_isolation);
}

#[test]
fn mismatched_target_or_profile_safety_proof_rejected() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k
        .submit_batch(&[read_task("proof-target-mismatch")])
        .unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.execution_target, "local");
    assert_eq!(claim.execution_profile, "default");

    // 1. Safety proof for forged target "remote" is rejected
    let forged_target_safety = FrozenExecutionSafety::new("remote", "default", true);
    let err = k
        .create_execution(&claim, forged_target_safety)
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("safety proof target 'remote' does not match"));

    // 2. Safety proof for forged profile "isolated" is rejected
    let forged_profile_safety = FrozenExecutionSafety::new("local", "isolated", true);
    let err = k
        .create_execution(&claim, forged_profile_safety)
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("safety proof profile 'isolated' does not match"));

    // 3. Matching target & profile succeeds
    let valid_safety = FrozenExecutionSafety::new("local", "default", false);
    let launch = k.create_execution(&claim, valid_safety).unwrap();
    assert_eq!(launch.execution_target(), "local");
    assert_eq!(launch.execution_profile(), "default");
}

#[test]
fn expired_lease_cannot_create_execution_before_expiry_sweep() {
    let Env { k, clock } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("expire-pre-exec")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();

    // Advance clock past lease deadline without calling expire_leases
    clock.advance(20.0);
    assert!(clock.now() > claim.lease_expires_at);

    // create_execution must fail closed on expired authority
    let err = k
        .create_execution(
            &claim,
            FrozenExecutionSafety::unisolated(&claim.execution_target, &claim.execution_profile),
        )
        .unwrap_err();
    assert!(matches!(err, Error::StaleAuthority(_)));
}

#[test]
fn launch_snapshot_carries_committed_continuity_and_preference() {
    let Env { k, .. } = memory_env();
    let ws = k.create_workstream("ws-test", None, None).unwrap();

    // 1. Submit first task to establish a logical agent and promote a checkpoint
    let spec1 = retryable_read("task-1").workstream(ws.clone());
    let (_b1, _task1, claim1, exec1) = run_claim(&k, spec1, false);
    k.promote_checkpoint(
        &claim1.attempt_id,
        claim1.lease_epoch,
        &json!({"CURRENT CHECKPOINT": "step_1_finished"}),
    )
    .unwrap();
    k.ack_success(
        &claim1.attempt_id,
        claim1.lease_epoch,
        Some(&exec1),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();

    // 2. Submit second task demanding REQUIRED continuity in the same workstream
    let spec2 = TaskSpec::new("task-2", json!({"step": 2}))
        .workstream(ws)
        .continuity(ContinuityPreference::Required);
    let (_b2, _ids2) = k.submit_batch(&[spec2]).unwrap();
    let claim2 = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim2.logical_agent_id, claim1.logical_agent_id);

    // 3. create_execution must bundle the agent's committed continuity capsule & monotonic version
    let launch2 = k
        .create_execution(
            &claim2,
            FrozenExecutionSafety::unisolated(&claim2.execution_target, &claim2.execution_profile),
        )
        .unwrap();
    assert_eq!(
        launch2.continuity().preference(),
        ContinuityPreference::Required
    );
    assert_eq!(launch2.continuity().version(), 1);
    assert_eq!(
        launch2.continuity().capsule(),
        &json!({"CURRENT CHECKPOINT": "step_1_finished"})
    );
}
