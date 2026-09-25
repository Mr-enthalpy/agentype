//! M5.8 physical observation: process-local freshness, not Lease authority.
//!
//! PhysicalObserver decides whether renewal remains eligible.
//! SupervisionRunner remains the only component allowed to perform renewal.

use crate::observation::adapter_invocation_failure_class;
use crate::supervision::{SupervisionIdentity, SupervisionService};
use crate::{
    persist_physical_end_then_nack, AdapterBindingKey, AdapterRegistry, AuthorityConsequence,
    ExecutionId,
};
use agentype_adapter_api::{AdapterError, ExecutionObservation};
use agentype_core::{Error, ExecutionState, FailureClass, UnixTime};
use agentype_storage_sqlite::Kernel;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Observer timing. Distinct from AdapterDeadlinePolicy and lease duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicalObserverConfig {
    poll_interval: f64,
    freshness_limit: f64,
    batch_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserverConfigError {
    NonPositive { field: &'static str },
    Unrepresentable { field: &'static str },
    Relation,
}

impl fmt::Display for ObserverConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositive { field } => {
                write!(f, "observer {field} must be finite and positive")
            }
            Self::Unrepresentable { field } => {
                write!(f, "observer {field} is not a representable duration")
            }
            Self::Relation => write!(
                f,
                "observer config must satisfy 0 < poll_interval < freshness_limit < lease_seconds"
            ),
        }
    }
}

impl std::error::Error for ObserverConfigError {}

impl PhysicalObserverConfig {
    pub fn new(
        poll_interval: f64,
        freshness_limit: f64,
        batch_limit: usize,
        lease_seconds: f64,
    ) -> Result<Self, ObserverConfigError> {
        for (field, value) in [
            ("poll_interval", poll_interval),
            ("freshness_limit", freshness_limit),
            ("lease_seconds", lease_seconds),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(ObserverConfigError::NonPositive { field });
            }
            if std::time::Duration::try_from_secs_f64(value).is_err() {
                return Err(ObserverConfigError::Unrepresentable { field });
            }
        }
        if batch_limit == 0 {
            return Err(ObserverConfigError::NonPositive {
                field: "batch_limit",
            });
        }
        if !(poll_interval < freshness_limit && freshness_limit < lease_seconds) {
            return Err(ObserverConfigError::Relation);
        }
        Ok(Self {
            poll_interval,
            freshness_limit,
            batch_limit,
        })
    }

    pub fn poll_interval(&self) -> f64 {
        self.poll_interval
    }

    pub fn freshness_limit(&self) -> f64 {
        self.freshness_limit
    }

    pub fn batch_limit(&self) -> usize {
        self.batch_limit
    }
}

/// Runtime-local scheduling identity. Not Task/Lease authority.
#[derive(Debug, Clone)]
pub struct ObservationTicket {
    execution_id: ExecutionId,
    identity: SupervisionIdentity,
    #[allow(dead_code)]
    request_id: agentype_core::RequestId,
}

impl ObservationTicket {
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
}

