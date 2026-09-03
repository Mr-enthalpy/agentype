//! M5.4 restart reconciliation: durable terminal replay and single-Execution
//! reconcile. The coordinator / StartupGuard compose these primitives; they
//! are not an authority path on their own.
//!
//! M5.5 extends StartupGuard so an uncommitted recovery also owns the
//! NotifierRunner. Cleanup signals both services to stop, then joins
//! supervision before notifier, so heartbeat cannot keep renewing while a
//! bounded RootBridge call finishes. Stopping the notifier stops delivery
//! work only: it does not ACK events, revert DELIVERED, revoke a Lease, or
//! terminate a worker.
//!
//! Category A (terminal replay) runs before any `reconcile_start`.
//! Category B (`reconcile_start`) is identity-preserving and never calls
//! `start_execution`. A persisted `state='RUNNING'` row never mints
//! admission: only a fresh `RunningAuthorityGrant` can.

use crate::notifier::{NotifierBinding, NotifierError, NotifierRunner};
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
    Notifier(NotifierError),
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
            Self::Notifier(e) => write!(f, "recovery notifier fault: {e}"),
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

/// Category A: replay a persisted terminal *authority* consequence.
///
/// Physical terminal evidence (`SUCCEEDED`/`FAILED`/`TERMINATED` +
/// `terminal_confirmed`) is a different machine from ACK/NACK. The legal
/// crash window is physical terminal durable, Attempt/Lease still ACTIVE,
/// Result none — never inferred from `outcome_json` on an UNKNOWN row.
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

    match snapshot.persisted_state() {
        ExecutionState::Succeeded => {
            if !snapshot.terminal_confirmed() || snapshot.failure_class().is_some() {
                return Err(RecoveryError::invariant(format!(
                    "execution {} is SUCCEEDED with inconsistent terminal evidence \
                     (terminal_confirmed={}, failure_class={:?})",
                    snapshot.execution_id(),
                    snapshot.terminal_confirmed(),
                    snapshot.failure_class()
                )));
            }
            replay_success_consequence(kernel, snapshot)
        }
        ExecutionState::Failed | ExecutionState::Terminated => {
            if !snapshot.terminal_confirmed() {
                return Err(RecoveryError::invariant(format!(
                    "execution {} is {} without terminal_confirmed",
                    snapshot.execution_id(),
                    snapshot.persisted_state().as_sql()
                )));
            }
            replay_failure_consequence(kernel, snapshot)
        }
        ExecutionState::Starting
        | ExecutionState::Running
        | ExecutionState::Unknown
        | ExecutionState::Lost => Ok(TerminalReplayOutcome::NotApplicable),
    }
}

