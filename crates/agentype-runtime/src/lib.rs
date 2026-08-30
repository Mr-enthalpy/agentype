//! M5 runtime configuration boundary, M4 recovery orchestration, and the
//! M5.2 dispatch commit boundary (adapter composition + one authoritative
//! physical start per claim). Heartbeat supervision, restart reconciliation,
//! notifier delivery, and the daemon loop belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

pub use agentype_execution_config::*;

use agentype_adapter_api::{AdapterError, ExecutionAdapter, ExecutionRequest, StartObservation};
use agentype_core::{
    AttemptId, AuthoritativeExecutionBinding, Claim, Error, ExecutionId, ExecutionState,
    ExpireReport, FailureClass, LeaseEpoch, RequestId, ResultId,
};
use agentype_storage_sqlite::Kernel;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Preparation failure of the canonical launch façade.
///
/// Configuration-resolution failures are frozen at this boundary to the
/// standardized Scheduler failure class `RESOURCE_UNAVAILABLE` (spec 16 §A2:
/// the supplied registry is authoritative; there is no adapter-default
/// fallback). Kernel authority rejections remain domain errors and are
/// deliberately NOT mapped to a Task failure class — in particular, a Claim
/// whose copies disagree with the durable Attempt is an authority rejection,
/// never a configuration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPreparationError {
    /// The authoritative registry lacks the Attempt-frozen target/profile or
    /// the pair is incompatible. Standardized as `FailureClass::ResourceUnavailable`.
    Configuration(ResolutionError),
    /// Authority validation or the fenced execution-creation transaction
    /// rejected the launch (domain/authority error, e.g. stale or invalid
    /// authority, tampered Claim copies).
    Kernel(Error),
}

impl ExecutionPreparationError {
    /// Standardized Task failure class for this preparation failure, if one
    /// is defined. Configuration failures are always
    /// `FailureClass::ResourceUnavailable`; kernel authority errors are not
    /// Task execution failures and yield `None`.
    pub fn standard_failure_class(&self) -> Option<FailureClass> {
        match self {
            Self::Configuration(_) => Some(FailureClass::ResourceUnavailable),
            Self::Kernel(_) => None,
        }
    }
}

impl std::fmt::Display for ExecutionPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(e) => write!(f, "execution configuration unavailable: {e}"),
            Self::Kernel(e) => write!(f, "execution launch rejected: {e}"),
        }
    }
}

impl std::error::Error for ExecutionPreparationError {}

/// Authoritative launch snapshot, the runtime-assembled worker request, and
/// the resolved environment that minted the persisted safety proof — bound to
/// the same resolved environment (resolution followed by fenced revalidation).
#[derive(Debug)]
pub struct PreparedExecutionLaunch {
    snapshot: ExecutionLaunchSnapshot,
    request: ExecutionRequest,
    resolved_environment: ResolvedExecutionEnvironment,
}

impl PreparedExecutionLaunch {
    pub fn snapshot(&self) -> &ExecutionLaunchSnapshot {
        &self.snapshot
    }

    pub fn request(&self) -> &ExecutionRequest {
        &self.request
    }

    /// The environment that minted this launch's persisted `attempt_isolation`
    /// proof.
    ///
    /// This is the same resolved environment the safety fact was frozen from:
    /// resolution is keyed by the durable `AuthoritativeExecutionBinding` and
    /// is followed by the fenced execution-creation transaction, which
    /// revalidates authority inside SQLite. The M5.2 dispatcher MUST select
    /// the adapter binding, options, and timeouts from this instance and MUST
    /// NOT re-resolve.
    pub fn resolved_environment(&self) -> &ResolvedExecutionEnvironment {
        &self.resolved_environment
    }
}

/// Authoritatively prepare and record an execution launch from a Scheduler claim.
///
/// Authority precedence is structural, in three steps:
///
/// 1. `Kernel::resolve_execution_binding` validates the claim's authority in a
///    short transaction (attempt/lease/epoch/expiry) and derives the
///    `AuthoritativeExecutionBinding` from the frozen Attempt row — a stale
///    Claim or a Claim whose target/profile copies disagree with the Attempt
///    is rejected here, before any configuration resolution.
/// 2. `resolve_execution_environment` performs pure in-memory configuration
///    resolution keyed by the durable binding.
/// 3. `Kernel::create_execution` revalidates the lease/epoch inside the
///    execution-creation transaction (no TOCTOU correctness hole) and freezes
///    the safety fact.
///
/// The configuration-resolution key therefore always comes from durable
/// Attempt state, never from the Claim DTO, so a tampered claim cannot steer
/// resolution or masquerade as a configuration failure.
pub fn prepare_execution_launch(
    kernel: &Kernel,
    claim: &Claim,
    mode: ExecutionResolutionMode<'_>,
) -> Result<PreparedExecutionLaunch, ExecutionPreparationError> {
    let binding = kernel
        .resolve_execution_binding(claim)
        .map_err(ExecutionPreparationError::Kernel)?;
    let environment = resolve_execution_environment(mode, &binding)
        .map_err(ExecutionPreparationError::Configuration)?;
    let snapshot = kernel
        .create_execution(claim, environment.safety())
        .map_err(ExecutionPreparationError::Kernel)?;
    let request = ExecutionRequest::from_launch(&snapshot, &environment)
        .map_err(|m| ExecutionPreparationError::Kernel(Error::invalid_authority(m.detail)))?;
    Ok(PreparedExecutionLaunch {
        snapshot,
        request,
        resolved_environment: environment,
    })
}

// ---------------------------------------------------------------------------
// M5.2 — runtime adapter composition and the dispatch commit boundary
// ---------------------------------------------------------------------------

/// Error raised while registering an adapter implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterRegistryError {
    InvalidKind(String),
    DuplicateKind(String),
}

impl std::fmt::Display for AdapterRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKind(m) => write!(f, "invalid adapter kind: {m}"),
            Self::DuplicateKind(k) => write!(f, "duplicate adapter kind registration: '{k}'"),
        }
    }
}

impl std::error::Error for AdapterRegistryError {}

/// The configured adapter kind is not installed in the runtime.
///
/// Authoritative empty resolution: there is no fallback to a first-available,
/// target-named, "default", or direct-mode adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterUnavailable {
    pub adapter_kind: String,
}

impl std::fmt::Display for AdapterUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no adapter installed for configured kind '{}'",
            self.adapter_kind
        )
    }
}

impl std::error::Error for AdapterUnavailable {}

/// Registry of installed physical adapter implementations, keyed by the
/// `adapter_kind` named by `ExecutionTargetConfig`.
///
/// This represents physical implementation availability only. It is NOT
/// SpawnSource and carries no semantic scheduling authority: target
/// configuration says what execution environment is requested, and this
/// registry says which physical implementation is currently installed. An
/// explicitly empty registry is authoritative — resolution fails closed.
#[derive(Default)]
pub struct AdapterRegistry {
    adapters: HashMap<String, Arc<dyn ExecutionAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        adapter_kind: impl Into<String>,
        adapter: Arc<dyn ExecutionAdapter>,
    ) -> Result<(), AdapterRegistryError> {
        let kind = adapter_kind.into();
        if kind.trim().is_empty() {
            return Err(AdapterRegistryError::InvalidKind(
                "adapter kind cannot be empty".into(),
            ));
        }
        if self.adapters.contains_key(&kind) {
            return Err(AdapterRegistryError::DuplicateKind(kind));
        }
        self.adapters.insert(kind, adapter);
        Ok(())
    }

    /// Fail-closed lookup by the configured `adapter_kind`. No fallback.
    pub fn resolve(
        &self,
        adapter_kind: &str,
    ) -> Result<Arc<dyn ExecutionAdapter>, AdapterUnavailable> {
        self.adapters
            .get(adapter_kind)
            .cloned()
            .ok_or_else(|| AdapterUnavailable {
                adapter_kind: adapter_kind.to_string(),
            })
    }
}

/// Mechanical normalization of adapter invocation errors into the existing
/// `FailureClass` vocabulary (task §14). Vendor-specific classification
/// belongs inside adapter implementations; no provider strings are parsed
/// at the runtime or core layer.
pub fn adapter_invocation_failure_class(err: &AdapterError) -> FailureClass {
    match err {
        AdapterError::Unavailable(_) => FailureClass::ResourceUnavailable,
        AdapterError::DeadlineExceeded(_) => FailureClass::Timeout,
        AdapterError::Protocol(_) => FailureClass::AdapterProtocolFailure,
        AdapterError::Other(_) => FailureClass::StartFailure,
    }
}

/// Failure to compose or execute the dispatch path.
///
/// Error-model categories (task §31): Authority, Configuration,
/// AdapterAvailability, AdapterInvocation, Persistence. Authority and
/// persistence errors are domain/storage errors and are deliberately NOT
/// mapped to Task failure classes; configuration and missing-adapter
/// availability normalize to `RESOURCE_UNAVAILABLE` before any physical
/// start; adapter invocation errors use the normalized adapter failure
/// classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// Durable authority validation rejected the claim (stale, expired, or
    /// tampered identity). No Execution exists and no adapter is consulted.
    Authority(Error),
    /// Target/profile/compatibility resolution failed against the
    /// authoritative `ExecutionRegistry`.
    Configuration(ResolutionError),
    /// The configured adapter_kind is not installed.
    AdapterAvailability(AdapterUnavailable),
    /// The adapter start invocation itself errored. The Execution was
    /// already persisted, so the start is treated as potentially
    /// side-effecting.
    AdapterInvocation(AdapterError),
    /// A post-start authoritative persistence step failed.
    Persistence(Error),
}

impl DispatchError {
    /// Standardized Task failure class for this dispatch failure, if one is
    /// defined. Only pre-start composition failures and normalized adapter
    /// invocation failures map to a class.
    pub fn standard_failure_class(&self) -> Option<FailureClass> {
        match self {
            Self::Authority(_) | Self::Persistence(_) => None,
            Self::Configuration(_) | Self::AdapterAvailability(_) => {
                Some(FailureClass::ResourceUnavailable)
            }
            Self::AdapterInvocation(err) => Some(adapter_invocation_failure_class(err)),
        }
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authority(e) => write!(f, "dispatch authority rejected: {e}"),
            Self::Configuration(e) => write!(f, "dispatch configuration unavailable: {e}"),
            Self::AdapterAvailability(e) => write!(f, "dispatch adapter unavailable: {e}"),
            Self::AdapterInvocation(e) => write!(f, "dispatch adapter invocation failed: {e}"),
            Self::Persistence(e) => write!(f, "dispatch persistence failed: {e}"),
        }
    }
}

impl std::error::Error for DispatchError {}

/// The complete physical environment required for a dispatch start.
///
/// The single source the Dispatcher uses for physical start: the durable
/// authority binding, the resolved target/profile configuration (carrying
/// the Attempt-bound safety facts and runtime options), and the installed
/// adapter implementation resolved by `adapter_kind`.
pub struct ResolvedPhysicalExecutionEnvironment {
    binding: AuthoritativeExecutionBinding,
    environment: ResolvedExecutionEnvironment,
    adapter: Arc<dyn ExecutionAdapter>,
}

impl ResolvedPhysicalExecutionEnvironment {
    pub fn binding(&self) -> &AuthoritativeExecutionBinding {
        &self.binding
    }

    pub fn environment(&self) -> &ResolvedExecutionEnvironment {
        &self.environment
    }

    pub fn adapter(&self) -> &Arc<dyn ExecutionAdapter> {
        &self.adapter
    }

