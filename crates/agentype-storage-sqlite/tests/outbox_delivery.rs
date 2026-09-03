//! M5.5 outbox delivery candidate + CAS (plan.txt §50).

mod common;

use agentype_core::*;
use agentype_storage_sqlite::Kernel;
use common::*;
use serde_json::json;

fn ready_event(k: &Kernel) -> (BatchId, TaskId, OutboxEventId) {
    let (batch, task_id, claim, execution_id) = run_claim(k, read_task("outbox-ready"), false);
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
    (batch, task_id, events[0].id.clone())
}

#[test]
fn due_reader_returns_only_pending() {
    let Env { k, clock } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    let due = k.due_outbox(clock.now(), 10).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].event_id(), &id);
    assert_eq!(due[0].event_type(), BATCH_RESULTS_READY);
    assert_eq!(due[0].delivery_attempts(), 0);

    k.commit_outbox_delivery_success(&id).unwrap();
    assert!(k.due_outbox(clock.now(), 10).unwrap().is_empty());
}

#[test]
fn future_next_delivery_at_is_excluded() {
    let Env { k, clock } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.commit_outbox_delivery_failure(&id, 30.0, "unavailable")
        .unwrap();
    assert!(k.due_outbox(clock.now(), 10).unwrap().is_empty());
    clock.advance(29.9);
    assert!(k.due_outbox(clock.now(), 10).unwrap().is_empty());
}

#[test]
fn exact_next_delivery_at_equals_now_is_due() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    let created = k.outbox_delivery(&id).unwrap().next_delivery_at;
    let due = k.due_outbox(created, 10).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].event_id(), &id);
}

#[test]
fn delivered_is_excluded_from_due() {
    let Env { k, clock } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.commit_outbox_delivery_success(&id).unwrap();
    assert_eq!(
        k.outbox_delivery(&id).unwrap().state,
        OutboxState::Delivered
    );
    assert!(k.due_outbox(clock.now(), 10).unwrap().is_empty());
}

#[test]
fn acked_is_excluded_from_due() {
    let Env { k, clock } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.ack_outbox(&id).unwrap();
    assert_eq!(k.outbox_delivery(&id).unwrap().state, OutboxState::Acked);
    assert!(k.due_outbox(clock.now(), 10).unwrap().is_empty());
}

#[test]
fn due_ordering_is_deterministic() {
    let Env { k, clock } = memory_env();
    let (_b1, _t1, first) = ready_event(&k);
    clock.advance(1.0);
    let (_b2, _t2, second) = ready_event(&k);
    let due = k.due_outbox(clock.now(), 10).unwrap();
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].event_id(), &first);
    assert_eq!(due[1].event_id(), &second);
    assert!(due[0].next_delivery_at() <= due[1].next_delivery_at());
    assert!(due[0].created_at() <= due[1].created_at());
}

#[test]
fn success_pending_to_delivered_increments_and_timestamps() {
    let Env { k, clock } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.commit_outbox_delivery_failure(&id, 0.0, "previous")
        .unwrap();
    clock.advance(5.0);
    let before = clock.now();
    let state = k.commit_outbox_delivery_success(&id).unwrap();
    assert_eq!(state, OutboxState::Delivered);
    let snap = k.outbox_delivery(&id).unwrap();
    assert_eq!(snap.state, OutboxState::Delivered);
    assert_eq!(snap.delivery_attempts, 2);
    assert!(snap.delivered_at.unwrap() >= before);
    assert!(snap.last_error.is_none());
}

#[test]
fn failure_stays_pending_increments_and_schedules_from_completion() {
    let Env { k, clock } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    let start = clock.now();
    clock.advance(100.0);
    let finished = clock.now();
    assert!(finished > start);
    let state = k
        .commit_outbox_delivery_failure(&id, 10.0, "timeout")
        .unwrap();
    assert_eq!(state, OutboxState::Pending);
    let snap = k.outbox_delivery(&id).unwrap();
    assert_eq!(snap.state, OutboxState::Pending);
    assert_eq!(snap.delivery_attempts, 1);
    assert!((snap.next_delivery_at - (finished + 10.0)).abs() < 0.001);
    assert_eq!(snap.last_error.as_deref(), Some("timeout"));
    assert!(
        snap.next_delivery_at > start + 10.0,
        "backoff must not be anchored at call start"
    );
}

