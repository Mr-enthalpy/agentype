//! M4 authority recovery and physical-history refinement (spec 16 §A / 14).
//! Crash-leftover history is constructed below the API boundary with storage
//! fixtures; every operation under test goes through the Kernel public API.

mod common;

use agentype_core::*;
use common::*;
use rusqlite::Connection;
use serde_json::json;

#[test]
fn restart_recovery_prevents_blind_duplicate_execution() {
    let db = FixtureDb::new("restart");
    let env = file_env(&db);
    let task_id;
    let attempt_id;
    {
        let k = &env.k;
        let (_b, ids) = k.submit_batch(&[retryable_write("restart")]).unwrap();
        task_id = ids["restart"].clone();
        let claim = k.claim_next_available().unwrap().unwrap();
        attempt_id = claim.attempt_id.clone();
        let _ = k.create_execution(&claim, false).unwrap();
    }
    env.clock.advance(20.0);
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
    let err = env.k.create_execution(&stale_claim, false).unwrap_err();
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

    let err = env
        .k
        .revive_agent(&agent, "local")
        .unwrap_err();
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
        env.k.incarnation(&IncarnationId::from_string(inc)).unwrap().state,
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
