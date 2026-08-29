//! Agentype domain types, closed state machines, and authority predicates.
//!
//! This crate MUST NOT depend on SQLite, Tokio, vendors, CLI, or env vars.

mod authority;
mod clock;
pub mod config;
mod decisions;
mod errors;
mod ids;
mod records;
mod states;

pub use authority::{
    completed_task_must_not_reopen, physical_transition_allowed, require_physical_transition,
    tags_match, task_create_establishes_authority, unavailable_configuration_failure,
    validate_authority, writer_is_safe_to_replace, AuthoritySnapshot,
};
pub use clock::{Clock, ManualClock, SystemClock, UnixTime};
pub use config::*;
pub use decisions::{
    agent_release_disposition, batch_next_state, claim_selection_rank, claim_task_eligible,
    claim_tiebreak, cross_target_cutover_safety, dependency_release_decision, durable_quiescence,
    excess_disposition, excess_rank_key, incarnation_presence, is_cross_target_execution_safe,
    move_candidate_eligible, move_rank_key, order_claim_tasks, partition_cutover_plan,
    plan_dependency_releases, plan_escalation_resolution, plan_move_cutover,
    post_safety_agent_disposition, retry_allowed, retry_backoff_seconds, select_claim_agent,
    sort_excess_candidates, sort_move_candidates, suspension_failure_class,
    AgentReleaseDisposition, BlockedTaskSnapshot, ClaimAgentSnapshot, ClaimIntent,
    ClaimTaskSnapshot, CrossTargetExecutionSnapshot, EscalatedWriterPresenceAction,
    EscalationOperation, EscalationResolutionPlan, EscalationResolutionSnapshot, ExcessDisposition,
    MoveCutoverPlan, PartitionCutoverDisposition, PoolMemberSnapshot, PostSafetyAgentDisposition,
    PresenceAction,
};
pub use errors::Error;
pub use ids::*;
pub use records::*;
pub use states::*;

