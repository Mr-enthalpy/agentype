//! M5.4-B candidate reader (plan §5/§26 group A): persisted Execution
//! identity for restart reconciliation. The snapshot is a fact record, not
//! a grant — `state='RUNNING'` never mints admission.

mod common;

use agentype_core::*;
use agentype_execution_config::FrozenPhysicalExecutionBinding;
use agentype_storage_sqlite::Kernel;
use common::*;
use serde_json::json;
use std::sync::Arc;

fn capacity_env(n: i64) -> Env {
    let clock = Arc::new(ManualClock::new(1_000_000.0));
    let k =
        Kernel::open_memory(clock.clone() as Arc<dyn Clock>, 10.0, CONTINUITY_MAX_BYTES).unwrap();
    k.upsert_partition(&PartitionSpec::new(
        "general",
        n,
        Retention::Resident,
        "local",
        "default",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();
    Env { k, clock }
}

fn launch_with_kind(
    k: &Kernel,
    claim: &Claim,
    kind: &str,
) -> agentype_execution_config::ExecutionLaunchSnapshot {
    k.create_execution(
        claim,
        FrozenPhysicalExecutionBinding::new(unisolated_safety(claim), kind).unwrap(),
    )
    .unwrap()
}

/// #1/#2/#3: STARTING, UNKNOWN, and RUNNING candidates each carry the
/// persisted RequestId frozen at execution commitment.
#[test]
fn starting_unknown_and_running_candidates_carry_persisted_request_id() {
    let env = capacity_env(3);
    let k = &env.k;

    let (_b, _ids) = k
        .submit_batch(&[
            read_task("starting"),
            read_task("unknown"),
            read_task("running"),
        ])
        .unwrap();

    let claim_starting = k.claim_next_available().unwrap().unwrap();
    let launch_starting = k
        .create_execution(&claim_starting, unisolated_launch_binding(&claim_starting))
        .unwrap();

    let claim_unknown = k.claim_next_available().unwrap().unwrap();
    let launch_unknown = k
        .create_execution(&claim_unknown, unisolated_launch_binding(&claim_unknown))
        .unwrap();
    k.record_physical_outcome(
        launch_unknown.execution_id(),
        ExecutionState::Unknown,
        Some(&json!({"hint": "unknown"})),
        None,
        None,
        false,
        false,
    )
    .unwrap();

    let claim_running = k.claim_next_available().unwrap().unwrap();
    let launch_running = k
        .create_execution(&claim_running, unisolated_launch_binding(&claim_running))
        .unwrap();
    k.confirm_running_and_renew(
        &claim_running.attempt_id,
        claim_running.lease_epoch,
        launch_running.execution_id(),
        &json!({"hint": "running"}),
    )
    .unwrap();

    let candidates = k.reconciliation_candidates().unwrap();
    assert_eq!(candidates.len(), 3);

    let by_exec = |id: &ExecutionId| {
        candidates
            .iter()
            .find(|c| c.execution_id() == id)
            .unwrap()
            .clone()
    };

    let starting = by_exec(launch_starting.execution_id());
    assert_eq!(starting.persisted_state(), ExecutionState::Starting);
    assert_eq!(starting.request_id(), launch_starting.request_id());
    assert_eq!(starting.attempt_id(), &claim_starting.attempt_id);
    assert_eq!(starting.lease_epoch(), claim_starting.lease_epoch);

    let unknown = by_exec(launch_unknown.execution_id());
    assert_eq!(unknown.persisted_state(), ExecutionState::Unknown);
    assert_eq!(unknown.request_id(), launch_unknown.request_id());
    assert_eq!(unknown.runtime_handle(), &json!({"hint": "unknown"}));

    let running = by_exec(launch_running.execution_id());
    assert_eq!(running.persisted_state(), ExecutionState::Running);
    assert_eq!(running.request_id(), launch_running.request_id());
    assert_eq!(running.runtime_handle(), &json!({"hint": "running"}));
    assert!(running.current_authority_hint().looks_current());
}

/// #4/#5: recovery identity is the frozen `adapter_kind`. Current
/// target/profile configuration is not on the snapshot and is not consulted.
#[test]
fn candidate_uses_frozen_adapter_kind_not_current_target_profile() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("frozen-kind")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    // Partition target/profile stay "local"/"default"; the frozen routing
    // identity is independently "codex".
    let launch = launch_with_kind(&k, &claim, "codex");

    let partition = k.partition("general").unwrap();
    assert_eq!(partition.execution_target, "local");
    assert_eq!(partition.execution_profile, "default");

    let candidates = k.reconciliation_candidates().unwrap();
    assert_eq!(candidates.len(), 1);
    let snap = &candidates[0];
    assert_eq!(snap.execution_id(), launch.execution_id());
    assert_eq!(snap.adapter_kind(), "codex");
    assert_eq!(snap.request_id(), launch.request_id());
    // The snapshot type carries no current target/profile lookup result
    // (M5.4 plan §5 forbidden list); routing identity is adapter_kind only.
}

/// #6: blank or corrupt durable adapter identity fails closed. A single
/// corrupt row fails the whole read — internal durable uncertainty stops
/// the Scheduler (M5.4 plan §14).
#[test]
fn blank_or_corrupt_durable_adapter_identity_fails_closed() {
    let db = FixtureDb::new("blank-adapter");
    let env = file_env(&db);
    let (_b, _ids) = env.k.submit_batch(&[read_task("blank")]).unwrap();
    let claim = env.k.claim_next_available().unwrap().unwrap();
    let launch = env
        .k
        .create_execution(&claim, unisolated_launch_binding(&claim))
        .unwrap();

    fixture_execution_adapter_kind(&db, launch.execution_id(), "");
    let err = reopen(&env, &db).reconciliation_candidates().unwrap_err();
    assert!(
        matches!(err, Error::InvariantViolation(_)),
        "blank adapter_kind must fail closed, got {err:?}"
    );
    assert!(err.to_string().contains("blank durable adapter"));

    fixture_execution_adapter_kind(&db, launch.execution_id(), "   ");
    let err = reopen(&env, &db).reconciliation_candidates().unwrap_err();
    assert!(matches!(err, Error::InvariantViolation(_)));

    fixture_execution_adapter_kind(&db, launch.execution_id(), "test");
    fixture_corrupt_json(
        &db,
        "executions",
        "runtime_handle_json",
        launch.execution_id().as_str(),
    );
    let err = reopen(&env, &db).reconciliation_candidates().unwrap_err();
    assert!(
        matches!(err, Error::InvariantViolation(_)),
        "corrupt durable JSON must fail closed, got {err:?}"
    );
    assert!(err.to_string().contains("corrupted durable JSON"));
}

/// #7: the candidate reader is not an authority path. It does not renew,
/// does not produce a grant, and `looks_current` is only a routing hint.
#[test]
fn candidate_read_does_not_grant_or_renew_authority() {
    let Env { k, clock: _ } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("no-grant")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();
    let launch = k
        .create_execution(&claim, unisolated_launch_binding(&claim))
        .unwrap();
    k.confirm_running_and_renew(
        &claim.attempt_id,
        claim.lease_epoch,
        launch.execution_id(),
        &json!({"alive": true}),
    )
    .unwrap();

    let before = k.lease_supervision_view(&claim.attempt_id).unwrap();
    let candidates = k.reconciliation_candidates().unwrap();
    let after = k.lease_supervision_view(&claim.attempt_id).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(before.expires_at, after.expires_at);
    assert_eq!(before.heartbeat_at, after.heartbeat_at);
    assert_eq!(before.state, after.state);

    let snap = &candidates[0];
    assert!(snap.current_authority_hint().looks_current());
    assert_eq!(snap.persisted_state(), ExecutionState::Running);
    // Fact record, not a capability: Clone is legal and does not consume
    // anything (contrast SupervisionAdmission).
    let cloned = snap.clone();
    assert_eq!(cloned.request_id(), snap.request_id());
    assert_eq!(cloned.lease_epoch(), claim.lease_epoch);
}

/// Availability order (M5.4 plan §21): current-authority candidates, by
/// nearest lease expiry, then stale physical-history candidates. Correctness
/// does not depend on this order.
#[test]
fn current_authority_candidates_are_ordered_before_stale_history() {
    let env = capacity_env(2);
    let k = &env.k;
    let (_b, _ids) = k
        .submit_batch(&[read_task("current"), retryable_read("stale")])
        .unwrap();

    let claim_current = k.claim_next_available().unwrap().unwrap();
    let launch_current = k
        .create_execution(&claim_current, unisolated_launch_binding(&claim_current))
        .unwrap();
    k.confirm_running_and_renew(
        &claim_current.attempt_id,
        claim_current.lease_epoch,
        launch_current.execution_id(),
        &json!({"current": true}),
    )
    .unwrap();

    let claim_stale = k.claim_next_available().unwrap().unwrap();
    let launch_stale = k
        .create_execution(&claim_stale, unisolated_launch_binding(&claim_stale))
        .unwrap();
    k.nack(
        &claim_stale.attempt_id,
        claim_stale.lease_epoch,
        FailureClass::ExecutionLost,
        Some(launch_stale.execution_id()),
        false,
        false,
        false,
    )
    .unwrap();

    let candidates = k.reconciliation_candidates().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].execution_id(), launch_current.execution_id());
    assert!(candidates[0].current_authority_hint().looks_current());
    assert_eq!(candidates[1].execution_id(), launch_stale.execution_id());
    assert!(!candidates[1].current_authority_hint().looks_current());
}