    pub fn attempt_isolation(&self) -> bool {
        self.environment.attempt_isolation()
    }
}

impl std::fmt::Debug for ResolvedPhysicalExecutionEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedPhysicalExecutionEnvironment")
            .field("binding", &self.binding)
            .field("environment", &self.environment)
            .field("adapter_kind", &self.environment.target().adapter_kind)
            .finish()
    }
}

/// Resolve the complete physical execution environment for one claim.
///
/// Composition order is the M5.2 invariant (task §4): durable authority
/// binding first, then target/profile configuration, then installed adapter
/// binding. No Execution is created and no adapter is invoked here — a real
/// `start_execution` call is only permitted once every composition
/// requirement has resolved successfully. This is the production composition
/// path: it accepts an authoritative `ExecutionRegistry` directly and cannot
/// use `DirectUnconfigured`.
pub fn resolve_physical_execution_environment(
    kernel: &Kernel,
    claim: &Claim,
    execution_registry: &ExecutionRegistry,
    adapters: &AdapterRegistry,
) -> Result<ResolvedPhysicalExecutionEnvironment, DispatchError> {
    let binding = kernel
        .resolve_execution_binding(claim)
        .map_err(DispatchError::Authority)?;
    let environment = resolve_execution_environment(
        ExecutionResolutionMode::Authoritative(execution_registry),
        &binding,
    )
    .map_err(DispatchError::Configuration)?;
    let adapter = adapters
        .resolve(environment.target().adapter_kind.as_str())
        .map_err(DispatchError::AdapterAvailability)?;
    Ok(ResolvedPhysicalExecutionEnvironment {
        binding,
        environment,
        adapter,
    })
}

/// The authority identity confirmed by the fenced RUNNING transaction.
///
/// Generated exclusively after `confirm_running_and_renew` succeeds. It
/// grants no new Scheduler authority; it carries exactly the identity the
/// fenced "Execution RUNNING + first lease renewal" transaction just
/// confirmed, so M5.3 supervision can consume it without re-deriving
/// launch authority.
#[derive(Debug, Clone, PartialEq)]
pub struct SupervisionAdmissionSeed {
    execution_id: ExecutionId,
    request_id: RequestId,
    attempt_id: AttemptId,
    lease_epoch: LeaseEpoch,
}

impl SupervisionAdmissionSeed {
    pub(crate) fn new(
        execution_id: ExecutionId,
        request_id: RequestId,
        attempt_id: AttemptId,
        lease_epoch: LeaseEpoch,
    ) -> Self {
        Self {
            execution_id,
            request_id,
            attempt_id,
            lease_epoch,
        }
    }

    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn lease_epoch(&self) -> LeaseEpoch {
        self.lease_epoch
    }
}

/// Immediate outcome of one dispatch attempt (task §25 vocabulary).
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchOneOutcome {
    /// Nothing was claimable; no state was touched.
    NoWork,
    /// Durable authority rejected the claim (stale, expired, or tampered
    /// identity). No Execution exists, no adapter was queried, nothing was
    /// mutated.
    AuthorityRejected,
    /// Target/profile configuration or installed-adapter resolution failed
    /// before physical start. Mechanically NACKed as RESOURCE_UNAVAILABLE
    /// through the existing scheduler primitive; no Execution is fabricated
    /// and no writer ambiguity is created (task §15).
    ConfigurationUnavailable { detail: String },
    /// The physical start reported RUNNING and the fenced RUNNING
    /// confirmation + first lease renewal succeeded. Supervision admission
    /// itself is M5.3 (task §13).
    StartedRunning { admission: SupervisionAdmissionSeed },
    /// The start is potentially side-effecting but unresolved (ambiguous
    /// observation, UNKNOWN/STARTING state, or stale authority after a
    /// RUNNING report). Ambiguous physical history is persisted; Task
    /// authority was never restored and quiescence was never claimed.
    StartAmbiguous {
        execution_id: ExecutionId,
        request_id: RequestId,
    },
    /// The start failed (terminal observation or invocation error). The
    /// existing NACK rules applied: failure row, physical history, retry
    /// policy, and writer safety.
    StartFailed {
        execution_id: ExecutionId,
        request_id: RequestId,
        failure_class: FailureClass,
    },
    /// The adapter completed synchronously and the authoritative ACK path
    /// ran. `result_id` is None only when writer safety suspended the task
    /// instead of completing it.
    CompletedSynchronously {
        execution_id: ExecutionId,
        request_id: RequestId,
        result_id: Option<ResultId>,
    },
}

/// Minimal dispatch service (task §9): take one eligible Scheduler claim and
/// either fail closed before physical execution, or make exactly one
/// authoritative physical start attempt whose request, adapter binding,
/// execution identity, safety facts, and semantics are all derived from
/// durable Scheduler authority plus authoritative runtime composition.
///
/// The Dispatcher accepts only authoritative composition objects — an
/// `ExecutionRegistry` and an `AdapterRegistry` — and therefore cannot use
/// `DirectUnconfigured` (task §8).
pub struct Dispatcher<'a> {
    kernel: &'a Kernel,
    execution_registry: &'a ExecutionRegistry,
    adapters: &'a AdapterRegistry,
}

impl<'a> Dispatcher<'a> {
    pub fn new(
        kernel: &'a Kernel,
        execution_registry: &'a ExecutionRegistry,
        adapters: &'a AdapterRegistry,
    ) -> Self {
        Self {
            kernel,
            execution_registry,
            adapters,
        }
    }

    /// Obtain one eligible claim and dispatch it.
    pub fn dispatch_one(&self) -> Result<DispatchOneOutcome, DispatchError> {
        let claim = match self
            .kernel
            .claim_next_available()
            .map_err(DispatchError::Persistence)?
        {
            Some(claim) => claim,
            None => return Ok(DispatchOneOutcome::NoWork),
        };
        self.dispatch_claim(&claim)
    }

    /// Dispatch an explicit claim. `claim` is used ONLY as the authority
    /// receipt entering the authoritative launch path (task §10): every
    /// physical request field is derived from the durable launch snapshot,
    /// never from the claim's semantic copies, and identity mismatches are
    /// rejected by the Kernel's authority validation.
    pub fn dispatch_claim(&self, claim: &Claim) -> Result<DispatchOneOutcome, DispatchError> {
        // Composition (task §4): authority, then target/profile
        // configuration, then installed adapter. Nothing exists and no
        // adapter is consulted until all three resolve.
        let physical = match resolve_physical_execution_environment(
            self.kernel,
            claim,
            self.execution_registry,
            self.adapters,
        ) {
            Ok(physical) => physical,
            Err(DispatchError::Authority(_)) => {
                return Ok(DispatchOneOutcome::AuthorityRejected);
            }
            Err(err @ DispatchError::Configuration(_))
            | Err(err @ DispatchError::AdapterAvailability(_)) => {
                // Pre-start composition failure (task §15):
                // RESOURCE_UNAVAILABLE, mechanically NACKed through the
                // existing scheduler primitive. No Execution is
                // fabricated, so no writer ambiguity can arise.
                let detail = err.to_string();
                self.nack_configuration_unavailable(claim)?;
                return Ok(DispatchOneOutcome::ConfigurationUnavailable { detail });
            }
            Err(other) => return Err(other),
        };

        // Create and freeze the Execution (STARTING) in its own fenced
        // transaction. From here the start is treated as potentially
        // side-effecting (task §11): the stable RequestId is persisted with
        // the Execution and is never regenerated.
        let snapshot = self
            .kernel
            .create_execution(claim, physical.environment().safety())
            .map_err(|err| match err {
                Error::StorageFailure(_) => DispatchError::Persistence(err),
                authority => DispatchError::Authority(authority),
            })?;
        let execution_id = snapshot.execution_id().clone();
        let request_id = snapshot.request_id().clone();
        let request = ExecutionRequest::from_launch(&snapshot, physical.environment())
            .map_err(|m| DispatchError::Authority(Error::invalid_authority(m.detail)))?;

        // Physical start — exactly once (task §16), outside any SQLite
        // transaction (task §26).
        let observation = match physical.adapter().start_execution(&request) {
            Ok(observation) => observation,
            Err(err) => {
                // Invocation error ≠ absence of execution: the start may
                // have had side effects. Nonterminal NACK (never quiescent)
                // keeps writer ambiguity intact for WRITE tasks (task §28).
                // No observation exists, so no handle can be preserved.
                let failure_class = adapter_invocation_failure_class(&err);
                self.persist_unresolved_physical_then_nack(
                    claim,
                    &execution_id,
                    failure_class,
                    None,
                )?;
                return Ok(DispatchOneOutcome::StartFailed {
                    execution_id,
                    request_id,
                    failure_class,
                });
            }
        };

        self.commit_start_observation(
            claim,
            physical.adapter(),
            &execution_id,
            &request_id,
            observation,
        )
    }