#[derive(Clone)]
struct WatchState {
    ticket: ObservationTicket,
    next_observation_at: UnixTime,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WatchDisposition {
    Keep,
    Forget,
}

fn watch_disposition(applied: &ObserveApply) -> WatchDisposition {
    match applied {
        ObserveApply::PhysicalEnded
        | ObserveApply::IdentityLost
        | ObserveApply::AuthorityAlreadyStale
        | ObserveApply::Dropped => WatchDisposition::Forget,
        ObserveApply::Refreshed
        | ObserveApply::ProtocolInvalidated
        | ObserveApply::InvocationIgnored => WatchDisposition::Keep,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalObservationKind {
    ExactRunning,
    PhysicalEnded,
    IdentityLost,
    ProtocolInvalid,
    InvocationError,
    /// Unresolved physical uncertainty. Not identity loss.
    Unresolved,
}

pub fn classify_execution_observation(
    observation: &ExecutionObservation,
) -> PhysicalObservationKind {
    if observation.state == ExecutionState::Running
        && (observation.terminal_confirmed || observation.quiescent_confirmed)
    {
        return PhysicalObservationKind::ProtocolInvalid;
    }
    if observation.state == ExecutionState::Running {
        return PhysicalObservationKind::ExactRunning;
    }
    if observation.state == ExecutionState::Terminated && !observation.terminal_confirmed {
        return PhysicalObservationKind::PhysicalEnded;
    }
    if observation.state == ExecutionState::Lost {
        return PhysicalObservationKind::IdentityLost;
    }
    if observation.state == ExecutionState::Unknown {
        return PhysicalObservationKind::Unresolved;
    }
    if matches!(
        observation.state,
        ExecutionState::Succeeded | ExecutionState::Failed | ExecutionState::Starting
    ) {
        return PhysicalObservationKind::ProtocolInvalid;
    }
    PhysicalObservationKind::Unresolved
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserverError {
    Fatal(Error),
    MissingAdapter { adapter_kind: String },
}

impl fmt::Display for ObserverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fatal(err) => write!(f, "physical observer fatal: {err}"),
            Self::MissingAdapter { adapter_kind } => write!(
                f,
                "supervised execution missing installed adapter '{adapter_kind}'"
            ),
        }
    }
}

impl std::error::Error for ObserverError {}

impl From<Error> for ObserverError {
    fn from(err: Error) -> Self {
        Self::Fatal(err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserveApply {
    Refreshed,
    PhysicalEnded,
    IdentityLost,
    ProtocolInvalidated,
    InvocationIgnored,
    AuthorityAlreadyStale,
    Dropped,
}

/// Deterministic physical observer. Adapter I/O never runs on the heartbeat
/// thread. Candidates come only from the process-local supervision registry.
pub struct PhysicalObserverService {
    kernel: Arc<Kernel>,
    adapters: AdapterRegistry,
    supervision: SupervisionService,
    config: PhysicalObserverConfig,
    watch: Mutex<HashMap<ExecutionId, WatchState>>,
}

impl PhysicalObserverService {
    pub fn new(
        kernel: Arc<Kernel>,
        adapters: AdapterRegistry,
        supervision: SupervisionService,
        config: PhysicalObserverConfig,
    ) -> Self {
        supervision.enable_freshness_gate(config.freshness_limit);
        Self {
            kernel,
            adapters,
            supervision,
            config,
            watch: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> PhysicalObserverConfig {
        self.config
    }

    pub fn observe_due_now(&self) -> Result<Vec<ObserveApply>, ObserverError> {
        self.observe_due(self.kernel.now())
    }

    fn supervision_wake_target(&self) -> &SupervisionService {
        &self.supervision
    }

    /// Wall time until the earliest scheduled observation. Zero means due now.
    fn wait_for_next_due(&self) -> Duration {
        let now = self.kernel.now();
        let earliest = self
            .watch
            .lock()
            .expect("physical watch lock")
            .values()
            .map(|state| state.next_observation_at)
            .fold(None, |best: Option<UnixTime>, due| {
                Some(best.map_or(due, |prev| prev.min(due)))
            });
        match earliest {
            Some(due) if due <= now => Duration::ZERO,
            Some(due) => Duration::from_secs_f64(due - now),
            None => Duration::from_secs_f64(self.config.poll_interval),
        }
    }

    pub fn observe_due(&self, now: UnixTime) -> Result<Vec<ObserveApply>, ObserverError> {
        let poll = self.config.poll_interval;
        {
            let mut watch = self.watch.lock().expect("physical watch lock");
            for snap in self.supervision.observation_snapshots() {
                watch
                    .entry(snap.identity.execution_id().clone())
                    .or_insert_with(|| WatchState {
                        ticket: ObservationTicket {
                            execution_id: snap.identity.execution_id().clone(),
                            request_id: snap.identity.request_id().clone(),
                            identity: snap.identity.clone(),
                        },
                        next_observation_at: now,
                    });
            }
        }
        let mut due: Vec<ObservationTicket> = {
            let watch = self.watch.lock().expect("physical watch lock");
            let mut items: Vec<WatchState> = watch
                .values()
                .filter(|state| now >= state.next_observation_at)
                .cloned()
                .collect();
            items.sort_by(|a, b| {
                a.next_observation_at
                    .partial_cmp(&b.next_observation_at)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        a.ticket
                            .execution_id
                            .as_str()
                            .cmp(b.ticket.execution_id.as_str())
                    })
            });
            items
                .into_iter()
                .take(self.config.batch_limit)
                .map(|state| state.ticket)
                .collect()
        };
        let mut out = Vec::with_capacity(due.len());
        for ticket in due.drain(..) {
            let applied = self.observe_ticket(&ticket, false)?;
            if watch_disposition(&applied) == WatchDisposition::Forget {
                self.watch
                    .lock()
                    .expect("physical watch lock")
                    .remove(&ticket.execution_id);
            } else {
                let completed = self.kernel.now();
                self.schedule_next(&ticket, completed + poll);
            }
            out.push(applied);
        }
        Ok(out)
    }

    fn schedule_next(&self, ticket: &ObservationTicket, next_at: UnixTime) {
        let mut watch = self.watch.lock().expect("physical watch lock");
        watch
            .entry(ticket.execution_id.clone())
            .and_modify(|state| state.next_observation_at = next_at)
            .or_insert(WatchState {
                ticket: ticket.clone(),
                next_observation_at: next_at,
            });
    }

    /// One bounded observation for every currently supervised Execution.
    /// Startup MUST NOT leave an unrefreshable current Execution behind READY.
    pub fn activation_sweep(&self) -> Result<Vec<ObserveApply>, ObserverError> {
        let tickets: Vec<ObservationTicket> = self
            .supervision
            .observation_snapshots()
            .into_iter()
            .map(|snap| ObservationTicket {
                execution_id: snap.identity.execution_id().clone(),
                request_id: snap.identity.request_id().clone(),
                identity: snap.identity,
            })
            .collect();
        let mut out = Vec::with_capacity(tickets.len());
        for ticket in tickets {
            out.push(self.observe_ticket(&ticket, true)?);
        }
        Ok(out)
    }

    fn observe_ticket(
        &self,
        ticket: &ObservationTicket,
        activation: bool,
    ) -> Result<ObserveApply, ObserverError> {
        let facts = self
            .kernel
            .execution_routing_facts(&ticket.execution_id)
            .map_err(ObserverError::from)?;
        let key = AdapterBindingKey::new(facts.adapter_binding_key)
            .map_err(|err| ObserverError::Fatal(Error::invalid_transition(err.to_string())))?;
        let adapter = self
            .adapters
            .resolve_exact(&facts.adapter_kind, &key)
            .map_err(|_| ObserverError::MissingAdapter {
                adapter_kind: facts.adapter_kind.clone(),
            })?;
        let handle = agentype_adapter_api::RuntimeHandle(facts.runtime_handle);
        match adapter.observe_execution(&handle) {
            Ok(observation) => {
                let kind = classify_execution_observation(&observation);
                if matches!(
                    kind,
                    PhysicalObservationKind::PhysicalEnded | PhysicalObservationKind::IdentityLost
                ) {
                    self.supervision.stop_renewal_eligibility(&ticket.identity);
                }
                let mut effective = handle.clone();
                let artifacts = if kind == PhysicalObservationKind::PhysicalEnded {
                    match adapter.collect_outcome(&handle) {
                        Ok(outcome) => outcome.artifact_refs,
                        Err(err) => {
                            if let Some(hint) = err.runtime_handle_hint() {
                                effective = hint.clone();
                            }
                            None
                        }
                    }
                } else {
                    None
                };
                self.apply_kind(ticket, kind, &effective, activation, None, artifacts)
            }
            Err(err) => self.apply_kind(
                ticket,
                PhysicalObservationKind::InvocationError,
                &handle,
                activation,
                Some(err),
                None,
            ),
        }
    }

    fn apply_kind(
        &self,
        ticket: &ObservationTicket,
        kind: PhysicalObservationKind,
        handle: &agentype_adapter_api::RuntimeHandle,
        activation: bool,
        invoke_err: Option<AdapterError>,
        artifacts: Option<serde_json::Value>,
    ) -> Result<ObserveApply, ObserverError> {
        let current = generation_current(&self.supervision, &ticket.identity);
        if !current
            && !matches!(
                kind,
                PhysicalObservationKind::PhysicalEnded
                    | PhysicalObservationKind::IdentityLost
                    | PhysicalObservationKind::InvocationError
            )
        {
            return Ok(ObserveApply::Dropped);
        }
        match kind {
            PhysicalObservationKind::ExactRunning => {
                self.supervision
                    .refresh_freshness(&ticket.identity, self.kernel.now());
                self.schedule_next(ticket, self.kernel.now() + self.config.poll_interval);
                Ok(ObserveApply::Refreshed)
            }
            PhysicalObservationKind::InvocationError => {
                let class = invoke_err
                    .as_ref()
                    .map(adapter_invocation_failure_class)
                    .unwrap_or(FailureClass::Unknown);
                if let Some(err) = invoke_err.as_ref() {
                    if let Some(hint) = err.runtime_handle_hint() {
                        self.kernel
                            .record_runtime_handle_hint(&ticket.execution_id, &hint.0)
                            .map_err(ObserverError::from)?;
                    }
                }
                if activation {
                    self.close_preserving(ticket, class)
                } else {
                    Ok(ObserveApply::InvocationIgnored)
                }
            }
            PhysicalObservationKind::Unresolved => {
                if activation {
                    self.close_preserving(ticket, FailureClass::Unknown)
                } else {
                    Ok(ObserveApply::InvocationIgnored)
                }
            }
            PhysicalObservationKind::ProtocolInvalid => {
                self.supervision.stop_renewal_eligibility(&ticket.identity);
                if activation {
                    self.close_preserving(ticket, FailureClass::AdapterProtocolFailure)
                } else {
                    Ok(ObserveApply::ProtocolInvalidated)
                }
            }
            PhysicalObservationKind::IdentityLost => {
                self.supervision.stop_renewal_eligibility(&ticket.identity);
                match self.kernel.record_physical_outcome(
                    &ticket.execution_id,
                    ExecutionState::Lost,
                    Some(&handle.0),
                    None,
                    Some(FailureClass::ExecutionLost),
                    false,
                    false,
                ) {
                    Ok(()) => {}
                    Err(Error::InvalidTransition(_)) => {
                        self.supervision.drop_if_current(&ticket.identity);
                        self.watch
                            .lock()
                            .expect("physical watch lock")
                            .remove(&ticket.execution_id);
                        return Ok(ObserveApply::Dropped);
                    }
                    Err(err) => return Err(ObserverError::from(err)),
                }
                self.nack_current(ticket, FailureClass::ExecutionLost)?;
                self.supervision.drop_if_current(&ticket.identity);
                Ok(ObserveApply::IdentityLost)
            }
            PhysicalObservationKind::PhysicalEnded => {
                self.supervision.stop_renewal_eligibility(&ticket.identity);
                match persist_physical_end_then_nack(
                    &self.kernel,
                    ticket.identity.attempt_id(),
                    ticket.identity.lease_epoch(),
                    &ticket.execution_id,
                    Some(&handle.0),
                    artifacts.as_ref(),
                )
                .map_err(ObserverError::from)?
                {
                    AuthorityConsequence::Applied { .. } => {
                        self.supervision.drop_if_current(&ticket.identity);
                        self.watch
                            .lock()
                            .expect("physical watch lock")
                            .remove(&ticket.execution_id);
                        Ok(ObserveApply::PhysicalEnded)
                    }
                    AuthorityConsequence::AuthorityAlreadyStale => {
                        self.supervision.drop_if_current(&ticket.identity);
                        self.watch
                            .lock()
                            .expect("physical watch lock")
                            .remove(&ticket.execution_id);
                        Ok(ObserveApply::AuthorityAlreadyStale)
                    }
                }
            }
        }
    }

    fn close_preserving(
        &self,
        ticket: &ObservationTicket,
        failure_class: FailureClass,
    ) -> Result<ObserveApply, ObserverError> {
        match self.kernel.nack_preserving_physical_history(
            ticket.identity.attempt_id(),
            ticket.identity.lease_epoch(),
            failure_class,
            Some(&ticket.execution_id),
        ) {
            Ok(_) => {
                self.supervision.drop_if_current(&ticket.identity);
                Ok(ObserveApply::ProtocolInvalidated)
            }
            Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                self.supervision.drop_if_current(&ticket.identity);
                Ok(ObserveApply::AuthorityAlreadyStale)
            }
            Err(err) => Err(ObserverError::from(err)),
        }
    }

    fn nack_current(
        &self,
        ticket: &ObservationTicket,
        failure_class: FailureClass,
    ) -> Result<(), ObserverError> {
        match self.kernel.nack(
            ticket.identity.attempt_id(),
            ticket.identity.lease_epoch(),
            failure_class,
            Some(&ticket.execution_id),
            false,
            false,
            false,
        ) {
            Ok(_) => Ok(()),
            Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => Ok(()),
            Err(err) => Err(ObserverError::from(err)),
        }
    }
}

fn generation_current(supervision: &SupervisionService, identity: &SupervisionIdentity) -> bool {
    supervision
        .observation_snapshots()
        .iter()
        .any(|snap| snap.identity.generation() == identity.generation())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ObserverRunnerPhase {
    Running,
    ShuttingDown,
    Failed,
}

struct ObserverRunnerState {
    phase: ObserverRunnerPhase,
    fatal: Option<ObserverError>,
}

/// Independent observer thread. Never shares Adapter I/O with heartbeat.
pub struct PhysicalObserverRunner {
    shared: Arc<(
        Mutex<ObserverRunnerState>,
        Arc<crate::supervision::ObserverWake>,
    )>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl PhysicalObserverRunner {
    pub fn start(
        service: PhysicalObserverService,
        fatal_gate: crate::DispatchGate,
    ) -> Result<Self, ObserverError> {
        let wake = service.supervision_wake_target().observer_wake();
        let shared = Arc::new((
            Mutex::new(ObserverRunnerState {
                phase: ObserverRunnerPhase::Running,
                fatal: None,
            }),
            wake.clone(),
        ));
        let thread_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("physical-observer".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
                    {
                        let state = thread_shared.0.lock().expect("observer runner state");
                        if state.phase != ObserverRunnerPhase::Running {
                            break;
                        }
                    }
                    if let Err(err) = service.observe_due_now() {
                        let mut state = thread_shared.0.lock().expect("observer runner state");
                        state.phase = ObserverRunnerPhase::Failed;
                        state.fatal = Some(err);
                        drop(state);
                        fatal_gate.fail();
                        break;
                    }
                    let wait = service.wait_for_next_due();
                    let state = thread_shared.0.lock().expect("observer runner state");
                    if state.phase != ObserverRunnerPhase::Running {
                        break;
                    }
                    if wait.is_zero() {
                        continue;
                    }
                    drop(state);
                    thread_shared.1.wait(wait);
                }));
                if result.is_err() {
                    let mut state = thread_shared.0.lock().expect("observer runner state");
                    state.phase = ObserverRunnerPhase::Failed;
                    state.fatal = Some(ObserverError::Fatal(Error::invariant(
                        "physical-observer thread panicked",
                    )));
                    drop(state);
                    fatal_gate.fail();
                }
            })
            .map_err(|err| {
                ObserverError::Fatal(Error::invariant(format!(
                    "failed to spawn physical-observer thread: {err}"
                )))
            })?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }

