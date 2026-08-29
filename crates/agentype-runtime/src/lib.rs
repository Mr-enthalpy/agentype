//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

pub use agentype_execution_config::*;

use agentype_adapter_api::ExecutionRequest;
use agentype_core::{Claim, Error, ExpireReport, FailureClass};
use agentype_storage_sqlite::Kernel;

/// Preparation failure of the canonical launch façade.
///
/// Configuration-resolution failures are frozen at this boundary to the
/// standardized Scheduler failure class `RESOURCE_UNAVAILABLE` (spec 16 §A2:
/// the supplied registry is authoritative; there is no adapter-default
/// fallback). Kernel authority rejections remain domain errors and are
/// deliberately NOT mapped to a Task failure class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPreparationError {
    /// The authoritative registry lacks the claimed target/profile or the
    /// pair is incompatible. Standardized as `FailureClass::ResourceUnavailable`.
    Configuration(ResolutionError),
    /// The fenced Kernel execution-creation transaction rejected the launch
    /// (domain/authority error, e.g. stale or invalid authority).
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
/// the resolved environment that minted the persisted safety proof — one
/// atomically bound unit.
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
    /// This is atomically the same environment the safety fact was frozen
    /// from: resolution happens inside `prepare_execution_launch`, immediately
    /// before the fenced execution-creation transaction, so a stale
    /// pre-resolved environment can never be replayed as launch authority.
    /// The M5.2 dispatcher MUST select the adapter binding, options, and
    /// timeouts from this instance and MUST NOT re-resolve.
    pub fn resolved_environment(&self) -> &ResolvedExecutionEnvironment {
        &self.resolved_environment
    }
}

/// Authoritatively prepare and record an execution launch from a Scheduler claim.
///
/// Configuration resolution happens here, immediately before the fenced
/// Kernel execution-creation transaction, so the persisted `attempt_isolation`
/// fact and the returned `resolved_environment` are bound to the same
/// authoritative registry state at the same instant. Callers pass the
/// currently-authoritative registry (or the explicit standalone resolution
/// mode); they cannot supply a pre-resolved environment, so an environment
/// resolved under an older configuration can never authorize a later attempt.
/// The Kernel still cross-validates the safety proof against the frozen
/// Attempt target/profile, so a tampered claim cannot steer resolution.
pub fn prepare_execution_launch(
    kernel: &Kernel,
    claim: &Claim,
    mode: ExecutionResolutionMode<'_>,
) -> Result<PreparedExecutionLaunch, ExecutionPreparationError> {
    let environment =
        resolve_execution_environment(mode, &claim.execution_target, &claim.execution_profile)
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
    use agentype_core::{
        Clock, FailureClass, LeaseEpoch, ManualClock, PartitionSpec, Retention, RetryPolicy,
        TaskSpec, TaskState,
    };
    use serde_json::Value;
    use std::sync::Arc;

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
                "remote-env",
                "default",
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

    /// Review P2 (round 2): configuration-resolution failures are frozen at
    /// the façade boundary to the standardized Task failure class
    /// RESOURCE_UNAVAILABLE (spec 16 §A2: the supplied registry is
    /// authoritative, no adapter default). Kernel authority errors are not
    /// Task failure classes.
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
}