    /// Classify and durably persist the immediate start observation using
    /// the existing fenced scheduler primitives (task §12). No heartbeat, no
    /// supervision admission (M5.3), no reconciliation loop (M5.4).
    fn commit_start_observation(
        &self,
        claim: &Claim,
        adapter: &Arc<dyn ExecutionAdapter>,
        execution_id: &ExecutionId,
        request_id: &RequestId,
        observation: StartObservation,
    ) -> Result<DispatchOneOutcome, DispatchError> {
        // RUNNING: fenced confirmation + first lease renewal must succeed
        // atomically before any supervision admission (M4 invariant).
        if observation.state == ExecutionState::Running && !observation.ambiguous {
            return match self.kernel.confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                execution_id,
                &observation.runtime_handle.0,
            ) {
                Ok(_) => Ok(DispatchOneOutcome::StartedRunning {
                    admission: SupervisionAdmissionSeed::new(
                        execution_id.clone(),
                        request_id.clone(),
                        claim.attempt_id.clone(),
                        claim.lease_epoch,
                    ),
                }),
                Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                    // Task §27: authority became stale between Execution
                    // creation and the start result. Never restore Task
                    // authority and never admit supervision; persist physical
                    // history only where legal (the observed handle is kept
                    // for M5.4 reconcile_start).
                    self.kernel
                        .record_physical_outcome(
                            execution_id,
                            ExecutionState::Unknown,
                            Some(&observation.runtime_handle.0),
                            None,
                            None,
                            false,
                            false,
                        )
                        .map_err(DispatchError::Persistence)?;
                    Ok(DispatchOneOutcome::StartAmbiguous {
                        execution_id: execution_id.clone(),
                        request_id: request_id.clone(),
                    })
                }
                Err(err) => Err(DispatchError::Persistence(err)),
            };
        }

        // Ambiguous / unresolved start: potentially side-effecting, never
        // quiescent, never blindly restarted. The observed physical history
        // (with the handle) is persisted, then the mechanical nonterminal
        // NACK lets the existing writer-safety rules decide between policy
        // retry and WRITER_QUIESCENCE_UNKNOWN suspension (task §28).
        if observation.ambiguous
            || matches!(
                observation.state,
                ExecutionState::Unknown | ExecutionState::Starting
            )
        {
            self.persist_unresolved_physical_then_nack(
                claim,
                execution_id,
                FailureClass::ExecutionLost,
                Some(&observation.runtime_handle.0),
            )?;
            return Ok(DispatchOneOutcome::StartAmbiguous {
                execution_id: execution_id.clone(),
                request_id: request_id.clone(),
            });
        }

        // Terminal observation (success OR failure): the collected outcome is
        // authoritative for ACK/NACK proof (spec 07) and must be re-classified
        // before any authoritative mutation. The start observation's own
        // terminal/quiescence claims are never trusted on their own — a start
        // that claims FAILED+quiescent while the physical execution may still
        // be RUNNING must not unlock a WRITE replacement, and a start that
        // claims SUCCEEDED may collect a failure, an unresolved state, or an
        // internally contradictory result.
        if observation.terminal_confirmed {
            let outcome = match adapter.collect_outcome(&observation.runtime_handle) {
                Ok(outcome) => outcome,
                Err(err) => {
                    // Collection failed after a terminal claim: the physical
                    // reality is unresolved, so the start's terminal/quiescence
                    // claims are NOT inherited. Persist unresolved history with
                    // the observed handle, then a nonterminal NACK — the
                    // Execution does not stay in STARTING with the handle
                    // missing from durable history.
                    let failure_class = adapter_invocation_failure_class(&err);
                    self.persist_unresolved_physical_then_nack(
                        claim,
                        execution_id,
                        failure_class,
                        Some(&observation.runtime_handle.0),
                    )?;
                    return Ok(DispatchOneOutcome::StartFailed {
                        execution_id: execution_id.clone(),
                        request_id: request_id.clone(),
                        failure_class,
                    });
                }
            };
            return self.commit_collected_outcome(
                claim,
                execution_id,
                request_id,
                &observation.runtime_handle.0,
                outcome,
            );
        }

        // Any other observation shape is unresolved by definition.
        self.persist_unresolved_physical_then_nack(
            claim,
            execution_id,
            FailureClass::ExecutionLost,
            Some(&observation.runtime_handle.0),
        )?;
        Ok(DispatchOneOutcome::StartAmbiguous {
            execution_id: execution_id.clone(),
            request_id: request_id.clone(),
        })
    }

    /// Authoritative classification of the collected outcome (spec 07:
    /// `collect_outcome` is authoritative for ACK/NACK proof). Only
    /// `SUCCEEDED` with terminal proof can ACK; nonterminal collections
    /// inherit zero terminal/quiescence proof; internally contradictory
    /// results are never ACKed as success.
    fn commit_collected_outcome(
        &self,
        claim: &Claim,
        execution_id: &ExecutionId,
        request_id: &RequestId,
        observed_handle: &Value,
        outcome: agentype_adapter_api::ExecutionOutcome,
    ) -> Result<DispatchOneOutcome, DispatchError> {
        // Internally contradictory: an ACTIVE physical state cannot carry
        // terminal proof. STARTING/RUNNING/UNKNOWN reported with
        // terminal/quiescence bits would otherwise inject a quiescence proof
        // through the failure path (durable_quiescent = terminal &&
        // quiescent) and unlock a WRITE replacement writer while the
        // physical execution is still active. Fail closed: unresolved
        // physical state, zero inherited proof, observed handle preserved.
        if outcome.terminal_confirmed && outcome.state.is_active_physical() {
            let failure_class = FailureClass::AdapterProtocolFailure;
            self.persist_unresolved_physical_then_nack(
                claim,
                execution_id,
                failure_class,
                Some(observed_handle),
            )?;
            return Ok(DispatchOneOutcome::StartFailed {
                execution_id: execution_id.clone(),
                request_id: request_id.clone(),
                failure_class,
            });
        }
        // Internally contradictory: success claimed without terminal proof.
        if outcome.state == ExecutionState::Succeeded && !outcome.terminal_confirmed {
            let failure_class = FailureClass::InvalidResult;
            self.persist_unresolved_physical_then_nack(
                claim,
                execution_id,
                failure_class,
                Some(observed_handle),
            )?;
            return Ok(DispatchOneOutcome::StartFailed {
                execution_id: execution_id.clone(),
                request_id: request_id.clone(),
                failure_class,
            });
        }
        // Internally contradictory: quiescence claimed without terminality.
        if outcome.quiescent_confirmed && !outcome.terminal_confirmed {
            let failure_class = FailureClass::AdapterProtocolFailure;
            self.persist_unresolved_physical_then_nack(
                claim,
                execution_id,
                failure_class,
                Some(observed_handle),
            )?;
            return Ok(DispatchOneOutcome::StartFailed {
                execution_id: execution_id.clone(),
                request_id: request_id.clone(),
                failure_class,
            });
        }

        if outcome.terminal_confirmed {
            if outcome.state == ExecutionState::Succeeded {
                // Authoritative success: the only path to ACK.
                let payload = outcome.payload.clone().unwrap_or(Value::Null);
                let result_id = match self.kernel.ack_success(
                    &claim.attempt_id,
                    claim.lease_epoch,
                    Some(execution_id),
                    &payload,
                    outcome.summary.as_deref(),
                    outcome.quiescent_confirmed,
                    outcome.incarnation_reusable,
                ) {
                    Ok(result_id) => result_id,
                    Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                        // §27: authority expired between the start and the
                        // authoritative ACK — persist physical history only
                        // (the collected success and its handle survive),
                        // never a Task-authority mutation.
                        self.kernel
                            .record_physical_outcome(
                                execution_id,
                                ExecutionState::Succeeded,
                                Some(observed_handle),
                                outcome.payload.as_ref(),
                                None,
                                true,
                                outcome.quiescent_confirmed,
                            )
                            .map_err(DispatchError::Persistence)?;
                        return Ok(DispatchOneOutcome::StartAmbiguous {
                            execution_id: execution_id.clone(),
                            request_id: request_id.clone(),
                        });
                    }
                    Err(err) => return Err(DispatchError::Persistence(err)),
                };
                return Ok(DispatchOneOutcome::CompletedSynchronously {
                    execution_id: execution_id.clone(),
                    request_id: request_id.clone(),
                    result_id,
                });
            }
            // Authoritative terminal failure.
            let failure_class = outcome.failure_class.unwrap_or(FailureClass::StartFailure);
            self.nack_start(
                claim,
                execution_id,
                failure_class,
                true,
                outcome.quiescent_confirmed,
                Some(observed_handle),
            )?;
            self.retain_terminal_handle(
                execution_id,
                observed_handle,
                outcome.quiescent_confirmed,
            )?;
            return Ok(DispatchOneOutcome::StartFailed {
                execution_id: execution_id.clone(),
                request_id: request_id.clone(),
                failure_class,
            });
        }

        // Nonterminal / unresolved collection: NO ACK, zero inherited
        // terminal/quiescence proof — the physical state stays unresolved.
        let failure_class = outcome.failure_class.unwrap_or(FailureClass::ExecutionLost);
        self.persist_unresolved_physical_then_nack(
            claim,
            execution_id,
            failure_class,
            Some(observed_handle),
        )?;
        Ok(DispatchOneOutcome::StartAmbiguous {
            execution_id: execution_id.clone(),
            request_id: request_id.clone(),
        })
    }

    /// Persist an unresolved physical observation and then run the mechanical
    /// nonterminal NACK.
    ///
    /// EVERY unresolved path (ambiguous observation, nonterminal or
    /// contradictory collected outcome, unusual observation shapes) MUST go
    /// through this helper: `Kernel::nack` itself does not write
    /// `runtime_handle_json`, so an observed adapter handle would be lost
    /// from durable history exactly when the scheduler considers the physical
    /// reality unresolved — the state M5.4 reconciliation depends on. The
    /// handle is recorded first (UNKNOWN with zero proof bits), then the NACK
    /// applies the scheduler's mechanical consequence.
    fn persist_unresolved_physical_then_nack(
        &self,
        claim: &Claim,
        execution_id: &ExecutionId,
        failure_class: FailureClass,
        observed_handle: Option<&Value>,
    ) -> Result<(), DispatchError> {
        if let Some(handle) = observed_handle {
            self.kernel
                .record_physical_outcome(
                    execution_id,
                    ExecutionState::Unknown,
                    Some(handle),
                    None,
                    Some(failure_class),
                    false,
                    false,
                )
                .map_err(DispatchError::Persistence)?;
        }
        self.nack_start(
            claim,
            execution_id,
            failure_class,
            false,
            false,
            observed_handle,
        )
    }

    /// Design choice (audit round 5): a terminal-failure NACK does not write
    /// the runtime handle (`Kernel::nack` has no handle parameter), but a
    /// terminal failure WITHOUT quiescence proof leaves physical
    /// cleanup/reconciliation to M5.4, so the observed handle is retained
    /// explicitly. A quiescence-proven terminal execution needs no physical
    /// cleanup, so its handle is deliberately not retained — the distinction
    /// is by design, not an accident of the Kernel parameter list.
    fn retain_terminal_handle(
        &self,
        execution_id: &ExecutionId,
        observed_handle: &Value,
        quiescent_confirmed: bool,
    ) -> Result<(), DispatchError> {
        if quiescent_confirmed {
            return Ok(());
        }
        self.kernel
            .record_physical_outcome(
                execution_id,
                ExecutionState::Failed,
                Some(observed_handle),
                None,
                None,
                true,
                false,
            )
            .map_err(DispatchError::Persistence)
    }

    /// Mechanical NACK through the existing scheduler semantics. On stale
    /// authority (task §27) only legal physical history is recorded — never
    /// a Task-authority mutation — and the observed runtime handle is kept
    /// whenever the adapter produced one.
    fn nack_start(
        &self,
        claim: &Claim,
        execution_id: &ExecutionId,
        failure_class: FailureClass,
        terminal_confirmed: bool,
        quiescent_confirmed: bool,
        observed_handle: Option<&Value>,
    ) -> Result<(), DispatchError> {
        match self.kernel.nack(
            &claim.attempt_id,
            claim.lease_epoch,
            failure_class,
            Some(execution_id),
            terminal_confirmed,
            quiescent_confirmed,
            false,
        ) {
            Ok(_) => Ok(()),
            Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => self
                .kernel
                .record_physical_outcome(
                    execution_id,
                    if terminal_confirmed {
                        ExecutionState::Failed
                    } else {
                        ExecutionState::Unknown
                    },
                    observed_handle,
                    None,
                    Some(failure_class),
                    terminal_confirmed,
                    quiescent_confirmed && terminal_confirmed,
                )
                .map_err(DispatchError::Persistence),
            Err(err) => Err(DispatchError::Persistence(err)),
        }
    }

    fn nack_configuration_unavailable(&self, claim: &Claim) -> Result<(), DispatchError> {
        match self.kernel.report_configuration_unavailable(
            &claim.attempt_id,
            claim.lease_epoch,
            "runtime composition unavailable (target/profile/adapter)",
        ) {
            Ok(_) => Ok(()),
            // Authority already expired: nothing to NACK; recovery cleans up.
            Err(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => Ok(()),
            Err(err) => Err(DispatchError::Persistence(err)),
        }
    }
}