    pub fn request_stop(&self) {
        let mut state = self.shared.0.lock().expect("observer runner state");
        if state.phase == ObserverRunnerPhase::Running {
            state.phase = ObserverRunnerPhase::ShuttingDown;
        }
        self.shared.1.poke();
    }

    pub fn is_failed(&self) -> bool {
        self.shared.0.lock().expect("observer runner state").phase == ObserverRunnerPhase::Failed
    }

    pub fn take_fatal(&self) -> Option<String> {
        self.shared
            .0
            .lock()
            .expect("observer runner state")
            .fatal
            .as_ref()
            .map(ToString::to_string)
    }

    pub(crate) fn join_fatal(mut self) -> Option<String> {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.take_fatal()
    }
}

impl Drop for PhysicalObserverRunner {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deadlines::test_deadlines;
    use crate::timing::RuntimeTimingConfig;
    use crate::SupervisionAdmission;
    use agentype_adapter_api::{FakeAdapter, PhysicalExecutionOutcome, PhysicalState};
    use agentype_core::{
        AuthoritativeExecutionBinding, ExecutionId, ManualClock, TaskSpec, TaskState,
    };
    use agentype_execution_config::{FrozenExecutionSafety, FrozenPhysicalExecutionBinding};
    use agentype_storage_sqlite::Kernel;
    use serde_json::json;
    use std::sync::Arc;

