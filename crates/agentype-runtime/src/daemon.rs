//! M5.8 production Scheduler daemon composition root.
//!
//! Recovery grants this Runtime the right to begin activation; it does not
//! by itself grant production dispatch capability.

use crate::control::{ControlError, ControlLoopRunner, ControlLoopService, DispatchGate};
use crate::notifier::{NotifierBinding, NotifierRunner};
use crate::observer::{
    ObserverError, PhysicalObserverConfig, PhysicalObserverRunner, PhysicalObserverService,
};
use crate::process_lock::{
    ProcessLockError, ReadyPermit, RuntimeProcessGuard, SqliteRuntimeConfig,
};
use crate::recovery::{recover_runtime_with_gate, RecoveryError};
use crate::supervision::SupervisionRunner;
use crate::timing::RuntimeTimingConfig;
use crate::{AdapterRegistry, ExecutionRegistry};
use agentype_core::{Clock, Error, SystemClock};
use agentype_storage_sqlite::Kernel;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonPhase {
    Ready,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug)]
pub enum DaemonExit {
    Stopped,
    Failed(String),
}

#[derive(Debug)]
pub enum DaemonError {
    AlreadyRunning,
    ProcessLock(ProcessLockError),
    Recovery(RecoveryError),
    Observer(ObserverError),
    Control(ControlError),
    Config(String),
    Persistence(Error),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "another scheduler daemon already owns this store"),
            Self::ProcessLock(err) => write!(f, "process lock: {err}"),
            Self::Recovery(err) => write!(f, "daemon recovery: {err}"),
            Self::Observer(err) => write!(f, "daemon observer: {err}"),
            Self::Control(err) => write!(f, "daemon control: {err}"),
            Self::Config(detail) => write!(f, "daemon configuration: {detail}"),
            Self::Persistence(err) => write!(f, "daemon persistence: {err}"),
        }
    }
}

impl std::error::Error for DaemonError {}

impl From<ProcessLockError> for DaemonError {
    fn from(err: ProcessLockError) -> Self {
        match err {
            ProcessLockError::AlreadyRunning => Self::AlreadyRunning,
            other => Self::ProcessLock(other),
        }
    }
}

/// Single-run builder. `start` consumes the builder.
///
/// Unlocked recovery and ungated dispatch are not public production APIs.
///
/// ```compile_fail
/// fn _no_unlocked_recovery() {
///     let _ = agentype_runtime::recover_runtime;
/// }
/// ```
/// ```compile_fail
/// fn _no_ungated_dispatch() {
///     let _ = agentype_runtime::Dispatcher::new;
/// }
/// ```
/// ```compile_fail
/// fn _no_public_supervision_start() {
///     let _ = agentype_runtime::SupervisionRunner::start;
/// }
/// ```
/// ```compile_fail
/// fn _no_public_reconcile() {
///     let _ = agentype_runtime::reconcile_one_execution;
/// }
/// ```
pub struct SchedulerDaemonBuilder {
    store: SqliteRuntimeConfig,
    timing: RuntimeTimingConfig,
    observer: PhysicalObserverConfig,
    execution_registry: ExecutionRegistry,
    adapters: AdapterRegistry,
    notifier: NotifierBinding,
    clock: Arc<dyn Clock>,
}

impl SchedulerDaemonBuilder {
    pub fn new(
        store: SqliteRuntimeConfig,
        timing: RuntimeTimingConfig,
        observer: PhysicalObserverConfig,
        execution_registry: ExecutionRegistry,
        adapters: AdapterRegistry,
        notifier: NotifierBinding,
    ) -> Result<Self, DaemonError> {
        if (observer.freshness_limit() - store.lease_seconds()).abs() < f64::EPSILON
            || observer.freshness_limit() >= store.lease_seconds()
        {
            return Err(DaemonError::Config(
                "observer freshness_limit must be < store lease_seconds".into(),
            ));
        }
        if (timing.lease_seconds() - store.lease_seconds()).abs() > f64::EPSILON {
            return Err(DaemonError::Config(
                "timing.lease_seconds must match the store lease_seconds".into(),
            ));
        }
        Ok(Self {
            store,
            timing,
            observer,
            execution_registry,
            adapters,
            notifier,
            clock: Arc::new(SystemClock),
        })
    }

