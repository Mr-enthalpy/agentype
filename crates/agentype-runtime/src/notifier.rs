//! M5.5 notifier: durable outbox delivery through a bounded RootBridge.
//!
//! The notifier owns delivery mechanics only. It MUST NOT claim Tasks,
//! renew Leases, ACK/NACK Attempts, create or ACK Results, recompute
//! Batches, or interpret Root wakeup semantics. The only durable state it
//! mutates is `notification_outbox` delivery metadata.
//!
//! RootBridge I/O is never performed while a SQLite transaction is open.
//! Backoff is anchored at call completion (timestamp sampled after
//! BEGIN IMMEDIATE of the short post-call write).

use agentype_core::{Error, OutboxEventId, OutboxState, UnixTime};
use agentype_root_bridge::{RootBridge, RootBridgeError, RootWakeup, WakeupEnvelopeError};
use agentype_storage_sqlite::{Kernel, OutboxDeliveryCandidate};
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Typed retry policy for outbox delivery. Finite, positive, deterministic,
/// overflow-safe exponential backoff. No jitter, no max-attempt ceiling,
/// no dead-letter: an undelivered event remains retryable until Root ACKs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotifierRetryPolicy {
    base_delay: f64,
    max_delay: f64,
}

impl NotifierRetryPolicy {
    pub fn new(base_delay: f64, max_delay: f64) -> Result<Self, NotifierError> {
        if !base_delay.is_finite() || !max_delay.is_finite() {
            return Err(NotifierError::InvalidConfig(
                "retry delays must be finite".into(),
            ));
        }
        if base_delay <= 0.0 || max_delay <= 0.0 {
            return Err(NotifierError::InvalidConfig(
                "retry delays must be positive".into(),
            ));
        }
        if max_delay < base_delay {
            return Err(NotifierError::InvalidConfig(
                "max_delay must be >= base_delay".into(),
            ));
        }
        Ok(Self {
            base_delay,
            max_delay,
        })
    }

    pub fn base_delay(&self) -> f64 {
        self.base_delay
    }

    pub fn max_delay(&self) -> f64 {
        self.max_delay
    }

    /// Delay after a completed bridge call whose attempt number is
    /// `next_attempt_number` (1-based, the attempt that just finished).
    pub fn delay_for(&self, next_attempt_number: u32) -> f64 {
        let exponent = next_attempt_number.saturating_sub(1).min(63);
        let raw = self.base_delay * 2f64.powi(exponent as i32);
        if !raw.is_finite() {
            self.max_delay
        } else {
            raw.min(self.max_delay)
        }
    }
}

/// Finite notifier loop configuration. Timing is independent of heartbeat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotifierConfig {
    poll_interval: f64,
    batch_limit: usize,
    retry_policy: NotifierRetryPolicy,
}

impl NotifierConfig {
    pub fn new(
        poll_interval: f64,
        batch_limit: usize,
        retry_policy: NotifierRetryPolicy,
    ) -> Result<Self, NotifierError> {
        if !poll_interval.is_finite() {
            return Err(NotifierError::InvalidConfig(
                "poll_interval must be finite".into(),
            ));
        }
        if poll_interval <= 0.0 {
            return Err(NotifierError::InvalidConfig(
                "poll_interval must be positive".into(),
            ));
        }
        if Duration::try_from_secs_f64(poll_interval).is_err() {
            return Err(NotifierError::InvalidConfig(
                "poll_interval is not representable as a Duration".into(),
            ));
        }
        if batch_limit == 0 {
            return Err(NotifierError::InvalidConfig(
                "batch_limit must be greater than zero".into(),
            ));
        }
        Ok(Self {
            poll_interval,
            batch_limit,
            retry_policy,
        })
    }

    pub fn poll_interval(&self) -> f64 {
        self.poll_interval
    }

    pub fn poll_duration(&self) -> Duration {
        Duration::from_secs_f64(self.poll_interval)
    }

    pub fn batch_limit(&self) -> usize {
        self.batch_limit
    }

    pub fn retry_policy(&self) -> NotifierRetryPolicy {
        self.retry_policy
    }
}

