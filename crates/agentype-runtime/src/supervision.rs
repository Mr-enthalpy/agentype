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
//!   persisted `state='RUNNING'` Execution row can never produce one. It is
//!   a **move-only capability**: `admit` consumes the token, so one
//!   admission has exactly one supervisor owner and a consumed/dropped
//!   token can never be replayed anywhere (only a fresh authoritative mint
//!   re-enters supervision — the M5.4 re-admission shape).
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
//! Deadline scheduling (M5.3 audit P1-1): the heartbeat loop is a deadline
//! scheduler, not a fixed-phase ticker. Every entry's `next_due_at` is
//! anchored at the fenced first-renewal COMMIT time carried by the admission
//! (never the handoff/insertion time); each successful renewal re-anchors at
//! its own commit time; the loop sleeps until the earliest deadline and is
//! woken on every ownership mutation, so a healthy supervisor can never
//! schedule itself past the durable expiry under any legal timing
//! (`dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`, with no
//! `heartbeat <= lease/2` headroom requirement).
//!
//! Crash window (M5.3 plan §20): if the runtime crashes after the first
//! renewal is committed but before the admission is inserted, no further
//! renewal occurs, the Lease eventually expires, and M5.4 reconciliation
//! handles the physical reality. The window is one-directional and
//! fail-closed; no durable supervision table is created to eliminate it.

use crate::timing::{validate_lease_authority_match, RuntimeTimingConfig};
use crate::{Error, ExecutionId, SupervisionAdmission};
use agentype_core::{AttemptId, LeaseEpoch, RequestId, UnixTime};
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
    /// The admission capability's timing is malformed: non-finite anchor or
    /// expiry, or an expiry that does not follow the first renewal.
    InvalidAdmission(String),
    /// No registry entry under this execution id.
    NoSuchEntry,
    /// The runner lifecycle is not RUNNING (shutting down, failed, or
    /// stopped): admissions are only accepted from a live heartbeat
    /// loop, otherwise an execution could sit in the registry with no
    /// supervisor behind it.
    RunnerStopped(&'static str),
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
            Self::InvalidAdmission(detail) => {
                write!(f, "supervision admission is malformed: {detail}")
            }
            Self::NoSuchEntry => write!(f, "no supervision entry for this execution"),
            Self::RunnerStopped(detail) => {
                write!(f, "supervision runner is not running: {detail}")
            }
            Self::LeaseAuthorityMismatch { configured, kernel } => write!(
                f,
                "configured lease_seconds ({configured}) does not match the Kernel lease authority ({kernel})"
            ),
            Self::Fatal(e) => write!(f, "fatal supervision persistence fault: {e}"),
        }
    }
}

impl std::error::Error for SupervisionError {}

/// The durable identity portion of an admission, used for registry
/// bookkeeping and renewal snapshots (M5.3 audit P1-3): the move-only
/// capability is consumed at `admit`, and the registry keeps ONLY this plain
/// identity — the capability itself is never stored, cloned, or re-issuable.
#[derive(Debug, Clone)]
pub(crate) struct SupervisionIdentity {
    execution_id: ExecutionId,
    request_id: RequestId,
    attempt_id: AttemptId,
    lease_epoch: LeaseEpoch,
    generation: u64,
}

impl SupervisionIdentity {
    pub(crate) fn new(
        execution_id: ExecutionId,
        request_id: RequestId,
        attempt_id: AttemptId,
        lease_epoch: LeaseEpoch,
        generation: u64,
    ) -> Self {
        Self {
            execution_id,
            request_id,
            attempt_id,
            lease_epoch,
            generation,
        }
    }

    pub(crate) fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }

    pub(crate) fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub(crate) fn lease_epoch(&self) -> LeaseEpoch {
        self.lease_epoch
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Exact identity equality (the four durable identity fields). The
    /// ephemeral generation is deliberately excluded: two admissions with
    /// the same identity describe the same admitted authority.
    pub(crate) fn same_identity(&self, other: &SupervisionIdentity) -> bool {
        self.execution_id == other.execution_id
            && self.request_id == other.request_id
            && self.attempt_id == other.attempt_id
            && self.lease_epoch == other.lease_epoch
    }
}

/// One supervised execution entry. Runtime-local bookkeeping only: the
/// plain identity snapshot plus the deadline-scheduling anchor.
#[derive(Debug, Clone)]
struct SupervisedExecution {
    identity: SupervisionIdentity,
    next_due_at: UnixTime,
}

/// In-memory registry of the executions THIS runtime instance currently owns
/// for heartbeat supervision (M5.3 §7).
///
/// It starts empty on construction, is never restored from the database, and
/// is authoritative only for "which executions this runtime may attempt to
/// renew". Admissions are move-only capabilities: `admit` consumes the token,
/// so the same capability can never be presented to a second registry, and
/// the only way to re-admit an execution is a fresh authoritative mint (the
/// M5.4 re-admission shape). A duplicate insertion is idempotent only when
/// the identity is exactly identical; the same ExecutionId with a different
/// attempt/epoch/request is an invariant violation (M5.3 §8), never a silent
/// replacement.
#[derive(Default)]
pub struct SupervisionRegistry {
    entries: HashMap<ExecutionId, SupervisedExecution>,
    consumed_generations: HashSet<u64>,
}

