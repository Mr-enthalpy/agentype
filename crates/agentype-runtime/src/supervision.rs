//! M5.3 supervision: admission authority, runtime-local ownership, and
//! fenced heartbeat renewal.
//!
//! Authority model (M5.3 plan §3):
//!
//! - **Scheduler Lease authority** (durable, Kernel-owned): Attempt, Lease,
//!   `lease_epoch`, `task.current_attempt_id`, `task.fencing_epoch`.
//! - **SupervisionAdmission** (runtime permission): proves only that at one
//!   specific moment this runtime observed RUNNING and successfully committed
//!   the fenced RUNNING + first-renewal transaction for this Attempt. It is
//!   minted exclusively as the live return value of that transaction — a
//!   persisted `state='RUNNING'` Execution row can never produce one.
//! - **Supervision ownership** (ephemeral, runtime-local): "this runtime
//!   instance currently owns heartbeat responsibility for this admitted
//!   Execution". It is NOT reconstructed from SQLite on startup; after a
//!   restart the supervision set is empty, and re-establishing it is
//!   exclusively an M5.4 reconciliation responsibility.
//!
//! Registry presence means only "this runtime currently intends to renew
//! this admitted authority" — never "the Execution is alive", "the worker
//! exists", or "the Task is running". The database remains authoritative for
//! whether a renewal actually succeeds.
//!
//! Crash window (M5.3 plan §20): if the runtime crashes after the first
//! renewal is committed but before the admission is inserted, no further
//! renewal occurs, the Lease eventually expires, and M5.4 reconciliation
//! handles the physical reality. The window is one-directional and
//! fail-closed; no durable supervision table is created to eliminate it.

use crate::timing::{validate_lease_authority_match, RuntimeTimingConfig};
use crate::{Error, ExecutionId, SupervisionAdmission};
use agentype_core::UnixTime;
use agentype_storage_sqlite::{Kernel, SupervisedRenewal};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Mechanical outcome of one periodic heartbeat renewal (M5.3 §31).
///
/// Persistence/invariant faults are never flattened into these outcomes:
/// they surface as `Err(SupervisionError::Fatal)`.
#[derive(Debug, Clone, PartialEq)]
pub enum RenewalOutcome {
    /// The Scheduler continues granting the current Attempt execution
    /// authority for another lease interval. This proves nothing about
    /// worker progress, quiescence, or Task success.
    Renewed {
        execution_id: ExecutionId,
        new_expires_at: UnixTime,
    },
    /// The admission's durable authority was rejected (stale/invalid
    /// authority, expired lease, closed Attempt). Supervision ownership is
    /// dropped; the old admission is consumed and can never be re-used.
    /// This is NOT proof that the physical writer stopped and establishes no
    /// quiescence or terminality.
    AuthorityLost { execution_id: ExecutionId },
    /// The Execution exists and belongs to the Attempt behind still-valid
    /// authority, but is no longer physically RUNNING. Supervision ownership
    /// is dropped; the durable physical state is never repaired from
    /// heartbeat code (M5.4 reconciliation owns it).
    NoLongerRunning { execution_id: ExecutionId },
}

/// Supervision failures. Ordinary authority loss is an outcome, not an
/// error; these are structural or fatal conditions (M5.3 §15).
#[derive(Debug, Clone, PartialEq)]
pub enum SupervisionError {
    /// The admission token was already consumed (removed after authority
    /// loss, terminal handling, or shutdown). There is NO
    /// Dropped → Admitted transition without a fresh authoritative mint.
    AdmissionConsumed,
    /// The same ExecutionId was presented with a different attempt_id,
    /// lease_epoch, or request_id. Never silently replaced.
    AdmissionIdentityConflict,
    /// No registry entry under this execution id.
    NoSuchEntry,
    /// The composed timing configuration disagrees with the Kernel's actual
    /// lease authority. The supervisor must never compute renewal durations
    /// independently from the Kernel.
    LeaseAuthorityMismatch { configured: f64, kernel: f64 },
    /// A persistence or invariant fault during renewal: SQLite failure,
    /// durable corruption, recovery-required condition. Fail closed — the
    /// supervision loop stops and surfaces the fault; it is never classified
    /// as ordinary authority loss.
    Fatal(Error),
}

impl fmt::Display for SupervisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdmissionConsumed => {
                write!(f, "supervision admission token was already consumed")
            }
            Self::AdmissionIdentityConflict => write!(
                f,
                "supervision admission identity conflicts with the existing entry"
            ),
            Self::NoSuchEntry => write!(f, "no supervision entry for this execution"),
            Self::LeaseAuthorityMismatch { configured, kernel } => write!(
                f,
                "configured lease_seconds ({configured}) does not match the Kernel lease authority ({kernel})"
            ),
            Self::Fatal(e) => write!(f, "fatal supervision persistence fault: {e}"),
        }
    }
}

