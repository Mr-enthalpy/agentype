//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

pub use agentype_execution_config::*;

use agentype_adapter_api::ExecutionRequest;
use agentype_core::{Claim, Error, ExpireReport};
use agentype_storage_sqlite::Kernel;

/// Authoritative launch snapshot plus the runtime-assembled worker request.
///
/// The runtime façade is the single composition point between durable
/// Scheduler facts and the physical worker contract: adapters never receive
/// raw launch pieces, and the worker prompt is never conflated with the
/// durable Task label.
pub struct PreparedExecutionLaunch {
    snapshot: ExecutionLaunchSnapshot,
    request: ExecutionRequest,
}

impl PreparedExecutionLaunch {
    pub fn snapshot(&self) -> &ExecutionLaunchSnapshot {
        &self.snapshot
    }

    pub fn request(&self) -> &ExecutionRequest {
        &self.request
    }
}

/// Authoritatively prepare and record an execution launch from a Scheduler claim and resolved environment.
///
/// Ensures the execution environment safety proof is passed directly from configuration resolution
/// to the Kernel without caller tampering, then assembles the worker request whose prompt is
/// deterministically derived from the snapshot (never the Task label, never caller text).
pub fn prepare_execution_launch(
    kernel: &Kernel,
    claim: &Claim,
    environment: &ResolvedExecutionEnvironment,
) -> Result<PreparedExecutionLaunch, Error> {
    let snapshot = kernel.create_execution(claim, environment.safety())?;
    let request = ExecutionRequest::from_launch(&snapshot);
    Ok(PreparedExecutionLaunch { snapshot, request })
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
        let env_unisolated = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            &claim_unisolated.execution_target,
            &claim_unisolated.execution_profile,
        )
        .unwrap();
        assert!(!env_unisolated.attempt_isolation());

        let launch_unisolated =
            prepare_execution_launch(&kernel, &claim_unisolated, &env_unisolated).unwrap();
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
        let env_isolated = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            &claim_isolated.execution_target,
            &claim_isolated.execution_profile,
        )
        .unwrap();
        assert!(env_isolated.attempt_isolation());

        let launch_isolated =
            prepare_execution_launch(&kernel, &claim_isolated, &env_isolated).unwrap();
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
        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&creation_registry),
            &claim.execution_target,
            &claim.execution_profile,
        )
        .unwrap();
        assert!(env.attempt_isolation());
        let launch = prepare_execution_launch(&kernel, &claim, &env).unwrap();
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
        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            &claim.execution_target,
            &claim.execution_profile,
        )
        .unwrap();
        assert!(!env.attempt_isolation());
        let launch = prepare_execution_launch(&kernel, &claim, &env).unwrap();
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
        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(registry),
            &claim.execution_target,
            &claim.execution_profile,
        )
        .unwrap();
        prepare_execution_launch(kernel, &claim, &env).unwrap()
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
}