    const LEASE: f64 = 10.0;

    #[test]
    fn unrepresentable_duration_is_rejected() {
        let err = PhysicalObserverConfig::new(f64::MAX, f64::MAX, 1, f64::MAX).unwrap_err();
        assert!(matches!(err, ObserverConfigError::Unrepresentable { .. }));
    }

    fn env() -> (Arc<ManualClock>, Arc<Kernel>) {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Arc::new(Kernel::open_memory(clock.clone(), LEASE, 16_384).unwrap());
        kernel
            .upsert_partition(&agentype_core::PartitionSpec::new(
                "general",
                1,
                agentype_core::Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        (clock, kernel)
    }

    fn timing() -> RuntimeTimingConfig {
        RuntimeTimingConfig::new(1.0, 2.0, LEASE).unwrap()
    }

    fn observer_cfg() -> PhysicalObserverConfig {
        PhysicalObserverConfig::new(1.0, 4.0, 8, LEASE).unwrap()
    }

    fn binding(claim: &agentype_core::Claim) -> FrozenPhysicalExecutionBinding {
        FrozenPhysicalExecutionBinding::new(
            FrozenExecutionSafety::unisolated(AuthoritativeExecutionBinding {
                attempt_id: claim.attempt_id.clone(),
                lease_epoch: claim.lease_epoch,
                execution_target: claim.execution_target.clone(),
                execution_profile: claim.execution_profile.clone(),
            }),
            "process",
            AdapterBindingKey::for_tests(),
        )
        .unwrap()
    }

    fn running(kernel: &Kernel, name: &str) -> (agentype_core::Claim, ExecutionId) {
        kernel
            .submit_batch(&[TaskSpec::new(name, json!({"o": name}))])
            .unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();
        let exec = kernel
            .create_execution(&claim, binding(&claim))
            .unwrap()
            .execution_id()
            .clone();
        kernel
            .confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, &exec, &json!({}))
            .unwrap();
        (claim, exec)
    }

    fn mint(
        claim: &agentype_core::Claim,
        exec: &ExecutionId,
        kernel: &Kernel,
    ) -> SupervisionAdmission {
        SupervisionAdmission::from_grant(
            kernel
                .confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, exec, &json!({}))
                .unwrap(),
        )
    }