impl std::error::Error for SupervisionError {}

/// One supervised execution entry. Runtime-local bookkeeping only.
#[derive(Debug, Clone)]
struct SupervisedExecution {
    admission: SupervisionAdmission,
    last_renewal_at: UnixTime,
}

/// In-memory registry of the executions THIS runtime instance currently owns
/// for heartbeat supervision (M5.3 §7).
///
/// It starts empty on construction, is never restored from the database, and
/// is authoritative only for "which executions this runtime may attempt to
/// renew". Duplicate insertion is idempotent only when the admission
/// identity is exactly identical; the same ExecutionId with a different
/// attempt/epoch/request is an invariant violation (M5.3 §8). A removed
/// admission's generation is consumed: re-presenting the same token can
/// never resume renewal — only a fresh authoritative mint may re-admit
/// (M5.3 §16/§41; the future M5.4 re-admission flow mints fresh tokens and
/// is unaffected).
#[derive(Default)]
pub struct SupervisionRegistry {
    entries: HashMap<ExecutionId, SupervisedExecution>,
    consumed_generations: HashSet<u64>,
}

impl SupervisionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit an execution for heartbeat supervision. The admission must be
    /// the live return value of a successful fenced RUNNING confirmation +
    /// first renewal transaction.
    pub fn admit(
        &mut self,
        admission: SupervisionAdmission,
        now: UnixTime,
    ) -> Result<(), SupervisionError> {
        if self.consumed_generations.contains(&admission.generation()) {
            return Err(SupervisionError::AdmissionConsumed);
        }
        match self.entries.get(admission.execution_id()) {
            Some(existing) if existing.admission.same_identity(&admission) => {
                // Exactly identical identity: idempotent re-admission of the
                // same authority.
                Ok(())
            }
            Some(_) => Err(SupervisionError::AdmissionIdentityConflict),
            None => {
                self.entries.insert(
                    admission.execution_id().clone(),
                    SupervisedExecution {
                        last_renewal_at: now,
                        admission,
                    },
                );
                Ok(())
            }
        }
    }

    /// Remove supervision ownership. This means ONLY "this runtime no
    /// longer owns lease-renewal responsibility" — it never mutates
    /// Execution state, never claims quiescence, and never revokes or
    /// completes Task authority by itself.
    pub fn remove(&mut self, execution_id: &ExecutionId) -> Result<(), SupervisionError> {
        let entry = self
            .entries
            .remove(execution_id)
            .ok_or(SupervisionError::NoSuchEntry)?;
        self.consumed_generations
            .insert(entry.admission.generation());
        Ok(())
    }

    /// Drop ALL supervision ownership (local shutdown). No Lease is revoked,
    /// no Execution is marked terminal, and nothing is claimed quiescent —
    /// the Leases simply stop being renewed and naturally expire (M5.3 §33).
    pub fn clear(&mut self) {
        for (_, entry) in self.entries.drain() {
            self.consumed_generations
                .insert(entry.admission.generation());
        }
    }

    pub fn contains(&self, execution_id: &ExecutionId) -> bool {
        self.entries.contains_key(execution_id)
    }

    pub fn active_count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry_admission(&self, execution_id: &ExecutionId) -> Option<SupervisionAdmission> {
        self.entries.get(execution_id).map(|e| e.admission.clone())
    }

    /// Snapshot the admissions due for renewal at `now`, cloned out so the
    /// caller never holds the registry lock across the DB transaction
    /// (M5.3 §51).
    fn due(&self, now: UnixTime, interval_seconds: f64) -> Vec<SupervisionAdmission> {
        self.entries
            .values()
            .filter(|entry| now >= entry.last_renewal_at + interval_seconds)
            .map(|entry| entry.admission.clone())
            .collect()
    }

    /// Apply a successful renewal result, but only if the entry's admission
    /// generation is still the one the renewal was performed for — an old
    /// renewal result must never mutate a newer registry entry (M5.3 §52).
    fn record_renewal(&mut self, admission: &SupervisionAdmission, at: UnixTime) {
        let current = self
            .entries
            .get(admission.execution_id())
            .is_some_and(|entry| entry.admission.generation() == admission.generation());
        if current {
            if let Some(entry) = self.entries.get_mut(admission.execution_id()) {
                entry.last_renewal_at = at;
            }
        }
    }

    /// Apply a drop result (authority loss / no-longer-running), again only
    /// if the generation is still current.
    fn remove_if_current(&mut self, admission: &SupervisionAdmission) {
        let current = self
            .entries
            .get(admission.execution_id())
            .is_some_and(|entry| entry.admission.generation() == admission.generation());
        if current {
            drop(self.entries.remove(admission.execution_id()));
            self.consumed_generations.insert(admission.generation());
        }
    }
}

