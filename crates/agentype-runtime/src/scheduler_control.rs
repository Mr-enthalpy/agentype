//! Narrow production control surface for one running Scheduler daemon.
//!
//! [`SchedulerControl`] is the only production path that mutates Scheduler
//! authority from outside the Runtime. It is deliberately narrower than
//! `Kernel`, and the narrowness is the point: the production composition root
//! must not hand an embedding host an escape hatch back into the boundaries
//! M5.7 closed.
//!
//! Three families are deliberately absent.
//!
//! **Worker acknowledgement.** `Kernel::ack_success`, `Kernel::nack`, and
//! `Kernel::nack_preserving_physical_history` mint or refuse a Task Result
//! from worker evidence. That is a data-plane capability owned by the worker
//! Result transport, whose DTOs already live in
//! `agentype_adapter_api::worker_protocol_v01` — not a host control
//! capability. A host that could call them could mint a Worker ACK, which is
//! exactly the boundary M5.7 closed.
//!
//! **Runtime-owned mechanics.** `claim_next_available`, `create_execution`,
//! `confirm_running_and_renew`, `record_physical_outcome`,
//! `record_runtime_handle_hint`, `record_pending_physical_terminal`,
//! `abort_before_physical_start`, `expire_leases`, `promote_retry_wait`,
//! `recover_authority`, `revive_agent`, `revive_eligible_agents`,
//! `renew_supervised_execution`, `renew_supervised_execution_guarded`,
//! `heartbeat`, and the outbox delivery pipeline belong to the Runtime
//! workers. A host holding them would be a second scheduler able to start
//! physical work, renew authority, or consume delivery attempts outside the
//! dispatch gate.
//!
//! **Writer-quiescence overrides.** `cancel_task` is exposed without its
//! `quiescence_confirmed` argument, so a host cannot assert a quiescence
//! proof it does not hold. Releasing a cancelled writer is an Escalation
//! decision with its own primitive; it is not a plain cancellation.
//!
//! What remains is host control and diagnostics: submit work, cancel work,
//! manage pool topology, acknowledge a consumed Result, acknowledge a
//! delivered Root notification, and read durable state.
//!
//! Deliberately **not** exposed in M5.8: `resolve_escalation`, and therefore
//! the `release_cancelled_writer` decision. Escalation resolution is a host
//! decision in principle, but it carries recovery-primitive selection, so it
//! is left out until the Escalation milestone owns its surface.

use agentype_core::{
    AttemptId, AttemptRecord, BatchId, BatchRecord, Error, EscalationRecord, ExecutionId,
    ExecutionRecord, IncarnationId, IncarnationRecord, LeaseRecord, LogicalAgentId,
    LogicalAgentRecord, OutboxEvent, OutboxEventId, OutboxState, PartitionRecord, PartitionSpec,
    ReconcileReport, ResultId, ResultRecord, TaskId, TaskRecord, TaskSpec, UnixTime,
};
use agentype_storage_sqlite::Kernel;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// Outcome of accepting a Batch for durable execution.
///
/// Pure host-facing output: it carries no authority and is not itself a
/// scheduler record.
#[derive(Debug, Clone)]
pub struct BatchSubmission {
    /// The durable Batch that now owns every submitted Task.
    pub batch_id: BatchId,
    /// Submitted Task name to durable Task identity.
    pub task_ids: HashMap<String, TaskId>,
}

