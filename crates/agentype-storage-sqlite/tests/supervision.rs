//! M5.3 supervised heartbeat renewal primitives (docs/specs/v0.2/16 §A2,
//! M5.3 plan §10/§11/§15/§23): renewal is fenced by the attempt_id +
//! lease_epoch + execution_id triple, requires the Execution to be physically
//! RUNNING, and can never revive expired or closed authority. A non-RUNNING
//! Execution is a drop-supervision signal, never a durable-state repair and
//! never a quiescence/terminality proof.

mod common;

use agentype_core::*;
use agentype_storage_sqlite::{Kernel, SupervisedRenewal as R};
use common::*;
use serde_json::json;
use std::sync::Arc;

fn assert_authority_loss(err: Error) {
    assert!(
        matches!(
            err,
            Error::StaleAuthority(_) | Error::InvalidAuthority(_) | Error::NotFound(_)
        ),
        "expected an authority-loss error, got {err:?}"
    );
}

/// #25/26/27/41: a positively RUNNING admitted Execution renews; the lease
/// expiry extends through Kernel policy and heartbeat_at is stamped; the
/// entry stays renewable.
#[test]
fn supervised_renewal_extends_lease_and_records_heartbeat() {
    let env = memory_env();
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("renew-ok"), false);
    assert_eq!(env.k.lease_seconds(), 10.0);
    let before = env.k.lease_supervision_view(&claim.attempt_id).unwrap();
    assert_eq!(before.state, LeaseState::Active);

    env.clock.advance(1.0);
    let now = env.k.now();
    let renewed = env
        .k
        .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
        .unwrap();
    assert_eq!(renewed, R::Renewed(now + 10.0));
    let after = env.k.lease_supervision_view(&claim.attempt_id).unwrap();
    assert_eq!(after.expires_at, now + 10.0);
    assert_eq!(after.heartbeat_at, now);
    assert_eq!(after.state, LeaseState::Active);

    // A later renewal keeps succeeding while authority holds.
    env.clock.advance(1.0);
    let now2 = env.k.now();
    assert_eq!(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap(),
        R::Renewed(now2 + 10.0)
    );
}

/// #28/29/30: renewal is fenced by the exact attempt_id + lease_epoch +
/// execution_id triple. A mismatched epoch, a foreign execution, or an
/// unknown execution never renews.
#[test]
fn supervised_renewal_requires_exact_attempt_epoch_and_execution() {
    // Capacity 2 so two concurrent RUNNING attempts can coexist.
    let clock = Arc::new(ManualClock::new(1_000_000.0));
    let k =
        Kernel::open_memory(clock.clone() as Arc<dyn Clock>, 10.0, CONTINUITY_MAX_BYTES).unwrap();
    k.upsert_partition(&PartitionSpec::new(
        "general",
        2,
        Retention::Resident,
        "local",
        "default",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();
    let env = Env { k, clock };

    let (_b1, _t1, claim1, exec1) = run_claim(&env.k, read_task("fence-a"), false);
    let (_b2, _t2, claim2, exec2) = run_claim(&env.k, read_task("fence-b"), false);

    // Wrong epoch on the same attempt.
    assert_authority_loss(
        env.k
            .renew_supervised_execution(
                &claim1.attempt_id,
                LeaseEpoch(claim1.lease_epoch.get() + 1),
                &exec1,
            )
            .unwrap_err(),
    );
    // Foreign execution (belongs to a different attempt).
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim1.attempt_id, claim1.lease_epoch, &exec2)
            .unwrap_err(),
    );
    // Unknown execution.
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim1.attempt_id, claim1.lease_epoch, &ExecutionId::new())
            .unwrap_err(),
    );
    // Wrong attempt with the right execution is still a rejection.
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim2.attempt_id, claim2.lease_epoch, &exec1)
            .unwrap_err(),
    );
    // The untouched execution still renews.
    assert!(matches!(
        env.k
            .renew_supervised_execution(&claim1.attempt_id, claim1.lease_epoch, &exec1)
            .unwrap(),
        R::Renewed(_)
    ));
}

/// #31/32: an expired Lease can never be revived — including at the exact
/// boundary `now == expires_at`, which frozen M4 authority validation treats
/// as stale.
#[test]
fn expired_lease_cannot_be_revived_even_at_exact_boundary() {
    let env = memory_env();
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("expire-boundary"), false);
    let before = env.k.lease_supervision_view(&claim.attempt_id).unwrap();

    env.clock.set(before.expires_at);
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );
    // Beyond the boundary stays stale as well.
    env.clock.advance(0.5);
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );
}

/// #33: after the lease expired and recovery expired the attempt, the stale
/// admission can never renew.
#[test]
fn expired_attempt_cannot_renew() {
    let env = memory_env();
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("stale-attempt"), false);
    env.clock.advance(11.0);
    env.k.expire_leases(false).unwrap();
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );
}

/// #34: a completed Task (authoritative success ACK) closes authority; the
/// admission can never renew.
#[test]
fn completed_task_cannot_renew() {
    let env = memory_env();
    let (_batch, task, claim, exec) = run_claim(&env.k, read_task("ack-closes"), false);
    env.k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&exec),
            &json!({"ok": true}),
            None,
            true,
            false,
        )
        .unwrap();
    assert!(kernel_task_completed(&env.k, &task));
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );
}