/// Deterministic supervision service (M5.3 §29): owns the registry and the
/// heartbeat policy; exposes single-step operations so tests validate
/// correctness without real-time sleeps. The long-running loop
/// (`SupervisionRunner`) shares the same kernel + registry and wraps these
/// operations in a dedicated thread.
///
/// Cloning the service shares the same registry: the composition layer
/// admits through one handle while the heartbeat loop renews through
/// another.
pub struct SupervisionService {
    kernel: Arc<Kernel>,
    registry: Arc<Mutex<SupervisionRegistry>>,
    heartbeat_interval: Duration,
}

impl Clone for SupervisionService {
    fn clone(&self) -> Self {
        Self {
            kernel: self.kernel.clone(),
            registry: self.registry.clone(),
            heartbeat_interval: self.heartbeat_interval,
        }
    }
}

// The runner shares a Kernel across threads; make the requirement explicit.
fn _assert_kernel_shareable(kernel: &Kernel) {
    fn requirement<T: Send + Sync>(_: &T) {}
    requirement(kernel);
}

impl SupervisionService {
    /// Compose the service against a Kernel. The configured lease duration
    /// must exactly match the Kernel's lease authority; the supervisor never
    /// decides lease extension durations independently (M5.3 §30).
    pub fn new(
        kernel: Arc<Kernel>,
        timing: &RuntimeTimingConfig,
    ) -> Result<Self, SupervisionError> {
        if validate_lease_authority_match(timing, kernel.lease_seconds()).is_err() {
            return Err(SupervisionError::LeaseAuthorityMismatch {
                configured: timing.lease_seconds(),
                kernel: kernel.lease_seconds(),
            });
        }
        Ok(Self {
            kernel,
            registry: Arc::new(Mutex::new(SupervisionRegistry::new())),
            heartbeat_interval: timing.heartbeat_interval(),
        })
    }

    /// Admit a successfully confirmed RUNNING execution for periodic
    /// renewal. The only way an execution enters supervision (M5.3 §21:
    /// registry insertion always follows the fenced first renewal, never
    /// precedes it).
    pub fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError> {
        let now = self.kernel.now();
        let mut registry = self.registry.lock().expect("supervision registry lock");
        registry.admit(admission, now)
    }

    /// Explicitly drop supervision ownership (terminal handling, invariant
    /// mismatch). Never touches durable state.
    pub fn remove(&self, execution_id: &ExecutionId) -> Result<(), SupervisionError> {
        let mut registry = self.registry.lock().expect("supervision registry lock");
        registry.remove(execution_id)
    }

    pub fn contains(&self, execution_id: &ExecutionId) -> bool {
        self.registry
            .lock()
            .expect("supervision registry lock")
            .contains(execution_id)
    }

    pub fn active_count(&self) -> usize {
        self.registry
            .lock()
            .expect("supervision registry lock")
            .active_count()
    }