    fn harness(
        kernel: Arc<Kernel>,
        fake: Arc<FakeAdapter>,
    ) -> (SupervisionService, PhysicalObserverService) {
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_kind("process", fake, test_deadlines())
            .unwrap();
        let observer = PhysicalObserverService::new(kernel, adapters, svc.clone(), observer_cfg());
        (svc, observer)
    }

    #[test]
    fn observer_config_rejects_bad_relation() {
        assert!(PhysicalObserverConfig::new(4.0, 1.0, 1, 10.0).is_err());
        assert!(PhysicalObserverConfig::new(1.0, 10.0, 1, 10.0).is_err());
        assert!(PhysicalObserverConfig::new(1.0, 4.0, 0, 10.0).is_err());
    }

    #[test]
    fn admission_seeds_freshness_and_positive_running_refreshes() {
        let (clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, observer) = harness(kernel.clone(), fake.clone());
        let (claim, exec) = running(&kernel, "obs-run");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        let seeded = svc.last_positive_observed_at(&exec).unwrap();
        fake.set_next_observe(ExecutionObservation {
            state: ExecutionState::Running,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: None,
        });
        clock.advance(1.0);
        assert_eq!(
            observer.activation_sweep().unwrap(),
            vec![ObserveApply::Refreshed]
        );
        let after = svc.last_positive_observed_at(&exec).unwrap();
        assert!(after > seeded);
        assert_eq!(svc.renewal_eligible(&exec), Some(true));
        let _ = clock;
    }