/// Production vs explicit test-only recovery binding.
///
/// There is no silent "no RootBridge configured" success path and no
/// `NoopRootBridge` that marks events DELIVERED.
pub enum NotifierBinding {
    Enabled {
        config: NotifierConfig,
        bridge: Arc<dyn RootBridge>,
    },
    DisabledForTests,
}

/// Ordinary per-event delivery result. Durable/invariant faults are `Err`.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryOutcome {
    Delivered {
        event_id: OutboxEventId,
    },
    RetryScheduled {
        event_id: OutboxEventId,
    },
    AlreadyTerminal {
        event_id: OutboxEventId,
        state: OutboxState,
    },
}

/// Notifier failures. Ordinary RootBridge errors are per-event data, not
/// these. Storage/invariant faults and worker panic fail the runner.
#[derive(Debug, Clone, PartialEq)]
pub enum NotifierError {
    Persistence(Error),
    Invariant(String),
    InvalidConfig(String),
    Envelope(String),
    RunnerStopped(&'static str),
}

impl NotifierError {
    fn from_kernel(err: Error) -> Self {
        match err {
            Error::InvariantViolation(msg) => Self::Invariant(msg),
            other => Self::Persistence(other),
        }
    }
}

impl fmt::Display for NotifierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence(e) => write!(f, "notifier persistence fault: {e}"),
            Self::Invariant(m) => write!(f, "notifier invariant violation: {m}"),
            Self::InvalidConfig(m) => write!(f, "notifier configuration: {m}"),
            Self::Envelope(m) => write!(f, "notifier wakeup envelope: {m}"),
            Self::RunnerStopped(m) => write!(f, "notifier runner is not running: {m}"),
        }
    }
}

impl std::error::Error for NotifierError {}

/// Deterministic delivery engine. Tests exercise this without threads.
pub struct NotifierService {
    kernel: Arc<Kernel>,
    bridge: Arc<dyn RootBridge>,
    retry: NotifierRetryPolicy,
}

impl NotifierService {
    pub fn new(
        kernel: Arc<Kernel>,
        bridge: Arc<dyn RootBridge>,
        retry: NotifierRetryPolicy,
    ) -> Self {
        Self {
            kernel,
            bridge,
            retry,
        }
    }

    fn now(&self) -> UnixTime {
        self.kernel.now()
    }

    /// Read-only due snapshot. The SQLite lock is released before return.
    pub fn due(
        &self,
        now: UnixTime,
        limit: usize,
    ) -> Result<Vec<OutboxDeliveryCandidate>, NotifierError> {
        self.kernel
            .due_outbox(now, limit)
            .map_err(NotifierError::from_kernel)
    }

    pub fn deliver_due(
        &self,
        now: UnixTime,
        limit: usize,
    ) -> Result<Vec<DeliveryOutcome>, NotifierError> {
        let candidates = self.due(now, limit)?;
        let mut out = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            out.push(self.deliver_one(&candidate)?);
        }
        Ok(out)
    }

    /// Deliver one already-owned candidate. No DB transaction is held
    /// across `RootBridge::deliver`.
    pub fn deliver_one(
        &self,
        candidate: &OutboxDeliveryCandidate,
    ) -> Result<DeliveryOutcome, NotifierError> {
        let wakeup = RootWakeup::from_outbox(
            candidate.event_id().clone(),
            candidate.event_type(),
            candidate.aggregate_type(),
            candidate.aggregate_id(),
            candidate.payload(),
        )
        .map_err(|e: WakeupEnvelopeError| NotifierError::Envelope(e.to_string()))?;

        let bridge_result = self.bridge.deliver(&wakeup);
        match bridge_result {
            Ok(_receipt) => {
                let state = self
                    .kernel
                    .commit_outbox_delivery_success(candidate.event_id())
                    .map_err(NotifierError::from_kernel)?;
                Ok(classify_commit(candidate.event_id(), state, true))
            }
            Err(err) => {
                let next_attempt = candidate.delivery_attempts().saturating_add(1);
                let delay = self.retry.delay_for(next_attempt);
                let state = self
                    .kernel
                    .commit_outbox_delivery_failure(
                        candidate.event_id(),
                        delay,
                        &bridge_diagnostic(&err),
                    )
                    .map_err(NotifierError::from_kernel)?;
                Ok(classify_commit(candidate.event_id(), state, false))
            }
        }
    }
}