    /// Test clock injection. Production startup uses [`SystemClock`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn start(self) -> Result<RunningSchedulerDaemon, DaemonError> {
        let lock = RuntimeProcessGuard::acquire(&self.store)?;
        let kernel = Arc::new(
            Kernel::open(
                self.store.path(),
                self.clock,
                self.store.lease_seconds(),
                self.store.continuity_max_bytes(),
            )
            .map_err(DaemonError::Persistence)?,
        );
        lock.lock()
            .confirm_store_identity(self.store.path())
            .map_err(DaemonError::from)?;
        let gate = DispatchGate::closed();
        let recovered = recover_runtime_with_gate(
            kernel.clone(),
            &self.adapters,
            self.timing,
            self.notifier,
            gate.clone(),
            self.observer.freshness_limit(),
        )
        .map_err(DaemonError::Recovery)?;
        if recovered.runner().is_failed() {
            return Err(DaemonError::Recovery(RecoveryError::Invariant(
                "supervision failed before activation".into(),
            )));
        }
        if recovered.notifier().is_some_and(|n| n.is_failed()) {
            return Err(DaemonError::Recovery(RecoveryError::Invariant(
                "notifier failed before activation".into(),
            )));
        }
        let (supervision, notifier) = recovered.into_parts();
        let mut startup = StartupGuard {
            supervision: Some(supervision),
            notifier,
            lock: Some(lock),
        };
        let supervision = startup.supervision.as_ref().expect("supervision");
        let observer_service = PhysicalObserverService::new(
            kernel.clone(),
            self.adapters.clone(),
            supervision.freshness_sink(),
            self.observer,
        );
        observer_service
            .activation_sweep()
            .map_err(DaemonError::Observer)?;
        kernel
            .expire_leases(false)
            .map_err(DaemonError::Persistence)?;
        kernel
            .promote_retry_wait()
            .map_err(DaemonError::Persistence)?;
        kernel.reconcile_pool().map_err(DaemonError::Persistence)?;
        kernel
            .revive_eligible_agents()
            .map_err(DaemonError::Persistence)?;
        if startup.supervision.as_ref().is_some_and(|s| s.is_failed())
            || startup.notifier.as_ref().is_some_and(|n| n.is_failed())
        {
            return Err(DaemonError::Recovery(RecoveryError::Invariant(
                "a required runner failed during activation".into(),
            )));
        }
        let freshness = self.observer.freshness_limit();
        let permit = ReadyPermit::mint();
        let control_service = ControlLoopService::new(
            kernel.clone(),
            self.execution_registry,
            self.adapters,
            startup
                .supervision
                .as_ref()
                .expect("supervision")
                .admit_sink(),
            &self.timing,
            permit,
            gate.clone(),
        );
        // Workers start PAUSED behind the READY publication boundary
        // (M5.8 audit round 3, P1-1). Neither observation nor maintenance may
        // run between the readiness validation and the gate flip, because
        // both can invalidate the READY invariant.
        let ready = crate::control::ReadyRelease::new();
        let observer = PhysicalObserverRunner::start(observer_service, gate.clone(), ready.clone())
            .map_err(DaemonError::Observer)?;
        let control = ControlLoopRunner::start(control_service, ready.clone())
            .map_err(DaemonError::Control)?;
        let supervision = startup.supervision.take().expect("supervision");
        let notifier = startup.notifier.take();
        let lock = startup.lock.take().expect("process lock");
        let inner = Arc::new(DaemonInner {
            control,
            observer,
            supervision,
            notifier,
            gate,
            stop: AtomicBool::new(false),
            phase: Mutex::new(DaemonPhase::Stopping),
        });
        let watchdog_inner = inner.clone();
        let watchdog = std::thread::Builder::new()
            .name("daemon-health".into())
            .spawn(move || loop {
                if watchdog_inner.stop.load(Ordering::SeqCst) {
                    break;
                }
                let failed = watchdog_inner.control.is_failed()
                    || watchdog_inner.observer.is_failed()
                    || watchdog_inner.supervision.is_failed()
                    || watchdog_inner
                        .notifier
                        .as_ref()
                        .is_some_and(|n| n.is_failed());
                if failed {
                    watchdog_inner.gate.fail();
                    let mut phase = watchdog_inner.phase.lock().expect("daemon phase");
                    *phase = DaemonPhase::Failed;
                    drop(phase);
                    watchdog_inner.control.request_stop();
                    watchdog_inner.observer.request_stop();
                    watchdog_inner.supervision.request_stop();
                    if let Some(n) = &watchdog_inner.notifier {
                        n.request_stop();
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            });
        let watchdog = match watchdog {
            Ok(handle) => handle,
            Err(err) => {
                let DaemonInner {
                    control,
                    observer,
                    supervision,
                    notifier,
                    gate,
                    ..
                } = Arc::try_unwrap(inner).unwrap_or_else(|_| {
                    panic!("health watchdog was not spawned, so the daemon arc is unique")
                });
                gate.fail();
                control.request_stop();
                observer.request_stop();
                supervision.request_stop();
                if let Some(n) = &notifier {
                    n.request_stop();
                }
                let _ = supervision.shutdown();
                drop(control);
                drop(observer);
                if let Some(n) = notifier {
                    let _ = n.shutdown();
                }
                return Err(DaemonError::Config(format!(
                    "required health watchdog failed to start: {err}"
                )));
            }
        };
        // READY publication: validation and the gate flip share one
        // supervision readiness barrier, so no worker can change renewal
        // eligibility or freshness between them. The workers are released
        // inside the barrier, strictly after the flip.
        //
        // Runner health is checked BEFORE the barrier, not inside it: reading
        // a runner's state lock while holding the supervision registry lock
        // would invert the state -> registry order that `refresh_positive`
        // establishes. It is also redundant inside, because every runner
        // fatal publishes to the same dispatch gate synchronously, so
        // `try_commit_ready` is the authoritative interlock.
        let already_failed = inner.control.is_failed()
            || inner.observer.is_failed()
            || inner.supervision.is_failed()
            || inner
                .notifier
                .as_ref()
                .is_some_and(|notifier| notifier.is_failed());
        let mut refusal: Option<DaemonError> = if already_failed {
            Some(DaemonError::Recovery(RecoveryError::Invariant(
                "a runner failed before READY could be committed".into(),
            )))
        } else {
            None
        };
        let committed = refusal.is_none()
            && inner
                .supervision
                .service()
                .with_readiness_barrier(|snapshots| {
                    if let Err(err) = require_fresh_running_authority(&kernel, snapshots, freshness)
                    {
                        refusal = Some(DaemonError::Recovery(err));
                        return false;
                    }
                    if !inner.gate.try_commit_ready() {
                        refusal = Some(DaemonError::Recovery(RecoveryError::Invariant(
                            "the dispatch gate was already failed or stopping before READY".into(),
                        )));
                        return false;
                    }
                    ready.release();
                    true
                });
        if !committed {
            inner.stop.store(true, Ordering::SeqCst);
            inner.control.request_stop();
            inner.observer.request_stop();
            inner.supervision.request_stop();
            if let Some(n) = &inner.notifier {
                n.request_stop();
            }
            let _ = watchdog.join();
            let DaemonInner {
                control,
                observer,
                supervision,
                notifier,
                ..
            } = match Arc::try_unwrap(inner) {
                Ok(inner) => inner,
                Err(shared) => {
                    drop(shared);
                    return Err(DaemonError::Recovery(RecoveryError::Invariant(
                        "runner failed before READY; cleanup could not join every worker".into(),
                    )));
                }
            };
            control.request_stop();
            observer.request_stop();
            supervision.request_stop();
            if let Some(n) = &notifier {
                n.request_stop();
            }
            let _ = supervision.shutdown();
            drop(control);
            drop(observer);
            if let Some(n) = notifier {
                let _ = n.shutdown();
            }
            return Err(refusal.unwrap_or_else(|| {
                DaemonError::Recovery(RecoveryError::Invariant(
                    "READY could not be published".into(),
                ))
            }));
        }
        Ok(RunningSchedulerDaemon {
            kernel,
            inner: Some(inner),
            watchdog: Some(watchdog),
            _lock: lock,
        })
    }
}

struct StartupGuard {
    supervision: Option<SupervisionRunner>,
    notifier: Option<NotifierRunner>,
    lock: Option<RuntimeProcessGuard>,
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        if let Some(supervision) = &self.supervision {
            supervision.request_stop();
        }
        if let Some(notifier) = &self.notifier {
            notifier.request_stop();
        }
        if let Some(supervision) = self.supervision.take() {
            let _ = supervision.shutdown();
        }
        if let Some(notifier) = self.notifier.take() {
            let _ = notifier.shutdown();
        }
        drop(self.lock.take());
    }
}

