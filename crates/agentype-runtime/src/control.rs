//! M5.8 control loop: mechanical maintenance + ReadyPermit-gated dispatch.
//!
//! This loop does not renew Leases and does not observe adapters.

use crate::recovery::AdmissionSink;
use crate::timing::RuntimeTimingConfig;
use crate::{
    AdapterRegistry, DispatchError, DispatchOneOutcome, Dispatcher, ExecutionId, ExecutionRegistry,
    ReadyPermit, SupervisionError,
};
use agentype_core::Error;
use agentype_core::FailureClass;
use agentype_storage_sqlite::Kernel;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Shared eligibility to start new physical executions. Revoked on
/// graceful shutdown and on the first structural runner failure.
#[derive(Clone, Debug)]
pub struct DispatchGate {
    allowed: Arc<AtomicBool>,
}

impl DispatchGate {
    pub(crate) fn open() -> Self {
        Self {
            allowed: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(crate) fn revoke(&self) {
        self.allowed.store(false, Ordering::SeqCst);
    }

    pub(crate) fn is_open(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }
}

/// Dispatch result after ControlLoop has consumed a `RunningAdmitted`
/// admission into supervision.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlDispatch {
    NoWork,
    RunningAdmitted {
        execution_id: ExecutionId,
    },
    StartIndeterminate {
        execution_id: ExecutionId,
        failure_class: Option<FailureClass>,
    },
    AuthorityRejected,
    ConfigurationUnavailable {
        detail: String,
    },
}

#[derive(Debug)]
pub struct ControlCycleReport {
    pub dispatch: ControlDispatch,
    pub wait: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlError {
    Persistence(Error),
    Dispatch(DispatchError),
    Fatal(SupervisionError),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence(err) => write!(f, "control-loop persistence: {err}"),
            Self::Dispatch(err) => write!(f, "control-loop dispatch: {err}"),
            Self::Fatal(err) => write!(f, "control-loop fatal: {err}"),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<Error> for ControlError {
    fn from(err: Error) -> Self {
        Self::Persistence(err)
    }
}

impl From<DispatchError> for ControlError {
    fn from(err: DispatchError) -> Self {
        Self::Dispatch(err)
    }
}

/// Deterministic control loop. Construction requires `ReadyPermit`.
pub struct ControlLoopService<S> {
    kernel: Arc<Kernel>,
    execution_registry: ExecutionRegistry,
    adapters: AdapterRegistry,
    supervision: S,
    poll: Duration,
    _permit: ReadyPermit,
    gate: DispatchGate,
}

impl<S: AdmissionSink> ControlLoopService<S> {
    pub(crate) fn new(
        kernel: Arc<Kernel>,
        execution_registry: ExecutionRegistry,
        adapters: AdapterRegistry,
        supervision: S,
        timing: &RuntimeTimingConfig,
        permit: ReadyPermit,
        gate: DispatchGate,
    ) -> Self {
        Self {
            kernel,
            execution_registry,
            adapters,
            supervision,
            poll: timing.dispatcher_poll_interval(),
            _permit: permit,
            gate,
        }
    }

    pub fn poll_interval(&self) -> Duration {
        self.poll
    }

    /// Expire / promote / pool / revive, then at most one dispatch.
    /// `RunningAdmitted` is handed to supervision before this returns.
    pub fn cycle(&self) -> Result<ControlCycleReport, ControlError> {
        self.kernel.expire_leases(false)?;
        self.kernel.promote_retry_wait()?;
        self.kernel.reconcile_pool()?;
        self.kernel.revive_eligible_agents()?;
        if !self.gate.is_open() {
            return Ok(ControlCycleReport {
                dispatch: ControlDispatch::NoWork,
                wait: self.poll,
            });
        }
        let dispatcher = Dispatcher::new(&self.kernel, &self.execution_registry, &self.adapters)
            .with_gate(&self.gate);
        let dispatch = match dispatcher.dispatch_one()? {
            DispatchOneOutcome::NoWork => ControlDispatch::NoWork,
            DispatchOneOutcome::AuthorityRejected => ControlDispatch::AuthorityRejected,
            DispatchOneOutcome::ConfigurationUnavailable { detail } => {
                ControlDispatch::ConfigurationUnavailable { detail }
            }
            DispatchOneOutcome::StartIndeterminate {
                execution_id,
                failure_class,
                ..
            } => ControlDispatch::StartIndeterminate {
                execution_id,
                failure_class,
            },
            DispatchOneOutcome::RunningAdmitted { admission } => {
                let execution_id = admission.execution_id().clone();
                match self.supervision.admit(admission) {
                    Ok(()) => ControlDispatch::RunningAdmitted { execution_id },
                    Err(SupervisionError::RunnerStopped(_)) if !self.gate.is_open() => {
                        ControlDispatch::NoWork
                    }
                    Err(err) => return Err(ControlError::Fatal(err)),
                }
            }
        };
        Ok(ControlCycleReport {
            dispatch,
            wait: self.poll,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RunnerPhase {
    Running,
    ShuttingDown,
    Failed,
    Stopped,
}

struct RunnerState {
    phase: RunnerPhase,
    fatal: Option<ControlError>,
}

struct RunnerShared {
    state: Mutex<RunnerState>,
    signal: Condvar,
}

/// Background control loop. Stopped on drop. Does not busy-spin on NoWork.
pub struct ControlLoopRunner {
    shared: Arc<RunnerShared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ControlLoopRunner {
    pub fn start<S>(service: ControlLoopService<S>) -> Result<Self, ControlError>
    where
        S: AdmissionSink + Send + 'static,
    {
        let shared = Arc::new(RunnerShared {
            state: Mutex::new(RunnerState {
                phase: RunnerPhase::Running,
                fatal: None,
            }),
            signal: Condvar::new(),
        });
        let thread_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("control-loop".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
                    {
                        let state = thread_shared.state.lock().expect("control runner state");
                        if state.phase != RunnerPhase::Running {
                            break;
                        }
                    }
                    match service.cycle() {
                        Ok(report) => {
                            let state = thread_shared.state.lock().expect("control runner state");
                            if state.phase != RunnerPhase::Running {
                                break;
                            }
                            let (guard, _) = thread_shared
                                .signal
                                .wait_timeout(state, report.wait)
                                .expect("control runner wait");
                            drop(guard);
                        }
                        Err(err) => {
                            let mut state =
                                thread_shared.state.lock().expect("control runner state");
                            state.phase = RunnerPhase::Failed;
                            state.fatal = Some(err);
                            break;
                        }
                    }
                }));
                if result.is_err() {
                    let mut state = thread_shared.state.lock().expect("control runner state");
                    state.phase = RunnerPhase::Failed;
                    state.fatal = Some(ControlError::Fatal(SupervisionError::Fatal(
                        Error::invariant("control-loop thread panicked"),
                    )));
                }
                let mut state = thread_shared.state.lock().expect("control runner state");
                if state.phase == RunnerPhase::Running {
                    state.phase = RunnerPhase::Stopped;
                }
            })
            .map_err(|err| {
                ControlError::Fatal(SupervisionError::Fatal(Error::invariant(format!(
                    "failed to spawn control-loop thread: {err}"
                ))))
            })?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    pub fn request_stop(&self) {
        let mut state = self.shared.state.lock().expect("control runner state");
        if state.phase == RunnerPhase::Running {
            state.phase = RunnerPhase::ShuttingDown;
        }
        self.shared.signal.notify_all();
    }

    pub fn fatal(&self) -> Option<String> {
        self.shared
            .state
            .lock()
            .expect("control runner state")
            .fatal
            .as_ref()
            .map(ToString::to_string)
    }

    pub fn is_failed(&self) -> bool {
        self.shared
            .state
            .lock()
            .expect("control runner state")
            .phase
            == RunnerPhase::Failed
    }

    pub(crate) fn join_fatal(mut self) -> Option<String> {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.fatal()
    }
}

impl Drop for ControlLoopRunner {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        let mut state = self.shared.state.lock().expect("control runner state");
        if state.phase == RunnerPhase::ShuttingDown {
            state.phase = RunnerPhase::Stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deadlines::test_deadlines;
    use crate::process_lock::ReadyPermit;
    use crate::{AdapterRegistry, SupervisionRunner, SupervisionService};
    use agentype_adapter_api::FakeAdapter;
    use agentype_core::{
        FailureClass, LeaseState, ManualClock, PartitionSpec, Retention, RetryPolicy, TaskSpec,
        TaskState,
    };
    use agentype_execution_config::{
        ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig,
    };
    use agentype_storage_sqlite::Kernel;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Instant;

    const LEASE: f64 = 10.0;

    fn timing() -> RuntimeTimingConfig {
        RuntimeTimingConfig::new(1.0, 2.0, LEASE).unwrap()
    }

    fn env() -> (
        Arc<ManualClock>,
        Arc<Kernel>,
        ExecutionRegistry,
        AdapterRegistry,
        Arc<FakeAdapter>,
    ) {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Arc::new(Kernel::open_memory(clock.clone(), LEASE, 16_384).unwrap());
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
            .register_kind("process", fake.clone(), test_deadlines())
            .unwrap();
        (clock, kernel, registry, adapters, fake)
    }

    fn retryable() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            retry_classes: vec![FailureClass::Timeout, FailureClass::ExecutionLost],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        }
    }

    fn loop_svc(
        kernel: Arc<Kernel>,
        registry: ExecutionRegistry,
        adapters: AdapterRegistry,
        supervision: SupervisionService,
    ) -> ControlLoopService<SupervisionService> {
        ControlLoopService::new(
            kernel,
            registry,
            adapters,
            supervision,
            &timing(),
            ReadyPermit::mint(),
            DispatchGate::open(),
        )
    }

    #[test]
    fn construction_requires_ready_permit() {
        let (_clock, kernel, registry, adapters, _fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let control = loop_svc(kernel, registry, adapters, svc);
        assert_eq!(control.poll_interval(), Duration::from_secs(1));
        assert!(matches!(
            control.cycle().unwrap().dispatch,
            ControlDispatch::NoWork
        ));
    }

    #[test]
    fn no_work_returns_poll_wait_without_busy_spin() {
        let (_clock, kernel, registry, adapters, _fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let control = loop_svc(kernel, registry, adapters, svc);
        let started = Instant::now();
        let report = control.cycle().unwrap();
        assert!(matches!(report.dispatch, ControlDispatch::NoWork));
        assert_eq!(report.wait, Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn running_admitted_is_handed_to_supervision() {
        let (_clock, kernel, registry, adapters, _fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        kernel
            .submit_batch(&[TaskSpec::new("ctrl-run", json!({"o": 1}))])
            .unwrap();
        let control = loop_svc(kernel.clone(), registry, adapters, svc.clone());
        match control.cycle().unwrap().dispatch {
            ControlDispatch::RunningAdmitted { execution_id } => {
                assert!(svc.contains(&execution_id));
            }
            other => panic!("expected RunningAdmitted, got {other:?}"),
        }
    }

    #[test]
    fn expire_sweeps_overdue_lease_without_restart() {
        let (clock, kernel, registry, adapters, _fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        kernel
            .submit_batch(&[TaskSpec::new("ctrl-exp", json!({"o": 1}))])
            .unwrap();
        let control = loop_svc(kernel.clone(), registry, adapters, svc);
        let report = control.cycle().unwrap();
        let ControlDispatch::RunningAdmitted { execution_id } = report.dispatch else {
            panic!("expected start, got {:?}", report.dispatch);
        };
        let exec = kernel.execution(&execution_id).unwrap();
        clock.advance(11.0);
        control.cycle().unwrap();
        let lease = kernel.lease_supervision_view(&exec.attempt_id).unwrap();
        assert_ne!(lease.state, LeaseState::Active);
    }

    #[test]
    fn retry_wait_is_promoted_then_redispatched() {
        let (clock, kernel, registry, adapters, fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        kernel
            .submit_batch(&[TaskSpec::new("ctrl-retry", json!({"o": 1})).retry(retryable())])
            .unwrap();
        fake.set_next_start_error(agentype_adapter_api::AdapterError::deadline_exceeded(
            "start blocked",
        ));
        let control = loop_svc(kernel.clone(), registry, adapters, svc);
        let first = control.cycle().unwrap();
        assert!(matches!(
            first.dispatch,
            ControlDispatch::StartIndeterminate { .. }
        ));
        clock.advance(2.0);
        fake.set_next_start_error(agentype_adapter_api::AdapterError::deadline_exceeded(
            "should not run before promote",
        ));
        let _ = control.cycle().unwrap();
        clock.advance(0.0);
        let task = kernel
            .task(kernel.reconciliation_candidates().unwrap()[0].task_id())
            .unwrap();
        assert!(
            task.state == TaskState::RetryWait || task.state == TaskState::Queued,
            "got {:?}",
            task.state
        );
    }

    #[test]
    fn ordinary_start_indeterminate_is_not_fatal() {
        let (_clock, kernel, registry, adapters, fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        kernel
            .submit_batch(&[TaskSpec::new("ctrl-indet", json!({"o": 1}))])
            .unwrap();
        fake.set_next_start_error(agentype_adapter_api::AdapterError::deadline_exceeded(
            "blocked",
        ));
        let control = loop_svc(kernel, registry, adapters, svc);
        let report = control.cycle().unwrap();
        assert!(matches!(
            report.dispatch,
            ControlDispatch::StartIndeterminate { .. }
        ));
    }

    #[test]
    fn supervision_admit_failure_is_fatal() {
        let (_clock, kernel, registry, adapters, _fake) = env();
        let runner = SupervisionRunner::start(kernel.clone(), timing()).unwrap();
        runner.request_stop();
        kernel
            .submit_batch(&[TaskSpec::new("ctrl-fatal", json!({"o": 1}))])
            .unwrap();
        let control = ControlLoopService::new(
            kernel,
            registry,
            adapters,
            runner,
            &timing(),
            ReadyPermit::mint(),
            DispatchGate::open(),
        );
        match control.cycle() {
            Err(ControlError::Fatal(SupervisionError::RunnerStopped(_))) => {}
            other => panic!("expected Fatal RunnerStopped, got {other:?}"),
        }
    }

    #[test]
    fn runner_stops_on_drop_without_busy_spin() {
        let (_clock, kernel, registry, adapters, _fake) = env();
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let control = loop_svc(kernel, registry, adapters, svc);
        let started = Instant::now();
        drop(ControlLoopRunner::start(control).unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