/// Host-facing control and diagnostics over one running Scheduler daemon.
///
/// Borrowed from [`crate::RunningSchedulerDaemon`], never owned: it cannot
/// outlive the process lock that makes the daemon the store's single owner.
///
/// Worker acknowledgement is unreachable through this type.
///
/// ```compile_fail
/// fn _no_worker_ack_through_control(control: &agentype_runtime::SchedulerControl<'_>) {
///     let _ = control.ack_success;
/// }
/// ```
///
/// ```compile_fail
/// fn _no_worker_nack_through_control(control: &agentype_runtime::SchedulerControl<'_>) {
///     let _ = control.nack;
/// }
/// ```
///
/// ```compile_fail
/// fn _no_worker_nack_preserving_history_through_control(
///     control: &agentype_runtime::SchedulerControl<'_>,
/// ) {
///     let _ = control.nack_preserving_physical_history;
/// }
/// ```
///
/// `cancel_task` takes no `quiescence_confirmed` argument; the shape of that
/// call is pinned by a test rather than by a doctest, because a missing
/// argument is not something a `compile_fail` can name.
///
/// ```compile_fail
/// fn _no_forged_control(
///     kernel: &agentype_storage_sqlite::Kernel,
///     store_identity: &str,
///     store_path: &std::path::Path,
/// ) {
///     let _ = agentype_runtime::SchedulerControl {
///         kernel,
///         store_identity,
///         store_path,
///     };
/// }
/// ```
pub struct SchedulerControl<'a> {
    kernel: &'a Kernel,
    store_identity: &'a str,
    store_path: &'a Path,
}

impl<'a> SchedulerControl<'a> {
    /// Not public: a control surface exists only while its daemon does.
    ///
    /// Borrows the diagnostic store identity rather than the process lock, so
    /// this type depends only on what it uses and can be exercised without
    /// taking OS ownership of a store.
    pub(crate) fn new(kernel: &'a Kernel, store_identity: &'a str, store_path: &'a Path) -> Self {
        Self {
            kernel,
            store_identity,
            store_path,
        }
    }