/// Restart authority barrier. Dispatch MUST NOT run until this returns.
///
/// Order (spec 14):
/// 1. expire/revoke overdue authority and claims with no Execution
/// 2. promote eligible retry waits
/// 3. reconcile pool / revive eligible non-RETIRED agents
///
/// Adapter physical reconcile is M5. This function is the M4 authority half.
pub fn recover_authority(kernel: &Kernel) -> Result<ExpireReport, Error> {
    kernel.recover_authority()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_adapter_api::{
        AdapterResult, ExecutionObservation, ExecutionOutcome, FakeAdapter, RuntimeHandle,
    };
    use agentype_core::{
        AttemptId, AuthoritativeExecutionBinding, Clock, FailureClass, LeaseEpoch, ManualClock,
        PartitionSpec, Retention, RetryPolicy, TaskSpec, TaskState,
    };
    use serde_json::Value;
    use std::sync::Arc;

    /// Synthetic durable binding for registry-only assertions (the attempt
    /// identity is irrelevant when only target/profile resolution is probed).
    fn synthetic_binding(target: &str, profile: &str) -> AuthoritativeExecutionBinding {
        AuthoritativeExecutionBinding {
            attempt_id: AttemptId::new(),
            lease_epoch: LeaseEpoch(1),
            execution_target: target.to_string(),
            execution_profile: profile.to_string(),
        }
    }

    #[test]
    fn recovery_does_not_dispatch() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, 16_384).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        let report = recover_authority(&kernel).unwrap();
        assert_eq!(report.retried, 0);
        assert_eq!(report.suspended, 0);
    }

    #[test]
    fn end_to_end_launch_preserves_registry_isolation_fact() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, 16_384).unwrap();

        // Configure partitions and registry
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new(
                "local-unisolated",
                "process",
                false,
            ))
            .unwrap();
        registry
            .register_target(ExecutionTargetConfig::new(
                "remote-isolated",
                "container",
                true,
            ))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();

        kernel
            .upsert_partition(&PartitionSpec::new(
                "p-unisolated",
                1,
                Retention::Resident,
                "local-unisolated",
                "default",
            ))
            .unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "p-isolated",
                1,
                Retention::Resident,
                "remote-isolated",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();

        // 1. Submit unisolated task -> launch must persist attempt_isolation = false
        kernel
            .submit_batch(
                &[TaskSpec::new("unisolated-task", Value::Null).partition("p-unisolated")],
            )
            .unwrap();
        let claim_unisolated = kernel.claim_next_available().unwrap().unwrap();
        let launch_unisolated = prepare_execution_launch(
            &kernel,
            &claim_unisolated,
            ExecutionResolutionMode::Authoritative(&registry),
        )
        .unwrap();
        assert!(!launch_unisolated.resolved_environment().attempt_isolation());
        assert!(!launch_unisolated.snapshot().attempt_isolation());
        let exec_unisolated = kernel
            .execution(launch_unisolated.snapshot().execution_id())
            .unwrap();
        assert!(!exec_unisolated.attempt_isolation);

        // 2. Submit isolated task -> launch must persist attempt_isolation = true
        kernel
            .submit_batch(&[TaskSpec::new("isolated-task", Value::Null).partition("p-isolated")])
            .unwrap();
        let claim_isolated = kernel.claim_next_available().unwrap().unwrap();
        let launch_isolated = prepare_execution_launch(
            &kernel,
            &claim_isolated,
            ExecutionResolutionMode::Authoritative(&registry),
        )
        .unwrap();
        assert!(launch_isolated.resolved_environment().attempt_isolation());
        assert!(launch_isolated.snapshot().attempt_isolation());
        let exec_isolated = kernel
            .execution(launch_isolated.snapshot().execution_id())
            .unwrap();
        assert!(exec_isolated.attempt_isolation);
    }

    fn retryable_write_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            retry_classes: vec![
                FailureClass::ExecutionLost,
                FailureClass::Timeout,
                FailureClass::TransientExternal,
            ],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        }
    }

    /// Review P2 (discriminating evidence): the persisted Execution carries
    /// attempt_isolation = true from creation-time registry configuration.
    /// After the registry is reconfigured to isolation = false, lease-expiry
    /// recovery must follow the persisted fact (safe retry -> RETRY_WAIT ->
    /// replacement Attempt), not the current registry — which, if consulted,
    /// would yield writer-quiescence-unknown SUSPENSION instead.
    #[test]
    fn recovery_follows_persisted_isolation_fact_despite_registry_reconfiguration() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();

        let mut creation_registry = ExecutionRegistry::new();
        creation_registry
            .register_target(ExecutionTargetConfig::new("remote-env", "container", true))
            .unwrap();
        creation_registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "p-isolated-writer",
                1,
                Retention::Resident,
                "remote-env",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();

        // A retryable WRITE task: recovery is allowed to re-dispatch, so the
        // persisted isolation fact is the ONLY thing deciding the branch.
        let spec = TaskSpec::new("isolated-writer-task", Value::Null)
            .partition("p-isolated-writer")
            .write()
            .retry(retryable_write_policy());
        kernel.submit_batch(&[spec]).unwrap();

        let claim = kernel.claim_next_available().unwrap().unwrap();
        let launch = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&creation_registry),
        )
        .unwrap();
        assert!(launch.resolved_environment().attempt_isolation());
        let execution_id = launch.snapshot().execution_id().clone();
        kernel
            .confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                &execution_id,
                &Value::Null,
            )
            .unwrap();
        assert!(kernel.execution(&execution_id).unwrap().attempt_isolation);

        // The registry later flips the same target to unisolated.
        let mut reconfigured_registry = ExecutionRegistry::new();
        reconfigured_registry
            .register_target(ExecutionTargetConfig::new("remote-env", "container", false))
            .unwrap();
        reconfigured_registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        assert!(
            !resolve_execution_environment(
                ExecutionResolutionMode::Authoritative(&reconfigured_registry),
                &synthetic_binding("remote-env", "default"),
            )
            .unwrap()
            .attempt_isolation(),
            "precondition: the current registry really is unisolated now"
        );

        clock.advance(25.0);
        let report = kernel.expire_leases(false).unwrap();
        assert_eq!(
            report.retried, 1,
            "recovery must take the safe-retry branch"
        );
        assert_eq!(report.suspended, 0);
        assert_eq!(
            kernel.task(&claim.task_id).unwrap().state,
            TaskState::RetryWait
        );
        let exec_after = kernel.execution(&execution_id).unwrap();
        assert!(
            exec_after.attempt_isolation,
            "persisted Execution retains its creation-time isolation fact"
        );

        // Past the retry backoff, the replacement Attempt becomes dispatchable.
        clock.advance(10.0);
        kernel.promote_retry_wait().unwrap();
        let retry_claim = kernel
            .claim_next_available()
            .unwrap()
            .expect("isolated writer must recover through a replacement Attempt");
        assert_eq!(retry_claim.attempt_number, 2);
        assert_eq!(retry_claim.lease_epoch, LeaseEpoch(2));
        assert_ne!(retry_claim.attempt_id, claim.attempt_id);
    }

    /// Control for the reconfiguration case: the identical retryable WRITE
    /// task launched under an unisolated target suspends with
    /// WRITER_QUIESCENCE_UNKNOWN. The only difference between the two tests
    /// is the persisted isolation fact, so the pair proves recovery reads
    /// frozen Execution state rather than current registry configuration.
    #[test]
    fn unisolated_writer_expiry_without_quiescence_suspends() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();

        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("remote-env", "container", false))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "p-writer",
                1,
                Retention::Resident,
                "remote-env",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();

        let spec = TaskSpec::new("unisolated-writer-task", Value::Null)
            .partition("p-writer")
            .write()
            .retry(retryable_write_policy());
        kernel.submit_batch(&[spec]).unwrap();

        let claim = kernel.claim_next_available().unwrap().unwrap();
        let launch = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&registry),
        )
        .unwrap();
        assert!(!launch.resolved_environment().attempt_isolation());
        let execution_id = launch.snapshot().execution_id().clone();
        kernel
            .confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                &execution_id,
                &Value::Null,
            )
            .unwrap();
        assert!(!kernel.execution(&execution_id).unwrap().attempt_isolation);

        clock.advance(25.0);
        let report = kernel.expire_leases(false).unwrap();
        assert_eq!(report.retried, 0);
        assert_eq!(report.suspended, 1);
        assert_eq!(
            kernel.task(&claim.task_id).unwrap().state,
            TaskState::Suspended
        );
        let esc = kernel.open_escalation_for_task(&claim.task_id).unwrap();
        assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);

        // Backoff time passing does not resurrect a suspended writer.
        clock.advance(60.0);
        kernel.promote_retry_wait().unwrap();
        assert!(kernel.claim_next_available().unwrap().is_none());
    }

    /// Kernel + "local"/"default" registry matching the default partition, for launch-path tests.
    fn prompt_env() -> (Kernel, ExecutionRegistry) {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, 16_384).unwrap();
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
        (kernel, registry)
    }

    fn launch_spec(
        kernel: &Kernel,
        registry: &ExecutionRegistry,
        spec: TaskSpec,
    ) -> PreparedExecutionLaunch {
        kernel.submit_batch(&[spec]).unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();
        prepare_execution_launch(
            kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(registry),
        )
        .unwrap()
    }

    #[test]
    fn read_only_launch_renders_v01_worker_prompt_protocol() {
        let (kernel, registry) = prompt_env();
        let mut spec = TaskSpec::new("inspect-foo", serde_json::json!({"goal": "read things"}));
        spec.acceptance = serde_json::json!({"criteria": ["diff is clean"]});
        let prepared = launch_spec(&kernel, &registry, spec);

        let expected = format!(
            "LOCAL AGENT SCHEDULER TASK\n\n\
TASK_ID\n{}\n\n\
ATTEMPT_ID\n{}\n\n\
LEASE_EPOCH\n{}\n\n\
WORKSTREAM\nnone\n\n\
OBJECTIVE\n{{\"goal\": \"read things\"}}\n\n\
ACCEPTANCE\n{{\"criteria\": [\"diff is clean\"]}}\n\n\
COMMITTED CONTINUITY\n{{}}\n\n\
RETURN\nReturn the authoritative result only when acceptance is satisfied. \
Do not claim Scheduler ACK; the Scheduler validates the current lease separately.",
            prepared.snapshot().task_id().as_str(),
            prepared.snapshot().attempt_id().as_str(),
            prepared.snapshot().lease_epoch(),
        );
        assert_eq!(prepared.request().prompt(), expected);
        assert!(
            !prepared
                .request()
                .prompt()
                .contains("WRITER RECOVERY RULES"),
            "read-only workers must not receive writer instructions"
        );
        // The durable label survives on the snapshot but is not the prompt.
        assert_eq!(prepared.snapshot().task_name(), "inspect-foo");
    }

    #[test]
    fn write_launch_prompt_appends_writer_recovery_rules() {
        let (kernel, registry) = prompt_env();
        let ws = kernel.create_workstream("ws-write", None, None).unwrap();
        let spec = TaskSpec::new(
            "write-foo",
            serde_json::json!({"goal": "edit the workspace"}),
        )
        .write()
        .workstream(ws.clone());
        let prepared = launch_spec(&kernel, &registry, spec);

        let prompt = prepared.request().prompt();
        assert!(
            prompt.contains(
                "WRITER RECOVERY RULES\n\
The current workspace is authoritative. Inspect assignment-scoped state and diff before writing; continue idempotently; do not revert unrelated work."
            ),
            "WRITE tasks must carry the V0.1 writer recovery rules"
        );
        let objective = prompt.find("OBJECTIVE\n").unwrap();
        let rules = prompt.find("WRITER RECOVERY RULES").unwrap();
        let ret = prompt.find("RETURN\n").unwrap();
        assert!(
            objective < rules && rules < ret,
            "section order must match V0.1"
        );
        assert!(
            prompt.contains(&format!("WORKSTREAM\n{}", ws.as_str())),
            "bound workstream must render its id"
        );
        assert!(!prompt.contains("WORKSTREAM\nnone"));
    }

    #[test]
    fn worker_prompt_is_derived_protocol_not_task_name() {
        let (kernel, registry) = prompt_env();
        let prepared = launch_spec(
            &kernel,
            &registry,
            TaskSpec::new(
                "implement-foo",
                serde_json::json!({"objective": "build feature foo"}),
            ),
        );

        assert_eq!(prepared.snapshot().task_name(), "implement-foo");
        let prompt = prepared.request().prompt();
        assert!(
            prompt.contains("OBJECTIVE\n{\"objective\": \"build feature foo\"}"),
            "the objective is the task payload, not the task name"
        );
        assert!(
            !prompt.contains("implement-foo"),
            "the worker prompt must never be the bare task name"
        );
    }

    /// Review P1 (round 2): the persisted attempt_isolation fact and the
    /// environment used for the physical start must be one atomically bound
    /// resolved environment. The façade resolves inside, so each launch binds
    /// the registry generation passed at launch time; a stale resolved
    /// environment cannot be replayed as launch authority (the API no longer
    /// accepts one).
    #[test]
    fn launch_binds_current_registry_state_not_a_stale_resolved_environment() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "remote-env",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();

        // Registry generation A: the target is isolated.
        let mut registry_a = ExecutionRegistry::new();
        registry_a
            .register_target(ExecutionTargetConfig::new("remote-env", "container", true))
            .unwrap();
        registry_a
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();

        let launch_a = launch_spec(
            &kernel,
            &registry_a,
            TaskSpec::new("task-a", serde_json::json!({"objective": "a"})),
        );
        assert!(launch_a.resolved_environment().attempt_isolation());
        assert!(launch_a.snapshot().attempt_isolation());
        let exec_a = launch_a.snapshot().execution_id().clone();
        kernel
            .ack_success(
                launch_a.snapshot().attempt_id(),
                launch_a.snapshot().lease_epoch(),
                Some(&exec_a),
                &serde_json::json!({"ok": true}),
                None,
                true,
                false,
            )
            .unwrap();

        // Registry generation B: same target name, now unisolated. The next
        // launch must bind generation B, not a hoarded generation-A environment.
        let mut registry_b = ExecutionRegistry::new();
        registry_b
            .register_target(ExecutionTargetConfig::new("remote-env", "container", false))
            .unwrap();
        registry_b
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();

        let launch_b = launch_spec(
            &kernel,
            &registry_b,
            TaskSpec::new("task-b", serde_json::json!({"objective": "b"})),
        );
        assert!(
            !launch_b.resolved_environment().attempt_isolation(),
            "the second launch must resolve from the current registry generation"
        );
        assert!(!launch_b.snapshot().attempt_isolation());
        assert!(
            !kernel
                .execution(launch_b.snapshot().execution_id())
                .unwrap()
                .attempt_isolation
        );
        // Generation A's persisted fact is untouched by generation B's launch.
        assert!(kernel.execution(&exec_a).unwrap().attempt_isolation);
    }

    /// Review P2 (round 2) + case 4 of the round-3 precedence set:
    /// configuration-resolution failures are frozen at the façade boundary to
    /// the standardized Task failure class RESOURCE_UNAVAILABLE (spec 16 §A2:
    /// the supplied registry is authoritative, no adapter default). The claim
    /// here is untampered and authority-current, so this is the legitimate
    /// configuration-failure path. Kernel authority errors are not Task
    /// failure classes.
    #[test]
    fn preparation_errors_standardize_configuration_failures_as_resource_unavailable() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, 16_384).unwrap();
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

        kernel
            .submit_batch(&[TaskSpec::new("prep-failure-task", Value::Null)])
            .unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();

        // Authoritative registry missing the claimed target.
        let empty = ExecutionRegistry::new();
        let err = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&empty),
        )
        .unwrap_err();
        assert!(matches!(
            &err,
            ExecutionPreparationError::Configuration(ResolutionError::TargetNotFound(_))
        ));
        assert_eq!(
            err.standard_failure_class(),
            Some(FailureClass::ResourceUnavailable)
        );

        // Missing profile is the same standardized class.
        let mut partial = ExecutionRegistry::new();
        partial
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        let err = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&partial),
        )
        .unwrap_err();
        assert!(matches!(
            &err,
            ExecutionPreparationError::Configuration(ResolutionError::ProfileNotFound(_))
        ));
        assert_eq!(
            err.standard_failure_class(),
            Some(FailureClass::ResourceUnavailable)
        );

        // Kernel authority errors are domain errors, not Task failure classes.
        let kernel_err = ExecutionPreparationError::Kernel(Error::not_found("unreachable"));
        assert_eq!(kernel_err.standard_failure_class(), None);

        // Failed preparations left the claim untouched: the same claim still
        // launches under a complete registry.
        let mut full = ExecutionRegistry::new();
        full.register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        full.register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        let prepared = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&full),
        )
        .unwrap();
        assert_eq!(prepared.snapshot().attempt_id(), &claim.attempt_id);
    }

    /// Review P1 (round 3): a Claim whose target copy disagrees with the
    /// durable Attempt is an authority rejection, never a configuration
    /// failure — even though the authoritative target itself is fully
    /// available in the registry. If resolution were keyed by the Claim DTO,
    /// this would surface as Configuration(TargetNotFound) →
    /// RESOURCE_UNAVAILABLE and an M5.2 dispatcher would mechanically
    /// retry/suspend a fully configured Task.
    #[test]
    fn tampered_claim_target_yields_authority_rejection_not_resource_unavailable() {
        let (kernel, registry) = prompt_env();
        kernel
            .submit_batch(&[TaskSpec::new("tamper-target", Value::Null)])
            .unwrap();
        let mut claim = kernel.claim_next_available().unwrap().unwrap();
        claim.execution_target = "missing-target".to_string();

        let err = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&registry),
        )
        .unwrap_err();
        assert!(matches!(
            &err,
            ExecutionPreparationError::Kernel(Error::InvalidAuthority(_))
        ));
        assert_eq!(err.standard_failure_class(), None);
    }

    /// Review P1 (round 3): same precedence for the profile copy.
    #[test]
    fn tampered_claim_profile_yields_authority_rejection_not_resource_unavailable() {
        let (kernel, registry) = prompt_env();
        kernel
            .submit_batch(&[TaskSpec::new("tamper-profile", Value::Null)])
            .unwrap();
        let mut claim = kernel.claim_next_available().unwrap().unwrap();
        claim.execution_profile = "missing-profile".to_string();

        let err = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&registry),
        )
        .unwrap_err();
        assert!(matches!(
            &err,
            ExecutionPreparationError::Kernel(Error::InvalidAuthority(_))
        ));
        assert_eq!(err.standard_failure_class(), None);
    }

    /// Review P1 (round 3): a stale/expired Claim fails authority validation
    /// BEFORE configuration resolution — even when the (empty) registry would
    /// also have failed, the error must be stale authority, not
    /// RESOURCE_UNAVAILABLE.
    #[test]
    fn stale_claim_authority_precedes_configuration_failure() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();
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
        kernel
            .submit_batch(&[TaskSpec::new("stale-claim", Value::Null)])
            .unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();

        clock.advance(25.0); // lease_seconds = 10 → authority expired
        let empty = ExecutionRegistry::new();
        let err = prepare_execution_launch(
            &kernel,
            &claim,
            ExecutionResolutionMode::Authoritative(&empty),
        )
        .unwrap_err();
        assert!(matches!(
            &err,
            ExecutionPreparationError::Kernel(Error::StaleAuthority(_))
        ));
        assert_eq!(err.standard_failure_class(), None);
    }

    /// Review P1 (round 4): the safety proof is Attempt-bound. A proof minted
    /// for attempt A (under a registry generation where the target was
    /// isolated) cannot authorize attempt B even when B freezes the identical
    /// target/profile — the Kernel rejects it on attempt/epoch identity —
    /// while the canonical façade for B under the current registry succeeds
    /// and persists the current isolation fact.
    #[test]
    fn stale_safety_proof_cannot_authorize_later_attempt_after_registry_reconfiguration() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Resident,
                "remote-env",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();

        // Registry generation A: the target is isolated. Launch attempt A.
        let mut registry_a = ExecutionRegistry::new();
        registry_a
            .register_target(ExecutionTargetConfig::new("remote-env", "container", true))
            .unwrap();
        registry_a
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();

        let launch_a = launch_spec(
            &kernel,
            &registry_a,
            TaskSpec::new("task-a", serde_json::json!({"objective": "a"})),
        );
        assert!(launch_a.snapshot().attempt_isolation());
        // Retain the resolved environment (and its Attempt-bound proof).
        let env_a = launch_a.resolved_environment().clone();
        let exec_a = launch_a.snapshot().execution_id().clone();
        kernel
            .ack_success(
                launch_a.snapshot().attempt_id(),
                launch_a.snapshot().lease_epoch(),
                Some(&exec_a),
                &serde_json::json!({"ok": true}),
                None,
                true,
                false,
            )
            .unwrap();

        // Registry generation B: same target name, now unisolated. Attempt B
        // freezes the same target/profile pair.
        let mut registry_b = ExecutionRegistry::new();
        registry_b
            .register_target(ExecutionTargetConfig::new("remote-env", "container", false))
            .unwrap();
        registry_b
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();

        kernel
            .submit_batch(&[TaskSpec::new(
                "task-b",
                serde_json::json!({"objective": "b"}),
            )])
            .unwrap();
        let claim_b = kernel.claim_next_available().unwrap().unwrap();

        // Replay attempt A's proof onto attempt B: rejected on identity, even
        // though target and profile coincide.
        let err = kernel
            .create_execution(&claim_b, env_a.safety())
            .unwrap_err();
        assert!(
            matches!(err, Error::InvalidAuthority(_)),
            "stale proof must be an authority rejection, got: {err:?}"
        );

        // The canonical façade with the current registry authorizes attempt B
        // and persists the current (unisolated) fact.
        let launch_b = prepare_execution_launch(
            &kernel,
            &claim_b,
            ExecutionResolutionMode::Authoritative(&registry_b),
        )
        .unwrap();
        assert!(!launch_b.snapshot().attempt_isolation());
        assert!(
            !kernel
                .execution(launch_b.snapshot().execution_id())
                .unwrap()
                .attempt_isolation
        );
        // Attempt A's persisted fact is untouched.
        assert!(kernel.execution(&exec_a).unwrap().attempt_isolation);
    }

    // ------------------------------------------------------------------
    // M5.2 composition: AdapterRegistry + physical environment resolution
    // ------------------------------------------------------------------

    fn compose_physical(
        kernel: &Kernel,
        registry: &ExecutionRegistry,
        adapters: &AdapterRegistry,
        spec: TaskSpec,
    ) -> (
        Claim,
        Result<ResolvedPhysicalExecutionEnvironment, DispatchError>,
    ) {
        kernel.submit_batch(&[spec]).unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();
        let composed = resolve_physical_execution_environment(kernel, &claim, registry, adapters);
        (claim, composed)
    }

    fn fake_adapters(kind: &str) -> AdapterRegistry {
        let mut adapters = AdapterRegistry::new();
        adapters
            .register(kind, Arc::new(FakeAdapter::new()))
            .unwrap();
        adapters
    }

    #[test]
    fn physical_composition_resolves_binding_config_and_adapter() {
        let (kernel, registry) = prompt_env();
        let adapters = fake_adapters("process");

        let (claim, composed) = compose_physical(
            &kernel,
            &registry,
            &adapters,
            TaskSpec::new("compose-ok", Value::Null),
        );
        let physical = composed.unwrap();
        assert_eq!(physical.binding().attempt_id, claim.attempt_id);
        assert_eq!(physical.binding().lease_epoch, claim.lease_epoch);
        assert_eq!(physical.environment().target().name, "local");
        assert_eq!(physical.environment().target().adapter_kind, "process");
        assert_eq!(physical.environment().profile().name, "default");
        assert!(!physical.attempt_isolation());
    }

    #[test]
    fn physical_composition_fails_closed_on_missing_target() {
        let (kernel, _registry) = prompt_env();
        let empty_registry = ExecutionRegistry::new();
        let adapters = fake_adapters("process");

        let (_claim, composed) = compose_physical(
            &kernel,
            &empty_registry,
            &adapters,
            TaskSpec::new("compose-missing-target", Value::Null),
        );
        assert!(matches!(
            composed.unwrap_err(),
            DispatchError::Configuration(ResolutionError::TargetNotFound(_))
        ));
    }

    #[test]
    fn physical_composition_fails_closed_on_missing_adapter_kind() {
        let (kernel, registry) = prompt_env();
        // Explicitly empty adapter registry is authoritative: no fallback.
        let adapters = AdapterRegistry::new();

        let (_claim, composed) = compose_physical(
            &kernel,
            &registry,
            &adapters,
            TaskSpec::new("compose-missing-adapter", Value::Null),
        );
        assert_eq!(
            composed.unwrap_err(),
            DispatchError::AdapterAvailability(AdapterUnavailable {
                adapter_kind: "process".to_string()
            })
        );
    }

    #[test]
    fn physical_composition_never_falls_back_to_another_installed_adapter() {
        let (kernel, registry) = prompt_env();
        // Only a different kind is installed; "process" must stay unavailable.
        let adapters = fake_adapters("other-frontend");

        let (_claim, composed) = compose_physical(
            &kernel,
            &registry,
            &adapters,
            TaskSpec::new("compose-no-fallback", Value::Null),
        );
        assert_eq!(
            composed.unwrap_err(),
            DispatchError::AdapterAvailability(AdapterUnavailable {
                adapter_kind: "process".to_string()
            })
        );
    }

    #[test]
    fn adapter_registry_registration_fails_closed() {
        let mut adapters = AdapterRegistry::new();
        assert_eq!(
            adapters
                .register("   ", Arc::new(FakeAdapter::new()))
                .unwrap_err(),
            AdapterRegistryError::InvalidKind("adapter kind cannot be empty".into())
        );
        adapters
            .register("process", Arc::new(FakeAdapter::new()))
            .unwrap();
        assert_eq!(
            adapters
                .register("process", Arc::new(FakeAdapter::new()))
                .unwrap_err(),
            AdapterRegistryError::DuplicateKind("process".into())
        );
    }

    #[test]
    fn dispatch_error_standard_failure_classes() {
        assert_eq!(
            DispatchError::Configuration(ResolutionError::TargetNotFound("t".into()))
                .standard_failure_class(),
            Some(FailureClass::ResourceUnavailable)
        );
        assert_eq!(
            DispatchError::AdapterAvailability(AdapterUnavailable {
                adapter_kind: "k".into()
            })
            .standard_failure_class(),
            Some(FailureClass::ResourceUnavailable)
        );
        assert_eq!(
            DispatchError::AdapterInvocation(AdapterError::Unavailable("u".into()))
                .standard_failure_class(),
            Some(FailureClass::ResourceUnavailable)
        );
        assert_eq!(
            DispatchError::AdapterInvocation(AdapterError::DeadlineExceeded("d".into()))
                .standard_failure_class(),
            Some(FailureClass::Timeout)
        );
        assert_eq!(
            DispatchError::AdapterInvocation(AdapterError::Protocol("p".into()))
                .standard_failure_class(),
            Some(FailureClass::AdapterProtocolFailure)
        );
        assert_eq!(
            DispatchError::AdapterInvocation(AdapterError::Other("o".into()))
                .standard_failure_class(),
            Some(FailureClass::StartFailure)
        );
        // Authority and persistence are domain/storage errors, never Task
        // failure classes.
        assert_eq!(
            DispatchError::Authority(Error::not_found("x")).standard_failure_class(),
            None
        );
        assert_eq!(
            DispatchError::Persistence(Error::not_found("x")).standard_failure_class(),
            None
        );
    }

    // ------------------------------------------------------------------
    // M5.2 dispatcher: one authoritative physical start per claim
    // ------------------------------------------------------------------

    fn dispatch_env() -> (
        Kernel,
        Arc<ManualClock>,
        ExecutionRegistry,
        AdapterRegistry,
        Arc<FakeAdapter>,
    ) {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();
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
            .register_target(
                ExecutionTargetConfig::new("local", "process", false)
                    .with_options(serde_json::json!({"endpoint": "local://dispatch"})),
            )
            .unwrap();
        registry
            .register_profile(
                ExecutionProfileConfig::new("default")
                    .with_timeout(30.0)
                    .with_options(serde_json::json!({"model_tier": "standard"})),
            )
            .unwrap();

        let fake = Arc::new(FakeAdapter::new());
        let mut adapters = AdapterRegistry::new();
        adapters.register("process", fake.clone()).unwrap();
        (kernel, clock, registry, adapters, fake)
    }

    /// Retry policy that also covers RESOURCE_UNAVAILABLE, so mechanical
    /// configuration NACKs demonstrably land in RETRY_WAIT.
    fn config_retryable_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 2,
            retry_classes: vec![
                FailureClass::ResourceUnavailable,
                FailureClass::ExecutionLost,
            ],
            base_backoff_seconds: 1.0,
            max_backoff_seconds: 8.0,
        }
    }

    fn ambiguous_start() -> StartObservation {
        StartObservation {
            state: ExecutionState::Unknown,
            runtime_handle: RuntimeHandle(serde_json::json!({"probe": 1})),
            ambiguous: true,
            failure_class: None,
            detail: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
        }
    }

    /// Test adapter that advances the shared manual clock inside
    /// start_execution, creating the real-world window in which Scheduler
    /// authority can expire between Execution creation and the start result
    /// (task §27).
    struct ClockAdvancingAdapter {
        inner: FakeAdapter,
        clock: Arc<ManualClock>,
        advance_seconds: f64,
    }

    impl ClockAdvancingAdapter {
        fn new(clock: Arc<ManualClock>, advance_seconds: f64) -> Self {
            Self {
                inner: FakeAdapter::new(),
                clock,
                advance_seconds,
            }
        }
    }

    impl ExecutionAdapter for ClockAdvancingAdapter {
        fn start_execution(&self, request: &ExecutionRequest) -> AdapterResult<StartObservation> {
            self.clock.advance(self.advance_seconds);
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
            request_id: &RequestId,
            persisted_handle: Option<&RuntimeHandle>,
        ) -> AdapterResult<StartObservation> {
            self.inner.reconcile_start(request_id, persisted_handle)
        }
    }

    #[test]
    fn dispatch_one_returns_no_work_without_claimable_tasks() {
        let (kernel, _clock, registry, adapters, _fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        assert_eq!(d.dispatch_one().unwrap(), DispatchOneOutcome::NoWork);
    }

    /// §1, §16-19, §21, §23: one eligible claim becomes exactly one
    /// authoritative physical start, with stable identity propagation and
    /// fenced RUNNING persistence.
    #[test]
    fn dispatch_one_starts_running_exactly_once() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let payload = serde_json::json!({"objective": "inspect"});
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("dispatch-run", payload.clone())])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();

        let outcome = d.dispatch_one().unwrap();
        let admission = match &outcome {
            DispatchOneOutcome::StartedRunning { admission } => admission,
            other => panic!("expected StartedRunning, got {other:?}"),
        };
        let execution_id = admission.execution_id().clone();
        let request_id = admission.request_id().clone();

        assert_eq!(fake.start_call_count(), 1);
        let last = fake.last_request().unwrap();
        assert_eq!(last.execution_id(), &execution_id);
        assert_eq!(last.request_id(), &request_id);
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.id, execution_id);
        assert_eq!(exec.task_id, task_id);
        assert_eq!(last.incarnation_id(), &exec.incarnation_id);
        // The seed carries exactly the authority identity the fenced RUNNING
        // transaction just confirmed - nothing re-derived.
        assert_eq!(admission.attempt_id(), &exec.attempt_id);
        assert_eq!(
            admission.lease_epoch(),
            kernel
                .lease_for_attempt(admission.attempt_id())
                .unwrap()
                .epoch
        );
        assert_eq!(last.payload(), &payload);
        assert_eq!(
            last.workspace_mode(),
            agentype_core::WorkspaceMode::ReadOnly
        );
        assert_eq!(exec.state, ExecutionState::Running);
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Running);
    }

    /// §2, §32: missing authoritative target fails closed before any
    /// physical start and mechanically NACKs as RESOURCE_UNAVAILABLE.
    #[test]
    fn dispatch_missing_target_fails_closed_without_physical_start() {
        let (kernel, _clock, _registry, adapters, fake) = dispatch_env();
        let empty_registry = ExecutionRegistry::new();
        let d = Dispatcher::new(&kernel, &empty_registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("cfg-target", Value::Null).retry(config_retryable_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();

        match d.dispatch_one().unwrap() {
            DispatchOneOutcome::ConfigurationUnavailable { .. } => {}
            other => panic!("expected ConfigurationUnavailable, got {other:?}"),
        }
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
    }

    /// §3: missing profile.
    #[test]
    fn dispatch_missing_profile_fails_closed_without_physical_start() {
        let (kernel, _clock, _registry, adapters, fake) = dispatch_env();
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("cfg-profile", Value::Null).retry(config_retryable_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();

        match d.dispatch_one().unwrap() {
            DispatchOneOutcome::ConfigurationUnavailable { .. } => {}
            other => panic!("expected ConfigurationUnavailable, got {other:?}"),
        }
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
    }

    /// §4: incompatible target/profile pair.
    #[test]
    fn dispatch_incompatible_pair_fails_closed_without_physical_start() {
        let (kernel, _clock, _registry, adapters, fake) = dispatch_env();
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        registry
            .register_profile(
                ExecutionProfileConfig::new("default").with_allowed_targets(["remote"]),
            )
            .unwrap();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("cfg-incompatible", Value::Null).retry(config_retryable_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();

        match d.dispatch_one().unwrap() {
            DispatchOneOutcome::ConfigurationUnavailable { .. } => {}
            other => panic!("expected ConfigurationUnavailable, got {other:?}"),
        }
        assert_eq!(fake.start_call_count(), 0);
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
    }

    /// §5, §6, §7, §31: a missing adapter_kind is authoritative, creates no
    /// fallback, and — because no Execution exists — creates no writer
    /// ambiguity for a WRITE task.
    #[test]
    fn dispatch_missing_adapter_creates_no_writer_ambiguity() {
        let (kernel, _clock, registry, _adapters, fake) = dispatch_env();
        let empty_adapters = AdapterRegistry::new();
        let d = Dispatcher::new(&kernel, &registry, &empty_adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("cfg-adapter", Value::Null)
                .write()
                .retry(config_retryable_policy())])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();

        match d.dispatch_one().unwrap() {
            DispatchOneOutcome::ConfigurationUnavailable { .. } => {}
            other => panic!("expected ConfigurationUnavailable, got {other:?}"),
        }
        assert_eq!(fake.start_call_count(), 0);
        // No Execution existed, so writer safety cannot be ambiguous: the
        // mechanical NACK takes the retry branch, not the WRITER_QUIESCENCE_
        // UNKNOWN suspension.
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
        assert!(kernel.open_escalation_for_task(&task_id).is_err());
    }

    /// §9: tampered target copy is an authority rejection before any adapter
    /// lookup — never a configuration failure.
    #[test]
    fn dispatch_tampered_claim_target_is_authority_rejected() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        kernel
            .submit_batch(&[TaskSpec::new("tamper-target", Value::Null)])
            .unwrap();
        let mut claim = kernel.claim_next_available().unwrap().unwrap();
        claim.execution_target = "missing-target".to_string();

        assert_eq!(
            d.dispatch_claim(&claim).unwrap(),
            DispatchOneOutcome::AuthorityRejected
        );
        assert_eq!(fake.start_call_count(), 0);
    }

    /// §10: tampered profile copy.
    #[test]
    fn dispatch_tampered_claim_profile_is_authority_rejected() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        kernel
            .submit_batch(&[TaskSpec::new("tamper-profile", Value::Null)])
            .unwrap();
        let mut claim = kernel.claim_next_available().unwrap().unwrap();
        claim.execution_profile = "missing-profile".to_string();

        assert_eq!(
            d.dispatch_claim(&claim).unwrap(),
            DispatchOneOutcome::AuthorityRejected
        );
        assert_eq!(fake.start_call_count(), 0);
    }

    /// §11: a stale claim is rejected before any adapter lookup.
    #[test]
    fn dispatch_stale_claim_is_authority_rejected() {
        let (kernel, clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        kernel
            .submit_batch(&[TaskSpec::new("stale-claim", Value::Null)])
            .unwrap();
        let claim = kernel.claim_next_available().unwrap().unwrap();
        clock.advance(25.0); // lease expired

        assert_eq!(
            d.dispatch_claim(&claim).unwrap(),
            DispatchOneOutcome::AuthorityRejected
        );
        assert_eq!(fake.start_call_count(), 0);
    }

    /// §12-15: mutating the Claim's semantic copies can never alter the
    /// physical request — every field comes from the durable launch snapshot.
    #[test]
    fn dispatch_claim_semantic_copies_cannot_alter_request() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let durable_payload = serde_json::json!({"objective": "durable"});
        let durable_acceptance = serde_json::json!({"criteria": "durable"});
        let mut spec = TaskSpec::new("tamper-semantics", durable_payload.clone());
        spec.acceptance = durable_acceptance.clone();
        kernel.submit_batch(&[spec]).unwrap();
        let mut claim = kernel.claim_next_available().unwrap().unwrap();

        claim.payload = serde_json::json!({"objective": "FORGED"});
        claim.acceptance = serde_json::json!({"criteria": "FORGED"});
        claim.workstream_id = Some(agentype_core::WorkstreamId::new());
        claim.batch_id = agentype_core::BatchId::new();

        let outcome = d.dispatch_claim(&claim).unwrap();
        assert!(matches!(outcome, DispatchOneOutcome::StartedRunning { .. }));
        let last = fake.last_request().unwrap();
        assert_eq!(last.payload(), &durable_payload);
        assert_eq!(last.acceptance(), &durable_acceptance);
        assert_eq!(last.workstream_id(), None);
    }

    /// §29: durable READ_ONLY authority wins over a Claim mutated to WRITE.
    #[test]
    fn dispatch_read_only_task_stays_read_only_even_if_claim_says_write() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        kernel
            .submit_batch(&[TaskSpec::new("ro-authority", Value::Null)])
            .unwrap();
        let mut claim = kernel.claim_next_available().unwrap().unwrap();
        claim.workspace_mode = agentype_core::WorkspaceMode::Write;

        let outcome = d.dispatch_claim(&claim).unwrap();
        assert!(matches!(outcome, DispatchOneOutcome::StartedRunning { .. }));
        assert_eq!(
            fake.last_request().unwrap().workspace_mode(),
            agentype_core::WorkspaceMode::ReadOnly
        );
    }

    /// §30: a WRITE launch comes only from durable WRITE Task authority.
    #[test]
    fn dispatch_write_task_requests_write_from_durable_authority() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        kernel
            .submit_batch(&[TaskSpec::new("write-authority", Value::Null).write()])
            .unwrap();

        let outcome = d.dispatch_one().unwrap();
        assert!(matches!(outcome, DispatchOneOutcome::StartedRunning { .. }));
        assert_eq!(
            fake.last_request().unwrap().workspace_mode(),
            agentype_core::WorkspaceMode::Write
        );
    }

    /// §22, §24: an ambiguous start is persisted as unresolved physical
    /// history, is never restarted through another start_execution, and the
    /// mechanical nonterminal NACK applies the retry policy.
    #[test]
    fn dispatch_ambiguous_start_is_persisted_and_never_restarted() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("ambig", Value::Null).retry(retryable_write_policy())])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(ambiguous_start());

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartAmbiguous {
                execution_id,
                request_id,
            } => {
                assert_ne!(*request_id, agentype_core::RequestId::new());
                execution_id.clone()
            }
            other => panic!("expected StartAmbiguous, got {other:?}"),
        };
        assert_eq!(fake.start_call_count(), 1);
        assert_eq!(
            kernel.execution(&execution_id).unwrap().state,
            ExecutionState::Unknown
        );
        assert_eq!(
            kernel.execution_runtime_handle(&execution_id).unwrap(),
            serde_json::json!({"probe": 1})
        );
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
        // No blind re-dispatch: the retry wait is not claimable and nothing
        // starts a second time.
        assert_eq!(d.dispatch_one().unwrap(), DispatchOneOutcome::NoWork);
        assert_eq!(fake.start_call_count(), 1);
    }

    /// §25: a terminal failure before RUNNING follows the existing NACK
    /// rules (failure row, physical history, retry policy).
    #[test]
    fn dispatch_terminal_start_failure_follows_nack_rules() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("start-fail", Value::Null).retry(retryable_write_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(Value::Null),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Failed,
            payload: None,
            summary: None,
            failure_class: Some(FailureClass::Timeout),
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed {
                execution_id,
                failure_class,
                ..
            } => {
                assert_eq!(*failure_class, FailureClass::Timeout);
                execution_id.clone()
            }
            other => panic!("expected StartFailed, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Failed);
        assert!(exec.terminal_confirmed);
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
    }

    /// §12: a synchronously completing adapter runs the authoritative ACK
    /// path and produces a durable Result.
    #[test]
    fn dispatch_synchronous_success_completes_authoritatively() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("sync-ok", Value::Null)])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Succeeded,
            runtime_handle: RuntimeHandle(serde_json::json!({"sync": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload: Some(serde_json::json!({"ok": true})),
            summary: Some("done".into()),
            failure_class: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        match &outcome {
            DispatchOneOutcome::CompletedSynchronously { result_id, .. } => {
                assert!(result_id.is_some());
            }
            other => panic!("expected CompletedSynchronously, got {other:?}"),
        }
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Completed);
        assert!(kernel.result_for_task(&task_id).is_ok());
    }

    /// Audit P1: the authoritative runtime configuration (target options,
    /// profile options, configured timeout input) crosses the composition
    /// boundary into the request the adapter actually receives.
    #[test]
    fn dispatch_request_carries_resolved_runtime_configuration() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        kernel
            .submit_batch(&[TaskSpec::new("opts", Value::Null)])
            .unwrap();

        let outcome = d.dispatch_one().unwrap();
        assert!(matches!(outcome, DispatchOneOutcome::StartedRunning { .. }));
        let last = fake.last_request().unwrap();
        assert_eq!(
            last.target_options(),
            &serde_json::json!({"endpoint": "local://dispatch"})
        );
        assert_eq!(
            last.profile_options(),
            &serde_json::json!({"model_tier": "standard"})
        );
        assert_eq!(last.profile_timeout_seconds(), Some(30.0));
    }

    /// §27: authority expiring between Execution creation and a RUNNING
    /// report must not restore Task authority — physical history only, with
    /// the observed handle preserved for M5.4 reconciliation.
    #[test]
    fn dispatch_stale_authority_after_running_never_restores_task() {
        let (kernel, clock, registry, _adapters, _fake) = dispatch_env();
        let advancing = Arc::new(ClockAdvancingAdapter::new(clock.clone(), 25.0));
        let mut adapters = AdapterRegistry::new();
        adapters.register("process", advancing).unwrap();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, _ids) = kernel
            .submit_batch(&[TaskSpec::new("stale-running", Value::Null)])
            .unwrap();

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartAmbiguous { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartAmbiguous, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Unknown);
        assert!(!exec.terminal_confirmed);
        assert!(!exec.quiescent_confirmed);
        // Task authority was NOT restored (no RUNNING task state).
        let task_id = exec.task_id.clone();
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Leased);
    }

    /// §27: authority expiring before a terminal failure report mutates
    /// physical history only — never Task/attempt state.
    #[test]
    fn dispatch_stale_authority_after_failure_records_physical_history_only() {
        let (kernel, clock, registry, _adapters, _fake) = dispatch_env();
        let advancing = Arc::new(ClockAdvancingAdapter::new(clock.clone(), 25.0));
        advancing.inner.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"stale": 7})),
            ambiguous: false,
            failure_class: Some(FailureClass::StartFailure),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: false,
        });
        advancing.inner.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Failed,
            payload: None,
            summary: None,
            failure_class: Some(FailureClass::StartFailure),
            terminal_confirmed: true,
            quiescent_confirmed: false,
            incarnation_reusable: false,
        });
        let mut adapters = AdapterRegistry::new();
        adapters.register("process", advancing).unwrap();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, _ids) = kernel
            .submit_batch(&[TaskSpec::new("stale-fail", Value::Null)])
            .unwrap();

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartFailed, got {other:?}"),
        };
        assert_eq!(
            kernel.execution(&execution_id).unwrap().state,
            ExecutionState::Failed
        );
        let task_id = kernel.execution(&execution_id).unwrap().task_id.clone();
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Leased);
    }

    /// §28: an ambiguous WRITE start is never quiescent — the mechanical
    /// nonterminal NACK suspends with WRITER_QUIESCENCE_UNKNOWN.
    #[test]
    fn dispatch_ambiguous_write_start_never_gains_quiescence() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("ambig-write", Value::Null)
                .write()
                .retry(retryable_write_policy())])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(ambiguous_start());

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartAmbiguous { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartAmbiguous, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Unknown);
        assert!(!exec.quiescent_confirmed);
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Suspended);
        let esc = kernel.open_escalation_for_task(&task_id).unwrap();
        assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
        assert_eq!(fake.start_call_count(), 1);
    }

    /// Audit P1 (authoritative collect): a start that claimed SUCCEEDED but
    /// collects a terminal failure is NACKed under the collected failure
    /// class — no durable Result can exist.
    #[test]
    fn dispatch_collect_overrides_start_success_with_failure() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("collect-fail", Value::Null).retry(retryable_write_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Succeeded,
            runtime_handle: RuntimeHandle(serde_json::json!({"sync": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Failed,
            payload: None,
            summary: None,
            failure_class: Some(FailureClass::Timeout),
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed {
                execution_id,
                failure_class,
                ..
            } => {
                assert_eq!(*failure_class, FailureClass::Timeout);
                execution_id.clone()
            }
            other => panic!("expected StartFailed, got {other:?}"),
        };
        assert_eq!(
            kernel.execution(&execution_id).unwrap().state,
            ExecutionState::Failed
        );
        assert!(kernel.result_for_task(&task_id).is_err());
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
    }

    /// Audit P1 (authoritative collect): a nonterminal collection is never
    /// ACKed and inherits zero terminal/quiescence proof — the physical
    /// state stays unresolved even though the start looked successful.
    #[test]
    fn dispatch_collect_nonterminal_never_acks_or_inherits_proof() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("collect-running", Value::Null).retry(retryable_write_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Succeeded,
            runtime_handle: RuntimeHandle(serde_json::json!({"sync": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Running,
            payload: Some(serde_json::json!({"forged": 1})),
            summary: None,
            failure_class: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartAmbiguous { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartAmbiguous, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Unknown);
        assert!(!exec.terminal_confirmed);
        assert!(!exec.quiescent_confirmed);
        assert!(kernel.result_for_task(&task_id).is_err());
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
    }

    /// Audit P1 (authoritative collect): a success collection without
    /// terminal proof is internally contradictory — INVALID_RESULT, never
    /// ACKed as success, zero inherited proof.
    #[test]
    fn dispatch_contradictory_success_collection_is_never_acked() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("collect-contradictory", Value::Null).retry(retryable_write_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Succeeded,
            runtime_handle: RuntimeHandle(serde_json::json!({"sync": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload: Some(serde_json::json!({"forged": 1})),
            summary: Some("forged".into()),
            failure_class: None,
            terminal_confirmed: false,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed {
                execution_id,
                failure_class,
                ..
            } => {
                assert_eq!(*failure_class, FailureClass::InvalidResult);
                execution_id.clone()
            }
            other => panic!("expected StartFailed, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Unknown);
        assert!(!exec.terminal_confirmed);
        assert!(!exec.quiescent_confirmed);
        assert!(kernel.result_for_task(&task_id).is_err());
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Suspended);
    }

    /// Audit P1 (authoritative collect): quiescence claimed without
    /// terminality is a protocol violation — never ACKed.
    #[test]
    fn dispatch_quiescence_without_terminal_is_protocol_failure() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[
                TaskSpec::new("collect-quiescent", Value::Null).retry(retryable_write_policy())
            ])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Succeeded,
            runtime_handle: RuntimeHandle(serde_json::json!({"sync": true})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Running,
            payload: None,
            summary: None,
            failure_class: None,
            terminal_confirmed: false,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        match &outcome {
            DispatchOneOutcome::StartFailed {
                failure_class: FailureClass::AdapterProtocolFailure,
                ..
            } => {}
            other => panic!("expected StartFailed(AdapterProtocolFailure), got {other:?}"),
        }
        assert!(kernel.result_for_task(&task_id).is_err());
        assert_eq!(fake.start_call_count(), 1);
    }

    /// Audit P1 (round 2): a terminal failure claim never bypasses the
    /// authoritative collect. A start claiming FAILED+quiescent while the
    /// physical execution may still be RUNNING collects a nonterminal
    /// outcome — the start's quiescence is not inherited, and a WRITE task
    /// suspends with WRITER_QUIESCENCE_UNKNOWN instead of unlocking a
    /// replacement writer.
    #[test]
    fn dispatch_start_failure_claim_never_bypasses_collect() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("collect-writer", Value::Null)
                .write()
                .retry(retryable_write_policy())])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"maybe": 1})),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Running,
            payload: None,
            summary: None,
            failure_class: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartAmbiguous { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartAmbiguous, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Unknown);
        assert!(!exec.terminal_confirmed);
        assert!(!exec.quiescent_confirmed);
        // The observed handle survives the unresolved path (M5.4 needs it).
        assert_eq!(
            kernel.execution_runtime_handle(&execution_id).unwrap(),
            serde_json::json!({"maybe": 1})
        );
        assert!(kernel.result_for_task(&task_id).is_err());
        // The start's quiescence claim was NOT used: no retryable WRITE
        // replacement — the writer-safety suspension stands.
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Suspended);
        let esc = kernel.open_escalation_for_task(&task_id).unwrap();
        assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
        assert_eq!(fake.start_call_count(), 1);
    }

    /// Audit P1 (round 2): the collected outcome also overrides a start that
    /// claimed terminal failure — collected success semantics (authoritative
    /// ACK) apply, not the start's failure classification.
    #[test]
    fn dispatch_collected_success_overrides_start_failure_claim() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("collect-success", Value::Null)])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"actually": 1})),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: false,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Succeeded,
            payload: Some(serde_json::json!({"ok": true})),
            summary: Some("done".into()),
            failure_class: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        match &outcome {
            DispatchOneOutcome::CompletedSynchronously { result_id, .. } => {
                assert!(result_id.is_some());
            }
            other => panic!("expected CompletedSynchronously, got {other:?}"),
        }
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Completed);
        assert!(kernel.result_for_task(&task_id).is_ok());
    }

    /// Audit P1 (round 3): an unusual nonterminal observation shape falls
    /// into the generic unresolved branch and must still keep its observed
    /// runtime handle — `Kernel::nack` does not write handles, so the
    /// unresolved history is persisted before the NACK.
    #[test]
    fn dispatch_unusual_nonterminal_observation_keeps_observed_handle() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(
                &[TaskSpec::new("odd-shape", Value::Null).retry(retryable_write_policy())],
            )
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Lost,
            runtime_handle: RuntimeHandle(serde_json::json!({"odd": 9})),
            ambiguous: false,
            failure_class: None,
            detail: None,
            terminal_confirmed: false,
            quiescent_confirmed: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartAmbiguous { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartAmbiguous, got {other:?}"),
        };
        assert_eq!(
            kernel.execution(&execution_id).unwrap().state,
            ExecutionState::Unknown
        );
        assert_eq!(
            kernel.execution_runtime_handle(&execution_id).unwrap(),
            serde_json::json!({"odd": 9})
        );
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::RetryWait);
        assert_eq!(fake.start_call_count(), 1);
    }

    /// Audit P1 (round 4): an ACTIVE physical state with terminal proof is a
    /// contradictory adapter outcome — the most dangerous shape, because
    /// durable_quiescent = terminal && quiescent would otherwise unlock a
    /// WRITE replacement writer while the execution is still RUNNING. Fail
    /// closed: UNKNOWN, zero proof, handle preserved, protocol failure.
    fn contradictory_active_outcome(state: ExecutionState) -> ExecutionOutcome {
        ExecutionOutcome {
            state,
            payload: Some(serde_json::json!({"forged": 1})),
            summary: Some("forged".into()),
            failure_class: Some(FailureClass::Timeout),
            terminal_confirmed: true,
            quiescent_confirmed: true,
            incarnation_reusable: false,
        }
    }

    #[test]
    fn dispatch_running_state_with_terminal_proof_is_protocol_failure() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("active-terminal", Value::Null)
                .write()
                .retry(retryable_write_policy())])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"live": 1})),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(contradictory_active_outcome(ExecutionState::Running));

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed {
                execution_id,
                failure_class: FailureClass::AdapterProtocolFailure,
                ..
            } => execution_id.clone(),
            other => panic!("expected StartFailed(AdapterProtocolFailure), got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Unknown);
        assert!(!exec.terminal_confirmed);
        assert!(!exec.quiescent_confirmed);
        assert_eq!(
            kernel.execution_runtime_handle(&execution_id).unwrap(),
            serde_json::json!({"live": 1})
        );
        assert!(kernel.result_for_task(&task_id).is_err());
        // The forged terminal+quiescent proof must NOT unlock a replacement
        // writer: the task suspends instead of retrying.
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Suspended);
        let esc = kernel.open_escalation_for_task(&task_id).unwrap();
        assert_eq!(esc.failure_class, FailureClass::WriterQuiescenceUnknown);
        assert_eq!(fake.start_call_count(), 1);
    }

    /// Audit P1 (round 4): STARTING + terminal proof — same contradiction.
    #[test]
    fn dispatch_starting_state_with_terminal_proof_is_protocol_failure() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("active-starting", Value::Null)])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"live": 2})),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(contradictory_active_outcome(ExecutionState::Starting));

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed {
                execution_id,
                failure_class: FailureClass::AdapterProtocolFailure,
                ..
            } => execution_id.clone(),
            other => panic!("expected StartFailed(AdapterProtocolFailure), got {other:?}"),
        };
        assert_eq!(
            kernel.execution(&execution_id).unwrap().state,
            ExecutionState::Unknown
        );
        assert!(kernel.result_for_task(&task_id).is_err());
    }

    /// Audit P1 (round 4): UNKNOWN + terminal proof — same contradiction.
    #[test]
    fn dispatch_unknown_state_with_terminal_proof_is_protocol_failure() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, ids) = kernel
            .submit_batch(&[TaskSpec::new("active-unknown", Value::Null)])
            .unwrap();
        let task_id = ids.values().next().unwrap().clone();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"live": 3})),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: true,
        });
        fake.set_next_outcome(contradictory_active_outcome(ExecutionState::Unknown));

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed {
                execution_id,
                failure_class: FailureClass::AdapterProtocolFailure,
                ..
            } => execution_id.clone(),
            other => panic!("expected StartFailed(AdapterProtocolFailure), got {other:?}"),
        };
        assert_eq!(
            kernel.execution(&execution_id).unwrap().state,
            ExecutionState::Unknown
        );
        assert!(kernel.result_for_task(&task_id).is_err());
    }

    /// Audit P2 / M5.4 hardening (round 5): a terminal failure NACK without
    /// quiescence proof retains the observed runtime handle for M5.4 physical
    /// cleanup — by explicit design, not by accident of Kernel::nack's
    /// parameter list. WRITE + terminal-without-quiescence suspends, so the
    /// durable handle is exactly what M5.4 cleanup will need.
    #[test]
    fn dispatch_terminal_failure_without_quiescence_retains_handle() {
        let (kernel, _clock, registry, adapters, fake) = dispatch_env();
        let d = Dispatcher::new(&kernel, &registry, &adapters);
        let (_batch, _ids) = kernel
            .submit_batch(&[TaskSpec::new("terminal-handle", Value::Null)
                .write()
                .retry(retryable_write_policy())])
            .unwrap();
        fake.set_next_start(StartObservation {
            state: ExecutionState::Failed,
            runtime_handle: RuntimeHandle(serde_json::json!({"cleanup": 5})),
            ambiguous: false,
            failure_class: Some(FailureClass::Timeout),
            detail: None,
            terminal_confirmed: true,
            quiescent_confirmed: false,
        });
        fake.set_next_outcome(ExecutionOutcome {
            state: ExecutionState::Failed,
            payload: None,
            summary: None,
            failure_class: Some(FailureClass::Timeout),
            terminal_confirmed: true,
            quiescent_confirmed: false,
            incarnation_reusable: false,
        });

        let outcome = d.dispatch_one().unwrap();
        let execution_id = match &outcome {
            DispatchOneOutcome::StartFailed { execution_id, .. } => execution_id.clone(),
            other => panic!("expected StartFailed, got {other:?}"),
        };
        let exec = kernel.execution(&execution_id).unwrap();
        assert_eq!(exec.state, ExecutionState::Failed);
        assert!(exec.terminal_confirmed);
        assert!(!exec.quiescent_confirmed);
        assert_eq!(
            kernel.execution_runtime_handle(&execution_id).unwrap(),
            serde_json::json!({"cleanup": 5})
        );
        // WRITE + terminal-without-quiescence suspends: the durable handle is
        // exactly what M5.4 physical cleanup will need.
        let task_id = exec.task_id.clone();
        assert_eq!(kernel.task(&task_id).unwrap().state, TaskState::Suspended);
    }
}