fn classify_commit(event_id: &OutboxEventId, state: OutboxState, success: bool) -> DeliveryOutcome {
    match state {
        OutboxState::Delivered if success => DeliveryOutcome::Delivered {
            event_id: event_id.clone(),
        },
        OutboxState::Pending => DeliveryOutcome::RetryScheduled {
            event_id: event_id.clone(),
        },
        other => DeliveryOutcome::AlreadyTerminal {
            event_id: event_id.clone(),
            state: other,
        },
    }
}

fn bridge_diagnostic(err: &RootBridgeError) -> String {
    format!("{err}")
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
    fatal: Option<NotifierError>,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            phase: RunnerPhase::Running,
            fatal: None,
        }
    }
}

struct RunnerShared {
    state: Mutex<RunnerState>,
    signal: Condvar,
}

impl RunnerShared {
    fn is_stopping(this: &Mutex<RunnerState>) -> bool {
        let phase = this.lock().expect("notifier runner state").phase;
        phase != RunnerPhase::Running
    }
}

/// One worker thread owning one [`NotifierService`]. Sequential bounded
/// `deliver()` calls are acceptable: Root throughput is not Scheduler
/// execution correctness.
pub struct NotifierRunner {
    shared: Arc<RunnerShared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl NotifierRunner {
    pub fn start(
        kernel: Arc<Kernel>,
        bridge: Arc<dyn RootBridge>,
        config: NotifierConfig,
    ) -> Result<Self, NotifierError> {
        let service = NotifierService::new(kernel, bridge, config.retry_policy());
        let shared = Arc::new(RunnerShared {
            state: Mutex::new(RunnerState::default()),
            signal: Condvar::new(),
        });
        let thread_shared = shared.clone();
        let poll = config.poll_duration();
        let batch_limit = config.batch_limit();
        let join = std::thread::Builder::new()
            .name("root-notifier".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
                    if RunnerShared::is_stopping(&thread_shared.state) {
                        break;
                    }
                    let now = service.now();
                    let candidates = match service.due(now, batch_limit) {
                        Ok(c) => c,
                        Err(e) => {
                            fail_runner(&thread_shared, e);
                            break;
                        }
                    };
                    for candidate in candidates {
                        if RunnerShared::is_stopping(&thread_shared.state) {
                            break;
                        }
                        if let Err(e) = service.deliver_one(&candidate) {
                            fail_runner(&thread_shared, e);
                            return;
                        }
                    }
                    let state = thread_shared.state.lock().expect("notifier runner state");
                    if state.phase != RunnerPhase::Running {
                        break;
                    }
                    let (s, _) = thread_shared
                        .signal
                        .wait_timeout(state, poll)
                        .expect("notifier runner state");
                    drop(s);
                }));
                if result.is_err() {
                    let mut state = thread_shared.state.lock().expect("notifier runner state");
                    state.phase = RunnerPhase::Failed;
                    if state.fatal.is_none() {
                        state.fatal = Some(NotifierError::Invariant(
                            "the notifier worker thread panicked".into(),
                        ));
                    }
                }
            })
            .map_err(|e| {
                NotifierError::Invariant(format!("failed to spawn the notifier thread: {e}"))
            })?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    /// Signal stop without joining. StartupGuard issues this to both
    /// services before any join so heartbeat cannot keep renewing while
    /// notifier waits on an in-flight bounded `deliver()`.
    pub fn request_stop(&self) {
        let mut state = self.shared.state.lock().expect("notifier runner state");
        if state.phase == RunnerPhase::Running {
            state.phase = RunnerPhase::ShuttingDown;
        }
        drop(state);
        self.shared.signal.notify_all();
    }

    pub fn take_fatal(&self) -> Option<NotifierError> {
        self.shared
            .state
            .lock()
            .expect("notifier runner state")
            .fatal
            .clone()
    }

    pub fn is_failed(&self) -> bool {
        self.shared
            .state
            .lock()
            .expect("notifier runner state")
            .phase
            == RunnerPhase::Failed
    }

    fn stop_and_join(&mut self) -> Option<NotifierError> {
        self.request_stop();
        if let Some(join) = self.join.take() {
            if join.join().is_err() {
                return Some(NotifierError::Invariant(
                    "the notifier worker thread panicked".into(),
                ));
            }
        }
        let mut state = self.shared.state.lock().expect("notifier runner state");
        if state.phase == RunnerPhase::ShuttingDown {
            state.phase = RunnerPhase::Stopped;
        }
        state.fatal.clone()
    }

    pub fn shutdown(mut self) -> Result<(), NotifierError> {
        self.stop_and_join().map_or(Ok(()), Err)
    }
}

