//! M5.8 production Scheduler daemon composition root.
//!
//! Recovery grants this Runtime the right to begin activation; it does not
//! by itself grant production dispatch capability.

use crate::control::{ControlError, ControlLoopRunner, ControlLoopService};
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
        if supervision.is_failed() {
            drop(observer_service);
            drop(notifier);
            drop(supervision);
            drop(lock);
            return Err(DaemonError::Recovery(RecoveryError::Invariant(
                "supervision failed during activation".into(),
            )));
        }
        let permit = ReadyPermit::mint();
        let control_service = ControlLoopService::new(
            kernel.clone(),
            self.execution_registry,
            self.adapters,
            supervision.admit_sink(),
            &self.timing,
            permit,
        );
        let observer =
            PhysicalObserverRunner::start(observer_service).map_err(DaemonError::Observer)?;
        let control = ControlLoopRunner::start(control_service).map_err(DaemonError::Control)?;
        Ok(RunningSchedulerDaemon {
            _lock: lock,
            kernel,
            control: Some(control),
            observer: Some(observer),
            supervision: Some(supervision),
            notifier,
            stop: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(Mutex::new(DaemonPhase::Ready)),
        })
    }
}

/// Running production daemon. Drop / `join` releases the process lock last.
pub struct RunningSchedulerDaemon {
    _lock: RuntimeProcessGuard,
    kernel: Arc<Kernel>,
    control: Option<ControlLoopRunner>,
    observer: Option<PhysicalObserverRunner>,
    supervision: Option<SupervisionRunner>,
    notifier: Option<NotifierRunner>,
    stop: Arc<AtomicBool>,
    phase: Arc<Mutex<DaemonPhase>>,
}

impl RunningSchedulerDaemon {
    pub fn phase(&self) -> DaemonPhase {
        *self.phase.lock().expect("daemon phase")
    }

    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    pub fn poll_health(&self) {
        let failed = self.control.as_ref().is_some_and(|c| c.is_failed())
            || self.observer.as_ref().is_some_and(|o| o.is_failed())
            || self.supervision.as_ref().is_some_and(|s| s.is_failed())
            || self.notifier.as_ref().is_some_and(|n| n.is_failed());
        if failed {
            let mut phase = self.phase.lock().expect("daemon phase");
            *phase = DaemonPhase::Failed;
            drop(phase);
            self.request_shutdown();
        }
    }

    pub fn request_shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let mut phase = self.phase.lock().expect("daemon phase");
        if *phase == DaemonPhase::Ready {
            *phase = DaemonPhase::Stopping;
        }
        if let Some(control) = &self.control {
            control.request_stop();
        }
        if let Some(observer) = &self.observer {
            observer.request_stop();
        }
        if let Some(supervision) = &self.supervision {
            supervision.request_stop();
        }
        if let Some(notifier) = &self.notifier {
            notifier.request_stop();
        }
    }

    pub fn join(mut self) -> DaemonExit {
        self.poll_health();
        self.request_shutdown();
        let failed = *self.phase.lock().expect("daemon phase") == DaemonPhase::Failed
            || self.control.as_ref().is_some_and(|c| c.is_failed())
            || self.observer.as_ref().is_some_and(|o| o.is_failed())
            || self.supervision.as_ref().is_some_and(|s| s.is_failed())
            || self.notifier.as_ref().is_some_and(|n| n.is_failed());
        drop(self.control.take());
        drop(self.observer.take());
        drop(self.supervision.take());
        drop(self.notifier.take());
        let mut phase = self.phase.lock().expect("daemon phase");
        let exit = if failed {
            *phase = DaemonPhase::Failed;
            DaemonExit::Failed("runtime worker failed".into())
        } else {
            *phase = DaemonPhase::Stopped;
            DaemonExit::Stopped
        };
        drop(phase);
        exit
        // `_lock` drops after runners.
    }
}

impl Drop for RunningSchedulerDaemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(control) = &self.control {
            control.request_stop();
        }
        if let Some(observer) = &self.observer {
            observer.request_stop();
        }
        if let Some(supervision) = &self.supervision {
            supervision.request_stop();
        }
        if let Some(notifier) = &self.notifier {
            notifier.request_stop();
        }
        self.control.take();
        self.observer.take();
        self.supervision.take();
        self.notifier.take();
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
        match daemon.join() {
            DaemonExit::Stopped | DaemonExit::Failed(_) => {}
        }
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
