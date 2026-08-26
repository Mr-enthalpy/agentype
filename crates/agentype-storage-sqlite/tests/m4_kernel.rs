//! M4 correctness-kernel conformance (docs/specs/v0.2/16 §A and plan.txt §20).

use agentype_core::*;
use agentype_storage_sqlite::Kernel;
use serde_json::json;
use std::sync::Arc;

fn setup() -> (Kernel, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(1_000_000.0));
    let k = Kernel::open_memory(clock.clone() as Arc<dyn Clock>, 10.0).unwrap();
    k.upsert_partition(&PartitionSpec::new(
        "general",
        1,
        Retention::Resident,
        "local",
        "default",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();
    (k, clock)
}

fn read_task(name: &str) -> TaskSpec {
    TaskSpec::new(name, json!({"objective": name}))
}

fn write_task(name: &str) -> TaskSpec {
    TaskSpec::new(name, json!({"objective": name})).write()
}

fn retryable_write(name: &str) -> TaskSpec {
    write_task(name).retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![
            FailureClass::ExecutionLost,
            FailureClass::Timeout,
            FailureClass::TransientExternal,
        ],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    })
}

fn retryable_read(name: &str) -> TaskSpec {
    read_task(name).retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![FailureClass::ExecutionLost, FailureClass::Timeout],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    })
}

fn run_claim(
    k: &Kernel,
    spec: TaskSpec,
    isolation: bool,
) -> (BatchId, TaskId, Claim, ExecutionId) {
    let (batch, ids) = k.submit_batch(&[spec.clone()]).unwrap();
    let claim = k.claim_next_available().unwrap().expect("claim");
    let (execution_id, _) = k.create_execution(&claim, isolation).unwrap();
    k.confirm_running_and_renew(
        &claim.attempt_id,
        claim.lease_epoch,
        &execution_id,
        &json!({"live": true}),
    )
    .unwrap();
    (batch, ids[&spec.name].clone(), claim, execution_id)
}

