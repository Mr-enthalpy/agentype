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
use crate::recovery::{recover_runtime, RecoveryError};
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
        let recovered = recover_runtime(kernel.clone(), &self.adapters, self.timing, self.notifier)
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
            supervision.service(),
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
        let now = kernel.now();
        let freshness = self.observer.freshness_limit();
        let local = startup
            .supervision
            .as_ref()
            .expect("supervision")
            .service()
            .observation_snapshots();
        for cand in kernel
            .reconciliation_candidates()
            .map_err(DaemonError::Persistence)?
        {
            if !cand.current_authority_hint().looks_current_at(now) {
                continue;
            }
            if cand.persisted_state() != agentype_core::ExecutionState::Running {
                return Err(DaemonError::Recovery(RecoveryError::Invariant(
                    "current authority is not a fresh RUNNING supervision".into(),
                )));
            }
            let fresh = local.iter().any(|snap| {
                snap.identity.execution_id() == cand.execution_id()
                    && snap.renewal_eligible
                    && now < snap.last_positive_observed_at + freshness
            });
            if !fresh {
                return Err(DaemonError::Recovery(RecoveryError::Invariant(
                    "current authority lacks fresh renewable supervision".into(),
                )));
            }
        }
        let permit = ReadyPermit::mint();
        let gate = DispatchGate::closed();
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
        let observer =
            PhysicalObserverRunner::start(observer_service).map_err(DaemonError::Observer)?;
        let control = ControlLoopRunner::start(control_service).map_err(DaemonError::Control)?;
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
        if !inner.gate.try_commit_ready() {
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
            return Err(DaemonError::Recovery(RecoveryError::Invariant(
                "a runner failed before READY could be committed".into(),
            )));
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

    pub fn kernel(&self) -> &Kernel {
        &self.kernel
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
                return if *shared.phase.lock().expect("daemon phase") == DaemonPhase::Failed {
                    DaemonExit::Failed("runtime worker failed".into())
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
        let failed = *phase.lock().expect("daemon phase") == DaemonPhase::Failed
            || control_fatal.is_some()
            || observer_fatal.is_some()
            || supervision_fatal.is_some()
            || notifier_fatal.is_some();
        if failed {
            DaemonExit::Failed("runtime worker failed".into())
        } else {
            DaemonExit::Stopped
        }
    }
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
    use agentype_core::{PartitionSpec, Retention, TaskSpec, TaskState};
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