struct DaemonInner {
    control: ControlLoopRunner,
    observer: PhysicalObserverRunner,
    supervision: SupervisionRunner,
    notifier: Option<NotifierRunner>,
    gate: DispatchGate,
    stop: AtomicBool,
    phase: Mutex<DaemonPhase>,
}

/// Running production daemon. Drop / `join` releases the process lock last.
pub struct RunningSchedulerDaemon {
    kernel: Arc<Kernel>,
    inner: Option<Arc<DaemonInner>>,
    watchdog: Option<std::thread::JoinHandle<()>>,
    _lock: RuntimeProcessGuard,
}

impl RunningSchedulerDaemon {
    pub fn phase(&self) -> DaemonPhase {
        self.inner
            .as_ref()
            .expect("daemon inner")
            .gate
            .published_phase()
    }

    /// Direct Scheduler authority. Not part of the production daemon API.
    #[cfg(any(test, feature = "test-support"))]
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// Narrow production control and diagnostics surface.
    ///
    /// This is the only production path that mutates Scheduler authority from
    /// outside the Runtime. Worker acknowledgement, Runtime-owned mechanics,
    /// and writer-quiescence overrides are not reachable through it.
    pub fn control(&self) -> crate::SchedulerControl<'_> {
        let lock = self._lock.lock();
        crate::SchedulerControl::new(&self.kernel, lock.identity_debug(), lock.store_path())
    }

    pub fn poll_health(&self) {
        let inner = self.inner.as_ref().expect("daemon inner");
        let failed = inner.control.is_failed()
            || inner.observer.is_failed()
            || inner.supervision.is_failed()
            || inner.notifier.as_ref().is_some_and(|n| n.is_failed());
        if failed {
            *inner.phase.lock().expect("daemon phase") = DaemonPhase::Failed;
            self.request_shutdown();
        }
    }

    pub fn request_shutdown(&self) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        inner.gate.begin_shutdown();
        inner.stop.store(true, Ordering::SeqCst);
        let mut phase = inner.phase.lock().expect("daemon phase");
        if *phase == DaemonPhase::Ready {
            *phase = DaemonPhase::Stopping;
        }
        drop(phase);
        inner.control.request_stop();
        inner.observer.request_stop();
        inner.supervision.request_stop();
        if let Some(notifier) = &inner.notifier {
            notifier.request_stop();
        }
    }

    pub fn join(mut self) -> DaemonExit {
        self.request_shutdown();
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
        let inner = match Arc::try_unwrap(self.inner.take().expect("daemon inner")) {
            Ok(inner) => inner,
            Err(shared) => {
                let cause = first_worker_fatal(&shared);
                return if *shared.phase.lock().expect("daemon phase") == DaemonPhase::Failed
                    || cause.is_some()
                {
                    DaemonExit::Failed(cause.unwrap_or_else(|| "runtime worker failed".into()))
                } else {
                    DaemonExit::Stopped
                };
            }
        };
        let DaemonInner {
            control,
            observer,
            supervision,
            notifier,
            phase,
            ..
        } = inner;
        let control_fatal = control.join_fatal();
        let observer_fatal = observer.join_fatal();
        let supervision_fatal = supervision.shutdown().err();
        let notifier_fatal = match notifier {
            Some(runner) => runner.shutdown().err(),
            None => None,
        };
        let cause = control_fatal
            .or(observer_fatal)
            .or(supervision_fatal.map(|err| err.to_string()))
            .or(notifier_fatal.map(|err| err.to_string()));
        let failed = *phase.lock().expect("daemon phase") == DaemonPhase::Failed || cause.is_some();
        if failed {
            DaemonExit::Failed(cause.unwrap_or_else(|| "runtime worker failed".into()))
        } else {
            DaemonExit::Stopped
        }
    }
}