    /// Renew one admitted execution right now (deterministic single step).
    pub fn renew_one(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<RenewalOutcome, SupervisionError> {
        let admission = {
            let registry = self.registry.lock().expect("supervision registry lock");
            registry
                .entry_admission(execution_id)
                .ok_or(SupervisionError::NoSuchEntry)?
        };
        self.renew_admission(admission)
    }

    /// Renew every entry whose renewal is due at `now` (deterministic tick).
    /// A fatal persistence fault on any entry stops the batch and propagates.
    pub fn renew_due(&self, now: UnixTime) -> Result<Vec<RenewalOutcome>, SupervisionError> {
        let interval_seconds = self.heartbeat_interval.as_secs_f64();
        let due = {
            let registry = self.registry.lock().expect("supervision registry lock");
            registry.due(now, interval_seconds)
        };
        let mut outcomes = Vec::with_capacity(due.len());
        for admission in due {
            // An entry removed concurrently between the snapshot and its
            // renewal is simply skipped: the drop already happened.
            match self.renew_admission(admission) {
                Ok(outcome) => outcomes.push(outcome),
                Err(SupervisionError::NoSuchEntry) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(outcomes)
    }

    /// Renew whatever is due at the Kernel's current time.
    pub fn renew_due_now(&self) -> Result<Vec<RenewalOutcome>, SupervisionError> {
        let now = self.kernel.now();
        self.renew_due(now)
    }

    /// The renewal itself: short fenced Kernel transaction, registry lock
    /// NOT held (M5.3 §50/§51). The result is applied to the registry only
    /// if the admission generation is still current.
    fn renew_admission(
        &self,
        admission: SupervisionAdmission,
    ) -> Result<RenewalOutcome, SupervisionError> {
        let execution_id = admission.execution_id().clone();
        let outcome = match self.kernel.renew_supervised_execution(
            admission.attempt_id(),
            admission.lease_epoch(),
            &execution_id,
        ) {
            Ok(SupervisedRenewal::Renewed(new_expires_at)) => RenewalOutcome::Renewed {
                execution_id,
                new_expires_at,
            },
            Ok(SupervisedRenewal::NotRunning) => RenewalOutcome::NoLongerRunning { execution_id },
            Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_) | Error::NotFound(_)) => {
                RenewalOutcome::AuthorityLost { execution_id }
            }
            Err(err) => return Err(SupervisionError::Fatal(err)),
        };
        let now = self.kernel.now();
        let mut registry = self.registry.lock().expect("supervision registry lock");
        match &outcome {
            RenewalOutcome::Renewed { .. } => registry.record_renewal(&admission, now),
            RenewalOutcome::AuthorityLost { .. } | RenewalOutcome::NoLongerRunning { .. } => {
                registry.remove_if_current(&admission);
            }
        }
        Ok(outcome)
    }

    /// Drop all supervision ownership (local shutdown path). No Lease is
    /// revoked and nothing is claimed quiescent: the Leases naturally expire
    /// unless another authoritative path acts (M5.3 §33).
    pub fn clear(&self) {
        let mut registry = self.registry.lock().expect("supervision registry lock");
        registry.clear();
    }
}

/// Shared runner state between the supervision thread and its handle.
#[derive(Default)]
struct RunnerState {
    shutting_down: bool,
    fatal: Option<SupervisionError>,
}

#[derive(Default)]
struct RunnerShared {
    state: Mutex<RunnerState>,
    signal: Condvar,
}

/// The long-running heartbeat supervision loop (M5.3 §18): one dedicated
/// thread owns a clone of the `SupervisionService` (sharing the same kernel
/// and registry with the composition layer); a central loop wakes, snapshots
/// the due admissions, renews each with a short fenced transaction, and
/// sleeps until the next due point. No SQLite transaction is ever held
/// across the wait, and no adapter I/O happens on the heartbeat path
/// (M5.3 §27/§50).
///
/// The thread is intentionally private and shared-nothing with the dispatcher
/// thread(s); the future notifier (M5.5) will receive its own thread, so
/// this structure does not pre-break notifier isolation.
///
/// Fatal semantics (M5.3 §34): a persistence/invariant fault stops the loop
/// and is recorded on the runner. The loop is never restarted and the
/// admissions are never reconstructed — that could renew stale authority.
/// A panic in the loop surfaces through `shutdown`'s join.
pub struct SupervisionRunner {
    service: SupervisionService,
    shared: Arc<RunnerShared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl SupervisionRunner {
    /// Start the heartbeat loop. Composition is validated before the thread
    /// spawns, so a timing/lease mismatch fails fast. Admissions enter
    /// through the runner's service handles (`admit`), always AFTER the
    /// dispatcher's fenced first renewal committed (M5.3 §21).
    pub fn start(
        kernel: Arc<Kernel>,
        timing: RuntimeTimingConfig,
    ) -> Result<Self, SupervisionError> {
        let service = SupervisionService::new(kernel.clone(), &timing)?;
        let shared = Arc::new(RunnerShared::default());
        let thread_shared = shared.clone();
        let thread_service = service.clone();
        let interval = timing.heartbeat_interval();
        let join = std::thread::Builder::new()
            .name("supervision-heartbeat".into())
            .spawn(move || {
                loop {
                    {
                        let state = thread_shared.state.lock().expect("runner state lock");
                        if state.shutting_down {
                            break;
                        }
                    }
                    match thread_service.renew_due_now() {
                        Ok(_) => {}
                        Err(e) => {
                            // Fatal (or unexpected) failure: stop the loop,
                            // surface the fault, never restart, never
                            // reconstruct admissions (M5.3 §34).
                            let mut state = thread_shared.state.lock().expect("runner state lock");
                            if state.fatal.is_none() {
                                state.fatal = Some(e);
                            }
                            break;
                        }
                    }
                    let state = thread_shared.state.lock().expect("runner state lock");
                    if state.shutting_down {
                        drop(state);
                        break;
                    }
                    let (state, _) = thread_shared
                        .signal
                        .wait_timeout(state, interval)
                        .expect("runner state lock");
                    if state.shutting_down {
                        drop(state);
                        break;
                    }
                }
                // Local shutdown: drop ownership. No revocation, no
                // terminality, no quiescence claim (M5.3 §33).
                thread_service.clear();
            })
            .map_err(|e| {
                SupervisionError::Fatal(Error::invariant(format!(
                    "failed to spawn the supervision heartbeat thread: {e}"
                )))
            })?;
        Ok(Self {
            service,
            shared,
            join: Some(join),
        })
    }

    /// Admit an execution whose fenced RUNNING confirmation + first renewal
    /// has just committed (dispatcher → supervision handoff, M5.3 §19).
    pub fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError> {
        self.service.admit(admission)
    }

    /// Drop supervision ownership for one execution. Never touches durable
    /// state.
    pub fn remove(&self, execution_id: &ExecutionId) -> Result<(), SupervisionError> {
        self.service.remove(execution_id)
    }

    pub fn contains(&self, execution_id: &ExecutionId) -> bool {
        self.service.contains(execution_id)
    }

    pub fn active_count(&self) -> usize {
        self.service.active_count()
    }

    /// A fatal supervision failure recorded by the loop, if any (M5.3 §34).
    pub fn take_fatal(&self) -> Option<SupervisionError> {
        self.shared
            .state
            .lock()
            .expect("runner state lock")
            .fatal
            .clone()
    }

    /// Stop the heartbeat loop and drop all local supervision ownership
    /// (M5.3 §33). Returns the recorded fatal fault, if the loop stopped
    /// because of one. The Leases simply stop being renewed and naturally
    /// expire — no revocation, no terminality, no quiescence claim.
    pub fn shutdown(mut self) -> Result<(), SupervisionError> {
        {
            let mut state = self.shared.state.lock().expect("runner state lock");
            state.shutting_down = true;
        }
        self.shared.signal.notify_all();
        if let Some(join) = self.join.take() {
            if join.join().is_err() {
                return Err(SupervisionError::Fatal(Error::invariant(
                    "the supervision heartbeat thread panicked",
                )));
            }
        }
        // Belt and suspenders: the thread cleared on its way out; clearing
        // here is idempotent (consumed generations persist).
        self.service.clear();
        if let Some(fatal) = self
            .shared
            .state
            .lock()
            .expect("runner state lock")
            .fatal
            .clone()
        {
            return Err(fatal);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::RuntimeTimingConfig;
    use agentype_core::{
        AuthoritativeExecutionBinding, Claim, Clock, ExecutionState, LeaseState, ManualClock,
        PartitionSpec, RequestId, Retention, RetryPolicy, SystemClock, TaskSpec, TaskState,
    };
    use agentype_execution_config::{FrozenExecutionSafety, FrozenPhysicalExecutionBinding};
    use serde_json::json;
    use std::sync::Arc;

    const LEASE_SECONDS: f64 = 10.0;

    fn timing() -> RuntimeTimingConfig {
        RuntimeTimingConfig::new(1.0, 2.0, LEASE_SECONDS).unwrap()
    }

    fn env() -> (Arc<ManualClock>, Arc<Kernel>) {
        env_with_capacity(1)
    }

    fn env_with_capacity(capacity: i64) -> (Arc<ManualClock>, Arc<Kernel>) {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Arc::new(
            Kernel::open_memory(clock.clone() as Arc<dyn Clock>, LEASE_SECONDS, 16_384).unwrap(),
        );
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                capacity,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        (clock, kernel)
    }

    fn launch_binding(claim: &Claim) -> FrozenPhysicalExecutionBinding {
        FrozenPhysicalExecutionBinding::new(
            FrozenExecutionSafety::unisolated(AuthoritativeExecutionBinding {
                attempt_id: claim.attempt_id.clone(),
                lease_epoch: claim.lease_epoch,
                execution_target: claim.execution_target.clone(),
                execution_profile: claim.execution_profile.clone(),
            }),
            "test",
        )
        .unwrap()
    }

    /// A positively confirmed RUNNING execution: the durable precondition of
    /// every supervision admission in these tests.
    fn running_execution(k: &Kernel, name: &str) -> (Claim, ExecutionId) {
        let (_batch, _ids) = k
            .submit_batch(&[TaskSpec::new(name, json!({"objective": name}))])
            .unwrap();
        let claim = k.claim_next_available().unwrap().expect("claim");
        let exec = k
            .create_execution(&claim, launch_binding(&claim))
            .unwrap()
            .execution_id()
            .clone();
        k.confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, &exec, &json!({}))
            .unwrap();
        (claim, exec)
    }

    fn mint(claim: &Claim, exec: &ExecutionId) -> SupervisionAdmission {
        // Crate-private mint: stands in for the dispatcher's post-commit
        // branch, which is the only production mint site.
        SupervisionAdmission::new(
            exec.clone(),
            RequestId::new(),
            claim.attempt_id.clone(),
            claim.lease_epoch,
        )
    }

    // ------------------------------------------------------------------
    // Registry (#15-24)
    // ------------------------------------------------------------------

    /// #15/#20: a new registry is empty; one admission inserts.
    #[test]
    fn registry_starts_empty_and_accepts_one_admission() {
        let mut registry = SupervisionRegistry::new();
        assert!(registry.is_empty());
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "reg-one");
        registry.admit(mint(&claim, &exec), kernel.now()).unwrap();
        assert!(registry.contains(&exec));
        assert_eq!(registry.active_count(), 1);
    }

    /// #16: an exactly-identical duplicate admission is idempotent.
    #[test]
    fn identical_duplicate_admission_is_idempotent() {
        let mut registry = SupervisionRegistry::new();
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "reg-idem");
        let admission = mint(&claim, &exec);
        registry.admit(admission.clone(), kernel.now()).unwrap();
        // Re-presenting the SAME token while present is idempotent.
        registry.admit(admission, kernel.now()).unwrap();
        assert_eq!(registry.active_count(), 1);
    }