fn fail_runner(shared: &RunnerShared, err: NotifierError) {
    let mut state = shared.state.lock().expect("notifier runner state");
    if state.fatal.is_none() {
        state.fatal = Some(err);
    }
    state.phase = RunnerPhase::Failed;
}

impl Drop for NotifierRunner {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervision::SupervisionRunner;
    use crate::timing::RuntimeTimingConfig;
    use crate::{
        AdapterRegistry, DispatchOneOutcome, Dispatcher, ExecutionProfileConfig, ExecutionRegistry,
        ExecutionTargetConfig, FrozenExecutionSafety, FrozenPhysicalExecutionBinding,
        SupervisionAdmission,
    };
    use agentype_adapter_api::FakeAdapter;
    use agentype_core::*;
    use agentype_root_bridge::{RecordingRootBridge, RootIndex};
    use agentype_storage_sqlite::Kernel;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    fn seeded_kernel(clock: Arc<ManualClock>, k: Kernel) -> (Arc<ManualClock>, Arc<Kernel>) {
        k.upsert_partition(&PartitionSpec::new(
            "general",
            2,
            Retention::Resident,
            "local",
            "default",
        ))
        .unwrap();
        k.reconcile_pool().unwrap();
        (clock, Arc::new(k))
    }

    fn env() -> (Arc<ManualClock>, Arc<Kernel>) {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let k = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();
        seeded_kernel(clock, k)
    }

    struct FileEnv {
        dir: PathBuf,
        path: PathBuf,
    }

    impl Drop for FileEnv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn file_env() -> (Arc<ManualClock>, Arc<Kernel>, FileEnv) {
        let dir = std::env::temp_dir().join(format!(
            "agentype-m55-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduler.db");
        let clock = Arc::new(ManualClock::new(1_000.0));
        let k = Kernel::open(&path, clock.clone(), 10.0, 16_384).unwrap();
        let (clock, k) = seeded_kernel(clock, k);
        (clock, k, FileEnv { dir, path })
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

    fn complete_one(k: &Kernel, name: &str, payload: Value) -> (BatchId, TaskId, OutboxEventId) {
        let spec = TaskSpec::new(name, payload);
        let (batch, ids) = k.submit_batch(std::slice::from_ref(&spec)).unwrap();
        let task_id = ids.values().next().unwrap().clone();
        let claim = k.claim_next_available().unwrap().unwrap();
        let launch = k.create_execution(&claim, binding(&claim)).unwrap();
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
            &json!({"secret": "result-body"}),
            Some("summary"),
            true,
            false,
        )
        .unwrap();
        let event = k.outbox_for_batch(&batch, BATCH_RESULTS_READY).unwrap()[0]
            .id
            .clone();
        (batch, task_id, event)
    }

    fn retry() -> NotifierRetryPolicy {
        NotifierRetryPolicy::new(1.0, 8.0).unwrap()
    }

    fn cfg() -> NotifierConfig {
        NotifierConfig::new(0.05, 8, retry()).unwrap()
    }