fn first_worker_fatal(inner: &DaemonInner) -> Option<String> {
    inner
        .control
        .fatal()
        .or_else(|| inner.observer.take_fatal())
        .or_else(|| inner.supervision.take_fatal().map(|err| err.to_string()))
        .or_else(|| {
            inner
                .notifier
                .as_ref()
                .and_then(|runner| runner.take_fatal().map(|err| err.to_string()))
        })
}

/// Post-activation READY gate. Recovery only required ownership.
/// This requires a fresh, renewal-eligible RUNNING supervision.
pub(crate) fn require_fresh_running_authority(
    kernel: &Kernel,
    snapshots: &[crate::supervision::SupervisedSnapshot],
    freshness_limit: f64,
) -> Result<(), RecoveryError> {
    let now = kernel.now();
    for cand in kernel
        .reconciliation_candidates()
        .map_err(RecoveryError::from)?
    {
        if !cand.current_authority_hint().looks_current_at(now) {
            continue;
        }
        if cand.persisted_state() != agentype_core::ExecutionState::Running {
            return Err(RecoveryError::Invariant(
                "current authority is not a fresh RUNNING supervision".into(),
            ));
        }
        let fresh = snapshots.iter().any(|snap| {
            snap.identity.execution_id() == cand.execution_id()
                && snap.renewal_eligible
                && now < snap.last_positive_observed_at + freshness_limit
        });
        if !fresh {
            return Err(RecoveryError::Invariant(
                "current authority lacks fresh renewable supervision".into(),
            ));
        }
    }
    Ok(())
}

