//! M4 pool-topology conformance: MOVE_CAPACITY / MERGE / RETIRE composition,
//! retention adoption, suspended identity, and lease-expiry ordering (spec
//! 11 / 16 §A). Topology is the historical bug-dense area; these regressions
//! pin convergence instead of "looks the same as V0.1".

mod common;

use agentype_core::*;
use common::*;
use serde_json::json;

fn partition(
    k: &agentype_storage_sqlite::Kernel,
    name: &str,
    capacity: i64,
    target: &str,
    retention: Retention,
) {
    k.upsert_partition(&PartitionSpec::new(
        name, capacity, retention, target, "default",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();
}

/// Both members of the general partition, ordered by id (fixture read).
fn ready_members(db: &FixtureDb) -> Vec<agentype_core::LogicalAgentId> {
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    let mut stmt = conn
        .prepare("SELECT id FROM logical_agents WHERE partition_name='general' AND state='READY' ORDER BY id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.into_iter().map(LogicalAgentId::from_string).collect()
}

#[test]
fn claim_prefers_oldest_available_since() {
    let db = FixtureDb::new("match-age");
    let env = file_env(&db);
    env.k.resize_partition("general", 2).unwrap();
    env.k.reconcile_pool().unwrap();

    let members = ready_members(&db);
    assert_eq!(members.len(), 2, "fixture needs two READY members");
    let older = members[1].clone();
    let newer = members[0].clone();

    // Force the ordering against the id tiebreak: the id-larger member is
    // older, so the frozen (available_since, id) order must still pick it.
    fixture_agent_available(&db, &older, 900_000.0);
    fixture_agent_available(&db, &newer, 1_100_000.0);

    let (_batch, _ids) = env.k.submit_batch(&[read_task("oldest-first")]).unwrap();
    let claim = env.k.claim_next_available().unwrap().unwrap();
    assert_eq!(
        claim.logical_agent_id, older,
        "oldest available_since must win the claim even when its id is larger"
    );
}

#[test]
fn claim_prefers_lowest_id_on_tied_availability() {
    let db = FixtureDb::new("match-tie");
    let env = file_env(&db);
    env.k.resize_partition("general", 2).unwrap();
    env.k.reconcile_pool().unwrap();

    let members = ready_members(&db);
    assert_eq!(members.len(), 2);
    // Tie the availability: the frozen tiebreak is lowest LogicalAgent ID.
    for m in &members {
        fixture_agent_available(&db, m, 1_000_000.0);
    }
    let expected = members[0].clone();

    let (_batch, _ids) = env.k.submit_batch(&[read_task("tie-break")]).unwrap();
    let claim = env.k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim.logical_agent_id, expected);
}

#[test]
fn assigned_topology_move_drains() {
    let Env { k, .. } = memory_env();
    partition(&k, "other", 0, "local", Retention::Resident);
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
fn merge_sums_desired_capacity() {
    let Env { k, .. } = memory_env();
    partition(&k, "extra", 2, "local", Retention::Resident);
    k.merge_partitions("extra", "general").unwrap();
    let general = k.partition("general").unwrap();
    assert_eq!(general.desired_capacity, 3);
    assert!(!k.partition("extra").unwrap().active);
}

#[test]
fn merge_migrates_future_task_classification() {
    let Env { k, .. } = memory_env();
    partition(&k, "src", 1, "local", Retention::Resident);
    let (_b, ids) = k
        .submit_batch(&[read_task("queued-elsewhere").partition("src")])
        .unwrap();
    k.merge_partitions("src", "general").unwrap();
    let task = k.task(&ids["queued-elsewhere"]).unwrap();
    assert_eq!(task.partition.as_str(), "general");
    assert_eq!(task.state, TaskState::Queued);
}

#[test]
fn claim_on_source_then_merge_before_execution_preserves_frozen_target() {
    let Env { k, .. } = memory_env();
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
    assert_eq!(claim.execution_profile, "profile-b");

    // MERGE partition src -> general
    k.merge_partitions("src", "general").unwrap();

    let attempt = k.attempt(&claim.attempt_id).unwrap();
    assert_eq!(attempt.state, AttemptState::Active);
    assert_eq!(attempt.execution_target, "local-b");
    assert_eq!(attempt.execution_profile, "profile-b");

    let lease = k.lease_for_attempt(&claim.attempt_id).unwrap();
    assert_eq!(lease.state, LeaseState::Active);
    assert_eq!(lease.epoch, claim.lease_epoch);

    // create_execution MUST succeed even though task.partition is now "general"
    let launch = k
        .create_execution(&claim, unisolated_launch_binding(&claim))
        .unwrap();
    let exec_id = launch.execution_id().clone();
    let exec = k.execution(&exec_id).unwrap();
    assert_eq!(exec.execution_target, "local-b");
    assert_eq!(exec.execution_profile, "profile-b");
    assert_eq!(launch.execution_target(), "local-b");
    assert_eq!(launch.execution_profile(), "profile-b");

    k.confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, &exec_id, &json!({}))
        .unwrap();

    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&exec_id),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
}

#[test]
fn tampered_claim_target_or_profile_rejected() {
    let Env { k, .. } = memory_env();
    let (_b, _ids) = k.submit_batch(&[read_task("t1")]).unwrap();
    let claim = k.claim_next_available().unwrap().unwrap();

    let mut tampered_target = claim.clone();
    tampered_target.execution_target = "forged-target".to_string();
    assert!(k
        .create_execution(
            &tampered_target,
            unisolated_launch_binding(&tampered_target)
        )
        .is_err());

    let mut tampered_profile = claim.clone();
    tampered_profile.execution_profile = "forged-profile".to_string();
    assert!(k
        .create_execution(
            &tampered_profile,
            unisolated_launch_binding(&tampered_profile)
        )
        .is_err());
}

#[test]
fn retry_after_merged_attempt_uses_new_partition_target() {
    let Env { k, clock } = memory_env();
    k.upsert_partition(&PartitionSpec::new(
        "src",
        1,
        Retention::Resident,
        "local-b",
        "profile-b",
    ))
    .unwrap();
    k.reconcile_pool().unwrap();

    let (_b, ids) = k
        .submit_batch(&[retryable_read("live").partition("src")])
        .unwrap();
    let task_id = &ids["live"];

    // Claim Attempt 1 under partition src
    let claim1 = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim1.execution_target, "local-b");
    assert_eq!(claim1.execution_profile, "profile-b");

    // Merge src -> general (general has target="local", profile="default")
    k.merge_partitions("src", "general").unwrap();

    // Attempt 1 execution succeeds under frozen local-b/profile-b
    let launch1 = k
        .create_execution(&claim1, unisolated_launch_binding(&claim1))
        .unwrap();
    let exec_id1 = launch1.execution_id().clone();
    let exec1 = k.execution(&exec_id1).unwrap();
    assert_eq!(exec1.execution_target, "local-b");
    assert_eq!(exec1.execution_profile, "profile-b");
    assert_eq!(launch1.execution_target(), "local-b");
    assert_eq!(launch1.execution_profile(), "profile-b");

    // Attempt 1 fails with retryable TIMEOUT failure
    k.confirm_running_and_renew(
        &claim1.attempt_id,
        claim1.lease_epoch,
        &exec_id1,
        &json!({}),
    )
    .unwrap();
    k.nack(
        &claim1.attempt_id,
        claim1.lease_epoch,
        FailureClass::Timeout,
        Some(&exec_id1),
        true,
        true,
        false,
    )
    .unwrap();

    // Task transitions to RETRY_WAIT and its scheduling partition is general
    let task = k.task(task_id).unwrap();
    assert_eq!(task.state, TaskState::RetryWait);
    assert_eq!(task.partition.as_str(), "general");

    clock.advance(10.0);
    k.promote_retry_wait().unwrap();
    assert_eq!(k.task(task_id).unwrap().state, TaskState::Queued);

    // Claim Attempt 2: must pick up migrated partition target and profile ("local", "default")
    let claim2 = k.claim_next_available().unwrap().unwrap();
    assert_eq!(claim2.execution_target, "local");
    assert_eq!(claim2.execution_profile, "default");

    let launch2 = k
        .create_execution(&claim2, unisolated_launch_binding(&claim2))
        .unwrap();
    let exec_id2 = launch2.execution_id().clone();
    let exec2 = k.execution(&exec_id2).unwrap();
    assert_eq!(exec2.execution_target, "local");
    assert_eq!(exec2.execution_profile, "default");
    assert_eq!(launch2.execution_target(), "local");
    assert_eq!(launch2.execution_profile(), "default");
}