    #[test]
    fn observe_timeout_does_not_refresh_or_invent_death() {
        let (clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, observer) = harness(kernel.clone(), fake.clone());
        let (claim, exec) = running(&kernel, "obs-to");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        let seeded = svc.last_positive_observed_at(&exec).unwrap();
        fake.set_next_observe_error(AdapterError::deadline_exceeded("observe blocked"));
        clock.advance(1.0);
        assert_eq!(
            observer.observe_due(kernel.now()).unwrap(),
            vec![ObserveApply::InvocationIgnored]
        );
        assert_eq!(svc.last_positive_observed_at(&exec), Some(seeded));
        assert_eq!(
            kernel.execution(&exec).unwrap().state,
            ExecutionState::Running
        );
        assert!(kernel.result_for_task(&claim.task_id).is_err());
    }

    #[test]
    fn stale_freshness_skips_renewal_without_claiming_death() {
        let (clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, _observer) = harness(kernel.clone(), fake);
        let (claim, exec) = running(&kernel, "obs-stale");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        clock.advance(5.0);
        assert_eq!(
            svc.renew_one(&exec).unwrap(),
            crate::RenewalOutcome::FreshnessStale {
                execution_id: exec.clone()
            }
        );
        assert_eq!(
            kernel.execution(&exec).unwrap().state,
            ExecutionState::Running
        );
        assert!(svc.owns(&exec));
        assert_eq!(svc.renewal_eligible(&exec), Some(false));
        assert!(!svc.contains(&exec));
        assert_ne!(
            kernel.task(&claim.task_id).unwrap().state,
            TaskState::Completed
        );
    }