    /// Borrow the Root semantic control surface for this running daemon.
    pub fn semantic_control(&self) -> crate::RootSemanticControl<'_> {
        crate::RootSemanticControl::new(self.kernel)
    }

    /// Borrow the proposal-only intent ingress for this running daemon.
    ///
    /// This surface can submit a result-backed suggestion but cannot admit,
    /// reject, freeze, or close anything.
    pub fn intent_ingress(&self) -> crate::IntentIngress<'_> {
        crate::IntentIngress::new(self.kernel)
    }

    /// Borrow the M6-B.3 operator provisioning surface: bind existing
    /// LogicalAgents to exact AgentType revisions and read typed requirements /
    /// match results. This is provisioning authority, not semantic admission.
    pub fn provisioning_admin(&self) -> crate::ProvisioningAdmin<'_> {
        crate::ProvisioningAdmin::new(self.kernel)
    }

    /// Borrow the M6-B operator catalog administration surface: publish the
    /// immutable B.2 capability / AgentType revisions that typed admission and
    /// binding require. Delegates to the validated B.2 catalog transactions.
    pub fn catalog_admin(&self) -> crate::CatalogAdmin<'_> {
        crate::CatalogAdmin::new(self.kernel)
    }

    // ------------------------------------------------------------ submit

    /// Accept a Batch of Tasks for durable execution.
    ///
    /// Submission is durable and independent of dispatch eligibility: work
    /// accepted while the daemon is stopping is dispatched by a later
    /// lifecycle. Shutdown stops Scheduler mechanics; it does not cancel
    /// semantic work.
    pub fn submit_batch(&self, tasks: &[TaskSpec]) -> Result<BatchSubmission, Error> {
        let (batch_id, task_ids) = self.kernel.submit_batch(tasks)?;
        Ok(BatchSubmission { batch_id, task_ids })
    }

    // ------------------------------------------------------------ cancel

    /// Cancel a Task without asserting writer quiescence.
    ///
    /// The `quiescence_confirmed` override is not reachable here: a host that
    /// has not proved the workspace writer stopped must not claim it did.
    pub fn cancel_task(&self, task_id: &TaskId) -> Result<(), Error> {
        self.kernel.cancel_task(task_id, false)
    }

    /// Cancel every nonterminal Task in a Batch.
    pub fn cancel_batch(&self, batch_id: &BatchId) -> Result<(), Error> {
        self.kernel.cancel_batch(batch_id)
    }

    // ---------------------------------------------------------- topology

    /// Idempotent structural partition upsert. Returns the topology revision.
    pub fn upsert_partition(&self, spec: &PartitionSpec) -> Result<i64, Error> {
        self.kernel.upsert_partition(spec)
    }

    /// Change a partition's desired resident capacity. Returns the revision.
    pub fn resize_partition(&self, name: &str, desired_capacity: i64) -> Result<i64, Error> {
        self.kernel.resize_partition(name, desired_capacity)
    }

    /// Move declared capacity between partitions. Returns the revision.
    pub fn move_capacity(&self, source: &str, target: &str, count: i64) -> Result<i64, Error> {
        self.kernel.move_capacity(source, target, count)
    }

    /// Merge a partition into another. Returns the revision.
    pub fn merge_partitions(&self, source: &str, target: &str) -> Result<i64, Error> {
        self.kernel.merge_partitions(source, target)
    }

    /// Retire an empty partition. Returns the revision.
    pub fn retire_partition(&self, name: &str) -> Result<i64, Error> {
        self.kernel.retire_partition(name)
    }

    /// Reconcile desired pool state against realized agents.
    pub fn reconcile_pool(&self) -> Result<ReconcileReport, Error> {
        self.kernel.reconcile_pool()
    }

    // --------------------------------------------------------- ack planes

    /// Acknowledge a consumed Task Result on behalf of a consumer.
    ///
    /// This is the Root/consumer acknowledgement plane. Root consumption does
    /// not control Task completion.
    pub fn ack_result(&self, result_id: &ResultId, consumer_ref: &str) -> Result<(), Error> {
        self.kernel.ack_result(result_id, consumer_ref)
    }

    /// Acknowledge a delivered Root notification.
    ///
    /// This is the consumer acknowledgement plane for the durable outbox, not
    /// the Notifier's own delivery pipeline.
    pub fn ack_outbox(&self, event_id: &OutboxEventId) -> Result<OutboxState, Error> {
        self.kernel.ack_outbox(event_id)
    }

    // ------------------------------------------------------- diagnostics

    /// Store identity. Diagnostic only; never Scheduler authority.
    pub fn store_identity(&self) -> &str {
        self.store_identity
    }

    /// The store path this daemon owns.
    pub fn store_path(&self) -> &Path {
        self.store_path
    }

    /// Scheduler clock reading used for durable decisions.
    pub fn now(&self) -> UnixTime {
        self.kernel.now()
    }

    /// Configured Lease duration in seconds.
    pub fn lease_seconds(&self) -> f64 {
        self.kernel.lease_seconds()
    }

    /// Applied schema version.
    pub fn schema_version(&self) -> Result<i64, Error> {
        self.kernel.schema_version()
    }

    /// Effective durability pragmas: `(journal_mode, synchronous, foreign_keys)`.
    pub fn pragmas(&self) -> Result<(String, i64, i64), Error> {
        self.kernel.pragmas()
    }

    pub fn task(&self, id: &TaskId) -> Result<TaskRecord, Error> {
        self.kernel.task(id)
    }

    pub fn batch(&self, id: &BatchId) -> Result<BatchRecord, Error> {
        self.kernel.batch(id)
    }

    pub fn result_for_task(&self, task_id: &TaskId) -> Result<ResultRecord, Error> {
        self.kernel.result_for_task(task_id)
    }

    pub fn attempt(&self, id: &AttemptId) -> Result<AttemptRecord, Error> {
        self.kernel.attempt(id)
    }

    pub fn lease_for_attempt(&self, attempt_id: &AttemptId) -> Result<LeaseRecord, Error> {
        self.kernel.lease_for_attempt(attempt_id)
    }

    pub fn logical_agent(&self, id: &LogicalAgentId) -> Result<LogicalAgentRecord, Error> {
        self.kernel.logical_agent(id)
    }

    pub fn incarnation(&self, id: &IncarnationId) -> Result<IncarnationRecord, Error> {
        self.kernel.incarnation(id)
    }

    pub fn execution(&self, id: &ExecutionId) -> Result<ExecutionRecord, Error> {
        self.kernel.execution(id)
    }

    /// The open Escalation for a Task, if any.
    ///
    /// Read-only here: resolving it is not part of the M5.8 control surface.
    pub fn open_escalation_for_task(&self, task_id: &TaskId) -> Result<EscalationRecord, Error> {
        self.kernel.open_escalation_for_task(task_id)
    }

    pub fn partition(&self, name: &str) -> Result<PartitionRecord, Error> {
        self.kernel.partition(name)
    }

    /// Durable notifications recorded for a Batch under one event type.
    pub fn outbox_for_batch(
        &self,
        batch_id: &BatchId,
        event_type: &str,
    ) -> Result<Vec<OutboxEvent>, Error> {
        self.kernel.outbox_for_batch(batch_id, event_type)
    }
}