/// #35: a cancelled Task closes authority; the admission can never renew.
#[test]
fn cancelled_task_cannot_renew() {
    let env = memory_env();
    let (_batch, task, claim, exec) = run_claim(&env.k, read_task("cancel-closes"), false);
    env.k.cancel_task(&task, true).unwrap();
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );
}

/// #38/39: a non-RUNNING Execution behind still-valid Attempt/Lease authority
/// must not renew. The primitive reports NotRunning (drop supervision) —
/// never a renewal, never a durable-state repair. UNKNOWN and LOST are
/// reachable with authority intact (a STARTING execution refines to UNKNOWN;
/// a RUNNING execution refines to LOST through physical-history observation).
#[test]
fn non_running_executions_behind_valid_authority_report_not_running() {
    // STARTING (committed, never confirmed) and its UNKNOWN refinement.
    let env = memory_env();
    let (_batch, _ids, claim) = claim_only(&env.k, read_task("starting-drift"));
    let exec = env
        .k
        .create_execution(&claim, unisolated_launch_binding(&claim))
        .unwrap()
        .execution_id()
        .clone();
    assert_eq!(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap(),
        R::NotRunning
    );
    env.k
        .record_physical_outcome(
            &exec,
            ExecutionState::Unknown,
            Some(&json!({"h": 1})),
            None,
            None,
            false,
            false,
        )
        .unwrap();
    assert_eq!(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap(),
        R::NotRunning
    );
    let lease = env.k.lease_supervision_view(&claim.attempt_id).unwrap();
    assert_eq!(lease.state, LeaseState::Active, "no authority was mutated");
    assert_eq!(
        env.k.attempt(&claim.attempt_id).unwrap().state,
        AttemptState::Active
    );

    // LOST refinement of a confirmed RUNNING execution.
    let env = memory_env();
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("lost-drift"), false);
    env.k
        .record_physical_outcome(
            &exec,
            ExecutionState::Lost,
            Some(&json!({"h": 2})),
            None,
            None,
            false,
            false,
        )
        .unwrap();
    assert_eq!(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap(),
        R::NotRunning
    );
    assert_eq!(
        env.k.attempt(&claim.attempt_id).unwrap().state,
        AttemptState::Active
    );
}

fn claim_only(k: &Kernel, spec: TaskSpec) -> (BatchId, TaskId, Claim) {
    let (_batch, ids) = k.submit_batch(std::slice::from_ref(&spec)).unwrap();
    let claim = k.claim_next_available().unwrap().expect("claim");
    let task_id = ids.values().next().unwrap().clone();
    (_batch, task_id, claim)
}

/// #36/37/40: FAILED, SUCCEEDED, and TERMINATED Executions sit behind closed
/// Task authority — renewal fails stale, never revives.
#[test]
fn closed_executions_cannot_renew() {
    // FAILED via authoritative terminal NACK.
    let env = memory_env();
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("failed-closed"), false);
    env.k
        .nack(
            &claim.attempt_id,
            claim.lease_epoch,
            FailureClass::Timeout,
            Some(&exec),
            true,
            true,
            false,
        )
        .unwrap();
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );

    // SUCCEEDED via authoritative ACK.
    let env = memory_env();
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("succeeded-closed"), false);
    env.k
        .ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(&exec),
            &json!(null),
            None,
            true,
            false,
        )
        .unwrap();
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );

    // TERMINATED via cancellation with quiescence.
    let env = memory_env();
    let (_batch, task, claim, exec) = run_claim(&env.k, read_task("terminated-closed"), false);
    env.k.cancel_task(&task, true).unwrap();
    assert_authority_loss(
        env.k
            .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
            .unwrap_err(),
    );
}

/// #44: a corrupted durable lease row is a persistence fault during renewal —
/// never an authority loss the supervisor could treat as "this admission
/// expired". The caller must classify it as fatal (M5.3 §15).
#[test]
fn corrupted_lease_is_a_persistence_fault_not_authority_loss() {
    let db = FixtureDb::new("supervision-fault");
    let env = file_env(&db);
    let (_batch, _task, claim, exec) = run_claim(&env.k, read_task("fault-renew"), false);

    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.execute(
        "UPDATE leases SET epoch='not-an-integer' WHERE attempt_id=?1",
        rusqlite::params![claim.attempt_id.as_str()],
    )
    .unwrap();
    drop(conn);

    let err = env
        .k
        .renew_supervised_execution(&claim.attempt_id, claim.lease_epoch, &exec)
        .unwrap_err();
    assert!(
        !matches!(
            err,
            Error::StaleAuthority(_) | Error::InvalidAuthority(_) | Error::NotFound(_)
        ),
        "a storage fault must never classify as authority loss: {err:?}"
    );
}

/// P1-4 closure: Kernel lease authority must be finite and positive. NaN
/// passes `<= 0.0` comparisons and an infinite lease could never naturally
/// expire — both are rejected at construction, fail closed.
#[test]
fn kernel_rejects_non_finite_lease_seconds() {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1.0));
    for bad in [f64::NAN, f64::INFINITY] {
        let err = match Kernel::open_memory(clock.clone(), bad, 16_384) {
            Err(e) => e,
            Ok(_) => panic!("expected a construction rejection for {bad}"),
        };
        assert!(
            matches!(err, Error::InvalidTransition(_)),
            "expected a construction rejection for {bad}, got {err:?}"
        );
    }
}

fn kernel_task_completed(k: &Kernel, task: &TaskId) -> bool {
    k.task(task).unwrap().state == TaskState::Completed
}