    #[test]
    fn physical_end_persists_terminated_without_result() {
        let (_clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, observer) = harness(kernel.clone(), fake.clone());
        let (claim, exec) = running(&kernel, "obs-end");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        fake.set_next_observe(ExecutionObservation {
            state: ExecutionState::Terminated,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: None,
        });
        fake.set_next_outcome(PhysicalExecutionOutcome {
            physical_state: PhysicalState::Terminated,
            exit_status: None,
            artifact_refs: Some(json!({"stdout": "out.txt"})),
            diagnostic: None,
        });
        assert_eq!(
            observer.activation_sweep().unwrap(),
            vec![ObserveApply::PhysicalEnded]
        );
        let row = kernel.execution(&exec).unwrap();
        assert_eq!(row.state, ExecutionState::Terminated);
        assert!(!row.terminal_confirmed);
        assert!(!row.quiescent_confirmed);
        assert_ne!(
            kernel.task(&claim.task_id).unwrap().state,
            TaskState::Completed
        );
        assert!(kernel.result_for_task(&claim.task_id).is_err());
        assert!(!svc.contains(&exec));
    }

    #[test]
    fn identity_lost_persists_lost_zero_proof() {
        let (_clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, observer) = harness(kernel.clone(), fake.clone());
        let (claim, exec) = running(&kernel, "obs-lost");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        fake.set_next_observe(ExecutionObservation {
            state: ExecutionState::Lost,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: None,
        });
        assert_eq!(
            observer.activation_sweep().unwrap(),
            vec![ObserveApply::IdentityLost]
        );
        let row = kernel.execution(&exec).unwrap();
        assert_eq!(row.state, ExecutionState::Lost);
        assert!(!row.terminal_confirmed);
        assert!(!row.quiescent_confirmed);
        assert!(kernel.result_for_task(&claim.task_id).is_err());
    }