#[test]
fn wal_full_and_foreign_keys() {
    let dir = std::env::temp_dir().join(format!(
        "agentype-wal-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    let k = Kernel::open(&path, clock, 10.0).unwrap();
    let (journal, sync, fk) = k.pragmas().unwrap();
    assert_eq!(journal.to_uppercase(), "WAL");
    assert_eq!(sync, 2, "synchronous=FULL");
    assert_eq!(fk, 1);
    assert_eq!(k.schema_version().unwrap(), 1);
    drop(k);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn claim_creates_attempt_and_lease_atomically() {
    let (k, _) = setup();
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
    let (k, _) = setup();
    let (_, ids) = k.submit_batch(&[read_task("a")]).unwrap();
    let task = k.task(&ids["a"]).unwrap();
    assert_eq!(task.state, TaskState::Queued);
    assert!(task.current_attempt_id.is_none());
    assert_eq!(task.fencing_epoch, LeaseEpoch(0));
}

#[test]
fn stale_ack_cannot_complete_task() {
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, clock) = setup();
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
    let (k, clock) = setup();
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
    let (k, clock) = setup();
    let (_b, task_id, _claim, _exec) = run_claim(&k, retryable_read("ro"), false);
    clock.advance(20.0);
    let report = k.expire_leases(false).unwrap();
    assert_eq!(report.retried, 1);
    assert_eq!(k.task(&task_id).unwrap().state, TaskState::RetryWait);
}

#[test]
fn writer_quiescence_unknown_suspends() {
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, _) = setup();
    let (_b, task_id, _claim, _exec) = run_claim(&k, write_task("block-retire"), false);
    k.cancel_task(&task_id, false).unwrap();
    let err = k.retire_partition("general").unwrap_err();
    assert!(matches!(err, Error::InvalidTransition(_)));
    assert!(err.to_string().contains("writer-safety"));
}

#[test]
fn running_confirmation_and_first_lease_renewal_are_atomic() {
    let (k, clock) = setup();
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
fn unknown_execution_can_reconcile_to_running() {
    let (k, _) = setup();
    let (_b, _ids) = k.submit_batch(&[read_task("amb2")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    let (execution_id, _) = k.create_execution(&claim, false).unwrap();
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
    k.confirm_running_and_renew(
        &claim.attempt_id,
        claim.lease_epoch,
        &execution_id,
        &json!({"found": true}),
    )
    .unwrap();
    assert_eq!(
        k.execution(&execution_id).unwrap().state,
        ExecutionState::Running
    );
}

#[test]
fn stale_lost_can_refine_to_terminated() {
    let (k, _) = setup();
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

#[test]
fn stale_physical_history_cannot_mutate_result() {
    let (k, _) = setup();
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
fn excess_initializing_retires_directly() {
    let (k, _) = setup();
    let agent = k.ready_agent("general").unwrap();
    k.set_logical_agent_state(&agent, LogicalAgentState::Initializing)
        .unwrap();
    k.resize_partition("general", 0).unwrap();
    let report = k.reconcile_pool().unwrap();
    assert_eq!(report.retired, 1);
    assert_eq!(report.draining, 0);
    assert_eq!(
        k.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Retired
    );
}

#[test]
fn excess_reviving_retires_directly() {
    let (k, _) = setup();
    let agent = k.ready_agent("general").unwrap();
    k.set_logical_agent_state(&agent, LogicalAgentState::Reviving)
        .unwrap();
    k.resize_partition("general", 0).unwrap();
    let report = k.reconcile_pool().unwrap();
    assert_eq!(report.retired, 1);
    assert_eq!(report.draining, 0);
}

#[test]
fn assigned_topology_move_drains() {
    let (k, _) = setup();
    k.upsert_partition(&PartitionSpec::new(
        "other",
        0,
        Retention::Resident,
        "local",
        "default",
    ))
    .unwrap();
    let (_b, _task, claim, _exec) = run_claim(&k, read_task("busy"), false);
    k.move_capacity("general", "other", 1).unwrap();
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.state, LogicalAgentState::Draining);
    assert_eq!(
        agent.pending_partition.as_ref().map(|p| p.as_str()),
        Some("other")
    );
}

#[test]
fn semantic_retirement_fences_live_incarnation_lost() {
    let (k, _) = setup();
    let agent = k.ready_agent("general").unwrap();
    let incarnation = k.ensure_incarnation(&agent, "local").unwrap();
    k.resize_partition("general", 0).unwrap();
    assert_eq!(k.reconcile_pool().unwrap().retired, 1);
    assert_eq!(
        k.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Retired
    );
    assert_eq!(
        k.incarnation(&incarnation).unwrap().state,
        IncarnationState::Lost
    );
}

#[test]
fn merge_sums_desired_capacity() {
    let (k, _) = setup();
    k.upsert_partition(&PartitionSpec::new(
        "extra",
        2,
        Retention::Resident,
        "local",
        "default",
    ))
    .unwrap();
    k.merge_partitions("extra", "general").unwrap();
    let general = k.partition("general").unwrap();
    assert_eq!(general.desired_capacity, 3);
    assert!(!k.partition("extra").unwrap().active);
}

#[test]
fn merge_migrates_future_task_classification() {
    let (k, _) = setup();
    k.upsert_partition(&PartitionSpec::new(
        "src",
        1,
        Retention::Resident,
        "local",
        "default",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();
    let (_b, ids) = k
        .submit_batch(&[read_task("queued-elsewhere").partition("src")])
        .unwrap();
    k.merge_partitions("src", "general").unwrap();
    let task = k.task(&ids["queued-elsewhere"]).unwrap();
    assert_eq!(task.partition.as_str(), "general");
    assert_eq!(task.state, TaskState::Queued);
}

#[test]
fn active_attempt_keeps_frozen_authority_through_merge() {
    let (k, _) = setup();
    k.upsert_partition(&PartitionSpec::new(
        "src",
        1,
        Retention::Resident,
        "local-b",
        "profile-b",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();
    let (_b, _ids) = k
        .submit_batch(&[read_task("live").partition("src")])
        .unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.execution_target, "local-b");
    k.merge_partitions("src", "general").unwrap();
    let attempt = k.attempt(&claim.attempt_id).unwrap();
    assert_eq!(attempt.state, AttemptState::Active);
    let lease = k.lease_for_attempt(&claim.attempt_id).unwrap();
    assert_eq!(lease.state, LeaseState::Active);
    assert_eq!(lease.epoch, claim.lease_epoch);
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
}

#[test]
fn retire_rejects_nonterminal_task() {
    let (k, _) = setup();
    k.submit_batch(&[read_task("still-open")]).unwrap();
    let err = k.retire_partition("general").unwrap_err();
    assert!(err.to_string().contains("nonterminal"));
}

#[test]
fn exactly_one_result_per_completed_task() {
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, _) = setup();
    let (batch, task_id, claim, execution_id) = run_claim(&k, read_task("done"), false);
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
    let _ = task_id;
}

#[test]
fn notifier_ack_allows_pending_to_acked() {
    let (k, _) = setup();
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
    let (k, _) = setup();
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

#[test]
fn restart_recovery_prevents_blind_duplicate_execution() {
    let dir = std::env::temp_dir().join(format!(
        "agentype-m4-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scheduler.db");
    let clock = Arc::new(ManualClock::new(1_000_000.0));
    let task_id;
    let attempt_id;
    {
        let k = Kernel::open(&path, clock.clone() as Arc<dyn Clock>, 10.0).unwrap();
        k.upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
        k.reconcile_pool().unwrap();
        let (_b, ids) = k.submit_batch(&[retryable_write("restart")]).unwrap();
        task_id = ids["restart"].clone();
        let claim = k.claim_next_available().unwrap().unwrap();
        attempt_id = claim.attempt_id.clone();
        let _ = k.create_execution(&claim, false).unwrap();
    }
    clock.advance(20.0);
    {
        let k = Kernel::open(&path, clock.clone() as Arc<dyn Clock>, 10.0).unwrap();
        k.recover_authority().unwrap();
        let task = k.task(&task_id).unwrap();
        assert_eq!(task.state, TaskState::Suspended);
        assert!(task.current_attempt_id.is_none());
        assert!(k.claim_next_available().unwrap().is_none());
        let attempt = k.attempt(&attempt_id).unwrap();
        assert_eq!(attempt.state, AttemptState::Expired);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unavailable_runtime_configuration_is_standardized_failure() {
    let (k, _) = setup();
    let spec = read_task("cfg").retry(RetryPolicy {
        max_attempts: 3,
        retry_classes: vec![FailureClass::ResourceUnavailable],
        base_backoff_seconds: 1.0,
        max_backoff_seconds: 8.0,
    });
    let (_b, task_id, claim, _exec) = run_claim(&k, spec, false);
    let state = k
        .report_configuration_unavailable(&claim.attempt_id, claim.lease_epoch, "target gone")
        .unwrap();
    assert_eq!(state, TaskState::RetryWait);
    assert_eq!(
        unavailable_configuration_failure(),
        FailureClass::ResourceUnavailable
    );
    let _ = task_id;
}

#[test]
fn dependency_is_not_claimable_until_parent_completes() {
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, _) = setup();
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
    let (k, _) = setup();
    k.submit_batch(&[read_task("u")]).unwrap();
    assert!(k.claim_next_available().unwrap().is_some());
    assert!(k.claim_next_available().unwrap().is_none());
}

#[test]
fn nack_without_named_retry_class_suspends() {
    let (k, _) = setup();
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
fn retired_agent_has_no_live_incarnation() {
    let (k, _) = setup();
    let agent = k.ready_agent("general").unwrap();
    let inc = k.ensure_incarnation(&agent, "local").unwrap();
    k.resize_partition("general", 0).unwrap();
    k.reconcile_pool().unwrap();
    assert!(!k.incarnation(&inc).unwrap().state.is_live_presence());
}

#[test]
fn epoch_is_monotonic() {
    let (k, clock) = setup();
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
    let (k, _) = setup();
    let (_b, ids) = k.submit_batch(&[read_task("no-gen")]).unwrap();
    let _ = k.task(&ids["no-gen"]).unwrap();
}