fn replay_success_consequence(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<TerminalReplayOutcome, RecoveryError> {
    let payload = snapshot.outcome_json().cloned().unwrap_or(Value::Null);
    match kernel.ack_success(
        snapshot.attempt_id(),
        snapshot.lease_epoch(),
        Some(snapshot.execution_id()),
        &payload,
        snapshot.summary(),
        snapshot.quiescent_confirmed(),
        snapshot.incarnation_reusable(),
    ) {
        Ok(Some(result_id)) => Ok(TerminalReplayOutcome::ResultReplayed { result_id }),
        Ok(None) => Ok(TerminalReplayOutcome::WriterSafetySuspended),
        Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
            Ok(TerminalReplayOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(RecoveryError::from(err)),
    }
}

fn replay_failure_consequence(
    kernel: &Kernel,
    snapshot: &ExecutionReconciliationSnapshot,
) -> Result<TerminalReplayOutcome, RecoveryError> {
    let failure_class = snapshot
        .failure_class()
        .unwrap_or(FailureClass::StartFailure);
    match kernel.nack(
        snapshot.attempt_id(),
        snapshot.lease_epoch(),
        failure_class,
        Some(snapshot.execution_id()),
        true,
        snapshot.quiescent_confirmed(),
        snapshot.incarnation_reusable(),
    ) {
        Ok(_) => Ok(TerminalReplayOutcome::FailureReplayed { failure_class }),
        Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
            Ok(TerminalReplayOutcome::PhysicalHistoryOnly)
        }
        Err(err) => Err(RecoveryError::from(err)),
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
    if !snapshot.current_authority_hint().structurally_current() {
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
            kernel
                .record_pending_physical_terminal(
                    snapshot.execution_id(),
                    ExecutionState::Succeeded,
                    Some(&observation.runtime_handle.0),
                    outcome.payload.as_ref(),
                    outcome.summary.as_deref(),
                    None,
                    outcome.quiescent_confirmed,
                    outcome.incarnation_reusable,
                )
                .map_err(RecoveryError::from)?;
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
            kernel
                .record_pending_physical_terminal(
                    snapshot.execution_id(),
                    ExecutionState::Failed,
                    Some(&observation.runtime_handle.0),
                    None,
                    outcome.summary.as_deref(),
                    Some(failure_class),
                    outcome.quiescent_confirmed,
                    outcome.incarnation_reusable,
                )
                .map_err(RecoveryError::from)?;
            match kernel.nack(
                snapshot.attempt_id(),
                snapshot.lease_epoch(),
                failure_class,
                Some(snapshot.execution_id()),
                true,
                outcome.quiescent_confirmed,
                outcome.incarnation_reusable,
            ) {
                Ok(_) => Ok(ReconcileExecutionOutcome::TerminalFailure { failure_class }),
                Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                    Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly)
                }
                Err(err) => Err(RecoveryError::from(err)),
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

/// Process-local startup cleanup scope (M5.4 plan §9, M5.5 §39-§41).
/// Dropping an uncommitted guard stops supervision and notifier and
/// clears every in-memory admission. Cleanup ≠ revoke Lease ≠ terminate
/// worker ≠ quiescence ≠ ACK/revert outbox delivery.
///
/// Stop-signal order: notifier first, then supervision. Join order:
/// supervision first, then notifier. Heartbeat must not continue merely
/// because notifier shutdown is waiting on a bounded RootBridge call.
struct StartupGuard {
    runner: Option<SupervisionRunner>,
    notifier: Option<NotifierRunner>,
    committed: bool,
}

impl StartupGuard {
    fn new(runner: SupervisionRunner, notifier: Option<NotifierRunner>) -> Self {
        Self {
            runner: Some(runner),
            notifier,
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
        if let Some(notifier) = self.notifier.as_ref() {
            if let Some(fatal) = notifier.take_fatal() {
                return Err(RecoveryError::Notifier(fatal));
            }
        }
        Ok(())
    }

    fn abort_uncommitted(&mut self) {
        if let Some(notifier) = self.notifier.as_ref() {
            notifier.request_stop();
        }
        if let Some(runner) = self.runner.as_ref() {
            runner.request_stop();
        }
        if let Some(runner) = self.runner.take() {
            let _ = runner.shutdown();
        }
        if let Some(notifier) = self.notifier.take() {
            let _ = notifier.shutdown();
        }
    }

    fn commit(mut self) -> Result<RecoveredRuntime, RecoveryError> {
        self.check_healthy()?;
        self.committed = true;
        Ok(RecoveredRuntime {
            runner: self.runner.take().expect("startup runner"),
            notifier: self.notifier.take(),
        })
    }
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        if !self.committed {
            self.abort_uncommitted();
        }
    }
}

/// A Runtime that has passed the restart barrier and may dispatch.
///
/// Production recovery with `NotifierBinding::Enabled` owns both runners.
/// There is no API that consumes supervision while dropping notifier
/// (`into_runner` was the silent discard hatch). Test-only recovery without
/// a notifier is `NotifierBinding::DisabledForTests`.
pub struct RecoveredRuntime {
    runner: SupervisionRunner,
    notifier: Option<NotifierRunner>,
}

impl RecoveredRuntime {
    pub fn runner(&self) -> &SupervisionRunner {
        &self.runner
    }

    pub fn notifier(&self) -> Option<&NotifierRunner> {
        self.notifier.as_ref()
    }
}

/// Full recovery barrier. Dispatch MUST NOT run until this returns.
///
/// Order: expire → empty SupervisionRunner → NotifierRunner (same
/// uncommitted cleanup scope) → terminal replay → reconcile
/// STARTING/UNKNOWN/RUNNING/LOST → final expiry sweep → promote retry →
/// pool → revive → both runners healthy → READY.
///
/// Delivery during RECOVERY is legal: a wakeup asserts a durable event
/// exists, not that the daemon is READY. Ordinary RootBridge unavailability
/// does not prevent READY. Durable notifier corruption does.
pub fn recover_runtime(
    kernel: Arc<Kernel>,
    adapters: &AdapterRegistry,
    timing: RuntimeTimingConfig,
    notifier: NotifierBinding,
) -> Result<RecoveredRuntime, RecoveryError> {
    recover_runtime_inner(kernel, adapters, timing, notifier, None)
}

/// Explicit test-only recovery without a notifier. Does NOT mark outbox
/// events DELIVERED and is not a production "no RootBridge" success path.
pub fn recover_runtime_without_notifier(
    kernel: Arc<Kernel>,
    adapters: &AdapterRegistry,
    timing: RuntimeTimingConfig,
) -> Result<RecoveredRuntime, RecoveryError> {
    recover_runtime(kernel, adapters, timing, NotifierBinding::DisabledForTests)
}

fn recover_runtime_inner(
    kernel: Arc<Kernel>,
    adapters: &AdapterRegistry,
    timing: RuntimeTimingConfig,
    notifier: NotifierBinding,
    fail_after_readmits: Option<usize>,
) -> Result<RecoveredRuntime, RecoveryError> {
    kernel.expire_leases(true).map_err(RecoveryError::from)?;

    let runner =
        SupervisionRunner::start(kernel.clone(), timing).map_err(RecoveryError::Supervision)?;
    let notifier_runner = match notifier {
        NotifierBinding::Enabled { config, bridge } => Some(
            NotifierRunner::start(kernel.clone(), bridge, config)
                .map_err(RecoveryError::Notifier)?,
        ),
        NotifierBinding::DisabledForTests => None,
    };
    let guard = StartupGuard::new(runner, notifier_runner);
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
    let mut readmits = 0usize;
    for snap in &candidates {
        guard.check_healthy()?;
        match snap.persisted_state() {
            ExecutionState::Starting | ExecutionState::Unknown | ExecutionState::Running => {
                if matches!(
                    reconcile_one_execution(&kernel, adapters, snap, guard.runner())?,
                    ReconcileExecutionOutcome::Readmitted
                ) {
                    readmits += 1;
                    if fail_after_readmits == Some(readmits) {
                        return Err(RecoveryError::invariant(
                            "injected startup fatal after successful readmission",
                        ));
                    }
                }
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
    assert_ready_invariant(&kernel, guard.runner())?;
    guard.commit()
}

#[cfg(test)]
fn recover_runtime_failing_after_readmits(
    kernel: Arc<Kernel>,
    adapters: &AdapterRegistry,
    timing: RuntimeTimingConfig,
    after: usize,
) -> Result<RecoveredRuntime, RecoveryError> {
    recover_runtime_inner(
        kernel,
        adapters,
        timing,
        NotifierBinding::DisabledForTests,
        Some(after),
    )
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
        if !snap.current_authority_hint().looks_current_at(kernel.now()) {
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
    use crate::notifier::{NotifierBinding, NotifierConfig, NotifierRetryPolicy};
    use crate::supervision::RenewalOutcome;
    use crate::timing::RuntimeTimingConfig;
    use crate::{AdapterRegistry, FrozenExecutionSafety, FrozenPhysicalExecutionBinding};
    use agentype_adapter_api::{
        AdapterResult, ExecutionAdapter, ExecutionObservation, ExecutionOutcome, FakeAdapter,
        RuntimeHandle, StartObservation,
    };
    use agentype_core::{
        AttemptState, AuthoritativeExecutionBinding, Claim, Clock, ExecutionState, FailureClass,
        IncarnationState, LeaseState, ManualClock, OutboxState, PartitionSpec, Retention,
        RetryPolicy, TaskSpec, TaskState, BATCH_RESULTS_READY,
    };
    use agentype_root_bridge::{RecordingRootBridge, RootBridgeError};
    use agentype_storage_sqlite::Kernel;
    use serde_json::json;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    fn timing() -> RuntimeTimingConfig {
        RuntimeTimingConfig::new(1.0, 2.0, 10.0).unwrap()
    }

    fn notifier_cfg() -> NotifierConfig {
        NotifierConfig::new(0.05, 8, NotifierRetryPolicy::new(1.0, 8.0).unwrap()).unwrap()
    }

    fn complete_batch(k: &Kernel) -> agentype_core::OutboxEventId {
        let (claim, launch) = start_named(k, TaskSpec::new("notify", json!({"o": 1})));
        k.confirm_running_and_renew(
            &claim.attempt_id,
            claim.lease_epoch,
            launch.execution_id(),
            &json!({}),
        )
        .unwrap();
        k.ack_success(
            &claim.attempt_id,
            claim.lease_epoch,
            Some(launch.execution_id()),
            &json!({"ok": true}),
            None,
            true,
            false,
        )
        .unwrap();
        k.outbox_for_batch(launch.batch_id(), BATCH_RESULTS_READY)
            .unwrap()[0]
            .id
            .clone()
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

    /// #38/#42: crash after physical SUCCEEDED is durable, before ACK,
    /// replays into exactly one Result while authority is current.
    #[test]
    fn crash_evidence_before_ack_replays_success() {
        let (_clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("replay-ok", json!({"o": 1})));
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Succeeded,
            Some(&json!({"h": 1})),
            Some(&json!({"answer": 7})),
            None,
            true,
            true,
        )
        .unwrap();

        let snap = snapshot_of(&k, launch.execution_id());
        assert!(snap.current_authority_hint().structurally_current());
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

    /// #39: crash after physical FAILED is durable, before NACK, replays
    /// the fenced NACK. Does not create a Result.
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
            ExecutionState::Failed,
            Some(&json!({"h": 1})),
            None,
            Some(FailureClass::Timeout),
            true,
            true,
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

    /// #41: stale physical success cannot create a Result.
    #[test]
    fn stale_persisted_success_produces_no_result() {
        let (clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("stale-ok", json!({"o": 1})));
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
        clock.advance(20.0);
        k.expire_leases(true).unwrap();

        let snap = snapshot_of(&k, launch.execution_id());
        assert!(!snap.current_authority_hint().structurally_current());
        match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
            TerminalReplayOutcome::PhysicalHistoryOnly => {}
            other => panic!("expected PhysicalHistoryOnly, got {other:?}"),
        }
        assert!(k.result_for_task(snap.task_id()).is_err());
        assert_ne!(k.task(snap.task_id()).unwrap().state, TaskState::Completed);
    }

    /// Crash-before-ACK must equal no-crash for summary + reusable WARM.
    #[test]
    fn crash_replay_matches_live_ack_for_reusable_success() {
        fn run(
            crash: bool,
        ) -> (
            String,
            serde_json::Value,
            TaskState,
            AttemptState,
            LeaseState,
            IncarnationState,
        ) {
            let (_clock, k) = env();
            let (claim, launch) = start_named(&k, TaskSpec::new("eq", json!({"o": 1})));
            k.record_pending_physical_terminal(
                launch.execution_id(),
                ExecutionState::Succeeded,
                Some(&json!({"session": 7})),
                Some(&json!({"ok": true})),
                Some("implemented parser"),
                None,
                true,
                true,
            )
            .unwrap();
            if crash {
                let snap = snapshot_of(&k, launch.execution_id());
                match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
                    TerminalReplayOutcome::ResultReplayed { .. } => {}
                    other => panic!("expected ResultReplayed, got {other:?}"),
                }
            } else {
                k.ack_success(
                    &claim.attempt_id,
                    claim.lease_epoch,
                    Some(launch.execution_id()),
                    &json!({"ok": true}),
                    Some("implemented parser"),
                    true,
                    true,
                )
                .unwrap();
            }
            let stored = k.result_for_task(launch.task_id()).unwrap();
            let task = k.task(launch.task_id()).unwrap().state;
            let attempt = k.attempt(&claim.attempt_id).unwrap().state;
            let lease = k.lease_supervision_view(&claim.attempt_id).unwrap().state;
            let inc = k.incarnation(launch.incarnation_id()).unwrap().state;
            (
                stored.summary.clone().unwrap_or_default(),
                stored.payload,
                task,
                attempt,
                lease,
                inc,
            )
        }

        let live = run(false);
        let crashed = run(true);
        assert_eq!(live.0, "implemented parser");
        assert_eq!(crashed, live);
        assert_eq!(live.5, IncarnationState::Warm);
    }

    /// Crash-before-NACK must equal live NACK for reusable terminal failure.
    #[test]
    fn crash_replay_matches_live_nack_for_reusable_failure() {
        fn run(crash: bool) -> (TaskState, AttemptState, IncarnationState) {
            let (_clock, k) = env();
            let spec = TaskSpec::new("fail-eq", json!({"o": 1})).retry(RetryPolicy {
                max_attempts: 3,
                retry_classes: vec![FailureClass::Timeout],
                base_backoff_seconds: 1.0,
                max_backoff_seconds: 8.0,
            });
            let (claim, launch) = start_named(&k, spec);
            k.record_pending_physical_terminal(
                launch.execution_id(),
                ExecutionState::Failed,
                Some(&json!({"session": 7})),
                None,
                Some("worker failed"),
                Some(FailureClass::Timeout),
                true,
                true,
            )
            .unwrap();
            if crash {
                let snap = snapshot_of(&k, launch.execution_id());
                match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
                    TerminalReplayOutcome::FailureReplayed { .. } => {}
                    other => panic!("expected FailureReplayed, got {other:?}"),
                }
            } else {
                k.nack(
                    &claim.attempt_id,
                    claim.lease_epoch,
                    FailureClass::Timeout,
                    Some(launch.execution_id()),
                    true,
                    true,
                    true,
                )
                .unwrap();
            }
            assert!(k.result_for_task(launch.task_id()).is_err());
            (
                k.task(launch.task_id()).unwrap().state,
                k.attempt(&claim.attempt_id).unwrap().state,
                k.incarnation(launch.incarnation_id()).unwrap().state,
            )
        }

        let live = run(false);
        let crashed = run(true);
        assert_eq!(crashed, live);
        assert_eq!(live.2, IncarnationState::Warm);
        assert_eq!(live.0, TaskState::RetryWait);
    }

    /// Old UNKNOWN failure_class must not contaminate a later terminal success.
    #[test]
    fn pending_terminal_success_overwrites_old_failure_class() {
        let (_clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("overwrite-class", json!({"o": 1})));
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Unknown,
            Some(&json!({"h": 1})),
            None,
            Some(FailureClass::ExecutionLost),
            false,
            false,
        )
        .unwrap();
        k.record_pending_physical_terminal(
            launch.execution_id(),
            ExecutionState::Succeeded,
            Some(&json!({"h": 1})),
            Some(&json!({"ok": true})),
            Some("done"),
            None,
            true,
            false,
        )
        .unwrap();
        let snap = snapshot_of(&k, launch.execution_id());
        assert!(snap.failure_class().is_none());
        match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
            TerminalReplayOutcome::ResultReplayed { .. } => {}
            other => panic!("expected ResultReplayed, got {other:?}"),
        }
        let stored = k.result_for_task(launch.task_id()).unwrap();
        assert_eq!(stored.payload, json!({"ok": true}));
        assert_eq!(stored.summary.as_deref(), Some("done"));
    }

    /// Old UNKNOWN outcome_json must not become the Result body when the
    /// collected success payload is None.
    #[test]
    fn pending_terminal_success_overwrites_old_outcome_json() {
        let (_clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("overwrite-payload", json!({"o": 1})));
        k.record_physical_outcome(
            launch.execution_id(),
            ExecutionState::Unknown,
            Some(&json!({"h": 1})),
            Some(&json!({"stale": true})),
            None,
            false,
            false,
        )
        .unwrap();
        k.record_pending_physical_terminal(
            launch.execution_id(),
            ExecutionState::Succeeded,
            Some(&json!({"h": 1})),
            None,
            None,
            None,
            true,
            false,
        )
        .unwrap();
        let snap = snapshot_of(&k, launch.execution_id());
        match replay_persisted_terminal_consequence(&k, &snap).unwrap() {
            TerminalReplayOutcome::ResultReplayed { .. } => {}
            other => panic!("expected ResultReplayed, got {other:?}"),
        }
        let stored = k.result_for_task(launch.task_id()).unwrap();
        assert_eq!(stored.payload, serde_json::Value::Null);
    }

    /// UNKNOWN + outcome_json is NOT authoritative terminal success proof.
    #[test]
    fn unknown_plus_outcome_json_is_not_success_evidence() {
        let (_clock, k) = env();
        let (_claim, launch) = start_named(&k, TaskSpec::new("not-proof", json!({"o": 1})));
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
            TerminalReplayOutcome::NotApplicable => {}
            other => panic!("expected NotApplicable, got {other:?}"),
        }
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
        assert!(!snap.current_authority_hint().structurally_current());

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
        let recovered =
            recover_runtime_without_notifier(kernel.clone(), &adapters, timing()).unwrap();
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
        let recovered =
            recover_runtime_without_notifier(kernel.clone(), &adapters, timing()).unwrap();
        assert_eq!(recovered.runner().active_count(), 0);
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(fake.reconcile_call_count(), 0);
    }

    /// Successful readmission then later startup fatal: StartupGuard stops
    /// the runner, clears admissions, and the lease is not renewed further.
    #[test]
    fn successful_readmission_then_later_fatal_clears_admissions() {
        let (clock, k) = env();
        let kernel = Arc::new(k);
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("then-fatal", json!({"o": 1})));
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let err =
            match recover_runtime_failing_after_readmits(kernel.clone(), &adapters, timing(), 1) {
                Err(err) => err,
                Ok(_) => panic!("expected injected startup fatal"),
            };
        assert!(
            matches!(err, RecoveryError::Invariant(_)),
            "expected invariant, got {err:?}"
        );
        let after_fail = kernel
            .lease_supervision_view(&_claim.attempt_id)
            .unwrap()
            .heartbeat_at;
        clock.advance(1.0);
        let later = kernel
            .lease_supervision_view(&_claim.attempt_id)
            .unwrap()
            .heartbeat_at;
        assert_eq!(later, after_fail);
        assert_eq!(fake.start_call_count(), 0);
        let _ = launch;
    }

    /// ACK before re-admission: grant rejected, no admit, Task never reopens.
    #[test]
    fn ack_wins_before_re_admission() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let (claim, launch) = start_named(&kernel, TaskSpec::new("ack-first", json!({"o": 1})));
        kernel
            .confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                launch.execution_id(),
                &json!({"live": true}),
            )
            .unwrap();
        kernel
            .ack_success(
                &claim.attempt_id,
                claim.lease_epoch,
                Some(launch.execution_id()),
                &json!({"ok": true}),
                None,
                true,
                false,
            )
            .unwrap();
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let snap = snapshot_of(&kernel, launch.execution_id());
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc) {
            Err(RecoveryError::Invariant(_)) => {}
            Ok(ReconcileExecutionOutcome::PhysicalHistoryOnly) => {}
            other => panic!("expected no admission after ACK, got {other:?}"),
        }
        assert_eq!(svc.active_count(), 0);
        assert_eq!(
            kernel.task(launch.task_id()).unwrap().state,
            TaskState::Completed
        );
        assert_eq!(fake.start_call_count(), 0);
    }

    /// Re-admission before ACK: lease briefly renewed, ACK closes, later
    /// heartbeat is AuthorityLost. Task never reopens.
    #[test]
    fn re_admission_wins_before_ack() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let (claim, launch) = start_named(&kernel, TaskSpec::new("grant-first", json!({"o": 1})));
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let snap = snapshot_of(&kernel, launch.execution_id());
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::Readmitted => {}
            other => panic!("expected Readmitted, got {other:?}"),
        }
        assert!(svc.contains(launch.execution_id()));
        kernel
            .ack_success(
                &claim.attempt_id,
                claim.lease_epoch,
                Some(launch.execution_id()),
                &json!({"ok": true}),
                None,
                true,
                false,
            )
            .unwrap();
        assert_eq!(
            svc.renew_one(launch.execution_id()).unwrap(),
            RenewalOutcome::AuthorityLost {
                execution_id: launch.execution_id().clone(),
            }
        );
        assert_eq!(
            kernel.task(launch.task_id()).unwrap().state,
            TaskState::Completed
        );
        assert!(!svc.contains(launch.execution_id()));
        assert_eq!(fake.start_call_count(), 0);
    }

    /// Cancellation before re-admission: no grant.
    #[test]
    fn cancellation_wins_before_re_admission() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("cancel-first", json!({"o": 1})));
        kernel.cancel_task(launch.task_id(), false).unwrap();
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let snap = snapshot_of(&kernel, launch.execution_id());
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::PhysicalHistoryOnly => {}
            other => panic!("expected PhysicalHistoryOnly, got {other:?}"),
        }
        assert_eq!(svc.active_count(), 0);
        assert_eq!(
            kernel.task(launch.task_id()).unwrap().state,
            TaskState::Cancelled
        );
        assert_eq!(fake.start_call_count(), 0);
    }

    /// Re-admission before cancellation: cancel later closes authority.
    #[test]
    fn re_admission_wins_before_cancellation() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let (_claim, launch) =
            start_named(&kernel, TaskSpec::new("grant-then-cancel", json!({"o": 1})));
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let snap = snapshot_of(&kernel, launch.execution_id());
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::Readmitted => {}
            other => panic!("expected Readmitted, got {other:?}"),
        }
        kernel.cancel_task(launch.task_id(), false).unwrap();
        assert_eq!(
            svc.renew_one(launch.execution_id()).unwrap(),
            RenewalOutcome::AuthorityLost {
                execution_id: launch.execution_id().clone(),
            }
        );
        assert_eq!(
            kernel.task(launch.task_id()).unwrap().state,
            TaskState::Cancelled
        );
        assert_eq!(fake.start_call_count(), 0);
    }

    /// Exact expiry boundary during recovery grant is STALE.
    #[test]
    fn exact_expiry_boundary_cannot_admit_during_recovery() {
        let (clock, k) = env();
        let kernel = Arc::new(k);
        let svc = supervisor(kernel.clone());
        let (claim, launch) = start_named(&kernel, TaskSpec::new("exact-exp", json!({"o": 1})));
        let expiry = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        clock.advance(expiry - kernel.now());
        assert_eq!(kernel.now(), expiry);
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let snap = snapshot_of(&kernel, launch.execution_id());
        match reconcile_one_execution(&kernel, &adapters, &snap, &svc).unwrap() {
            ReconcileExecutionOutcome::PhysicalHistoryOnly => {}
            other => panic!("expected STALE PhysicalHistoryOnly, got {other:?}"),
        }
        assert_eq!(svc.active_count(), 0);
        assert_eq!(fake.start_call_count(), 0);
    }

    /// Lease expires during adapter I/O; final sweep leaves no unsupervised
    /// current authority at READY.
    #[test]
    fn lease_expiring_during_adapter_io_is_swept_before_ready() {
        let (clock, k) = env();
        let kernel = Arc::new(k);
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("io-expire", json!({"o": 1})));
        let inner = FakeAdapter::new();
        inner.set_next_reconcile(running_obs());
        let advancing = Arc::new(ClockAdvancingAdapter {
            inner,
            clock: clock.clone(),
            advance: 20.0,
        });
        let mut adapters = AdapterRegistry::new();
        adapters.register("process", advancing.clone()).unwrap();
        let recovered =
            recover_runtime_without_notifier(kernel.clone(), &adapters, timing()).unwrap();
        assert_eq!(recovered.runner().active_count(), 0);
        assert!(!recovered.runner().contains(launch.execution_id()));
        assert_ne!(
            kernel.task(launch.task_id()).unwrap().state,
            TaskState::Running
        );
        assert_eq!(advancing.inner.start_call_count(), 0);
    }

    #[test]
    fn recovery_starts_notifier_and_returns_ownership() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let event = complete_batch(&kernel);
        let fake = Arc::new(FakeAdapter::new());
        let adapters = adapters(&fake);
        let bridge = Arc::new(RecordingRootBridge::new());
        let recovered = recover_runtime(
            kernel.clone(),
            &adapters,
            timing(),
            NotifierBinding::Enabled {
                config: notifier_cfg(),
                bridge: bridge.clone(),
            },
        )
        .unwrap();
        assert!(recovered.notifier().is_some());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while kernel.outbox_delivery(&event).unwrap().state != OutboxState::Delivered {
            assert!(
                std::time::Instant::now() < deadline,
                "notifier did not deliver during/after recovery"
            );
            thread::sleep(Duration::from_millis(10));
        }
        drop(recovered);
    }

    #[test]
    fn ordinary_rootbridge_unavailability_does_not_prevent_ready() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let event = complete_batch(&kernel);
        let fake = Arc::new(FakeAdapter::new());
        let adapters = adapters(&fake);
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.script_err(RootBridgeError::Unavailable("root down".into()));
        let recovered = recover_runtime(
            kernel.clone(),
            &adapters,
            timing(),
            NotifierBinding::Enabled {
                config: notifier_cfg(),
                bridge,
            },
        )
        .unwrap();
        assert!(recovered.notifier().is_some());
        assert!(!recovered.notifier().unwrap().is_failed());
        thread::sleep(Duration::from_millis(80));
        assert_eq!(
            kernel.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );
        drop(recovered);
    }

    #[test]
    fn later_recovery_fatal_stops_notifier_and_supervision() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let event = complete_batch(&kernel);
        let (_claim, launch) = start_named(&kernel, TaskSpec::new("fatal-both", json!({"o": 1})));
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_reconcile(running_obs());
        let adapters = adapters(&fake);
        let bridge = Arc::new(RecordingRootBridge::new());
        let err = match recover_runtime_inner(
            kernel.clone(),
            &adapters,
            timing(),
            NotifierBinding::Enabled {
                config: notifier_cfg(),
                bridge: bridge.clone(),
            },
            Some(1),
        ) {
            Err(err) => err,
            Ok(_) => panic!("expected injected startup fatal"),
        };
        assert!(matches!(err, RecoveryError::Invariant(_)));
        thread::sleep(Duration::from_millis(50));
        let delivered_after = bridge.deliver_count();
        thread::sleep(Duration::from_millis(80));
        assert_eq!(
            bridge.deliver_count(),
            delivered_after,
            "notifier must not keep selecting after failed recovery"
        );
        let _ = (event, launch);
    }

    #[test]
    fn delivered_event_is_not_redelivered_after_restart() {
        let (_clock, k) = env();
        let kernel = Arc::new(k);
        let event = complete_batch(&kernel);
        kernel.commit_outbox_delivery_success(&event).unwrap();
        let fake = Arc::new(FakeAdapter::new());
        let adapters = adapters(&fake);
        let bridge = Arc::new(RecordingRootBridge::new());
        let recovered = recover_runtime(
            kernel.clone(),
            &adapters,
            timing(),
            NotifierBinding::Enabled {
                config: notifier_cfg(),
                bridge: bridge.clone(),
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(80));
        assert_eq!(bridge.deliver_count(), 0);
        assert_eq!(
            kernel.outbox_delivery(&event).unwrap().state,
            OutboxState::Delivered
        );
        drop(recovered);
    }

    #[test]
    fn pending_future_backoff_stays_deferred_after_restart() {
        let (clock, k) = env();
        let kernel = Arc::new(k);
        let event = complete_batch(&kernel);
        kernel
            .commit_outbox_delivery_failure(&event, 30.0, "later")
            .unwrap();
        let fake = Arc::new(FakeAdapter::new());
        let adapters = adapters(&fake);
        let bridge = Arc::new(RecordingRootBridge::new());
        let recovered = recover_runtime(
            kernel.clone(),
            &adapters,
            timing(),
            NotifierBinding::Enabled {
                config: notifier_cfg(),
                bridge: bridge.clone(),
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(80));
        assert_eq!(bridge.deliver_count(), 0);
        assert_eq!(
            kernel.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );
        assert!(kernel.outbox_delivery(&event).unwrap().next_delivery_at > clock.now());
        drop(recovered);
    }

    struct ClockAdvancingAdapter {
        inner: FakeAdapter,
        clock: Arc<ManualClock>,
        advance: f64,
    }

    impl ExecutionAdapter for ClockAdvancingAdapter {
        fn start_execution(
            &self,
            request: &agentype_adapter_api::ExecutionRequest,
        ) -> AdapterResult<StartObservation> {
            self.inner.start_execution(request)
        }

        fn observe_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation> {
            self.inner.observe_execution(handle)
        }

        fn interrupt_execution(
            &self,
            handle: &RuntimeHandle,
        ) -> AdapterResult<ExecutionObservation> {
            self.inner.interrupt_execution(handle)
        }

        fn terminate_execution(
            &self,
            handle: &RuntimeHandle,
        ) -> AdapterResult<ExecutionObservation> {
            self.inner.terminate_execution(handle)
        }

        fn collect_outcome(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionOutcome> {
            self.inner.collect_outcome(handle)
        }

        fn reconcile_start(
            &self,
            request_id: &agentype_core::RequestId,
            persisted_handle: Option<&RuntimeHandle>,
        ) -> AdapterResult<StartObservation> {
            self.clock.advance(self.advance);
            self.inner.reconcile_start(request_id, persisted_handle)
        }
    }
}