    #[test]
    fn activation_invocation_error_closes_authority_without_inventing_state() {
        let (_clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, observer) = harness(kernel.clone(), fake.clone());
        let (claim, exec) = running(&kernel, "obs-act-to");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        fake.set_next_observe_error(AdapterError::deadline_exceeded("activation observe"));
        let applied = observer.activation_sweep().unwrap();
        assert_eq!(applied, vec![ObserveApply::ProtocolInvalidated]);
        assert_eq!(
            kernel.execution(&exec).unwrap().state,
            ExecutionState::Running
        );
        assert_ne!(
            kernel.task(&claim.task_id).unwrap().state,
            TaskState::Leased
        );
    }

    #[test]
    fn activation_timeout_keeps_timeout_class_for_retry() {
        let (_clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_kind("process", fake.clone(), test_deadlines())
            .unwrap();
        let observer =
            PhysicalObserverService::new(kernel.clone(), adapters, svc.clone(), observer_cfg());
        kernel
            .submit_batch(&[TaskSpec::new("act-to", json!({"o": 1})).retry(
                agentype_core::RetryPolicy {
                    max_attempts: 3,
                    retry_classes: vec![agentype_core::FailureClass::Timeout],
                    base_backoff_seconds: 1.0,
                    max_backoff_seconds: 4.0,
                },
            )])
            .unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();
        let exec = kernel
            .create_execution(&claim, binding(&claim))
            .unwrap()
            .execution_id()
            .clone();
        kernel
            .confirm_running_and_renew(&claim.attempt_id, claim.lease_epoch, &exec, &json!({}))
            .unwrap();
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        fake.set_next_observe_error(AdapterError::deadline_exceeded("activation observe"));
        observer.activation_sweep().unwrap();
        assert_eq!(
            kernel.task(&claim.task_id).unwrap().state,
            TaskState::RetryWait
        );
    }

    #[test]
    fn failed_observation_does_not_starve_a_healthy_peer() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Arc::new(Kernel::open_memory(clock.clone(), LEASE, 16_384).unwrap());
        kernel
            .upsert_partition(&agentype_core::PartitionSpec::new(
                "general",
                2,
                agentype_core::Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        let fake = Arc::new(FakeAdapter::new());
        let svc = SupervisionService::new(kernel.clone(), &timing()).unwrap();
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_kind("process", fake.clone(), test_deadlines())
            .unwrap();
        let observer = PhysicalObserverService::new(
            kernel.clone(),
            adapters,
            svc.clone(),
            PhysicalObserverConfig::new(1.0, 4.0, 1, LEASE).unwrap(),
        );
        let (c1, e1) = running(&kernel, "starve-a");
        let (c2, _e2) = running(&kernel, "starve-b");
        svc.admit(mint(&c1, &e1, &kernel)).unwrap();
        svc.admit(mint(&c2, &_e2, &kernel)).unwrap();
        clock.advance(1.0);
        fake.set_next_observe_error(AdapterError::deadline_exceeded("a blocked"));
        let first = observer.observe_due(kernel.now()).unwrap();
        assert_eq!(first.len(), 1);
        let second = observer.observe_due(kernel.now()).unwrap();
        assert_eq!(second.len(), 1);
        assert_ne!(first[0], second[0]);
        assert_eq!(fake.observe_call_count(), 2);
    }

    #[test]
    fn physical_end_via_observe_due_is_not_requeued() {
        let (clock, kernel) = env();
        let fake = Arc::new(FakeAdapter::new());
        let (svc, observer) = harness(kernel.clone(), fake.clone());
        let (claim, exec) = running(&kernel, "obs-forget");
        svc.admit(mint(&claim, &exec, &kernel)).unwrap();
        clock.advance(1.0);
        fake.set_next_observe(ExecutionObservation {
            state: ExecutionState::Terminated,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            detail: None,
        });
        let first = observer.observe_due(kernel.now()).unwrap();
        assert!(matches!(
            first.as_slice(),
            [ObserveApply::PhysicalEnded] | [ObserveApply::AuthorityAlreadyStale]
        ));
        let calls = fake.observe_call_count();
        clock.advance(5.0);
        let again = observer.observe_due(kernel.now()).unwrap();
        assert!(
            again.is_empty(),
            "ended watch must not be requeued: {again:?}"
        );
        assert_eq!(fake.observe_call_count(), calls);
    }
}