    fn service(k: Arc<Kernel>, bridge: Arc<RecordingRootBridge>) -> NotifierService {
        NotifierService::new(k, bridge, retry())
    }

    #[test]
    fn retry_policy_is_bounded_exponential() {
        let p = retry();
        assert_eq!(p.delay_for(1), 1.0);
        assert_eq!(p.delay_for(2), 2.0);
        assert_eq!(p.delay_for(4), 8.0);
        assert_eq!(p.delay_for(10), 8.0);
        assert!(NotifierRetryPolicy::new(2.0, 1.0).is_err());
        assert!(NotifierRetryPolicy::new(f64::NAN, 1.0).is_err());
    }

    #[test]
    fn successful_bridge_marks_delivered_and_does_not_ack() {
        let (clock, k) = env();
        let (batch, task_id, event) = complete_one(&k, "ok", json!({"o": 1}));
        let result_before = k.result_for_task(&task_id).unwrap();
        let durable = k.outbox_delivery(&event).unwrap();
        let bridge = Arc::new(RecordingRootBridge::new());
        let svc = service(k.clone(), bridge.clone());
        let due = svc.due(clock.now(), 8).unwrap();
        assert_eq!(due[0].event_id(), &durable.event_id);
        assert_eq!(due[0].event_type(), durable.event_type);
        assert_eq!(due[0].aggregate_type(), durable.aggregate_type);
        assert_eq!(due[0].aggregate_id(), durable.aggregate_id);
        assert_eq!(due[0].payload(), &durable.payload);
        let outcomes = svc.deliver_due(clock.now(), 8).unwrap();
        assert!(matches!(outcomes[0], DeliveryOutcome::Delivered { .. }));
        let snap = k.outbox_delivery(&event).unwrap();
        assert_eq!(snap.state, OutboxState::Delivered);
        assert_eq!(
            k.result_for_task(&task_id).unwrap().state,
            result_before.state
        );
        assert_eq!(result_before.state, ResultState::Available);
        assert_eq!(k.batch(&batch).unwrap().state, BatchState::Completed);
        assert_eq!(bridge.deliver_count(), 1);
        let wakeup = &bridge.deliveries()[0];
        assert_eq!(wakeup.event_id(), &event);
        assert_eq!(wakeup.event_type(), BATCH_RESULTS_READY);
        assert_eq!(wakeup.aggregate_type(), "batch");
        assert_eq!(wakeup.aggregate_id(), batch.as_str());
        assert_eq!(
            wakeup.indexes().get("batch_id"),
            Some(&RootIndex::Id(batch.as_str().into()))
        );
        assert!(!wakeup.indexes().contains_key("secret"));
        assert!(!wakeup.indexes().contains_key("result_body"));
    }

    #[test]
    fn failed_bridge_remains_pending_and_does_not_touch_task() {
        let (clock, k) = env();
        let (_batch, task_id, event) = complete_one(&k, "fail", json!({"o": 1}));
        let task_state = k.task(&task_id).unwrap().state;
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.script_err(RootBridgeError::Unavailable("down".into()));
        let svc = service(k.clone(), bridge.clone());
        let outcomes = svc.deliver_due(clock.now(), 8).unwrap();
        assert!(matches!(
            outcomes[0],
            DeliveryOutcome::RetryScheduled { .. }
        ));
        let snap = k.outbox_delivery(&event).unwrap();
        assert_eq!(snap.state, OutboxState::Pending);
        assert_eq!(snap.delivery_attempts, 1);
        assert!((snap.next_delivery_at - (clock.now() + 1.0)).abs() < 0.001);
        assert_eq!(k.task(&task_id).unwrap().state, task_state);
        assert_eq!(
            k.result_for_task(&task_id).unwrap().state,
            ResultState::Available
        );
    }