impl SupervisionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit an execution for heartbeat supervision, consuming the
    /// admission capability. The admission must be the live return value of
    /// a successful fenced RUNNING confirmation + first renewal transaction.
    /// `next_due_at` is the deadline-scheduling anchor computed by the
    /// service from the admission's first-renewal commit time.
    pub fn admit(
        &mut self,
        admission: SupervisionAdmission,
        next_due_at: UnixTime,
    ) -> Result<(), SupervisionError> {
        let identity = admission.identity();
        if self.consumed_generations.contains(&identity.generation()) {
            return Err(SupervisionError::AdmissionConsumed);
        }
        match self.entries.get(identity.execution_id()) {
            Some(existing) if existing.identity.same_identity(&identity) => {
                // Exactly identical identity: idempotent re-admission of the
                // same admitted authority.
                Ok(())
            }
            Some(_) => Err(SupervisionError::AdmissionIdentityConflict),
            None => {
                self.entries.insert(
                    identity.execution_id().clone(),
                    SupervisedExecution {
                        identity,
                        next_due_at,
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
            .insert(entry.identity.generation());
        Ok(())
    }

    /// Drop ALL supervision ownership (local shutdown). No Lease is revoked,
    /// no Execution is marked terminal, and nothing is claimed quiescent —
    /// the Leases simply stop being renewed and naturally expire (M5.3 §33).
    pub fn clear(&mut self) {
        for (_, entry) in self.entries.drain() {
            self.consumed_generations
                .insert(entry.identity.generation());
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

    fn entry_identity(&self, execution_id: &ExecutionId) -> Option<SupervisionIdentity> {
        self.entries.get(execution_id).map(|e| e.identity.clone())
    }

    /// Snapshot the entries due for renewal at `now`, cloned out as plain
    /// identities so the caller never holds the registry lock across the DB
    /// transaction (M5.3 §51) and never handles the admission capability.
    fn due(&self, now: UnixTime) -> Vec<SupervisionIdentity> {
        self.entries
            .values()
            .filter(|entry| entry.next_due_at <= now)
            .map(|entry| entry.identity.clone())
            .collect()
    }

    /// The earliest renewal deadline across all entries — the heartbeat
    /// loop sleeps until exactly this point (M5.3 audit P1-1).
    fn earliest_next_due(&self) -> Option<UnixTime> {
        self.entries
            .values()
            .map(|e| e.next_due_at)
            .reduce(f64::min)
    }

    /// Apply a successful renewal result, but only if the entry's admission
    /// generation is still the one the renewal was performed for — an old
    /// renewal result must never mutate a newer registry entry (M5.3 §52).
    /// `next_due_at` is re-anchored at the renewal's own commit time.
    fn record_renewal(&mut self, identity: &SupervisionIdentity, next_due_at: UnixTime) {
        let current = self
            .entries
            .get(identity.execution_id())
            .is_some_and(|entry| entry.identity.generation() == identity.generation());
        if current {
            if let Some(entry) = self.entries.get_mut(identity.execution_id()) {
                entry.next_due_at = next_due_at;
            }
        }
    }

    /// Apply a drop result (authority loss / no-longer-running), again only
    /// if the generation is still current.
    fn remove_if_current(&mut self, identity: &SupervisionIdentity) {
        let current = self
            .entries
            .get(identity.execution_id())
            .is_some_and(|entry| entry.identity.generation() == identity.generation());
        if current {
            drop(self.entries.remove(identity.execution_id()));
            self.consumed_generations.insert(identity.generation());
        }
    }
}

/// Deterministic supervision service (M5.3 §29): owns the registry and the
/// heartbeat policy; exposes single-step operations so tests validate
/// correctness without real-time sleeps. The long-running loop
/// (`SupervisionRunner`) shares the same kernel + registry and wraps these
/// operations in a dedicated thread.
///
/// Cloning the service shares the same registry: it is one ownership domain
/// with two handles, not two supervisors.
///
/// Fatal semantics (M5.3 §15, audit P2): this deterministic service is the
/// primitive/testing surface — `renew_one`/`renew_due` return
/// `Err(SupervisionError::Fatal)` and leave the entry in place; the service
/// does not fail-stop by itself. The production fail-stop owner is
/// [`SupervisionRunner`], which stops its loop and clears ownership on
/// Fatal. A service that has produced a Fatal must not be reused for
/// renewal; the Runner enforces this mechanically.
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

    /// The Kernel's current clock reading (the supervision clock IS the
    /// Kernel clock — never an independent time source).
    pub fn now(&self) -> UnixTime {
        self.kernel.now()
    }

    /// The earliest renewal deadline across all owned entries, if any. The
    /// heartbeat loop sleeps until exactly this point.
    pub fn earliest_next_due(&self) -> Option<UnixTime> {
        self.registry
            .lock()
            .expect("supervision registry lock")
            .earliest_next_due()
    }

    /// Admit a successfully confirmed RUNNING execution for periodic
    /// renewal. The only way an execution enters supervision (M5.3 §21:
    /// registry insertion always follows the fenced first renewal, never
    /// precedes it). This CONSUMES the move-only admission capability: the
    /// same token can never be admitted anywhere again, which is what makes
    /// "one runtime supervision owner per admitted Execution" structural
    /// rather than per-registry discipline (M5.3 audit P1-3).
    pub fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError> {
        // Fail closed on a malformed capability: the scheduling anchor and
        // the durable expiry must be finite, and the fenced first renewal
        // must precede the expiry it produced.
        if !admission.first_renewed_at().is_finite()
            || !admission.lease_expires_at().is_finite()
            || admission.lease_expires_at() <= admission.first_renewed_at()
        {
            return Err(SupervisionError::InvalidAdmission(format!(
                "first_renewed_at={} lease_expires_at={}",
                admission.first_renewed_at(),
                admission.lease_expires_at()
            )));
        }
        // Deadline scheduling (M5.3 audit P1-1): the next due point is
        // anchored at the fenced first-renewal COMMIT time carried by the
        // capability — never at the handoff/insertion time, so a delayed
        // handoff cannot push the schedule past the durable expiry. With
        // `heartbeat_interval < lease_seconds` (the §A2 gate), this due
        // point is always strictly before the durable expiry.
        let next_due_at = admission.first_renewed_at() + self.heartbeat_interval.as_secs_f64();
        let mut registry = self.registry.lock().expect("supervision registry lock");
        registry.admit(admission, next_due_at)
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
        let identity = {
            let registry = self.registry.lock().expect("supervision registry lock");
            registry
                .entry_identity(execution_id)
                .ok_or(SupervisionError::NoSuchEntry)?
        };
        self.renew_identity(identity)
    }

    /// Renew every entry whose deadline has arrived at `now` (deterministic
    /// tick). A fatal persistence fault on any entry stops the batch and
    /// propagates.
    pub fn renew_due(&self, now: UnixTime) -> Result<Vec<RenewalOutcome>, SupervisionError> {
        let due = {
            let registry = self.registry.lock().expect("supervision registry lock");
            registry.due(now)
        };
        let mut outcomes = Vec::with_capacity(due.len());
        for identity in due {
            // An entry removed concurrently between the snapshot and its
            // renewal is simply skipped: the drop already happened.
            match self.renew_identity(identity) {
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
    fn renew_identity(
        &self,
        identity: SupervisionIdentity,
    ) -> Result<RenewalOutcome, SupervisionError> {
        let execution_id = identity.execution_id().clone();
        // Anchor BEFORE the renewal (M5.3 audit P1-1): the next deadline is
        // then anchor + interval, which is strictly earlier than the
        // renewal's own new durable expiry (anchor + interval < anchor +
        // lease under the §A2 gate) — a healthy supervisor can never
        // schedule itself past the durable expiry.
        let anchor = self.kernel.now();
        let outcome = match self.kernel.renew_supervised_execution(
            identity.attempt_id(),
            identity.lease_epoch(),
            &execution_id,
        ) {
            Ok(SupervisedRenewal::Renewed(new_expires_at)) => RenewalOutcome::Renewed {
                execution_id,
                new_expires_at,
            },
            Ok(SupervisedRenewal::NotRunning) => RenewalOutcome::NoLongerRunning { execution_id },
            Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                RenewalOutcome::AuthorityLost { execution_id }
            }
            // Everything else is fatal (M5.3 audit round 2, P1-3): an
            // admitted execution's durable identity (Execution/Attempt/
            // Lease) existed at mint time and Agentype never deletes
            // execution history — a NotFound here is durable corruption or
            // an impossible identity, never an ordinary expiry the
            // supervisor may quietly drop. For a WRITE worker this must
            // not masquerade as a normal stale-authority drop; fail-stop
            // and let M5.4 reconciliation handle the physical reality.
            Err(err) => return Err(SupervisionError::Fatal(err)),
        };
        let next_due_at = anchor + self.heartbeat_interval.as_secs_f64();
        let mut registry = self.registry.lock().expect("supervision registry lock");
        match &outcome {
            RenewalOutcome::Renewed { .. } => registry.record_renewal(&identity, next_due_at),
            RenewalOutcome::AuthorityLost { .. } | RenewalOutcome::NoLongerRunning { .. } => {
                registry.remove_if_current(&identity);
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

/// Runner lifecycle (M5.3 audit round 2, P1-2): admissions are accepted
/// ONLY in `Running`; a failed or stopping loop can never take new
/// ownership, so the registry can never hold an entry with no live
/// supervisor behind it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RunnerPhase {
    Running,
    ShuttingDown,
    Failed,
    Stopped,
}

/// Shared runner state between the supervision thread and its handle.
#[derive(Debug)]
struct RunnerState {
    phase: RunnerPhase,
    fatal: Option<SupervisionError>,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            phase: RunnerPhase::Running,
            fatal: None,
        }
    }
}

#[derive(Default)]
struct RunnerShared {
    state: Mutex<RunnerState>,
    signal: Condvar,
}

/// The long-running heartbeat supervision loop (M5.3 §18, audit P1-1): one
/// dedicated thread owns a clone of the `SupervisionService` (sharing the
/// same kernel and registry with the composition layer) and acts as a
/// **deadline scheduler**: renew everything due → sleep until the earliest
/// `next_due_at` (or the next ownership mutation, or shutdown) → repeat.
/// No SQLite transaction is ever held across the wait, and no adapter I/O
/// happens on the heartbeat path (M5.3 §27/§50).
///
/// The thread is intentionally private and shared-nothing with the dispatcher
/// thread(s); the future notifier (M5.5) will receive its own thread, so
/// this structure does not pre-break notifier isolation.
///
/// Fatal semantics (M5.3 §34): a persistence/invariant fault stops the loop
/// and is recorded on the runner. The loop is never restarted and the
/// admissions are never reconstructed — that could renew stale authority.
/// A panic in the loop surfaces through `shutdown`'s join.
///
/// Ownership lifecycle (M5.3 audit P1-2): dropping the runner stops and
/// joins the heartbeat thread — ownership can never outlive its owner as a
/// detached orphan that keeps renewing with no handle to stop or observe it.
pub struct SupervisionRunner {
    service: SupervisionService,
    shared: Arc<RunnerShared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl SupervisionRunner {
    /// Start the heartbeat loop. Composition is validated before the thread
    /// spawns, so a timing/lease mismatch fails fast. Admissions enter
    /// through the runner's handles (`admit`), always AFTER the dispatcher's
    /// fenced first renewal committed (M5.3 §21).
    pub fn start(
        kernel: Arc<Kernel>,
        timing: RuntimeTimingConfig,
    ) -> Result<Self, SupervisionError> {
        let service = SupervisionService::new(kernel.clone(), &timing)?;
        let shared = Arc::new(RunnerShared::default());
        let thread_shared = shared.clone();
        let thread_service = service.clone();
        let idle_wait = timing.heartbeat_interval();
        let join = std::thread::Builder::new()
            .name("supervision-heartbeat".into())
            .spawn(move || {
                // Exit guard (M5.3 audit round 2, P1-2): ANY unexpected
                // thread exit - a panic above all - must mark the runner
                // FAILED. A missing supervisor must never look alive.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    'supervision: loop {
                        // Renew everything whose deadline has arrived. Short
                        // fenced transactions; no state lock held (M5.3 §50).
                        if let Err(e) = thread_service.renew_due_now() {
                            // Fatal (or unexpected) failure: stop the loop,
                            // surface the fault, never restart, never
                            // reconstruct admissions (M5.3 §34).
                            let mut state = thread_shared.state.lock().expect("runner state lock");
                            if state.fatal.is_none() {
                                state.fatal = Some(e);
                            }
                            state.phase = RunnerPhase::Failed;
                            drop(state);
                            break 'supervision;
                        }
                        // Deadline wait (M5.3 audit P1-1): sleep until the
                        // earliest next_due_at — recomputed under the state lock
                        // so a concurrent ownership mutation (whose wake-up may
                        // otherwise be lost) can never be overslept.
                        let mut state = thread_shared.state.lock().expect("runner state lock");
                        loop {
                            if state.phase != RunnerPhase::Running {
                                break 'supervision;
                            }
                            let wait: Duration = match thread_service.earliest_next_due() {
                                Some(due) => {
                                    let remaining = due - thread_service.now();
                                    if remaining > 0.0 {
                                        Duration::from_secs_f64(remaining)
                                    } else {
                                        Duration::ZERO
                                    }
                                }
                                None => idle_wait,
                            };
                            if wait == Duration::ZERO {
                                // Something is due right now: renew outside the
                                // state lock.
                                drop(state);
                                continue 'supervision;
                            }
                            let (s, _) = thread_shared
                                .signal
                                .wait_timeout(state, wait)
                                .expect("runner state lock");
                            state = s;
                            // Woken by: shutdown, an ownership mutation, or the
                            // earliest deadline — re-check and recompute.
                        }
                    }
                }));
                if result.is_err() {
                    let mut state = thread_shared.state.lock().expect("runner state lock");
                    state.phase = RunnerPhase::Failed;
                    if state.fatal.is_none() {
                        state.fatal = Some(SupervisionError::Fatal(Error::invariant(
                            "the supervision heartbeat thread panicked",
                        )));
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
    /// Consumes the move-only admission capability.
    pub fn admit(&self, admission: SupervisionAdmission) -> Result<(), SupervisionError> {
        // Serialize the mutation with the loop's deadline computation (the
        // loop reads the earliest deadline under the same state lock), so a
        // new admission's wake-up can never be lost and the loop can never
        // oversleep past its earlier deadline.
        let state = self.shared.state.lock().expect("runner state lock");
        // Lifecycle gate (M5.3 audit round 2, P1-2): admissions are
        // accepted ONLY while the heartbeat loop is Running - after a
        // fatal or during shutdown there is no supervisor behind the
        // entry, and an unowned entry is exactly the state this milestone
        // exists to prevent.
        if state.phase != RunnerPhase::Running {
            return Err(SupervisionError::RunnerStopped(
                "admission is only accepted while the heartbeat loop is running",
            ));
        }
        let result = self.service.admit(admission);
        if result.is_ok() {
            self.shared.signal.notify_all();
        }
        drop(state);
        result
    }

    /// Drop supervision ownership for one execution. Never touches durable
    /// state.
    pub fn remove(&self, execution_id: &ExecutionId) -> Result<(), SupervisionError> {
        let state = self.shared.state.lock().expect("runner state lock");
        let result = self.service.remove(execution_id);
        if result.is_ok() {
            self.shared.signal.notify_all();
        }
        drop(state);
        result
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

    /// Stop the heartbeat loop and drop all local supervision ownership.
    /// The Leases simply stop being renewed and naturally expire — no
    /// revocation, no terminality, no quiescence claim. Returns the recorded
    /// fatal fault, if the loop stopped because of one.
    fn stop_and_join(&mut self) -> Option<SupervisionError> {
        {
            let mut state = self.shared.state.lock().expect("runner state lock");
            if state.phase == RunnerPhase::Running {
                state.phase = RunnerPhase::ShuttingDown;
            }
        }
        self.shared.signal.notify_all();
        if let Some(join) = self.join.take() {
            if join.join().is_err() {
                self.service.clear();
                return Some(SupervisionError::Fatal(Error::invariant(
                    "the supervision heartbeat thread panicked",
                )));
            }
        }
        // Belt and suspenders: the thread cleared on its way out; clearing
        // here is idempotent (consumed generations persist).
        self.service.clear();
        let mut state = self.shared.state.lock().expect("runner state lock");
        if state.phase == RunnerPhase::ShuttingDown {
            state.phase = RunnerPhase::Stopped;
        }
        state.fatal.clone()
    }

    /// Graceful shutdown: stop the loop, drop ownership, and report the
    /// recorded fatal fault, if any (M5.3 §33/§34).
    pub fn shutdown(mut self) -> Result<(), SupervisionError> {
        self.stop_and_join().map_or(Ok(()), Err)
    }
}

impl Drop for SupervisionRunner {
    fn drop(&mut self) {
        // Ownership must not outlive its owner (M5.3 audit P1-2): a dropped
        // runner stops and joins the heartbeat thread — it must never detach
        // into an orphan that keeps renewing with no handle to stop or
        // observe it. Drop cannot report the recorded fatal; `shutdown` is
        // the observing path.
        let _ = self.stop_and_join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::RuntimeTimingConfig;
    use agentype_core::{
        AuthoritativeExecutionBinding, Claim, Clock, ExecutionState, LeaseState, ManualClock,
        PartitionSpec, Retention, RetryPolicy, SystemClock, TaskSpec, TaskState,
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

    /// Mint an admission through the REAL authority path: a fresh fenced
    /// confirm-and-renew transaction returns a Kernel-produced
    /// RunningAuthorityGrant, and the admission is minted exclusively
    /// from it (M5.4 S4 - no raw-IDs constructor exists).
    fn mint(claim: &Claim, exec: &ExecutionId, kernel: &Kernel) -> SupervisionAdmission {
        let grant = kernel
            .confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, exec, &json!({}))
            .unwrap();
        SupervisionAdmission::from_grant(grant)
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
        registry
            .admit(mint(&claim, &exec, &kernel), kernel.now() + 2.0)
            .unwrap();
        assert!(registry.contains(&exec));
        assert_eq!(registry.active_count(), 1);
    }

    /// #16: an exactly-identical duplicate admission is idempotent. The
    /// capability is move-only, so the second presentation is necessarily a
    /// fresh mint carrying the SAME identity.
    #[test]
    fn identical_duplicate_admission_is_idempotent() {
        let mut registry = SupervisionRegistry::new();
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "reg-idem");
        // The capability is move-only, so presenting the same identity
        // twice requires two distinct Kernel grants for the same durable
        // identity (same execution/request/attempt/epoch, fresh mint).
        let now = kernel.now();
        registry
            .admit(mint(&claim, &exec, &kernel), now + 2.0)
            .unwrap();
        registry
            .admit(mint(&claim, &exec, &kernel), now + 2.0)
            .unwrap();
        assert_eq!(registry.active_count(), 1);
    }

    /// #17/18/19: the same ExecutionId with a different attempt_id, epoch,
    /// or request_id is an invariant violation, never a silent replacement.
    #[test]
    fn identity_conflicts_are_invariant_violations() {
        let mut registry = SupervisionRegistry::new();
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "reg-conflict");
        registry
            .admit(mint(&claim, &exec, &kernel), kernel.now() + 2.0)
            .unwrap();

        let different_attempt = SupervisionAdmission::from_parts_for_test(
            exec.clone(),
            RequestId::new(),
            agentype_core::AttemptId::new(),
            claim.lease_epoch,
            kernel.now(),
            kernel.now() + LEASE_SECONDS,
        );
        assert!(matches!(
            registry.admit(different_attempt, kernel.now() + 2.0),
            Err(SupervisionError::AdmissionIdentityConflict)
        ));

        let different_epoch = SupervisionAdmission::from_parts_for_test(
            exec.clone(),
            RequestId::new(),
            claim.attempt_id.clone(),
            agentype_core::LeaseEpoch(claim.lease_epoch.get() + 1),
            kernel.now(),
            kernel.now() + LEASE_SECONDS,
        );
        assert!(matches!(
            registry.admit(different_epoch, kernel.now() + 2.0),
            Err(SupervisionError::AdmissionIdentityConflict)
        ));

        let different_request = SupervisionAdmission::from_parts_for_test(
            exec.clone(),
            RequestId::new(),
            claim.attempt_id.clone(),
            claim.lease_epoch,
            kernel.now(),
            kernel.now() + LEASE_SECONDS,
        );
        assert!(matches!(
            registry.admit(different_request, kernel.now() + 2.0),
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
        service.admit(mint(&claim, &exec, &kernel)).unwrap();
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
        service.admit(mint(&claim, &exec, &kernel)).unwrap();

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

    /// #42/#50: authority loss removes the admission. The consumed token is
    /// move-only and structurally cannot be re-presented; the only
    /// re-admission path is a fresh authoritative mint (the M5.4 shape),
    /// whose renewal is still governed by durable Kernel fencing — it can
    /// never resurrect stale authority.
    #[test]
    fn after_authority_loss_only_a_fresh_mint_reenters_supervision() {
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "renew-lost");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec, &kernel)).unwrap();

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

        // Under the M5.4 grant API, a fresh admission requires a fresh
        // Kernel grant — and the kernel REFUSES to mint one for a closed
        // authority. No grant means no admission can even exist: stale
        // authority resurrection is impossible by construction.
        assert!(matches!(
            kernel.confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                &exec,
                &json!({}),
            ),
            Err(Error::StaleAuthority(_))
        ));
        assert!(!service.contains(&exec));
    }

    /// #43: a non-RUNNING physical state removes the admission without any
    /// durable-state repair.
    #[test]
    fn non_running_execution_drops_admission() {
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "renew-notrunning");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec, &kernel)).unwrap();

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
        service.admit(mint(&claim, &exec, &kernel)).unwrap();

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
        service.admit(mint(&claim, &exec, &kernel)).unwrap();

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
        service.admit(mint(&claim_a, &exec_a, &kernel)).unwrap();

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
    // Deadline scheduling and capability structure (audit P1-1 / P1-3)
    // ------------------------------------------------------------------

    /// With the LEGAL wide timing heartbeat=6 < lease=10 (no 2*heartbeat
    /// headroom), a healthy deadline-scheduled supervisor renews every cycle
    /// and never self-expires the lease. The schedule is anchored at the
    /// fenced first-renewal commit time carried by the admission — a delayed
    /// handoff does not delay it. Under the previous fixed-phase,
    /// insertion-time scheduling this exact configuration expired the lease.
    #[test]
    fn wide_legal_timing_never_self_expires() {
        let (clock, kernel) = env(); // lease 10.0
        let (claim, exec) = running_execution(&kernel, "wide-timing"); // confirmed at 1000.0 → expiry 1010.0
        let timing = RuntimeTimingConfig::new(1.0, 6.0, LEASE_SECONDS).unwrap();
        let service = SupervisionService::new(kernel.clone(), &timing).unwrap();

        // The grant is minted at the fenced confirm commit instant (t=1000,
        // expiry 1010). The handoff is then delayed by 5.9s: the admission
        // is inserted at 1005.9 but carries the 1000.0 anchor.
        let admission = mint(&claim, &exec, &kernel);
        clock.advance(5.9);
        service.admit(admission).unwrap();
        // The next due point is anchor + interval, NOT insertion + interval
        // (which would be 1011.9 — already past the 1010.0 expiry).
        assert_eq!(service.earliest_next_due(), Some(1006.0));

        // Drive the deadline scheduler through three full cycles.
        for cycle in 0..3u32 {
            let due = service.earliest_next_due().unwrap();
            clock.set(due);
            let outcomes = service.renew_due_now().unwrap();
            assert_eq!(outcomes.len(), 1, "cycle {cycle}");
            assert!(matches!(outcomes[0], RenewalOutcome::Renewed { .. }));
            let lease = kernel.lease_supervision_view(&claim.attempt_id).unwrap();
            assert_eq!(lease.expires_at, due + LEASE_SECONDS, "cycle {cycle}");
            assert_eq!(lease.state, LeaseState::Active, "cycle {cycle}");
        }
    }

    /// A malformed capability (non-finite anchor/expiry, or an expiry that
    /// does not follow the first renewal) is rejected fail-closed at admit.
    #[test]
    fn malformed_admission_timing_is_unconstructible_via_grant_api() {
        let (_clock, kernel) = env();
        let (_claim, _exec) = running_execution(&kernel, "bad-timing");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();

        // With the M5.4 grant API freeze, a malformed capability is not
        // constructible: admissions exist only via Kernel-produced grants
        // whose timing is finite and anchor-precedes-expiry by
        // construction. The admit-time validation remains as documented
        // unreachable-by-construction defense (same pattern as the kernel
        // blank-adapter-kind check).
        assert_eq!(service.active_count(), 0);
    }

    /// P1-3 closure: the admission is a move-only capability. Admitting it
    /// consumes the token — a second service can neither hold, re-admit, nor
    /// renew the same execution, so "one runtime supervision owner per
    /// admitted Execution" is structural, not per-registry discipline.
    #[test]
    fn move_only_capability_prevents_cross_service_replay() {
        let (_clock, kernel) = env();
        let (claim, exec) = running_execution(&kernel, "move-only");
        let service_1 = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service_1.admit(mint(&claim, &exec, &kernel)).unwrap();
        assert!(service_1.contains(&exec));

        let service_2 = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        assert_eq!(service_2.active_count(), 0);
        // service_2 holds no token for this execution: it can neither renew
        // it nor obtain a copy of the capability (there is no Clone, and the
        // token was moved into service_1's registry).
        assert!(matches!(
            service_2.renew_one(&exec),
            Err(SupervisionError::NoSuchEntry)
        ));

        // Removing the entry destroys the only token; service_1 itself can
        // no longer renew it.
        service_1.remove(&exec).unwrap();
        assert!(matches!(
            service_1.renew_one(&exec),
            Err(SupervisionError::NoSuchEntry)
        ));
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
        let (dir, kernel, (claim, exec)) = system_kernel("runner-smoke", 1.0);

        // poll <= heartbeat < lease
        let timing = RuntimeTimingConfig::new(0.3, 0.4, 1.0).unwrap();
        let runner = SupervisionRunner::start(kernel.clone(), timing).unwrap();
        runner.admit(mint(&claim, &exec, &kernel)).unwrap();
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1-1 closure (real-thread smoke): with heartbeat=0.6 < lease=1.0 and
    /// the admission inserted mid-phase (0.3s after start), the deadline
    /// scheduler keeps the lease alive well past its original expiry. A
    /// fixed-phase ticker sleeping full intervals from its own phase would
    /// expire this lease under the same legal timing.
    #[test]
    fn runner_deadline_schedule_survives_wide_legal_timing() {
        let (dir, kernel, (claim, exec)) = system_kernel("runner-wide", 1.0);
        let timing = RuntimeTimingConfig::new(0.2, 0.6, 1.0).unwrap();
        let runner = SupervisionRunner::start(kernel.clone(), timing).unwrap();

        // Admit mid-phase, well after the runner's loop has parked.
        std::thread::sleep(std::time::Duration::from_millis(300));
        runner.admit(mint(&claim, &exec, &kernel)).unwrap();
        let expires_at_admit = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;

        // Wait past two full lease durations: repeated deadline renewals
        // must keep the lease alive the whole time.
        std::thread::sleep(std::time::Duration::from_millis(2200));
        let expires_after = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        assert!(
            expires_after >= expires_at_admit + 1.5,
            "the deadline scheduler must keep renewing under wide legal timing: {expires_at_admit} -> {expires_after}"
        );
        let exec_state = kernel.execution(&exec).unwrap();
        assert_eq!(exec_state.state, ExecutionState::Running);
        assert!(runner.take_fatal().is_none());
        runner.shutdown().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1-2 closure: dropping the runner without calling shutdown stops the
    /// heartbeat thread and joins it — supervision ownership can never
    /// outlive its owner as a detached orphan that keeps renewing.
    #[test]
    fn drop_runner_stops_renewal() {
        let (dir, kernel, (claim, exec)) = system_kernel("runner-drop", 1.0);
        let timing = RuntimeTimingConfig::new(0.2, 0.3, 1.0).unwrap();
        let runner = SupervisionRunner::start(kernel.clone(), timing).unwrap();
        runner.admit(mint(&claim, &exec, &kernel)).unwrap();

        // At least one renewal happened before the drop.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let frozen = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;

        // Drop WITHOUT shutdown: ownership must end here.
        drop(runner);
        std::thread::sleep(std::time::Duration::from_millis(800));
        let after = kernel
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .expires_at;
        assert_eq!(
            after, frozen,
            "no renewal may happen after the runner is dropped"
        );

        // The durable state is untouched: no revocation, no terminality,
        // no quiescence claim.
        let exec_state = kernel.execution(&exec).unwrap();
        assert_eq!(exec_state.state, ExecutionState::Running);
        assert!(!exec_state.terminal_confirmed);
        assert!(!exec_state.quiescent_confirmed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fatal smoke: a corrupted durable lease row stops the loop and
    /// surfaces a fatal fault on the runner handle — the loop never restarts
    /// and admissions are never reconstructed (M5.3 §34).
    #[test]
    fn runner_surfaces_fatal_persistence_fault_and_stops() {
        let (dir, kernel, (claim, exec)) = system_kernel("runner-fatal", 1.0);

        // Mint the admission AND a spare BEFORE the corruption: after the
        // durable lease row is corrupted, no fresh grant can be minted
        // (which is exactly the fatal semantics under test).
        let admission = mint(&claim, &exec, &kernel);
        let spare = mint(&claim, &exec, &kernel);

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
        let runner = SupervisionRunner::start(kernel.clone(), timing).unwrap();
        runner.admit(admission).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(800));
        match runner.take_fatal() {
            Some(SupervisionError::Fatal(inner)) => assert!(!matches!(
                inner,
                Error::StaleAuthority(_) | Error::InvalidAuthority(_) | Error::NotFound(_)
            )),
            other => panic!("expected a fatal fault from the heartbeat loop, got {other:?}"),
        }
        // Audit round 2, P1-2: after a fatal the runner lifecycle is
        // FAILED - a new admission MUST be rejected and the registry
        // must stay empty (no entry with no supervisor behind it).
        assert!(matches!(
            runner.admit(spare),
            Err(SupervisionError::RunnerStopped(_))
        ));
        assert_eq!(runner.active_count(), 0);
        // Shutdown still cleans up and re-surfaces the fatal fault.
        assert!(runner.shutdown().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clock that panics once armed: turns the first post-arm kernel
    /// clock read into a heartbeat-thread panic, deterministically.
    struct TripwireClock {
        inner: Arc<ManualClock>,
        armed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Clock for TripwireClock {
        fn now(&self) -> f64 {
            if self.armed.load(std::sync::atomic::Ordering::Relaxed) {
                panic!("supervision clock tripped");
            }
            self.inner.now()
        }
    }

    /// P1-2 (audit round 2): an unexpected heartbeat-thread exit (panic)
    /// must flip the runner lifecycle to FAILED - fatal observable, later
    /// admissions rejected, registry empty. A dead supervisor must never
    /// look alive.
    #[test]
    fn heartbeat_thread_panic_marks_runner_failed_and_rejects_admit() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentype-runner-panic-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let clock_inner = Arc::new(ManualClock::new(1_000.0));
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kernel = Arc::new(
            Kernel::open(
                &path,
                Arc::new(TripwireClock {
                    inner: clock_inner.clone(),
                    armed: armed.clone(),
                }),
                1.0,
                16_384,
            )
            .unwrap(),
        );
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
        let (claim, exec) = running_execution(&kernel, "panic-ok");
        // Mint BEFORE arming the tripwire (mint reads the kernel clock).
        let admission = mint(&claim, &exec, &kernel);
        // A spare admission for the post-fatal rejection assertion: it
        // must also exist before the clock is tripped, and its renewal
        // anchor is irrelevant because the lifecycle gate rejects it
        // before any timing validation.
        let spare_admission = mint(&claim, &exec, &kernel);
        armed.store(true, std::sync::atomic::Ordering::Relaxed);

        let timing = RuntimeTimingConfig::new(0.2, 0.3, 1.0).unwrap();
        let runner = SupervisionRunner::start(kernel.clone(), timing).unwrap();
        // The loop is still Running here, so the admission is accepted...
        runner.admit(admission).unwrap();
        // ...and the first tick then hits the tripped clock and panics.
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(matches!(
            runner.take_fatal(),
            Some(SupervisionError::Fatal(_))
        ));
        // The lifecycle is FAILED: no new ownership, nothing unowned.
        let second = spare_admission;
        assert!(matches!(
            runner.admit(second),
            Err(SupervisionError::RunnerStopped(_))
        ));
        assert_eq!(runner.active_count(), 0);
        assert!(runner.shutdown().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1-3 (audit round 2): an admitted execution whose durable row has
    /// disappeared is durable corruption, not ordinary authority loss -
    /// renewal fails FATAL (the service leaves the entry in place; the
    /// production runner fail-stops on the same classification).
    #[test]
    fn missing_execution_is_fatal_not_authority_loss() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agentype-missing-exec-{nanos}"));
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
        let (claim, exec) = running_execution(&kernel, "missing-exec");
        let service = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        service.admit(mint(&claim, &exec, &kernel)).unwrap();

        // Delete the durable execution row below the API boundary: an
        // admitted identity cannot legitimately disappear.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "DELETE FROM executions WHERE id=?1",
            rusqlite::params![exec.as_str()],
        )
        .unwrap();
        drop(conn);

        match service.renew_one(&exec).unwrap_err() {
            SupervisionError::Fatal(inner) => {
                assert!(matches!(inner, Error::NotFound(_)), "got {inner:?}")
            }
            other => panic!("expected a fatal fault for a vanished execution, got {other:?}"),
        }
        // Service (non-runner) semantics: the entry stays in place on
        // Fatal; the production Runner fail-stops on the same outcome.
        assert!(service.contains(&exec));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