#[test]
fn pending_and_delivered_ack_and_acked_is_idempotent() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, pending) = ready_event(&k);
    assert_eq!(k.ack_outbox(&pending).unwrap(), OutboxState::Acked);
    assert_eq!(k.ack_outbox(&pending).unwrap(), OutboxState::Acked);

    let (_batch2, _task2, delivered) = ready_event(&k);
    k.commit_outbox_delivery_success(&delivered).unwrap();
    assert_eq!(k.ack_outbox(&delivered).unwrap(), OutboxState::Acked);
    assert_eq!(k.ack_outbox(&delivered).unwrap(), OutboxState::Acked);
}

#[test]
fn success_after_ack_leaves_acked() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.ack_outbox(&id).unwrap();
    assert_eq!(
        k.commit_outbox_delivery_success(&id).unwrap(),
        OutboxState::Acked
    );
    assert_eq!(k.outbox_delivery(&id).unwrap().state, OutboxState::Acked);
}

#[test]
fn failure_after_ack_leaves_acked_without_retry() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.ack_outbox(&id).unwrap();
    assert_eq!(
        k.commit_outbox_delivery_failure(&id, 9.0, "late").unwrap(),
        OutboxState::Acked
    );
    let snap = k.outbox_delivery(&id).unwrap();
    assert_eq!(snap.state, OutboxState::Acked);
    assert_eq!(snap.delivery_attempts, 0);
    assert!(snap.last_error.is_none());
}

#[test]
fn duplicate_success_does_not_regress_or_double_count() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.commit_outbox_delivery_success(&id).unwrap();
    k.commit_outbox_delivery_success(&id).unwrap();
    let snap = k.outbox_delivery(&id).unwrap();
    assert_eq!(snap.state, OutboxState::Delivered);
    assert_eq!(snap.delivery_attempts, 1);
}

#[test]
fn failure_cannot_regress_delivered_to_pending() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    k.commit_outbox_delivery_success(&id).unwrap();
    assert_eq!(
        k.commit_outbox_delivery_failure(&id, 1.0, "late").unwrap(),
        OutboxState::Delivered
    );
    assert_eq!(
        k.outbox_delivery(&id).unwrap().state,
        OutboxState::Delivered
    );
}

#[test]
fn missing_event_is_fail_closed() {
    let Env { k, .. } = memory_env();
    let missing = OutboxEventId::from_string("event_does_not_exist");
    let err = k.commit_outbox_delivery_success(&missing).unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)));
    let err = k
        .commit_outbox_delivery_failure(&missing, 1.0, "x")
        .unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)));
    let err = k.outbox_delivery(&missing).unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)));
}

#[test]
fn last_error_is_bounded() {
    let Env { k, .. } = memory_env();
    let (_batch, _task, id) = ready_event(&k);
    let huge = "x".repeat(4096);
    k.commit_outbox_delivery_failure(&id, 1.0, &huge).unwrap();
    let snap = k.outbox_delivery(&id).unwrap();
    assert!(snap.last_error.as_ref().unwrap().chars().count() <= 512);
}

#[test]
fn outbox_ops_do_not_mutate_task_result_or_batch() {
    let Env { k, .. } = memory_env();
    let (batch, task_id, id) = ready_event(&k);
    let task_before = k.task(&task_id).unwrap();
    let batch_before = k.batch(&batch).unwrap();
    let result_before = k.result_for_task(&task_id).unwrap();
    k.commit_outbox_delivery_failure(&id, 1.0, "down").unwrap();
    k.commit_outbox_delivery_success(&id).unwrap();
    k.ack_outbox(&id).unwrap();
    assert_eq!(k.task(&task_id).unwrap().state, task_before.state);
    assert_eq!(k.batch(&batch).unwrap().state, batch_before.state);
    assert_eq!(
        k.result_for_task(&task_id).unwrap().state,
        result_before.state
    );
    assert_eq!(result_before.state, ResultState::Available);
}

#[test]
fn first_batch_completion_still_inserts_exactly_one_batch_results_ready() {
    let Env { k, clock } = memory_env();
    let (batch, _task, id) = ready_event(&k);
    let events = k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, id);
    assert_eq!(events[0].state, OutboxState::Pending);
    let due = k.due_outbox(clock.now(), 10).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].payload()["batch_id"], json!(batch.as_str()));
}