    #[test]
    fn one_failed_event_does_not_starve_later_due_event() {
        let (clock, k) = env();
        let (_b1, _t1, first) = complete_one(&k, "a", json!({"o": 1}));
        clock.advance(1.0);
        let (_b2, _t2, second) = complete_one(&k, "b", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.script_err(RootBridgeError::DeadlineExceeded("slow".into()));
        let svc = service(k.clone(), bridge.clone());
        let outcomes = svc.deliver_due(clock.now(), 8).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(
            outcomes[0],
            DeliveryOutcome::RetryScheduled { .. }
        ));
        assert!(matches!(outcomes[1], DeliveryOutcome::Delivered { .. }));
        assert_eq!(
            k.outbox_delivery(&first).unwrap().state,
            OutboxState::Pending
        );
        assert_eq!(
            k.outbox_delivery(&second).unwrap().state,
            OutboxState::Delivered
        );
    }

    #[test]
    fn completion_time_backoff_uses_post_call_clock() {
        let (clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "slow", json!({"o": 1}));
        let start = clock.now();
        let bridge = Arc::new(RecordingRootBridge::with_clock(clock.clone()));
        bridge.set_advance_on_deliver(100.0);
        bridge.script_err(RootBridgeError::Unavailable("slow-fail".into()));
        let svc = service(k.clone(), bridge);
        svc.deliver_due(start, 8).unwrap();
        let snap = k.outbox_delivery(&event).unwrap();
        assert!(
            (snap.next_delivery_at - (start + 100.0 + 1.0)).abs() < 0.001,
            "got next_delivery_at={} start={}",
            snap.next_delivery_at,
            start
        );
        assert!(snap.next_delivery_at > start + 10.0);
    }

    #[test]
    fn ack_before_scan_skips_delivery() {
        let (clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "acked", json!({"o": 1}));
        k.ack_outbox(&event).unwrap();
        let bridge = Arc::new(RecordingRootBridge::new());
        let svc = service(k, bridge.clone());
        assert!(svc.deliver_due(clock.now(), 8).unwrap().is_empty());
        assert_eq!(bridge.deliver_count(), 0);
    }

    #[test]
    fn ack_during_inflight_success_stays_acked() {
        let (clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "race-ok", json!({"o": 1}));
        let due = k.due_outbox(clock.now(), 8).unwrap();
        k.ack_outbox(&event).unwrap();
        let bridge = Arc::new(RecordingRootBridge::new());
        let svc = service(k.clone(), bridge.clone());
        let outcome = svc.deliver_one(&due[0]).unwrap();
        assert!(matches!(
            outcome,
            DeliveryOutcome::AlreadyTerminal {
                state: OutboxState::Acked,
                ..
            }
        ));
        assert_eq!(k.outbox_delivery(&event).unwrap().state, OutboxState::Acked);
        assert_eq!(bridge.deliver_count(), 1);
    }

    #[test]
    fn ack_during_inflight_failure_stays_acked() {
        let (clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "race-fail", json!({"o": 1}));
        let due = k.due_outbox(clock.now(), 8).unwrap();
        k.ack_outbox(&event).unwrap();
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.script_err(RootBridgeError::Rejected("no".into()));
        let svc = service(k.clone(), bridge);
        let outcome = svc.deliver_one(&due[0]).unwrap();
        assert!(matches!(
            outcome,
            DeliveryOutcome::AlreadyTerminal {
                state: OutboxState::Acked,
                ..
            }
        ));
        let snap = k.outbox_delivery(&event).unwrap();
        assert_eq!(snap.state, OutboxState::Acked);
        assert_eq!(snap.delivery_attempts, 0);
    }

