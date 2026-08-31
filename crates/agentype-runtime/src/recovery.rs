//! M5.4 restart reconciliation: durable terminal replay and single-Execution
//! reconcile. The coordinator / StartupGuard (M5.4-F) compose these
//! primitives; they are not an authority path on their own.
//!
//! Category A (terminal replay) runs before any `reconcile_start`.
//! Category B (`reconcile_start`) is identity-preserving and never calls
//! `start_execution`. A persisted `state='RUNNING'` row never mints
//! admission: only a fresh `RunningAuthorityGrant` can.

use crate::observation::{
    adapter_invocation_failure_class, normalize_collected_outcome, normalize_start_observation,
    CollectedOutcomeKind, StartObservationKind,
};
use crate::supervision::{SupervisionError, SupervisionRunner, SupervisionService};
use crate::timing::RuntimeTimingConfig;
use crate::{AdapterRegistry, SupervisionAdmission};

/// Where a freshly minted admission is consumed. The live runner must be
/// used when one is running (lifecycle gate + deadline wake-up); the
/// deterministic `SupervisionService` is enough for single-execution tests.
pub trait AdmissionSink {
    fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError>;
}

impl AdmissionSink for SupervisionService {
    fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError> {
        SupervisionService::admit(self, admission)
    }
}

impl AdmissionSink for SupervisionRunner {
    fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError> {
        SupervisionRunner::admit(self, admission)
    }
}
use agentype_adapter_api::{ExecutionAdapter, RuntimeHandle, StartObservation};
use agentype_core::{Error, ExecutionState, FailureClass, ResultId};
use agentype_storage_sqlite::{ExecutionReconciliationSnapshot, Kernel};
use serde_json::Value;
use std::sync::Arc;

/// Recovery failures. Per-Execution adapter/protocol uncertainty is an
/// *outcome*, not an error. These are internal durable / structural faults
/// that stop the Scheduler (M5.4 plan §14).
#[derive(Debug)]
pub enum RecoveryError {
    Persistence(Error),
    Invariant(String),
    Supervision(SupervisionError),
}

impl RecoveryError {
    fn invariant(msg: impl Into<String>) -> Self {
        Self::Invariant(msg.into())
    }
}

impl From<Error> for RecoveryError {
    fn from(err: Error) -> Self {
        match err {
            Error::InvariantViolation(msg) => Self::Invariant(msg),
            other => Self::Persistence(other),
        }
    }
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Persistence(e) => write!(f, "recovery persistence fault: {e}"),
            Self::Invariant(m) => write!(f, "recovery invariant violation: {m}"),
            Self::Supervision(e) => write!(f, "recovery supervision fault: {e}"),
        }
    }
}

impl std::error::Error for RecoveryError {}

/// Mechanical outcome of Category A (durable terminal-consequence replay).
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalReplayOutcome {
    /// This snapshot is not a Category A candidate.
    NotApplicable,
    /// ACK applied; exactly one Result was created.
    ResultReplayed { result_id: ResultId },
    /// ACK applied the physical success, but writer safety suspended the
    /// Task. Deliberately not a Result.
    WriterSafetySuspended,
    /// NACK applied for persisted terminal-looking failure evidence.
    FailureReplayed { failure_class: FailureClass },
    /// A Result already exists; replay is a no-op (exactly-one).
    AlreadyApplied { result_id: ResultId },
    /// Authority stale or already closed. Physical history may be refined;
    /// Task/Result/Batch are not mutated.
    PhysicalHistoryOnly,
}

/// Mechanical outcome of Category B/C (one Execution reconcile).
#[derive(Debug)]
pub enum ReconcileExecutionOutcome {
    /// Fresh grant minted and admitted into `supervisor`.
    Readmitted,
    /// Observed RUNNING (or other history) persisted; no grant, no admission.
    PhysicalHistoryOnly,
    TaskCompleted {
        result_id: ResultId,
    },
    WriterSafetySuspendedAfterSuccess,
    TerminalFailure {
        failure_class: FailureClass,
    },
    /// Nonterminal / unresolved: mechanical NACK applied when authority was
    /// current; writer safety / retry policy decided the Task state.
    Unresolved {
        failure_class: FailureClass,
    },
}