#[test]
fn retire_rejects_nonterminal_task() {
    let Env { k, .. } = memory_env();
    k.submit_batch(&[read_task("still-open")]).unwrap();
    let err = k.retire_partition("general").unwrap_err();
    assert!(err.to_string().contains("nonterminal"));
}

/// Semantic retirement fences every live Incarnation of the agent in the same
/// transaction — here with a fixture-placed live presence.
#[test]
fn semantic_retirement_fences_live_incarnation_lost() {
    let db = FixtureDb::new("fence-live-inc");
    let env = file_env(&db);
    let agent = env.k.ready_agent("general").unwrap();
    let inc = fixture_incarnation(&db, &agent, 1, "local", "WARM");
    env.k.resize_partition("general", 0).unwrap();
    assert_eq!(env.k.reconcile_pool().unwrap().retired, 1);
    assert_eq!(
        env.k.logical_agent(&agent).unwrap().state,
        LogicalAgentState::Retired
    );
    assert_eq!(
        env.k
            .incarnation(&IncarnationId::from_string(&inc))
            .unwrap()
            .state,
        IncarnationState::Lost
    );
    // No scheduler-authoritative live presence remains.
    assert!(!env
        .k
        .incarnation(&IncarnationId::from_string(&inc))
        .unwrap()
        .state
        .is_live_presence());
}