    /// #17/18/19: the same ExecutionId with a different attempt_id, epoch,
    /// or request_id is an invariant violation, never a silent replacement.
    #[test]
    fn identity_conflicts_are_invariant_violations() {
        let mut registry = SupervisionRegistry::new();
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "reg-conflict");
        registry.admit(mint(&claim, &exec), kernel.now()).unwrap();

        let different_attempt = SupervisionAdmission::new(
            exec.clone(),
            RequestId::new(),
            agentype_core::AttemptId::new(),
            claim.lease_epoch,
        );
        assert!(matches!(
            registry.admit(different_attempt, kernel.now()),
            Err(SupervisionError::AdmissionIdentityConflict)
        ));

        let different_epoch = SupervisionAdmission::new(
            exec.clone(),
            RequestId::new(),
            claim.attempt_id.clone(),
            agentype_core::LeaseEpoch(claim.lease_epoch.get() + 1),
        );
        assert!(matches!(
            registry.admit(different_epoch, kernel.now()),
            Err(SupervisionError::AdmissionIdentityConflict)
        ));

        let different_request = SupervisionAdmission::new(
            exec.clone(),
            RequestId::new(),
            claim.attempt_id.clone(),
            claim.lease_epoch,
        );
        assert!(matches!(
            registry.admit(different_request, kernel.now()),
            Err(SupervisionError::AdmissionIdentityConflict)
        ));
        assert_eq!(registry.active_count(), 1);
    }