    #[test]
    fn malformed_index_is_fatal_not_bridge_failure() {
        let (clock, k, db) = file_env();
        let (_batch, _task, event) = complete_one(&k, "bad-index", json!({"o": 1}));
        {
            let conn = rusqlite::Connection::open(&db.path).unwrap();
            conn.execute(
                "UPDATE notification_outbox SET payload_json=?1 WHERE id=?2",
                rusqlite::params![r#"{"batch_id":1}"#, event.as_str()],
            )
            .unwrap();
        }
        let due = k.due_outbox(clock.now(), 8).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].event_id(), &event);
        let bridge = Arc::new(RecordingRootBridge::new());
        let svc = service(k.clone(), bridge.clone());
        let err = svc.deliver_one(&due[0]).unwrap_err();
        assert!(matches!(err, NotifierError::Envelope(_)));
        assert_eq!(bridge.deliver_count(), 0);
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );
    }

    #[test]
    fn result_ack_does_not_ack_outbox_and_outbox_ack_does_not_ack_result() {
        let (_clock, k) = env();
        let (_batch, task_id, event) = complete_one(&k, "indep", json!({"o": 1}));
        let result = k.result_for_task(&task_id).unwrap();
        k.ack_result(&result.id, "root").unwrap();
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );
        k.ack_outbox(&event).unwrap();
        assert_eq!(
            k.result_for_task(&task_id).unwrap().state,
            ResultState::Acked
        );
    }

    #[test]
    fn runner_delivers_due_event_and_drop_joins() {
        let (_clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "run", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        let runner = NotifierRunner::start(k.clone(), bridge.clone(), cfg()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while k.outbox_delivery(&event).unwrap().state != OutboxState::Delivered {
            assert!(
                std::time::Instant::now() < deadline,
                "notifier did not deliver"
            );
            thread::sleep(Duration::from_millis(10));
        }
        runner.shutdown().unwrap();
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Delivered
        );
    }

    #[test]
    fn ordinary_bridge_failure_does_not_fail_runner() {
        let (_clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "retry-run", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.script_err(RootBridgeError::Unavailable("tmp".into()));
        let runner = NotifierRunner::start(k.clone(), bridge, cfg()).unwrap();
        thread::sleep(Duration::from_millis(80));
        assert!(!runner.is_failed());
        assert!(runner.take_fatal().is_none());
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );
        runner.shutdown().unwrap();
    }

    #[test]
    fn stop_does_not_start_another_event_after_inflight() {
        let (clock, k) = env();
        let (_b1, _t1, first) = complete_one(&k, "one", json!({"o": 1}));
        clock.advance(1.0);
        let (_b2, _t2, second) = complete_one(&k, "two", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.hold();
        let runner = NotifierRunner::start(k.clone(), bridge.clone(), cfg()).unwrap();
        assert!(bridge.wait_until_in_flight_timeout(Duration::from_secs(2)));
        runner.request_stop();
        bridge.release();
        runner.shutdown().unwrap();
        let first_state = k.outbox_delivery(&first).unwrap().state;
        assert_eq!(first_state, OutboxState::Delivered);
        assert_eq!(
            k.outbox_delivery(&second).unwrap().state,
            OutboxState::Pending
        );
        assert_eq!(bridge.deliver_count(), 1);
    }

    #[test]
    fn worker_panic_becomes_observable_failed() {
        let (_clock, k) = env();
        let (_batch, _task, _event) = complete_one(&k, "panic", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.script_panic("injected notifier panic");
        let runner = NotifierRunner::start(k, bridge, cfg()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !runner.is_failed() {
            assert!(
                std::time::Instant::now() < deadline,
                "panic was not observed"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let fatal = runner.take_fatal().expect("fatal");
        assert!(matches!(fatal, NotifierError::Invariant(_)));
        // Drop joins; shutdown reports the panic.
        let err = runner.shutdown().unwrap_err();
        assert!(matches!(err, NotifierError::Invariant(_)));
    }

    #[test]
    fn inflight_bounded_call_persists_before_shutdown() {
        let (_clock, k) = env();
        let (_batch, _task, event) = complete_one(&k, "inflight", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.hold();
        let runner = NotifierRunner::start(k.clone(), bridge.clone(), cfg()).unwrap();
        assert!(bridge.wait_until_in_flight_timeout(Duration::from_secs(2)));
        let stopper = thread::spawn(move || runner.shutdown());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );
        bridge.release();
        stopper.join().unwrap().unwrap();
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Delivered
        );
    }

    #[test]
    fn slow_bridge_does_not_hold_sqlite_or_block_heartbeat() {
        let (clock, k) = env();
        let spec = TaskSpec::new("live", json!({"o": 1}));
        k.submit_batch(std::slice::from_ref(&spec)).unwrap();
        let claim = k.claim_next_available().unwrap().unwrap();
        let launch = k.create_execution(&claim, binding(&claim)).unwrap();
        let grant = k
            .confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                launch.execution_id(),
                &json!({}),
            )
            .unwrap();
        let timing = RuntimeTimingConfig::new(0.05, 0.1, 10.0).unwrap();
        let supervisor = SupervisionRunner::start(k.clone(), timing).unwrap();
        supervisor
            .admit(SupervisionAdmission::from_grant(grant))
            .unwrap();
        let before = k
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .heartbeat_at;

        let (_batch, _task, event) = complete_one(&k, "blocked", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.hold();
        let notifier = NotifierRunner::start(k.clone(), bridge.clone(), cfg()).unwrap();
        assert!(bridge.wait_until_in_flight_timeout(Duration::from_secs(2)));

        // Write transactions must proceed while deliver() is in flight.
        k.expire_leases(false).unwrap();
        clock.advance(0.2);
        thread::sleep(Duration::from_millis(150));
        let after = k
            .lease_supervision_view(&claim.attempt_id)
            .unwrap()
            .heartbeat_at;
        assert!(
            after > before,
            "heartbeat must renew while RootBridge is blocked"
        );
        assert_eq!(
            k.outbox_delivery(&event).unwrap().state,
            OutboxState::Pending
        );

        bridge.release();
        notifier.shutdown().unwrap();
        let still = k.lease_supervision_view(&claim.attempt_id).unwrap();
        assert_eq!(still.state, LeaseState::Active);
        supervisor.shutdown().unwrap();
    }

    #[test]
    fn notifier_shutdown_does_not_revoke_lease_or_change_execution() {
        let (_clock, k) = env();
        let spec = TaskSpec::new("keep", json!({"o": 1}));
        k.submit_batch(std::slice::from_ref(&spec)).unwrap();
        let claim = k.claim_next_available().unwrap().unwrap();
        let launch = k.create_execution(&claim, binding(&claim)).unwrap();
        k.confirm_running_and_renew(
            &claim.attempt_id,
            claim.lease_epoch,
            launch.execution_id(),
            &json!({}),
        )
        .unwrap();
        let exec_before = k.execution(launch.execution_id()).unwrap().state;
        let lease_before = k.lease_for_attempt(&claim.attempt_id).unwrap().state;
        let (_batch, _task, _event) = complete_one(&k, "n", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        let notifier = NotifierRunner::start(k.clone(), bridge, cfg()).unwrap();
        notifier.shutdown().unwrap();
        assert_eq!(
            k.execution(launch.execution_id()).unwrap().state,
            exec_before
        );
        assert_eq!(
            k.lease_for_attempt(&claim.attempt_id).unwrap().state,
            lease_before
        );
        assert_eq!(exec_before, ExecutionState::Running);
        assert_eq!(lease_before, LeaseState::Active);
    }

    #[test]
    fn slow_bridge_does_not_block_dispatch_one() {
        let (_clock, k) = env();
        let (_batch, _task, _event) = complete_one(&k, "blocked-dispatch", json!({"o": 1}));
        let bridge = Arc::new(RecordingRootBridge::new());
        bridge.hold();
        let notifier = NotifierRunner::start(k.clone(), bridge.clone(), cfg()).unwrap();
        assert!(bridge.wait_until_in_flight_timeout(Duration::from_secs(2)));

        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        let fake = Arc::new(FakeAdapter::new());
        let mut adapters = AdapterRegistry::new();
        adapters.register("process", fake.clone()).unwrap();
        k.submit_batch(&[TaskSpec::new("while-blocked", json!({"o": 1}))])
            .unwrap();
        let d = Dispatcher::new(k.as_ref(), &registry, &adapters);
        match d.dispatch_one().unwrap() {
            DispatchOneOutcome::RunningAdmitted { .. } => {}
            other => panic!("expected RunningAdmitted while notifier blocked, got {other:?}"),
        }

        bridge.release();
        notifier.shutdown().unwrap();
    }
}
