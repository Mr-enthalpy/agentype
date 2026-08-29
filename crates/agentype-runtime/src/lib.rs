//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

pub use agentype_execution_config::*;

use agentype_adapter_api::{AdapterError, ExecutionAdapter, ExecutionRequest};
use agentype_core::{AuthoritativeExecutionBinding, Claim, Error, ExpireReport, FailureClass};
use agentype_storage_sqlite::Kernel;
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
    let request = ExecutionRequest::from_launch(&snapshot);
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
    use agentype_adapter_api::FakeAdapter;
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
}