/// Assignment-boundary retirement: a DRAINING assigned member retires only at
/// the release boundary, and its live Incarnation is fenced LOST in that same
/// transaction (spec 13 storage invariant).
#[test]
fn assignment_boundary_retirement_fences_incarnation_at_release() {
    let db = FixtureDb::new("assign-fence");
    let env = file_env(&db);
    let (_b, _task_id, claim, exec) = run_claim(&env.k, read_task("boundary"), false);

    env.k.resize_partition("general", 0).unwrap();
    let report = env.k.reconcile_pool().unwrap();
    assert_eq!(report.draining, 1);
    let agent = env.k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.state, LogicalAgentState::Draining);
    assert!(agent.retirement_requested);

    // Release boundary: ack completes the task and retires+fences in one tx.
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
    let agent = env.k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.state, LogicalAgentState::Retired);
    // The release boundary never leaves a scheduler-authoritative live
    // Incarnation behind: a quiescent ack already terminated it, and any
    // still-live presence would be fenced LOST by the retirement transaction.
    let incarnation = env
        .k
        .attempt(&claim.attempt_id)
        .unwrap()
        .incarnation_id
        .unwrap();
    assert!(!env
        .k
        .incarnation(&incarnation)
        .unwrap()
        .state
        .is_live_presence());
}