impl fmt::Debug for SchedulerControl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchedulerControl")
            .field("store_identity", &self.store_identity)
            .field("store_path", &self.store_path)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_core::BATCH_RESULTS_READY;
    use agentype_core::{
        BatchState, ManualClock, PartitionSpec, ResultState, Retention, TaskState,
    };
    use agentype_storage_sqlite::SCHEMA_VERSION;
    use std::sync::Arc;

    const STORE_ID: &str = "dev:ino:4242";

    fn store_path() -> &'static Path {
        Path::new("stores/scheduler.sqlite")
    }

    /// A store with one ready pool partition, reached without taking OS
    /// ownership: the control surface is bound to a daemon, not to the lock.
    fn control_env() -> Kernel {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, 16_384).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        kernel
    }

    fn with_control<T>(kernel: &Kernel, f: impl FnOnce(SchedulerControl<'_>) -> T) -> T {
        f(SchedulerControl::new(kernel, STORE_ID, store_path()))
    }

    #[test]
    fn host_submits_and_reads_back_a_batch() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            let submission = control
                .submit_batch(&[
                    TaskSpec::new("inspect", serde_json::json!({"goal": "read"})),
                    TaskSpec::new("verify", serde_json::json!({"goal": "check"}))
                        .depends_on(["inspect"]),
                ])
                .unwrap();

            assert_eq!(submission.task_ids.len(), 2);
            // A batch with an available pool is ACTIVE as soon as its
            // consumers exist; nothing has been claimed or dispatched yet.
            assert_eq!(
                control.batch(&submission.batch_id).unwrap().state,
                BatchState::Active
            );

            let inspect = &submission.task_ids["inspect"];
            let record = control.task(inspect).unwrap();
            assert_eq!(record.state, TaskState::Queued);
            assert_eq!(record.batch_id, submission.batch_id);
            assert_eq!(record.name, "inspect");
        });
    }

    #[test]
    fn host_cancels_a_task_without_asserting_writer_quiescence() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            let submission = control
                .submit_batch(&[TaskSpec::new("doomed", serde_json::json!({}))])
                .unwrap();
            let task_id = submission.task_ids["doomed"].clone();

            // One argument only: the `quiescence_confirmed` override is not
            // reachable through the control surface. Adding a second
            // parameter here would stop this test compiling.
            control.cancel_task(&task_id).unwrap();

            // The Task settles as CANCELLED. The Batch is recomputed and
            // suspends rather than completing: a batch that lost one of its
            // Tasks to cancellation can no longer finish on its own, which is
            // a Root decision. `cancel_batch` is the primitive that cancels
            // the Batch itself.
            assert_eq!(control.task(&task_id).unwrap().state, TaskState::Cancelled);
            assert_eq!(
                control.batch(&submission.batch_id).unwrap().state,
                BatchState::Suspended
            );

            // Cancelling without a quiescence proof raised no writer
            // obligation, because a read-only Task has no workspace writer to
            // quiesce. Nothing here let the host assert a proof it lacks.
            assert!(control.open_escalation_for_task(&task_id).is_err());
        });
    }

    #[test]
    fn host_cancels_every_nonterminal_task_in_a_batch() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            let submission = control
                .submit_batch(&[
                    TaskSpec::new("one", serde_json::json!({})),
                    TaskSpec::new("two", serde_json::json!({})),
                ])
                .unwrap();

            control.cancel_batch(&submission.batch_id).unwrap();

            for task_id in submission.task_ids.values() {
                assert_eq!(control.task(task_id).unwrap().state, TaskState::Cancelled);
            }
        });
    }

    #[test]
    fn host_reads_pool_topology_and_moves_capacity() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            let first = control
                .upsert_partition(&PartitionSpec::new(
                    "spare",
                    0,
                    Retention::Resident,
                    "local",
                    "default",
                ))
                .unwrap();
            assert!(first > 0);

            let moved = control.move_capacity("general", "spare", 1).unwrap();
            assert!(moved > first);

            let resized = control.resize_partition("spare", 3).unwrap();
            assert!(resized > moved);

            let spare = control.partition("spare").unwrap();
            assert_eq!(spare.desired_capacity, 3);
            assert_eq!(control.partition("general").unwrap().desired_capacity, 0);

            control.reconcile_pool().unwrap();
        });
    }

    #[test]
    fn host_retires_an_empty_partition() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            control
                .upsert_partition(&PartitionSpec::new(
                    "empty",
                    0,
                    Retention::Resident,
                    "local",
                    "default",
                ))
                .unwrap();
            let revision = control.retire_partition("empty").unwrap();
            assert!(revision > 0);
            assert!(!control.partition("empty").unwrap().active);
        });
    }

    #[test]
    fn host_merges_partitions() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            control
                .upsert_partition(&PartitionSpec::new(
                    "absorbed",
                    0,
                    Retention::Resident,
                    "local",
                    "default",
                ))
                .unwrap();
            let revision = control.merge_partitions("absorbed", "general").unwrap();
            assert!(revision > 0);
            assert_eq!(
                control.partition("absorbed").unwrap().merged_into,
                Some(agentype_core::PartitionId::new("general"))
            );
        });
    }

    #[test]
    fn diagnostics_describe_the_store_without_minting_authority() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            assert_eq!(control.store_identity(), STORE_ID);
            assert_eq!(control.store_path(), store_path());
            assert_eq!(control.schema_version().unwrap(), SCHEMA_VERSION);
            assert_eq!(control.lease_seconds(), 10.0);
            assert_eq!(control.now(), 1_000.0);

            // `foreign_keys` must be on for every connection.
            let (_, _, foreign_keys) = control.pragmas().unwrap();
            assert_eq!(foreign_keys, 1);

            let rendered = format!("{control:?}");
            assert!(rendered.contains(STORE_ID), "{rendered}");
        });
    }

    #[test]
    fn host_acknowledges_a_consumed_result() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            let submission = control
                .submit_batch(&[TaskSpec::new("work", serde_json::json!({}))])
                .unwrap();
            let task_id = submission.task_ids["work"].clone();

            // Worker authority is a fixture here: it is not reachable through
            // `control`, so the test drives the Kernel directly.
            let claim = kernel.claim_next_available().unwrap().unwrap();
            let result_id = kernel
                .ack_success(
                    &claim.attempt_id,
                    claim.lease_epoch,
                    None,
                    &serde_json::json!({"ok": true}),
                    None,
                    false,
                    false,
                )
                .unwrap()
                .expect("ack_success mints a Result");

            let result = control.result_for_task(&task_id).unwrap();
            assert_eq!(result.id, result_id);
            assert_eq!(result.state, ResultState::Available);

            control.ack_result(&result_id, "root-1").unwrap();
            assert_eq!(
                control.result_for_task(&task_id).unwrap().state,
                ResultState::Acked
            );
        });
    }

    #[test]
    fn host_acknowledges_a_delivered_root_notification() {
        let kernel = control_env();
        with_control(&kernel, |control| {
            let submission = control
                .submit_batch(&[TaskSpec::new("work", serde_json::json!({}))])
                .unwrap();

            let claim = kernel.claim_next_available().unwrap().unwrap();
            kernel
                .ack_success(
                    &claim.attempt_id,
                    claim.lease_epoch,
                    None,
                    &serde_json::json!({"ok": true}),
                    None,
                    false,
                    false,
                )
                .unwrap();

            let events = control
                .outbox_for_batch(&submission.batch_id, BATCH_RESULTS_READY)
                .unwrap();
            assert_eq!(events.len(), 1, "batch completion records one wakeup");
            assert_eq!(events[0].state, OutboxState::Pending);

            let state = control.ack_outbox(&events[0].id).unwrap();
            assert_eq!(state, OutboxState::Acked);
        });
    }
}
