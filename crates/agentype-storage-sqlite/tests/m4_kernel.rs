//! M4 correctness-kernel conformance (docs/specs/v0.2/16 §A):
//! Task/Attempt/Lease/Result/Batch machines, writer safety, proof bits,
//! Outbox atomicity.

mod common;

use agentype_core::*;
use agentype_storage_sqlite::Kernel;
use common::*;
use serde_json::json;

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
    let (execution_id, _) = k.create_execution(&claim, false).unwrap();
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
    assert_eq!(k.execution(&execution_id).unwrap().state, ExecutionState::Running);
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
    let (execution_id, _) = k.create_execution(&claim, false).unwrap();

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
    let (execution_id, _) = env.k.create_execution(&claim, false).unwrap();
    // Simulate corrupted pre-fix history below the API boundary...
    fixture_execution(&db, &execution_id, "UNKNOWN", true, true);
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
    let (batch, ids) = env.k
        .submit_batch(&[read_task("p-done"), write_task("p-stuck")])
        .unwrap();
    assert_eq!(env.k.batch(&batch).unwrap().state, BatchState::Active);

    let done_id = ids["p-done"].clone();
    let stuck_id = ids["p-stuck"].clone();

    // Claim order is by (priority, created_at, random id): dispatch each ack
    // by task identity so the writer ends SUSPENDED (success without
    // quiescence proof) and the read task completes when claimable.
    let first = env.k.claim_next_available().unwrap().unwrap();
    let (exec1, _) = env.k.create_execution(&first, false).unwrap();
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
        let (exec2, _) = env.k.create_execution(&second, false).unwrap();
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
    let (batch, ids) = env.k
        .submit_batch(&[write_task("stuck"), read_task("sibling")])
        .unwrap();
    let stuck_id = ids["stuck"].clone();

    // Suspend the batch through a cancelled non-quiescent writer. Claim order
    // is by (priority, created_at, random id), so dispatch by identity:
    // complete the read sibling cleanly, then cancel the writer with unknown
    // quiescence to open the obligation and suspend the batch.
    let first = env.k.claim_next_available().unwrap().unwrap();
    let (exec1, _) = env.k.create_execution(&first, false).unwrap();
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
        let (exec2, _) = env.k.create_execution(&second, false).unwrap();
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
    let (execution_id, _) = k.create_execution(&claim, false).unwrap();
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
    assert_eq!(k.batch(&claim.batch_id).unwrap().state, BatchState::Suspended);
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