/// Consecutive MOVE_CAPACITY rebases an assigned member's pending destination:
/// general -> mid -> final; release lands it in the latest destination.
#[test]
fn consecutive_move_rebases_pending_destination() {
    let Env { k, .. } = memory_env();
    partition(&k, "mid", 0, "local", Retention::Resident);
    partition(&k, "final", 0, "local", Retention::Resident);

    let (_b, task_id, claim, exec) = run_claim(&k, read_task("moving"), false);
    k.move_capacity("general", "mid", 1).unwrap();
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(
        agent.pending_partition.as_ref().map(|p| p.as_str()),
        Some("mid")
    );

    k.move_capacity("mid", "final", 1).unwrap();
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(
        agent.pending_partition.as_ref().map(|p| p.as_str()),
        Some("final"),
        "second move must rebase the pending destination"
    );

    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&exec),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(agent.partition.as_str(), "final");
    assert!(agent.pending_partition.is_none());
    let _ = task_id;
}

/// MERGE also rebases pending destinations of draining members.
#[test]
fn merge_rebases_pending_destination_of_draining_member() {
    let Env { k, .. } = memory_env();
    partition(&k, "mid", 0, "local", Retention::Resident);
    partition(&k, "dst", 0, "local", Retention::Resident);

    let (_b, _task_id, claim, exec) = run_claim(&k, read_task("merging"), false);
    k.move_capacity("general", "mid", 1).unwrap();
    assert_eq!(
        k.logical_agent(&claim.logical_agent_id)
            .unwrap()
            .pending_partition
            .as_ref()
            .map(|p| p.as_str()),
        Some("mid")
    );

    k.merge_partitions("mid", "dst").unwrap();
    let agent = k.logical_agent(&claim.logical_agent_id).unwrap();
    assert_eq!(
        agent.pending_partition.as_ref().map(|p| p.as_str()),
        Some("dst"),
        "merge must rebase pending destination onto the survivor"
    );

    k.ack_success(
        &claim.attempt_id,
        claim.lease_epoch,
        Some(&exec),
        &json!({"ok": true}),
        None,
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        k.logical_agent(&claim.logical_agent_id)
            .unwrap()
            .partition
            .as_str(),
        "dst"
    );
}

/// A released member adopts the destination partition's retention; ephemeral
/// adoption ends the physical membership at the next release boundary.
#[test]
fn target_retention_adoption_on_cutover_and_release() {
    let db = FixtureDb::new("retention");
    let env = file_env(&db);
    partition(&env.k, "eph", 1, "local", Retention::Ephemeral);

    // Idle READY member is cut over immediately by MOVE_CAPACITY.
    let agent = env.k.ready_agent("general").unwrap();
    env.k.move_capacity("general", "eph", 1).unwrap();
    let moved = env.k.logical_agent(&agent).unwrap();
    assert_eq!(moved.partition.as_str(), "eph");
    assert_eq!(moved.retention, Retention::Ephemeral);

    // Next release boundary on the ephemeral partition retires the member.
    let (_b, _task_id, claim, exec) =
        run_claim(&env.k, read_task("eph-work").partition("eph"), false);
    assert_eq!(claim.execution_target.as_str(), "local");
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
    assert_eq!(
        env.k.logical_agent(&claim.logical_agent_id).unwrap().state,
        LogicalAgentState::Retired
    );
}

/// SUSPENDED identity survives temporary replacement: reconcile births a
/// replacement for the missing capacity while the safety-obligation holder
/// keeps its identity; after the obligation resolves the holder revives and
/// the pool converges without ever silently discarding it.
#[test]
fn suspended_identity_survives_temporary_replacement_convergence() {
    let Env { k, clock } = memory_env();
    let (_b, task_id, claim, _exec) = run_claim(&k, retryable_write("holder"), false);
    let original = claim.logical_agent_id.clone();

    // Make room so the replacement can coexist once the holder revives.
    k.resize_partition("general", 2).unwrap();
    k.reconcile_pool().unwrap(); // births the temporary replacement
    assert!(k
        .logical_agent(&original)
        .unwrap()
        .current_task_id
        .is_some());

    clock.advance(20.0);
    k.expire_leases(false).unwrap(); // writer quiescence unknown
    assert_eq!(
        k.logical_agent(&original).unwrap().state,
        LogicalAgentState::Suspended
    );

    // The suspended identity itself is still tracked (not silently discarded).
    assert_eq!(
        k.logical_agent(&original).unwrap().partition.as_str(),
        "general"
    );

    // Resolve the obligation with the retry primitive (the task is SUSPENDED,
    // not CANCELLED), then converge through normal recovery.
    let esc = k.open_escalation_for_task(&task_id).unwrap();
    k.resolve_escalation(&esc.id, "retry", true).unwrap();
    k.recover_authority().unwrap();

    let revived = k.logical_agent(&original).unwrap();
    assert_eq!(revived.state, LogicalAgentState::Ready);
    assert_eq!(revived.partition.as_str(), "general");
    assert!(revived.current_task_id.is_none());
    assert_eq!(
        k.task(&task_id).unwrap().state,
        TaskState::Queued,
        "retry returns the task to the queue for a fresh claim"
    );
}