pub type CoreResult<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_may_become_running() {
        assert!(physical_transition_allowed(
            ExecutionState::Unknown,
            ExecutionState::Running
        ));
        assert!(physical_transition_allowed(
            ExecutionState::Lost,
            ExecutionState::Terminated
        ));
        assert!(!physical_transition_allowed(
            ExecutionState::Terminated,
            ExecutionState::Running
        ));
    }

    #[test]
    fn expired_lease_is_stale_before_sweeper() {
        let attempt = AttemptId::new();
        let snap = AuthoritySnapshot {
            attempt_id: attempt.clone(),
            attempt_state: AttemptState::Active,
            lease_state: LeaseState::Active,
            lease_epoch: LeaseEpoch(1),
            lease_expires_at: 10.0,
            task_current_attempt_id: Some(attempt),
            task_fencing_epoch: LeaseEpoch(1),
        };
        assert!(validate_authority(&snap, LeaseEpoch(1), 10.0).is_err());
        assert!(validate_authority(&snap, LeaseEpoch(1), 9.9).is_ok());
    }

    #[test]
    fn writer_safety_ignores_omitted_execution_when_row_exists() {
        assert!(!writer_is_safe_to_replace(true, true, false, false));
        assert!(writer_is_safe_to_replace(true, false, false, false));
        assert!(writer_is_safe_to_replace(true, true, false, true));
        assert!(writer_is_safe_to_replace(false, true, false, false));
    }

    #[test]
    fn retry_backoff_is_bounded() {
        let p = RetryPolicy {
            max_attempts: 8,
            retry_classes: vec![FailureClass::Timeout],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        };
        assert_eq!(p.delay_for_attempt(1), 1.0);
        assert_eq!(p.delay_for_attempt(2), 2.0);
        assert_eq!(p.delay_for_attempt(4), 8.0);
        assert_eq!(p.delay_for_attempt(10), 8.0);
        assert!(!p.allows(FailureClass::Unknown, 1));
        assert!(p.allows(FailureClass::Timeout, 1));
        assert!(!p.allows(FailureClass::Timeout, 8));
    }

    #[test]
    fn claim_rank_prefers_workstream_continuity() {
        use decisions::*;
        assert_eq!(
            claim_selection_rank(ContinuityPreference::Required, true),
            0
        );
        assert_eq!(
            claim_selection_rank(ContinuityPreference::Preferred, false),
            1
        );
        // Plain "none" continuity is generic placement even on same workstream.
        assert_eq!(claim_selection_rank(ContinuityPreference::None, true), 1);
    }

    #[test]
    fn claim_tiebreak_is_oldest_availability_then_lowest_id() {
        use decisions::*;
        let older = claim_tiebreak(Some(900.0), 1000.0, "zzz");
        let newer = claim_tiebreak(Some(1100.0), 900.0, "aaa");
        assert!(older < newer, "availability must dominate the id tiebreak");
        let tied_low = claim_tiebreak(Some(5.0), 5.0, "a");
        let tied_high = claim_tiebreak(Some(5.0), 5.0, "b");
        assert!(tied_low < tied_high);
        // NULL availability falls back to created_at.
        assert_eq!(claim_tiebreak(None, 42.0, "x").0, 42.0);
    }

    #[test]
    fn durable_quiescence_requires_terminality() {
        use decisions::*;
        assert!(!durable_quiescence(false, true));
        assert!(!durable_quiescence(true, false));
        assert!(durable_quiescence(true, true));
    }

    #[test]
    fn suspension_class_escapes_to_writer_safety() {
        use decisions::*;
        assert_eq!(
            suspension_failure_class(true, FailureClass::ExecutionLost),
            FailureClass::ExecutionLost
        );
        assert_eq!(
            suspension_failure_class(false, FailureClass::ResourceUnavailable),
            FailureClass::WriterQuiescenceUnknown
        );
    }

    #[test]
    fn batch_aggregate_states() {
        use decisions::*;
        assert_eq!(batch_next_state(true, false, false), BatchState::Suspended);
        assert_eq!(batch_next_state(false, true, false), BatchState::Suspended);
        assert_eq!(batch_next_state(false, false, false), BatchState::Active);
        assert_eq!(batch_next_state(false, false, true), BatchState::Completed);
    }

    #[test]
    fn excess_disposition_retires_only_idle_unassigned() {
        use decisions::*;
        for state in [
            LogicalAgentState::Ready,
            LogicalAgentState::Initializing,
            LogicalAgentState::Reviving,
        ] {
            assert_eq!(
                excess_disposition(state, false),
                ExcessDisposition::RetireDirectly
            );
            assert_eq!(
                excess_disposition(state, true),
                ExcessDisposition::DrainForRetirement
            );
        }
        assert_eq!(
            excess_disposition(LogicalAgentState::Assigned, true),
            ExcessDisposition::DrainForRetirement
        );
        assert_eq!(
            excess_disposition(LogicalAgentState::Suspended, false),
            ExcessDisposition::DrainForRetirement
        );
    }

    #[test]
    fn incarnation_presence_actions_match_oracle_branches() {
        use decisions::*;
        // Proof of life promotes warmth.
        assert_eq!(
            incarnation_presence(ExecutionState::Running, false, false, false),
            PresenceAction::PromoteWarm
        );
        // Non-terminal observations never touch a fenced presence.
        for s in [ExecutionState::Starting, ExecutionState::Unknown] {
            assert_eq!(
                incarnation_presence(s, false, false, false),
                PresenceAction::Ignore
            );
        }
        // Declared-reusable quiet end keeps the presence warm.
        assert_eq!(
            incarnation_presence(ExecutionState::Succeeded, true, true, true),
            PresenceAction::PromoteWarm
        );
        // Confirmed quiet end (not from LOST) terminates.
        assert_eq!(
            incarnation_presence(ExecutionState::Succeeded, true, true, false),
            PresenceAction::FenceTerminated
        );
        // LOST can never be a confirmed end; unproven ends stay LOST.
        assert_eq!(
            incarnation_presence(ExecutionState::Lost, true, true, false),
            PresenceAction::FenceLost
        );
        assert_eq!(
            incarnation_presence(ExecutionState::Failed, true, false, false),
            PresenceAction::FenceLost
        );
        assert_eq!(
            incarnation_presence(ExecutionState::Terminated, true, true, false),
            PresenceAction::FenceTerminated
        );
    }

    fn task_snap(id: &str, priority: i64, created_at: f64) -> decisions::ClaimTaskSnapshot {
        decisions::ClaimTaskSnapshot {
            id: id.to_string(),
            state: TaskState::Queued,
            batch_state: BatchState::Active,
            partition_active: true,
            next_eligible_at: None,
            priority,
            created_at,
        }
    }

    #[test]
    fn claim_task_ordering_and_eligibility_are_semantic() {
        use decisions::*;
        let now = 100.0;
        let mut high = task_snap("b", 5, 1.0);
        let low = task_snap("a", 1, 0.0);
        let suspended = task_snap("c", 9, 0.0);
        suspended_state(&mut high);

        // Highest priority first regardless of id/created_at; ineligible rows
        // never surface even with extreme priority.
        let order = order_claim_tasks(&[high.clone(), low.clone(), task_snap("d", 7, 0.0)], now);
        assert_eq!(order, vec!["d".to_string(), "a".to_string()]);

        // A queued task whose next_eligible_at is still in the future is not
        // claimable yet (backoff), independent of SQL text.
        let mut delayed = task_snap("e", 99, 0.0);
        delayed.next_eligible_at = Some(now + 1.0);
        assert!(!claim_task_eligible(&delayed, now));
        // Suspended batch / inactive partition / LEASED state all reject.
        let mut s = task_snap("f", 99, 0.0);
        s.batch_state = BatchState::Suspended;
        assert!(!claim_task_eligible(&s, now));
        let mut g = task_snap("g", 99, 0.0);
        g.partition_active = false;
        assert!(!claim_task_eligible(&g, now));
        let mut h = task_snap("h", 99, 0.0);
        h.state = TaskState::Leased;
        assert!(!claim_task_eligible(&h, now));
        let _ = suspended;
    }

    fn suspended_state(t: &mut decisions::ClaimTaskSnapshot) {
        t.state = TaskState::Leased;
    }

    #[test]
    fn claim_agent_selection_is_frozen_order_over_snapshot() {
        use decisions::*;
        let intent_defaults = |partition: &'static str| ClaimIntent {
            partition,
            required_tags: &[],
            workstream_id: None,
            continuity: ContinuityPreference::None,
        };
        let agent = |id: &str, avail: f64| ClaimAgentSnapshot {
            id: id.to_string(),
            state: LogicalAgentState::Ready,
            assigned_to_task: false,
            partition: "general".to_string(),
            workstream_id: None,
            tags: vec![],
            available_since: Some(avail),
            created_at: 0.0,
        };
        // Oldest availability wins even when its id sorts larger.
        let agents = vec![agent("zzz-older", 10.0), agent("aaa-newer", 20.0)];
        let picked = select_claim_agent(&agents, &intent_defaults("general")).unwrap();
        assert_eq!(picked.id, "zzz-older");
        // Tied availability falls back to lowest id.
        let tied = vec![agent("b", 5.0), agent("a", 5.0)];
        assert_eq!(
            select_claim_agent(&tied, &intent_defaults("general"))
                .unwrap()
                .id,
            "a"
        );

        // Wrong partition is invisible to the selector.
        let mut other = agent("x", 0.0);
        other.partition = "elsewhere".to_string();
        assert!(
            select_claim_agent(std::slice::from_ref(&other), &intent_defaults("general")).is_none()
        );

        // Assigned members and non-READY states are not consumers.
        let mut busy = agent("busy", 0.0);
        busy.assigned_to_task = true;
        assert!(
            select_claim_agent(std::slice::from_ref(&busy), &intent_defaults("general")).is_none()
        );
        let mut reviving = agent("rev", 0.0);
        reviving.state = LogicalAgentState::Reviving;
        assert!(
            select_claim_agent(std::slice::from_ref(&reviving), &intent_defaults("general"))
                .is_none()
        );

        // Required tag missing rejects; subset accepts.
        let tagged_intent = ClaimIntent {
            partition: "general",
            required_tags: &["gpu".to_string()],
            workstream_id: None,
            continuity: ContinuityPreference::None,
        };
        let mut plain = agent("plain", 0.0);
        plain.tags = vec!["cpu".to_string()];
        let mut capable = agent("gpu-one", 0.0);
        capable.tags = vec!["cpu".to_string(), "gpu".to_string()];
        let pool2 = vec![plain, capable];
        assert_eq!(
            select_claim_agent(&pool2, &tagged_intent).unwrap().id,
            "gpu-one"
        );

        // Required continuity across a different workstream rejects outright.
        let mut ws_agent = agent("ws", 0.0);
        ws_agent.workstream_id = Some("w2".to_string());
        let strict = ClaimIntent {
            partition: "general",
            required_tags: &[],
            workstream_id: Some("w1"),
            continuity: ContinuityPreference::Required,
        };
        assert!(select_claim_agent(std::slice::from_ref(&ws_agent), &strict).is_none());
    }

    #[test]
    fn pool_rankings_match_v01_parity() {
        use decisions::*;
        let member =
            |id: &str, state: LogicalAgentState, assigned: bool, avail: f64| PoolMemberSnapshot {
                id: id.to_string(),
                state,
                assigned_to_task: assigned,
                retirement_requested: false,
                available_since: Some(avail),
                created_at: 0.0,
            };
        // Excess: unassigned before assigned, READY before others, then id.
        let mut excess = vec![
            member("m3", LogicalAgentState::Reviving, false, 5.0),
            member("m4", LogicalAgentState::Assigned, true, 0.0),
            member("m2", LogicalAgentState::Ready, false, 9.0),
            member("m1", LogicalAgentState::Ready, false, 1.0),
        ];
        sort_excess_candidates(&mut excess);
        let ids: Vec<&str> = excess.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["m1", "m2", "m3", "m4"]);

        // Move: idle READY first, then oldest availability, then id.
        let mut move_pool = vec![
            member("x9", LogicalAgentState::Suspended, false, 1.0),
            member("x8", LogicalAgentState::Ready, true, 0.0),
            member("x7", LogicalAgentState::Ready, false, 50.0),
        ];
        sort_move_candidates(&mut move_pool);
        let ids: Vec<&str> = move_pool.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["x7", "x8", "x9"],
            "idle READY outranks; rest by availability"
        );
        assert!(move_candidate_eligible(&move_pool[0]));
    }

    #[test]
    fn move_cutover_plan_stages_only_busy_members() {
        use decisions::*;
        // Anything holding (or handing off) a Task stages under DRAINING.
        assert_eq!(
            plan_move_cutover(LogicalAgentState::Assigned, true),
            MoveCutoverPlan::StageDrain
        );
        assert_eq!(
            plan_move_cutover(LogicalAgentState::Reviving, true),
            MoveCutoverPlan::StageDrain
        );
        // Idle DRAINING reconnects and restores availability.
        assert_eq!(
            plan_move_cutover(LogicalAgentState::Draining, false),
            MoveCutoverPlan::ReconnectCutover {
                restore_ready: true
            }
        );
        // Plain idle members just cut over.
        assert_eq!(
            plan_move_cutover(LogicalAgentState::Ready, false),
            MoveCutoverPlan::ReconnectCutover {
                restore_ready: false
            }
        );
        assert_eq!(
            plan_move_cutover(LogicalAgentState::Suspended, false),
            MoveCutoverPlan::ReconnectCutover {
                restore_ready: false
            }
        );
    }

    #[test]
    fn cross_target_cutover_safety_rules() {
        use decisions::*;
        let safe_exec = CrossTargetExecutionSnapshot {
            attempt_state: AttemptState::Failed,
            lease_state: Some(LeaseState::Expired),
            lease_expires_at: Some(5.0),
            workspace_mode: WorkspaceMode::ReadOnly,
            attempt_isolation: false,
            quiescent_confirmed: false,
        };
        assert!(is_cross_target_execution_safe(&safe_exec, 10.0));
        assert!(cross_target_cutover_safety(
            std::slice::from_ref(&safe_exec),
            10.0
        ));

        // Active attempt is unsafe.
        let mut active_attempt = safe_exec.clone();
        active_attempt.attempt_state = AttemptState::Active;
        active_attempt.lease_state = Some(LeaseState::Active);
        active_attempt.lease_expires_at = Some(20.0);
        assert!(!is_cross_target_execution_safe(&active_attempt, 10.0));

        // Active lease unexpired is unsafe even if attempt is non-active.
        let mut active_lease = safe_exec.clone();
        active_lease.lease_state = Some(LeaseState::Active);
        active_lease.lease_expires_at = Some(20.0);
        assert!(!is_cross_target_execution_safe(&active_lease, 10.0));

        // Write workspace without isolation or confirmed quiescence is unsafe.
        let mut write_unsafe = safe_exec.clone();
        write_unsafe.workspace_mode = WorkspaceMode::Write;
        assert!(!is_cross_target_execution_safe(&write_unsafe, 10.0));

        // Write workspace with isolation is safe.
        let mut write_isolated = write_unsafe.clone();
        write_isolated.attempt_isolation = true;
        assert!(is_cross_target_execution_safe(&write_isolated, 10.0));

        // Write workspace with confirmed quiescence is safe.
        let mut write_quiescent = write_unsafe;
        write_quiescent.quiescent_confirmed = true;
        assert!(is_cross_target_execution_safe(&write_quiescent, 10.0));
    }

    #[test]
    fn partition_cutover_plan_dispositions() {
        use decisions::*;
        // Active attempt rejects immediately with drain required.
        assert_eq!(
            partition_cutover_plan(LogicalAgentState::Assigned, true, true, true),
            PartitionCutoverDisposition::RejectAssignedDrainRequired
        );
        // Unsafe cross-target execution on idle suspended stages pending destination.
        assert_eq!(
            partition_cutover_plan(LogicalAgentState::Suspended, false, false, false),
            PartitionCutoverDisposition::StagePendingDestination
        );
        // Unsafe cross-target execution on non-suspended rejects.
        assert_eq!(
            partition_cutover_plan(LogicalAgentState::Ready, false, false, false),
            PartitionCutoverDisposition::RejectUnsafeExecution
        );
        // Unsafe cross-target execution on suspended with current task rejects.
        assert_eq!(
            partition_cutover_plan(LogicalAgentState::Suspended, false, true, false),
            PartitionCutoverDisposition::RejectUnsafeExecution
        );
        // Safe cross-target execution commits.
        assert_eq!(
            partition_cutover_plan(LogicalAgentState::Ready, false, false, true),
            PartitionCutoverDisposition::Commit
        );
        assert_eq!(
            partition_cutover_plan(LogicalAgentState::Suspended, false, false, true),
            PartitionCutoverDisposition::Commit
        );
    }

    #[test]
    fn agent_release_and_post_safety_dispositions() {
        use decisions::*;
        // Release: retirement requested or ephemeral -> Retire; otherwise -> BecomeReady.
        assert_eq!(
            agent_release_disposition(true, Retention::Resident),
            AgentReleaseDisposition::Retire
        );
        assert_eq!(
            agent_release_disposition(false, Retention::Ephemeral),
            AgentReleaseDisposition::Retire
        );
        assert_eq!(
            agent_release_disposition(false, Retention::Resident),
            AgentReleaseDisposition::BecomeReady
        );

        // Post-safety: RETIRED is no-op.
        assert_eq!(
            post_safety_agent_disposition(LogicalAgentState::Retired, false, Retention::Resident),
            PostSafetyAgentDisposition::NoAction
        );
        // Post-safety: otherwise maps BecomeReady -> Revive, Retire -> Retire.
        assert_eq!(
            post_safety_agent_disposition(LogicalAgentState::Suspended, false, Retention::Resident),
            PostSafetyAgentDisposition::Revive
        );
        assert_eq!(
            post_safety_agent_disposition(LogicalAgentState::Suspended, true, Retention::Resident),
            PostSafetyAgentDisposition::Retire
        );
        assert_eq!(
            post_safety_agent_disposition(
                LogicalAgentState::Suspended,
                false,
                Retention::Ephemeral
            ),
            PostSafetyAgentDisposition::Retire
        );
    }

    #[test]
    fn dependency_release_unblocking() {
        use decisions::*;
        assert!(dependency_release_decision(&[
            TaskState::Completed,
            TaskState::Completed
        ]));
        assert!(!dependency_release_decision(&[
            TaskState::Completed,
            TaskState::Running
        ]));
        assert!(!dependency_release_decision(&[
            TaskState::Completed,
            TaskState::Cancelled
        ]));

        let blocked = vec![
            BlockedTaskSnapshot {
                task_id: "t1".to_string(),
                parent_states: vec![TaskState::Completed, TaskState::Completed],
            },
            BlockedTaskSnapshot {
                task_id: "t2".to_string(),
                parent_states: vec![TaskState::Completed, TaskState::Queued],
            },
            BlockedTaskSnapshot {
                task_id: "t3".to_string(),
                parent_states: vec![TaskState::Completed],
            },
        ];
        let ready = plan_dependency_releases(&blocked);
        assert_eq!(ready, vec!["t1", "t3"]);
    }

    #[test]
    fn escalation_resolution_decisions() {
        use decisions::*;

        let base_snap = EscalationResolutionSnapshot {
            escalation_is_open: true,
            failure_class: FailureClass::WriterQuiescenceUnknown,
            task_state: TaskState::Suspended,
            workspace_mode: WorkspaceMode::Write,
            frozen_isolation: false,
            has_agent: true,
        };

        // Closed escalation fails
        let mut closed_snap = base_snap.clone();
        closed_snap.escalation_is_open = false;
        assert!(
            plan_escalation_resolution(&closed_snap, EscalationOperation::Retry, true).is_err()
        );

        // Retry writer without quiescence or isolation fails
        assert!(plan_escalation_resolution(&base_snap, EscalationOperation::Retry, false).is_err());

        // Retry writer with confirmed quiescence succeeds
        let plan =
            plan_escalation_resolution(&base_snap, EscalationOperation::Retry, true).unwrap();
        assert_eq!(
            plan,
            EscalationResolutionPlan::Retry {
                next_task_state: TaskState::Queued,
                reactivate_batch: true,
                writer_presence: EscalatedWriterPresenceAction::FinalizePresence,
                revive_agent: true,
                resolve_escalation: true,
            }
        );

        // Cancel task for writer unknown
        let cancel_plan =
            plan_escalation_resolution(&base_snap, EscalationOperation::CancelTask, false).unwrap();
        assert_eq!(
            cancel_plan,
            EscalationResolutionPlan::CancelTask {
                next_task_state: TaskState::Cancelled,
                resolve_escalation: false,
                recompute_batch_only: true,
            }
        );

        // Release cancelled writer requires cancelled task state
        assert!(plan_escalation_resolution(
            &base_snap,
            EscalationOperation::ReleaseCancelledWriter,
            true
        )
        .is_err());

        let mut cancelled_snap = base_snap.clone();
        cancelled_snap.task_state = TaskState::Cancelled;
        let release_plan = plan_escalation_resolution(
            &cancelled_snap,
            EscalationOperation::ReleaseCancelledWriter,
            true,
        )
        .unwrap();
        assert_eq!(
            release_plan,
            EscalationResolutionPlan::ReleaseCancelledWriter {
                writer_presence: EscalatedWriterPresenceAction::FinalizePresence,
                revive_agent: true,
                resolve_escalation: true,
            }
        );
    }
}