impl Drop for RunningSchedulerDaemon {
    fn drop(&mut self) {
        if let Some(inner) = &self.inner {
            inner.gate.begin_shutdown();
            inner.stop.store(true, Ordering::SeqCst);
            inner.control.request_stop();
            inner.observer.request_stop();
            inner.supervision.request_stop();
            if let Some(notifier) = &inner.notifier {
                notifier.request_stop();
            }
        }
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
        if let Some(arc) = self.inner.take() {
            if let Ok(inner) = Arc::try_unwrap(arc) {
                drop(inner.control.join_fatal());
                drop(inner.observer.join_fatal());
                drop(inner.supervision);
                drop(inner.notifier);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deadlines::test_deadlines;
    use crate::process_lock::RuntimeProcessGuard;
    use agentype_adapter_api::FakeAdapter;
    use agentype_core::{ExecutionState, PartitionSpec, Retention, TaskSpec, TaskState};
    use agentype_execution_config::{ExecutionProfileConfig, ExecutionTargetConfig};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const LEASE: f64 = 10.0;

    fn temp_store() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("agentype-daemon-{nanos}.sqlite"))
    }

    fn timing() -> RuntimeTimingConfig {
        RuntimeTimingConfig::new(1.0, 2.0, LEASE).unwrap()
    }

    fn observer() -> PhysicalObserverConfig {
        PhysicalObserverConfig::new(1.0, 4.0, 8, LEASE).unwrap()
    }

    fn composition() -> (ExecutionRegistry, AdapterRegistry) {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        let fake = Arc::new(FakeAdapter::new());
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_kind("process", fake, test_deadlines())
            .unwrap();
        (registry, adapters)
    }

    fn builder_for(path: &std::path::Path) -> SchedulerDaemonBuilder {
        let store = SqliteRuntimeConfig::new(path, LEASE, 16_384).unwrap();
        let (registry, adapters) = composition();
        SchedulerDaemonBuilder::new(
            store,
            timing(),
            observer(),
            registry,
            adapters,
            NotifierBinding::DisabledForTests,
        )
        .unwrap()
    }

    /// Process adapter that panics on observation once armed, so the
    /// observer thread's panic path is exercised through the real daemon
    /// composition rather than a synthetic fault-injection framework.
    struct PanickingObserverAdapter {
        inner: FakeAdapter,
        armed: std::sync::atomic::AtomicBool,
    }

    impl PanickingObserverAdapter {
        fn new() -> Self {
            Self {
                inner: FakeAdapter::new(),
                armed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn arm(&self) {
            self.armed.store(true, Ordering::SeqCst);
        }
    }

    impl agentype_adapter_api::ExecutionAdapter for PanickingObserverAdapter {
        fn start_execution(
            &self,
            request: &agentype_adapter_api::EnvironmentStartRequest,
            deadline: &agentype_adapter_api::AdapterDeadline,
        ) -> agentype_adapter_api::AdapterResult<agentype_adapter_api::StartObservation> {
            self.inner.start_execution(request, deadline)
        }

        fn observe_execution(
            &self,
            handle: &agentype_adapter_api::RuntimeHandle,
            deadline: &agentype_adapter_api::AdapterDeadline,
        ) -> agentype_adapter_api::AdapterResult<agentype_adapter_api::ExecutionObservation>
        {
            assert!(
                !self.armed.load(Ordering::SeqCst),
                "armed observer fault for the daemon fatal chain"
            );
            self.inner.observe_execution(handle, deadline)
        }

        fn interrupt_execution(
            &self,
            handle: &agentype_adapter_api::RuntimeHandle,
            deadline: &agentype_adapter_api::AdapterDeadline,
        ) -> agentype_adapter_api::AdapterResult<agentype_adapter_api::ExecutionObservation>
        {
            self.inner.interrupt_execution(handle, deadline)
        }

        fn terminate_execution(
            &self,
            handle: &agentype_adapter_api::RuntimeHandle,
            deadline: &agentype_adapter_api::AdapterDeadline,
        ) -> agentype_adapter_api::AdapterResult<agentype_adapter_api::ExecutionObservation>
        {
            self.inner.terminate_execution(handle, deadline)
        }

        fn collect_outcome(
            &self,
            handle: &agentype_adapter_api::RuntimeHandle,
            deadline: &agentype_adapter_api::AdapterDeadline,
        ) -> agentype_adapter_api::AdapterResult<agentype_adapter_api::PhysicalExecutionOutcome>
        {
            self.inner.collect_outcome(handle, deadline)
        }

        fn reconcile_start(
            &self,
            request_id: &agentype_core::RequestId,
            persisted_handle: Option<&agentype_adapter_api::RuntimeHandle>,
            deadline: &agentype_adapter_api::AdapterDeadline,
        ) -> agentype_adapter_api::AdapterResult<agentype_adapter_api::StartObservation> {
            self.inner
                .reconcile_start(request_id, persisted_handle, deadline)
        }
    }

    fn adapter_composition(
        adapter: Arc<dyn agentype_adapter_api::ExecutionAdapter>,
    ) -> (ExecutionRegistry, AdapterRegistry) {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_kind("process", adapter, test_deadlines())
            .unwrap();
        (registry, adapters)
    }

    fn builder_with(
        path: &std::path::Path,
        adapters: AdapterRegistry,
        registry: ExecutionRegistry,
    ) -> SchedulerDaemonBuilder {
        let store = SqliteRuntimeConfig::new(path, LEASE, 16_384).unwrap();
        SchedulerDaemonBuilder::new(
            store,
            timing(),
            observer(),
            registry,
            adapters,
            NotifierBinding::DisabledForTests,
        )
        .unwrap()
    }

    /// M5.8 audit round 3, P1-2: the global fatal chain, end to end.
    ///
    /// M5.8 is the milestone that *composes* the runners, so component-level
    /// fatal tests are not composition closure. This drives a real observer
    /// fatal through a running daemon and checks what the whole runtime does:
    /// stop new dispatch, stop renewals, join every worker, release the lock,
    /// and let the next daemon recover.
    #[test]
    fn observer_fatal_stops_dispatch_renewal_and_releases_the_lock() {
        let path = temp_store();
        let adapter = Arc::new(PanickingObserverAdapter::new());
        let (registry, adapters) = adapter_composition(adapter.clone());
        let daemon = builder_with(&path, adapters, registry).start().unwrap();
        assert_eq!(daemon.phase(), DaemonPhase::Ready);

        // Bring up capacity and one long-running execution, and wait until it
        // is actually supervised and being renewed.
        let control = daemon.control();
        control
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        control.reconcile_pool().unwrap();
        let submission = control
            .submit_batch(&[TaskSpec::new("fatal-run", json!({"o": 1}))])
            .unwrap();
        let task_id = submission.task_ids["fatal-run"].clone();
        let kernel = daemon.kernel();
        wait_until(Duration::from_secs(8), || {
            kernel.reconciliation_candidates().unwrap().iter().any(|c| {
                kernel.execution(c.execution_id()).unwrap().state == ExecutionState::Running
            })
        });
        let candidate = kernel
            .reconciliation_candidates()
            .unwrap()
            .into_iter()
            .find(|c| kernel.execution(c.execution_id()).unwrap().state == ExecutionState::Running)
            .expect("a supervised execution");
        let attempt_id = candidate.attempt_id().clone();

        // Arm the fault: the observer thread panics on its next observation.
        adapter.arm();
        wait_until(Duration::from_secs(8), || {
            daemon.phase() == DaemonPhase::Failed
        });
        assert_eq!(
            daemon.phase(),
            DaemonPhase::Failed,
            "an observer fatal must fail the daemon"
        );

        // The durable lease stops being renewed.
        let expired_at = kernel
            .lease_supervision_view(&attempt_id)
            .unwrap()
            .expires_at;
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(
            kernel
                .lease_supervision_view(&attempt_id)
                .unwrap()
                .expires_at,
            expired_at,
            "renewal must stop once the runtime has failed"
        );

        // No new Claim, Attempt, or physical start after the fatal.
        let before_attempts = kernel.attempt_count_for_task(&task_id).unwrap();
        let late = control
            .submit_batch(&[TaskSpec::new("after-fatal", json!({"o": 2}))])
            .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            kernel
                .attempt_count_for_task(&late.task_ids["after-fatal"])
                .unwrap(),
            0,
            "a failed runtime must not create an Attempt"
        );
        assert_eq!(
            kernel.attempt_count_for_task(&task_id).unwrap(),
            before_attempts
        );

        // Every worker joins, the lock is released, and the next daemon can
        // take the store and recover.
        assert!(matches!(daemon.join(), DaemonExit::Failed(_)));
        let recovered = builder_for(&path).start().unwrap();
        assert_eq!(recovered.phase(), DaemonPhase::Ready);
        recovered.join();
        let _ = std::fs::remove_file(&path);
    }

    fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn start_reaches_ready_and_shutdown_does_not_complete_tasks() {
        let path = temp_store();
        let daemon = builder_for(&path).start().unwrap();
        assert_eq!(daemon.phase(), DaemonPhase::Ready);
        daemon
            .kernel()
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        daemon.kernel().reconcile_pool().unwrap();
        daemon
            .kernel()
            .submit_batch(&[TaskSpec::new("d-run", json!({"o": 1}))])
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let tasks: Vec<_> = daemon
            .kernel()
            .reconciliation_candidates()
            .unwrap()
            .into_iter()
            .map(|s| daemon.kernel().task(s.task_id()).unwrap().state)
            .collect();
        assert!(
            tasks.iter().all(|s| *s != TaskState::Completed),
            "shutdown must not mint Results; got {tasks:?}"
        );
        assert!(matches!(daemon.join(), DaemonExit::Stopped));
        let again = builder_for(&path).start().unwrap();
        assert_eq!(again.phase(), DaemonPhase::Ready);
        again.join();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn same_process_second_start_is_already_running() {
        let path = temp_store();
        let first = builder_for(&path).start().unwrap();
        match builder_for(&path).start() {
            Err(DaemonError::AlreadyRunning) => {}
            Err(err) => panic!("expected AlreadyRunning, got {err}"),
            Ok(_) => panic!("second daemon started while the first still holds the lock"),
        }
        first.join();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn process_lock_is_held_for_the_store_path() {
        let path = temp_store();
        let daemon = builder_for(&path).start().unwrap();
        let cfg = SqliteRuntimeConfig::new(&path, LEASE, 16_384).unwrap();
        match RuntimeProcessGuard::acquire(&cfg) {
            Err(ProcessLockError::AlreadyRunning) => {}
            Err(err) => panic!("daemon must hold the store lock, got {err}"),
            Ok(_) => panic!("second lock acquired while daemon is running"),
        }
        daemon.join();
        let _ = std::fs::remove_file(path);
    }
}