/// MOVE before expiry and expiry before MOVE both converge to: task
/// suspended behind a writer-safety decision, attempt/lease expired, agent
/// out of the source partition.
#[test]
fn move_and_lease_expiry_converge_in_both_orders() {
    // Order A: expire first, then MOVE.
    {
        let Env { k, clock } = memory_env();
        partition(&k, "other", 1, "local", Retention::Resident);
        let (_b, task_a, claim_a, _exec) = run_claim(&k, retryable_write("order-a"), false);
        clock.advance(20.0);
        let report = k.expire_leases(false).unwrap();
        assert_eq!(report.suspended, 1);
        k.move_capacity("general", "other", 1).unwrap();
        let agent = k.logical_agent(&claim_a.logical_agent_id).unwrap();
        assert_eq!(agent.state, LogicalAgentState::Suspended);
        assert_eq!(agent.partition.as_str(), "other");
        assert_eq!(k.task(&task_a).unwrap().state, TaskState::Suspended);
        assert_eq!(
            k.attempt(&claim_a.attempt_id).unwrap().state,
            AttemptState::Expired
        );

        // Resolve through the same primitives as order B: identical end state.
        let esc = k.open_escalation_for_task(&task_a).unwrap();
        k.resolve_escalation(&esc.id, "retry", true).unwrap();
        k.recover_authority().unwrap();
        let agent = k.logical_agent(&claim_a.logical_agent_id).unwrap();
        assert_eq!(agent.state, LogicalAgentState::Ready);
        assert_eq!(agent.partition.as_str(), "other");
    }
    // Order B: MOVE first, then expire.
    {
        let Env { k, clock } = memory_env();
        partition(&k, "other", 1, "local", Retention::Resident);
        let (_b, task_b, claim_b, _exec) = run_claim(&k, retryable_write("order-b"), false);
        k.move_capacity("general", "other", 1).unwrap();
        let draining = k.logical_agent(&claim_b.logical_agent_id).unwrap();
        assert_eq!(draining.state, LogicalAgentState::Draining);
        clock.advance(20.0);
        k.expire_leases(false).unwrap();
        let agent = k.logical_agent(&claim_b.logical_agent_id).unwrap();
        assert_eq!(agent.state, LogicalAgentState::Suspended);
        assert_eq!(
            agent.pending_partition.as_ref().map(|p| p.as_str()),
            Some("other"),
            "pending cutover survives suspension until the safety decision"
        );
        assert_eq!(k.task(&task_b).unwrap().state, TaskState::Suspended);

        // Resolution commits the pending cutover: same end state as order A.
        let esc = k.open_escalation_for_task(&task_b).unwrap();
        k.resolve_escalation(&esc.id, "retry", true).unwrap();
        k.recover_authority().unwrap();
        let agent = k.logical_agent(&claim_b.logical_agent_id).unwrap();
        assert_eq!(agent.partition.as_str(), "other");
        assert_eq!(agent.state, LogicalAgentState::Ready);
    }
}