/// Category A: replay a persisted terminal consequence, or report that this
/// snapshot is not a terminal-replay candidate.
///
/// Legal crash window: `collect_outcome` persisted evidence (`outcome_json`
/// and/or `failure_class` on an still-active physical state) then crashed
/// before ACK/NACK. Kernel ACK/NACK is atomic with closing Attempt/Lease, so
/// a row already in SUCCEEDED/FAILED/TERMINATED MUST NOT still hold current
/// authority — that is inconsistent durable evidence and fails closed.
pub fn replay_persisted_terminal_consequence(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<TerminalReplayOutcome, RecoveryError> {
    match kernel.result_for_task(snapshot.task_id()) {
        Ok(existing) => {
            return Ok(TerminalReplayOutcome::AlreadyApplied {
                result_id: existing.id,
            });
        }
        Err(Error::NotFound(_)) => {}
        Err(err) => return Err(RecoveryError::from(err)),
    }

    let current = snapshot.current_authority_hint().looks_current();
    match snapshot.persisted_state() {
        ExecutionState::Succeeded | ExecutionState::Failed | ExecutionState::Terminated => {
            if current {
                return Err(RecoveryError::invariant(format!(
                    "execution {} is durable {} but still holds current Attempt/Lease authority; \
                     ACK/NACK is atomic with authority close",
                    snapshot.execution_id(),
                    snapshot.persisted_state().as_sql()
                )));
            }
            Ok(TerminalReplayOutcome::PhysicalHistoryOnly)
        }
        ExecutionState::Starting | ExecutionState::Running | ExecutionState::Unknown => {
            let success_evidence =
                snapshot.outcome_json().is_some() && snapshot.failure_class().is_none();
            let failure_evidence = snapshot.failure_class().is_some();
            if success_evidence {
                replay_success_evidence(kernel, snapshot)
            } else if failure_evidence {
                replay_failure_evidence(kernel, snapshot)
            } else {
                Ok(TerminalReplayOutcome::NotApplicable)
            }
        }
        ExecutionState::Lost => Ok(TerminalReplayOutcome::NotApplicable),
    }
}

fn replay_success_evidence(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<TerminalReplayOutcome, RecoveryError> {
    let payload = snapshot.outcome_json().cloned().unwrap_or(Value::Null);
    match kernel.ack_success(
        snapshot.attempt_id(),
        snapshot.lease_epoch(),
        Some(snapshot.execution_id()),
        &payload,
        None,
        snapshot.quiescent_confirmed(),
        false,
    ) {
        Ok(Some(result_id)) => Ok(TerminalReplayOutcome::ResultReplayed { result_id }),
        Ok(None) => Ok(TerminalReplayOutcome::WriterSafetySuspended),
        Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
            persist_stale_success_history(kernel, snapshot)?;
            Ok(TerminalReplayOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(RecoveryError::from(err)),
    }
}

fn persist_stale_success_history(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<(), RecoveryError> {
    if matches!(
        snapshot.persisted_state(),
        ExecutionState::Starting | ExecutionState::Running | ExecutionState::Unknown
    ) {
        kernel
            .record_physical_outcome(
                snapshot.execution_id(),
                ExecutionState::Succeeded,
                Some(snapshot.runtime_handle()),
                snapshot.outcome_json(),
                None,
                true,
                snapshot.quiescent_confirmed(),
            )
            .map_err(RecoveryError::from)?;
    }
    Ok(())
}

fn replay_failure_evidence(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<TerminalReplayOutcome, RecoveryError> {
    let failure_class = snapshot
        .failure_class()
        .expect("failure evidence carries a class");
    // Crash-before-NACK evidence is UNKNOWN with a failure class and zero
    // proof bits. Replay the mechanical NACK; do not invent terminality or
    // quiescence (the evidence row itself recorded neither).
    match apply_nack(
        kernel,
        snapshot,
        failure_class,
        snapshot.terminal_confirmed(),
        snapshot.quiescent_confirmed(),
        Some(snapshot.runtime_handle()),
    ) {
        Ok(()) => Ok(TerminalReplayOutcome::FailureReplayed { failure_class }),
        Err(RecoveryError::Persistence(Error::StaleAuthority(_) | Error::InvalidAuthority(_))) => {
            Ok(TerminalReplayOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(err),
    }
}

fn apply_nack(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
    failure_class: FailureClass,
    terminal_confirmed: bool,
    quiescent_confirmed: bool,
    handle: Option<&Value>,
) -> Result<(), RecoveryError> {
    if let Some(handle) = handle {
        // Preserve the observed handle before NACK. `Kernel::nack` does not
        // write runtime_handle_json. Only persist UNKNOWN when the physical
        // graph allows it (STARTING/UNKNOWN); RUNNING keeps RUNNING until
        // NACK rewrites it; LOST is never rewritten.
        match snapshot.persisted_state() {
            ExecutionState::Starting | ExecutionState::Unknown => {
                kernel
                    .record_physical_outcome(
                        snapshot.execution_id(),
                        ExecutionState::Unknown,
                        Some(handle),
                        None,
                        Some(failure_class),
                        false,
                        false,
                    )
                    .map_err(RecoveryError::from)?;
            }
            ExecutionState::Running => {
                kernel
                    .record_physical_outcome(
                        snapshot.execution_id(),
                        ExecutionState::Running,
                        Some(handle),
                        None,
                        None,
                        false,
                        false,
                    )
                    .map_err(RecoveryError::from)?;
            }
            ExecutionState::Lost
            | ExecutionState::Succeeded
            | ExecutionState::Failed
            | ExecutionState::Terminated => {}
        }
    }
    kernel
        .nack(
            snapshot.attempt_id(),
            snapshot.lease_epoch(),
            failure_class,
            Some(snapshot.execution_id()),
            terminal_confirmed,
            quiescent_confirmed,
            false,
        )
        .map(|_| ())
        .map_err(RecoveryError::from)
}

/// Category B/C: reconcile one STARTING / UNKNOWN / RUNNING / LOST
/// Execution. MUST NOT call `start_execution`. Routes by persisted
/// `adapter_kind` only.
///
/// A successful grant is admitted immediately so heartbeat can begin
/// during recovery (M5.4 plan §8).
pub fn reconcile_one_execution(
    kernel: &Kernel,
    adapters: &AdapterRegistry,
    snapshot: &ExecutionReconciliationSnapshot,
    supervisor: &impl AdmissionSink,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    match snapshot.persisted_state() {
        ExecutionState::Lost => close_lost(kernel, snapshot),
        ExecutionState::Succeeded | ExecutionState::Failed | ExecutionState::Terminated => {
            // Category A owns these. Reaching here is a coordinator bug.
            Err(RecoveryError::invariant(format!(
                "reconcile_one_execution called for terminal execution {}",
                snapshot.execution_id()
            )))
        }
        ExecutionState::Starting | ExecutionState::Running | ExecutionState::Unknown => {
            reconcile_active_physical(kernel, adapters, snapshot, supervisor)
        }
    }
}

fn close_lost(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    if !snapshot.current_authority_hint().looks_current() {
        return Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly);
    }
    match apply_nack(
        kernel,
        snapshot,
        FailureClass::ExecutionLost,
        false,
        false,
        Some(snapshot.runtime_handle()),
    ) {
        Ok(()) => Ok(ReconcileExecutionOutcome::Unresolved {
            failure_class: FailureClass::ExecutionLost,
        }),
        Err(RecoveryError::Persistence(Error::StaleAuthority(_) | Error::InvalidAuthority(_))) => {
            Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(err),
    }
}

fn reconcile_active_physical(
    kernel: &Kernel,
    adapters: &AdapterRegistry,
    snapshot: &ExecutionReconciliationSnapshot,
    supervisor: &impl AdmissionSink,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    let adapter = match adapters.resolve(snapshot.adapter_kind()) {
        Ok(adapter) => adapter,
        Err(_) => return missing_adapter(kernel, snapshot),
    };

    let hint = persisted_handle_hint(snapshot.runtime_handle());
    let observation = match adapter.reconcile_start(snapshot.request_id(), hint.as_ref()) {
        Ok(observation) => observation,
        Err(err) => {
            let failure_class = adapter_invocation_failure_class(&err);
            return unresolved_or_history(
                kernel,
                snapshot,
                failure_class,
                Some(snapshot.runtime_handle()),
            );
        }
    };

    apply_reconcile_observation(kernel, adapter, snapshot, observation, supervisor)
}

fn missing_adapter(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    // Availability failure, not physical proof: no death, no quiescence,
    // no TERMINATED, no fallback adapter (M5.4 plan §13).
    unresolved_or_history(
        kernel,
        snapshot,
        FailureClass::ResourceUnavailable,
        Some(snapshot.runtime_handle()),
    )
}

fn apply_reconcile_observation(
    kernel: &Kernel,
    adapter: Arc<dyn ExecutionAdapter>,
    snapshot: &ExecutionReconciliationSnapshot,
    observation: StartObservation,
    supervisor: &impl AdmissionSink,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    match normalize_start_observation(&observation) {
        StartObservationKind::ExactRunning => {
            admit_or_history(kernel, snapshot, &observation, supervisor)
        }
        StartObservationKind::TerminalCandidate => {
            collect_and_apply(kernel, adapter, snapshot, &observation)
        }
        StartObservationKind::Unresolved { failure_class } => unresolved_or_history(
            kernel,
            snapshot,
            failure_class,
            Some(&observation.runtime_handle.0),
        ),
    }
}

fn admit_or_history(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
    observation: &StartObservation,
    supervisor: &impl AdmissionSink,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    match kernel.confirm_running_and_renew(
        snapshot.attempt_id(),
        snapshot.lease_epoch(),
        snapshot.execution_id(),
        &observation.runtime_handle.0,
    ) {
        Ok(grant) => {
            let admission = SupervisionAdmission::from_grant(grant);
            supervisor
                .admit(admission)
                .map_err(RecoveryError::Supervision)?;
            Ok(ReconcileExecutionOutcome::Readmitted)
        }
        Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
            kernel
                .record_physical_outcome(
                    snapshot.execution_id(),
                    ExecutionState::Running,
                    Some(&observation.runtime_handle.0),
                    None,
                    None,
                    false,
                    false,
                )
                .map_err(RecoveryError::from)?;
            Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(RecoveryError::from(err)),
    }
}

fn collect_and_apply(
    kernel: &Kernel,
    adapter: Arc<dyn ExecutionAdapter>,
    snapshot: &ExecutionReconciliationSnapshot,
    observation: &StartObservation,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    let outcome = match adapter.collect_outcome(&observation.runtime_handle) {
        Ok(outcome) => outcome,
        Err(err) => {
            let failure_class = adapter_invocation_failure_class(&err);
            return unresolved_or_history(
                kernel,
                snapshot,
                failure_class,
                Some(&observation.runtime_handle.0),
            );
        }
    };
    match normalize_collected_outcome(&outcome) {
        CollectedOutcomeKind::Unresolved { failure_class } => unresolved_or_history(
            kernel,
            snapshot,
            failure_class,
            Some(&observation.runtime_handle.0),
        ),
        CollectedOutcomeKind::TerminalSuccess => {
            let payload = outcome.payload.clone().unwrap_or(Value::Null);
            if !outcome.quiescent_confirmed || outcome.incarnation_reusable {
                kernel
                    .record_physical_outcome(
                        snapshot.execution_id(),
                        ExecutionState::Unknown,
                        Some(&observation.runtime_handle.0),
                        outcome.payload.as_ref(),
                        None,
                        false,
                        false,
                    )
                    .map_err(RecoveryError::from)?;
            }
            match kernel.ack_success(
                snapshot.attempt_id(),
                snapshot.lease_epoch(),
                Some(snapshot.execution_id()),
                &payload,
                outcome.summary.as_deref(),
                outcome.quiescent_confirmed,
                outcome.incarnation_reusable,
            ) {
                Ok(Some(result_id)) => Ok(ReconcileExecutionOutcome::TaskCompleted { result_id }),
                Ok(None) => Ok(ReconcileExecutionOutcome::WriterSafetySuspendedAfterSuccess),
                Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                    kernel
                        .record_physical_outcome(
                            snapshot.execution_id(),
                            ExecutionState::Succeeded,
                            Some(&observation.runtime_handle.0),
                            outcome.payload.as_ref(),
                            None,
                            true,
                            outcome.quiescent_confirmed,
                        )
                        .map_err(RecoveryError::from)?;
                    Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly)
                }
                Err(err) => Err(RecoveryError::from(err)),
            }
        }
        CollectedOutcomeKind::TerminalFailure { failure_class } => {
            match apply_nack(
                kernel,
                snapshot,
                failure_class,
                true,
                outcome.quiescent_confirmed,
                Some(&observation.runtime_handle.0),
            ) {
                Ok(()) => Ok(ReconcileExecutionOutcome::TerminalFailure { failure_class }),
                Err(RecoveryError::Persistence(
                    Error::StaleAuthority(_) | Error::InvalidAuthority(_),
                )) => {
                    kernel
                        .record_physical_outcome(
                            snapshot.execution_id(),
                            ExecutionState::Failed,
                            Some(&observation.runtime_handle.0),
                            None,
                            Some(failure_class),
                            true,
                            outcome.quiescent_confirmed,
                        )
                        .map_err(RecoveryError::from)?;
                    Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly)
                }
                Err(err) => Err(err),
            }
        }
    }
}

fn unresolved_or_history(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
    failure_class: FailureClass,
    handle: Option<&Value>,
) -> Result<ReconcileExecutionOutcome, RecoveryError> {
    match apply_nack(kernel, snapshot, failure_class, false, false, handle) {
        Ok(()) => Ok(ReconcileExecutionOutcome::Unresolved { failure_class }),
        Err(RecoveryError::Persistence(Error::StaleAuthority(_) | Error::InvalidAuthority(_))) => {
            Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(err),
    }
}

/// Process-local startup cleanup scope (M5.4 plan §9). Dropping an
/// uncommitted guard stops the runner and clears every in-memory
/// admission. Cleanup ≠ revoke Lease ≠ terminate worker ≠ quiescence.
struct StartupGuard {
    runner: Option<SupervisionRunner>,
    committed: bool,
}

impl StartupGuard {
    fn new(runner: SupervisionRunner) -> Self {
        Self {
            runner: Some(runner),
            committed: false,
        }
    }

    fn runner(&self) -> &SupervisionRunner {
        self.runner.as_ref().expect("startup runner")
    }

    fn check_healthy(&self) -> Result<(), RecoveryError> {
        if let Some(fatal) = self.runner().take_fatal() {
            return Err(RecoveryError::Supervision(fatal));
        }
        Ok(())
    }

    fn commit(mut self) -> SupervisionRunner {
        self.committed = true;
        self.runner.take().expect("startup runner")
    }
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(runner) = self.runner.take() {
                let _ = runner.shutdown();
            }
        }
    }
}

/// A Runtime that has passed the restart barrier and may dispatch.
pub struct RecoveredRuntime {
    runner: SupervisionRunner,
}

impl RecoveredRuntime {
    pub fn runner(&self) -> &SupervisionRunner {
        &self.runner
    }

    pub fn into_runner(self) -> SupervisionRunner {
        self.runner
    }
}

/// Full M5.4 recovery barrier. Dispatch MUST NOT run until this returns.
///
/// Order (plan §17): expire → empty runner → terminal replay → reconcile
/// STARTING/UNKNOWN/RUNNING/LOST → final expiry sweep → promote retry →
/// pool → revive → runner health → READY.
pub fn recover_runtime(
    kernel: Arc<Kernel>,
    adapters: &AdapterRegistry,
    timing: RuntimeTimingConfig,
) -> Result<RecoveredRuntime, RecoveryError> {
    kernel.expire_leases(true).map_err(RecoveryError::from)?;

    let runner =
        SupervisionRunner::start(kernel.clone(), timing).map_err(RecoveryError::Supervision)?;
    let guard = StartupGuard::new(runner);
    guard.check_healthy()?;

    let candidates = kernel
        .reconciliation_candidates()
        .map_err(RecoveryError::from)?;
    for snap in &candidates {
        guard.check_healthy()?;
        replay_persisted_terminal_consequence(&kernel, snap)?;
    }

    let candidates = kernel
        .reconciliation_candidates()
        .map_err(RecoveryError::from)?;
    for snap in &candidates {
        guard.check_healthy()?;
        match snap.persisted_state() {
            ExecutionState::Starting | ExecutionState::Unknown | ExecutionState::Running => {
                reconcile_one_execution(&kernel, adapters, snap, guard.runner())?;
            }
            ExecutionState::Lost => {
                reconcile_one_execution(&kernel, adapters, snap, guard.runner())?;
            }
            ExecutionState::Succeeded | ExecutionState::Failed | ExecutionState::Terminated => {}
        }
        guard.check_healthy()?;
    }

    kernel.expire_leases(false).map_err(RecoveryError::from)?;
    kernel.promote_retry_wait().map_err(RecoveryError::from)?;
    kernel.reconcile_pool().map_err(RecoveryError::from)?;
    kernel
        .revive_eligible_agents()
        .map_err(RecoveryError::from)?;
    guard.check_healthy()?;

    assert_ready_invariant(&kernel, guard.runner())?;

    let runner = guard.commit();
    Ok(RecoveredRuntime { runner })
}

/// READY invariant: every Task that still holds current Attempt/Lease
/// authority and has an Execution is supervised, or that authority is gone.
fn assert_ready_invariant(
    kernel: &Kernel,
    runner: &SupervisionRunner,
) -> Result<(), RecoveryError> {
    let candidates = kernel
        .reconciliation_candidates()
        .map_err(RecoveryError::from)?;
    for snap in candidates {
        if !snap.current_authority_hint().looks_current() {
            continue;
        }
        match snap.persisted_state() {
            ExecutionState::Running => {
                if !runner.contains(snap.execution_id()) {
                    return Err(RecoveryError::invariant(format!(
                        "READY with unsupervised current RUNNING execution {}",
                        snap.execution_id()
                    )));
                }
            }
            ExecutionState::Starting | ExecutionState::Unknown | ExecutionState::Lost => {
                return Err(RecoveryError::invariant(format!(
                    "READY with unresolved current execution {} in state {}",
                    snap.execution_id(),
                    snap.persisted_state().as_sql()
                )));
            }
            ExecutionState::Succeeded | ExecutionState::Failed | ExecutionState::Terminated => {
                return Err(RecoveryError::invariant(format!(
                    "READY with current authority still attached to terminal execution {}",
                    snap.execution_id()
                )));
            }
        }
    }
    Ok(())
}

fn persisted_handle_hint(value: &Value) -> Option<RuntimeHandle> {
    if value.is_null() {
        return None;
    }
    if value.as_object().map(|o| o.is_empty()).unwrap_or(false) {
        return None;
    }
    Some(RuntimeHandle(value.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::RuntimeTimingConfig;
    use crate::{AdapterRegistry, FrozenExecutionSafety, FrozenPhysicalExecutionBinding};
    use agentype_adapter_api::{FakeAdapter, StartObservation};
    use agentype_core::{
        AuthoritativeExecutionBinding, Claim, ExecutionState, FailureClass, ManualClock,
        PartitionSpec, Retention, RetryPolicy, TaskSpec, TaskState,
    };
    use agentype_storage_sqlite::Kernel;
    use serde_json::json;
    use std::sync::Arc;

    fn timing() -> RuntimeTimingConfig {
        RuntimeTimingConfig::new(1.0, 2.0, 10.0).unwrap()
    }

    fn env() -> (Arc<ManualClock>, Kernel) {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let k = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();
        k.upsert_partition(&PartitionSpec::new(
            "general",
            1,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
        k.reconcile_pool().unwrap();
        (clock, k)
    }

    fn binding(claim: &Claim) -> FrozenPhysicalExecutionBinding {
        FrozenPhysicalExecutionBinding::new(
            FrozenExecutionSafety::unisolated(AuthoritativeExecutionBinding {
                attempt_id: claim.attempt_id.clone(),
                lease_epoch: claim.lease_epoch,
                execution_target: claim.execution_target.clone(),
                execution_profile: claim.execution_profile.clone(),
            }),
            "process",
        )
        .unwrap()
    }

    fn start_named(
        k: &Kernel,
        spec: TaskSpec,
    ) -> (Claim, agentype_execution_config::ExecutionLaunchSnapshot) {
        k.submit_batch(std::slice::from_ref(&spec)).unwrap();
        let claim = k.claim_next_available().unwrap().unwrap();
        let launch = k.create_execution(&claim, binding(&claim)).unwrap();
        (claim, launch)
    }

    fn snapshot_of(
        k: &Kernel,
        execution_id: &agentype_core::ExecutionId,
    ) -> ExecutionReconciliationSnapshot {
        k.reconciliation_candidates()
            .unwrap()
            .into_iter()
            .find(|s| s.execution_id() == execution_id)
            .expect("candidate")
    }

    fn supervisor(kernel: Arc<Kernel>) -> SupervisionService {
        SupervisionService::new(kernel, &timing()).unwrap()
    }

    fn adapters(fake: &Arc<FakeAdapter>) -> AdapterRegistry {
        let mut adapters = AdapterRegistry::new();
        adapters.register("process", fake.clone()).unwrap();
        adapters
    }

    fn running_obs() -> StartObservation {
        StartObservation {
            state: ExecutionState::Running,
            runtime_handle: RuntimeHandle(json!({"reconciled": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
        }
    }

    /// #38/#42: crash after terminal success evidence, before ACK, replays
    /// into exactly one Result while authority is current.
    #[test]
    fn crash_evidence_before_ack_replays_success() {
        let (_clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("replay-ok", json!({"o": 1})));
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Unknown,
            Some(&json!({"h": 1})),
            Some(&json!({"answer": 7})),
            None,
            false,
            false,
        )
        .unwrap();

        let snap = snapshot_of(&k, launch.execution_id());
        match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
            TerminalReplayOutcome::ResultReplayed { result_id } => {
                let stored = k.result_for_task(launch.task_id()).unwrap();
                assert_eq!(stored.id, result_id);
                assert_eq!(stored.payload["answer"], 7);
            }
            other => panic!("expected ResultReplayed, got {other:?}"),
        }
        assert_eq!(k.task(snap.task_id()).unwrap().state, TaskState::Completed);

        let snap2 = snapshot_of(&k, launch.execution_id());
        match replay_persisted_terminal_consequence(&k, &snap2).unwrap() {
            TerminalReplayOutcome::AlreadyApplied { .. } => {}
            other => panic!("expected AlreadyApplied, got {other:?}"),
        }
    }

    /// #39: crash after failure evidence, before NACK, replays the mechanical
    /// NACK. Does not invent terminality.
    #[test]
    fn crash_evidence_before_nack_replays_failure() {
        let (_clock, k) = env();
        let spec = TaskSpec::new("replay-fail", json!({"o": 1})).retry(RetryPolicy {
            max_attempts: 3,
            retry_classes: vec![FailureClass::Timeout],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        });
        let (_claim, launch) = start_named(&k, spec);
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Unknown,
            Some(&json!({"h": 1})),
            None,
            Some(FailureClass::Timeout),
            false,
            false,
        )
        .unwrap();

        let snap = snapshot_of(&k, launch.execution_id());
        match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
            TerminalReplayOutcome::FailureReplayed { failure_class } => {
                assert_eq!(failure_class, FailureClass::Timeout);
            }
            other => panic!("expected FailureReplayed, got {other:?}"),
        }
        assert_eq!(k.task(snap.task_id()).unwrap().state, TaskState::RetryWait);
        assert!(k.result_for_task(snap.task_id()).is_err());
    }

    /// #41: stale success evidence cannot create a Result.
    #[test]
    fn stale_persisted_success_produces_no_result() {
        let (clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("stale-ok", json!({"o": 1})));
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Unknown,
            Some(&json!({"h": 1})),
            Some(&json!({"answer": 1})),
            None,
            false,
            false,
        )
        .unwrap();
        clock.advance(20.0);
        k.expire_leases(true).unwrap();

        let snap = snapshot_of(&k, launch.execution_id());
        assert!(!snap.current_authority_hint().looks_current());
        match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
            TerminalReplayOutcome::PhysicalHistoryOnly => {}
            other => panic!("expected PhysicalHistoryOnly, got {other:?}"),
        }
        assert!(k.result_for_task(snap.task_id()).is_err());
        assert_ne!(k.task(snap.task_id()).unwrap().state, TaskState::Completed);
    }

    /// #43: SUCCEEDED while Attempt/Lease still current is inconsistent —
    /// Kernel ACK is atomic with authority close.
    #[test]
    fn succeeded_with_current_authority_fails_closed() {
        let (_clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("inconsistent", json!({"o": 1})));
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Succeeded,
            Some(&json!({"h": 1})),
            Some(&json!({"answer": 1})),
            None,
            true,
            true,
        )
        .unwrap();
        let snap = snapshot_of(&k, launch.execution_id());
        assert!(snap.current_authority_hint().looks_current());
        let err = replay_persisted_terminal_consequence(&k, &snap).unwrap_err();
        assert!(
            matches!(err, RecoveryError::Invariant(_)),
            "expected invariant, got {err:?}"
        );
        assert!(k.result_for_task(snap.task_id()).is_err());
    }

    /// #8/#9/#10: STARTING / UNKNOWN / RUNNING → adapter RUNNING → grant →
    /// admitted. `start_execution` is never called. UNKNOWN → RUNNING is legal.
    #[test]
    fn reconcile_running_observation_readmits() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);

        let (_claim, launch) = start_named(&kernel, TaskSpec::new("re-run", json!({"o": 1})));
        kernel
            .record_physical_outcome(
                launch.execution_id(),
                ExecutionState::Unknown,
                Some(&json!({"old": true})),
                None,
                None,
                false,
                false,
            )
            .unwrap();
        let snap = snapshot_of(&kernel, launch.execution_id());
        assert_eq!(snap.persisted_state(), ExecutionState::Unknown);

        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::Readmitted => {}
            other => panic!("expected Readmitted, got {other:?}"),
        }
        assert!(svc.contains(launch.execution_id()));
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(fake.reconcile_call_count(), 1);
        assert_eq!(
            fake.last_reconcile_request_id().as_ref(),
            Some(launch.request_id())
        );
        assert_eq!(
            kernel.execution(launch.execution_id()).unwrap().state,
            ExecutionState::Running
        );
    }

    /// #11/#12: a persisted RUNNING row is not an admission, and adapter
    /// presence alone does not admit. Reconcile that returns unresolved
    /// UNKNOWN never enters supervision.
    #[test]
    fn persisted_running_and_adapter_presence_never_admit_alone() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        assert_eq!(svc.active_count(), 0);

        let (claim, launch) = start_named(&kernel, TaskSpec::new("running-row", json!({"o": 1})));
        kernel
            .confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                launch.execution_id(),
                &json!({"live": true}),
            )
            .unwrap();
        let snap = snapshot_of(&kernel, launch.execution_id());
        assert_eq!(snap.persisted_state(), ExecutionState::Running);
        assert_eq!(svc.active_count(), 0);

        let fake = Arc::new(FakeAdapter::new());
        // Default reconcile is UNKNOWN+ambiguous — adapter is present.
        let adapters = adapters(&fake);
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::Unresolved { failure_class } => {
                assert_eq!(failure_class, FailureClass::ExecutionLost);
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
        assert_eq!(svc.active_count(), 0);
        assert_eq!(fake.start_call_count(), 0);
    }

    /// #13/#14: stale Attempt / expired Lease cannot admit.
    #[test]
    fn stale_authority_cannot_admit_running_observation() {
        let (clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);

        let (_claim, launch) = start_named(&kernel, TaskSpec::new("stale-run", json!({"o": 1})));
        clock.advance(20.0);
        kernel.expire_leases(true).unwrap();
        let snap = snapshot_of(&kernel, launch.execution_id());
        assert!(!snap.current_authority_hint().looks_current());

        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::PhysicalHistoryOnly => {}
            other => panic!("expected PhysicalHistoryOnly, got {other:?}"),
        }
        assert_eq!(svc.active_count(), 0);
        assert_eq!(
            kernel.execution(launch.execution_id()).unwrap().state,
            ExecutionState::Running
        );
        assert!(kernel.result_for_task(snap.task_id()).is_err());
    }

    /// #23/#24/#25: LOST is never admitted. Missing adapter never proves
    /// process death or quiescence.
    #[test]
    fn lost_never_admitted_and_missing_adapter_is_availability_failure() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());

        let spec = TaskSpec::new("lost", json!({"o": 1}))
            .write()
            .retry(RetryPolicy {
                max_attempts: 3,
                retry_classes: vec![
                    FailureClass::ExecutionLost,
                    FailureClass::ResourceUnavailable,
                ],
                base_backoff_seconds: 1.0,
                max_backoff_seconds: 8.0,
            });
        let (_claim, launch) = start_named(&kernel, spec);
        kernel
            .record_physical_outcome(
                launch.execution_id(),
                ExecutionState::Lost,
                Some(&json!({"h": 1})),
                None,
                Some(FailureClass::ExecutionLost),
                false,
                false,
            )
            .unwrap();
        let snap = snapshot_of(&kernel, launch.execution_id());
        let empty = AdapterRegistry::new();
        match reconcile_one_execution(&kernel, &empty, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::Unresolved {
                failure_class: FailureClass::ExecutionLost,
            } => {}
            other => panic!("expected LOST close, got {other:?}"),
        }
        assert_eq!(svc.active_count(), 0);
        // Unisolated WRITE + unknown quiescence → SUSPENDED, not silent retry.
        assert_eq!(
            kernel.task(snap.task_id()).unwrap().state,
            TaskState::Suspended
        );

        let (_clock2, k2) = env();
        let kernel2 = Arc::new(k2);
        let svc2 = supervisor(kernel2.clone());
        let spec2 = TaskSpec::new("no-adapter", json!({"o": 1})).retry(RetryPolicy {
            max_attempts: 3,
            retry_classes: vec![FailureClass::ResourceUnavailable],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        });
        let (_c2, launch2) = start_named(&kernel2, spec2);
        let snap2 = snapshot_of(&kernel2, launch2.execution_id());
        match reconcile_one_execution(&kernel2, &empty, &snap2, &svc2).unwrap() {
            ReconcileExecutionOutcome::Unresolved {
                failure_class: FailureClass::ResourceUnavailable,
            } => {}
            other => panic!("expected missing-adapter close, got {other:?}"),
        }
        assert_eq!(svc2.active_count(), 0);
        assert_ne!(
            kernel2.execution(launch2.execution_id()).unwrap().state,
            ExecutionState::Terminated
        );
    }

    /// #30/#32: terminal-looking reconcile requires collect; collected
    /// success with current authority creates a Result.
    #[test]
    fn terminal_reconcile_requires_collect_and_may_ack() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(StartObservation {
            state: ExecutionState::Succeeded,
            runtime_handle: RuntimeHandle(json!({"done": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(agentype_adapter_api::ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload: Some(json!({"ok": true})),
            summary: Some("done".into()),
            failure_class: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });
        let adapters = adapters(&fake);
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("term", json!({"o": 1})));
        let snap = snapshot_of(&kernel, launch.execution_id());
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::TaskCompleted { .. } => {}
            other => panic!("expected TaskCompleted, got {other:?}"),
        }
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(fake.collect_call_count(), 1);
        assert_eq!(svc.active_count(), 0);
        assert_eq!(
            kernel.task(snap.task_id()).unwrap().state,
            TaskState::Completed
        );
    }

    /// #44/#45/#59: runner starts empty, successful readmission is supervised
    /// at READY, and `start_execution` is never called.
    #[test]
    fn recover_runtime_readmits_and_satisfies_ready_invariant() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("barrier", json!({"o": 1})));
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let recovered = recover_runtime(kernel.clone(), &adapters, timing()).unwrap();
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(fake.reconcile_call_count(), 1);
        assert!(recovered.runner().contains(launch.execution_id()));
        assert_eq!(recovered.runner().active_count(), 1);
        assert_eq!(
            kernel.execution(launch.execution_id()).unwrap().state,
            ExecutionState::Running
        );
    }

    /// #55: recovery itself never dispatches. An empty database is READY
    /// with zero admissions.
    #[test]
    fn recover_runtime_empty_does_not_dispatch() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let fake = Arc::new(FakeAdapter::new());
        let adapters = adapters(&fake);
        let recovered = recover_runtime(kernel.clone(), &adapters, timing()).unwrap();
        assert_eq!(recovered.runner().active_count(), 0);
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(fake.reconcile_call_count(), 0);
    }

    /// #43/#46/#47: a later recovery invariant failure does not return READY
    /// and does not leave a renewable admission behind.
    #[test]
    fn failed_startup_does_not_return_ready() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("bad", json!({"o": 1})));
        kernel
            .record_physical_outcome(
                launch.execution_id(),
                ExecutionState::Succeeded,
                Some(&json!({"h": 1})),
                Some(&json!({"answer": 1})),
                None,
                true,
                true,
            )
            .unwrap();
        let fake = Arc::new(FakeAdapter::new());
        let adapters = adapters(&fake);
        let err = match recover_runtime(kernel.clone(), &adapters, timing()) {
            Err(err) => err,
            Ok(_) => panic!("expected failed startup"),
        };
        assert!(
            matches!(err, RecoveryError::Invariant(_)),
            "expected invariant, got {err:?}"
        );
        assert_eq!(fake.start_call_count(), 0);
    }
}