    /// #21: a fresh runtime (new service) starts with EMPTY supervision
    /// ownership even when the database holds a RUNNING Execution.
    #[test]
    fn new_service_is_empty_despite_persisted_running_row() {
        let (_clock, kernel) = env();
        let (_claim, exec) = running_execution(&kernel, "restart-empty");
        assert_eq!(
            kernel.execution(&exec).unwrap().state,
            ExecutionState::Running
        );
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        assert_eq!(service.active_count(), 0);
        assert!(!service.contains(&exec));
    }

    /// #22/23/24: removing ownership never mutates the Execution, never
    /// claims quiescence/terminality, and never touches Task/Lease authority.
    #[test]
    fn registry_removal_touches_no_durable_state() {
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "reg-remove");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec)).unwrap();
        service.remove(&exec).unwrap();

        let execution = kernel.execution(&exec).unwrap();
        assert_eq!(
            execution.state,
            ExecutionState::Running,
            "removal does not change physical state"
        );
        assert!(!execution.terminal_confirmed, "no terminality claim");
        assert!(!execution.quiescent_confirmed, "no quiescence claim");
        let lease = kernel.lease_supervision_view(&claim.attempt_id).unwrap();
        assert_eq!(lease.state, LeaseState::Active, "no lease revocation");
        assert!(!service.contains(&exec));
        // Removing again reports no entry.
        assert!(matches!(
            service.remove(&exec),
            Err(SupervisionError::NoSuchEntry)
        ));
    }

    // ------------------------------------------------------------------
    // Heartbeat service (#25-44)
    // ------------------------------------------------------------------

    /// #25/26/27/41: an admitted RUNNING execution renews; the expiry
    /// extends through Kernel policy, heartbeat_at is stamped, and the
    /// admission stays active.
    #[test]
    fn admitted_running_execution_renews() {
        let (clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "renew-ok");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec)).unwrap();

        clock.advance(1.0);
        let now = kernel.now();
        assert_eq!(
            service.renew_one(&exec).unwrap(),
            RenewalOutcome::Renewed {
                execution_id: exec.clone(),
                new_expires_at: now + LEASE_SECONDS,
            }
        );
        let lease = kernel.lease_supervision_view(&claim.attempt_id).unwrap();
        assert_eq!(lease.expires_at, now + LEASE_SECONDS);
        assert_eq!(lease.heartbeat_at, now);
        assert!(service.contains(&exec));
    }

    /// #42: authority loss removes the admission, and the consumed token can
    /// never resume renewal (#50): only a fresh mint may re-admit.
    #[test]
    fn authority_loss_removes_admission_and_consumes_the_token() {
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "renew-lost");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let admission = mint(&claim, &exec);
        service.admit(admission.clone()).unwrap();

        // ACK closes the durable authority; the heartbeat then loses fencing.
        kernel
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
        assert_eq!(
            service.renew_one(&exec).unwrap(),
            RenewalOutcome::AuthorityLost {
                execution_id: exec.clone()
            }
        );
        assert!(!service.contains(&exec));

        // The consumed admission cannot re-enter supervision (#50).
        assert!(matches!(
            service.admit(admission),
            Err(SupervisionError::AdmissionConsumed)
        ));
    }

    /// #43: a non-RUNNING physical state removes the admission without any
    /// durable-state repair.
    #[test]
    fn non_running_execution_drops_admission() {
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "renew-notrunning");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec)).unwrap();

        kernel
            .record_physical_outcome(
                &exec,
                ExecutionState::Lost,
                Some(&json!({"h": 1})),
                None,
                None,
                false,
                false,
            )
            .unwrap();
        assert_eq!(
            service.renew_one(&exec).unwrap(),
            RenewalOutcome::NoLongerRunning {
                execution_id: exec.clone()
            }
        );
        assert!(!service.contains(&exec));
        let lease = kernel.lease_supervision_view(&claim.attempt_id).unwrap();
        assert_eq!(lease.state, LeaseState::Active, "no authority mutation");
    }

    /// #29/#33-style stale Attempt at service level: after the lease expired,
    /// recovery expired the attempt, and a replacement attempt took over,
    /// the old admission loses fencing and cannot renew.
    #[test]
    fn stale_attempt_admission_loses_renewal() {
        let (clock, kernel) = env();
        let (_batch, _ids) = kernel
            .submit_batch(
                &[TaskSpec::new("renew-stale", json!({})).retry(RetryPolicy {
                    max_attempts: 3,
                    retry_classes: vec![agentype_core::FailureClass::ExecutionLost],
                    base_backoff_seconds: 1.0,
                    max_backoff_seconds: 2.0,
                })],
            )
            .unwrap();
        let claim = kernel.claim_next_available().unwrap().expect("claim");
        let exec = kernel
            .create_execution(&claim, launch_binding(&claim))
            .unwrap()
            .execution_id()
            .clone();
        kernel
            .confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, &exec, &json!({}))
            .unwrap();
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec)).unwrap();

        clock.advance(11.0);
        kernel.expire_leases(false).unwrap();
        kernel.promote_retry_wait().unwrap();
        assert_eq!(
            service.renew_one(&exec).unwrap(),
            RenewalOutcome::AuthorityLost {
                execution_id: exec.clone()
            }
        );
        assert!(!service.contains(&exec));
    }

    /// #44: a corrupted durable lease row during renewal is a FATAL
    /// persistence fault — never ordinary authority loss (M5.3 §15).
    #[test]
    fn persistence_fault_is_fatal_not_authority_loss() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentype-supervision-fault-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel =
            Arc::new(Kernel::open(&path, clock as Arc<dyn Clock>, LEASE_SECONDS, 16_384).unwrap());
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
        let (claim, exec) = running_execution(&kernel, "renew-fatal");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec)).unwrap();

        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "UPDATE leases SET epoch='not-an-integer' WHERE attempt_id=?1",
            rusqlite::params![claim.attempt_id.as_str()],
        )
        .unwrap();
        drop(conn);

        match service.renew_one(&exec).unwrap_err() {
            SupervisionError::Fatal(inner) => assert!(!matches!(
                inner,
                Error::StaleAuthority(_) | Error::InvalidAuthority(_) | Error::NotFound(_)
            )),
            other => panic!("expected a fatal persistence fault, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #52 (local half) + §21 ordering: renew_due only renews entries that
    /// are actually admitted; a never-admitted RUNNING execution is never
    /// renewed by any service tick.
    #[test]
    fn renew_due_only_touches_admitted_entries() {
        let (clock, kernel) = env_with_capacity(2);
        let (claim_a, exec_a) = running_execution(&kernel, "due-a");
        let (_claim_b, exec_b) = running_execution(&kernel, "due-b");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim_a, &exec_a)).unwrap();

        clock.advance(3.0);
        let outcomes = service.renew_due_now().unwrap();
        assert_eq!(outcomes.len(), 1, "only the admitted entry renews");
        assert!(matches!(
            service.renew_one(&exec_b),
            Err(SupervisionError::NoSuchEntry)
        ));
    }

    /// Crash-window simulation (#52): first renewal committed (via the
    /// dispatcher's fenced confirmation), but the admission was never
    /// inserted — no subsequent renewal happens, and the Lease later expires
    /// safely through the normal recovery path.
    #[test]
    fn crash_window_without_admission_never_renews() {
        let (clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "crash-window");
        let initial_expires = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        // The admission is deliberately NOT inserted (crash before admit).

        clock.advance(3.0);
        let _ = service.renew_due_now().unwrap();
        let after = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        assert_eq!(
            after, initial_expires,
            "no renewal may happen without an admitted entry"
        );

        // The Lease later expires safely through the normal recovery path.
        clock.advance(8.0);
        kernel.expire_leases(false).unwrap();
        let task_id = kernel.execution(&exec).unwrap().task_id.clone();
        let task = kernel.task(&task_id).unwrap();
        assert_eq!(task.state, TaskState::Suspended);
    }

    // ------------------------------------------------------------------
    // Runner lifecycle (§18/§33/§34) — the only real-thread tests.
    // ------------------------------------------------------------------

    fn system_kernel(
        tag: &str,
        lease_seconds: f64,
    ) -> (std::path::PathBuf, Arc<Kernel>, (Claim, ExecutionId)) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentype-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let kernel =
            Arc::new(Kernel::open(&path, Arc::new(SystemClock), lease_seconds, 16_384).unwrap());
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
        let launched = running_execution(&kernel, "runner-ok");
        (dir, kernel, launched)
    }

    /// Lifecycle smoke: a dedicated heartbeat thread keeps an admitted
    /// RUNNING execution renewed in real time; shutdown drops ownership
    /// without revoking anything (fail closed — the Lease then naturally
    /// expires).
    #[test]
    fn runner_renews_admitted_execution_until_shutdown() {
        let (_dir, kernel, (claim, exec)) = system_kernel("runner-smoke", 1.0);

        // poll <= heartbeat < lease
        let timing = RuntimeTimingConfig::new(0.3, 0.4, 1.0).unwrap();
        let runner = SupervisionRunner::start(kernel.clone(), timing).unwrap();
        runner.admit(mint(&claim, &exec)).unwrap();
        assert!(runner.contains(&exec));

        let initial_expires = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        // Wait long enough for at least two heartbeat ticks.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let renewed_expires = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        assert!(
            renewed_expires > initial_expires + 0.5,
            "the heartbeat loop must have renewed the lease: {initial_expires} -> {renewed_expires}"
        );
        assert!(runner.take_fatal().is_none());

        // Shutdown drops ownership and stops renewing; the durable authority
        // is untouched (no revocation, no terminality, no quiescence claim).
        runner.shutdown().unwrap();
        let exec_state = kernel.execution(&exec).unwrap();
        assert_eq!(exec_state.state, ExecutionState::Running);
        assert!(!exec_state.terminal_confirmed);
        assert!(!exec_state.quiescent_confirmed);
        let lease_after_shutdown = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        std::thread::sleep(std::time::Duration::from_millis(600));
        let expires_later = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        assert_eq!(
            expires_later, lease_after_shutdown,
            "no renewal may happen after shutdown"
        );
    }

    /// Fatal smoke: a corrupted durable lease row stops the loop and
    /// surfaces a fatal fault on the runner handle — the loop never restarts
    /// and admissions are never reconstructed (M5.3 §34).
    #[test]
    fn runner_surfaces_fatal_persistence_fault_and_stops() {
        let (dir, kernel, (claim, exec)) = system_kernel("runner-fatal", 1.0);

        // Corrupt the durable lease row below the API boundary BEFORE the
        // loop starts: the first renewal tick must surface a fatal fault.
        let path = dir.join("scheduler.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "UPDATE leases SET epoch='not-an-integer' WHERE attempt_id=?1",
            rusqlite::params![claim.attempt_id.as_str()],
        )
        .unwrap();
        drop(conn);

        let timing = RuntimeTimingConfig::new(0.2, 0.3, 1.0).unwrap();
        let runner = SupervisionRunner::start(kernel, timing).unwrap();
        runner.admit(mint(&claim, &exec)).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(800));
        match runner.take_fatal() {
            Some(SupervisionError::Fatal(inner)) => assert!(!matches!(
                inner,
                Error::StaleAuthority(_) | Error::InvalidAuthority(_) | Error::NotFound(_)
            )),
            other => panic!("expected a fatal fault from the heartbeat loop, got {other:?}"),
        }
        // Shutdown still cleans up and re-surfaces the fatal fault.
        assert!(runner.shutdown().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
