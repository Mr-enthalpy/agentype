//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

pub use agentype_execution_config::*;

use agentype_core::{Claim, Error, ExpireReport};
use agentype_storage_sqlite::Kernel;

/// Authoritatively prepare and record an execution launch from a Scheduler claim and resolved environment.
///
/// Ensures the execution environment safety proof is passed directly from configuration resolution
/// to the Kernel without caller tampering.
pub fn prepare_execution_launch(
    kernel: &Kernel,
    claim: &Claim,
    environment: &ResolvedExecutionEnvironment,
) -> Result<ExecutionLaunchSnapshot, Error> {
    kernel.create_execution(claim, environment.safety())
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
    use agentype_core::{Clock, ManualClock, PartitionSpec, Retention, TaskSpec};
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
        assert!(!launch_unisolated.attempt_isolation());
        let exec_unisolated = kernel.execution(launch_unisolated.execution_id()).unwrap();
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
        assert!(launch_isolated.attempt_isolation());
        let exec_isolated = kernel.execution(launch_isolated.execution_id()).unwrap();
        assert!(exec_isolated.attempt_isolation);
    }

    #[test]
    fn reconfigured_registry_does_not_alter_persisted_execution_safety_or_writer_recovery() {
        let clock = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock.clone(), 10.0, 16_384).unwrap();

        // 1. Initial configuration: target "remote-env" has attempt_isolation = true
        let mut initial_registry = ExecutionRegistry::new();
        initial_registry
            .register_target(ExecutionTargetConfig::new("remote-env", "container", true))
            .unwrap();
        initial_registry
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

        // 2. Submit an IsolatedWriter task and launch execution under the authoritative isolated environment
        let task_spec = TaskSpec::new("isolated-writer-task", Value::Null)
            .partition("p-isolated-writer")
            .write();
        kernel.submit_batch(&[task_spec]).unwrap();

        let claim = kernel.claim_next_available().unwrap().unwrap();
        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&initial_registry),
            &claim.execution_target,
            &claim.execution_profile,
        )
        .unwrap();
        assert!(env.attempt_isolation());

        let launch = prepare_execution_launch(&kernel, &claim, &env).unwrap();
        assert!(launch.attempt_isolation());
        let execution_id = launch.execution_id().clone();

        // Confirm execution running
        kernel
            .confirm_running_and_renew(
                &claim.attempt_id,
                claim.lease_epoch,
                &execution_id,
                &Value::Null,
            )
            .unwrap();

        let exec_before = kernel.execution(&execution_id).unwrap();
        assert!(exec_before.attempt_isolation);

        // 3. Reconfigure / mutate runtime registry: "remote-env" is now marked attempt_isolation = false
        // (or completely destroyed and replaced)
        let mut reconfigured_registry = ExecutionRegistry::new();
        reconfigured_registry
            .register_target(ExecutionTargetConfig::new("remote-env", "container", false))
            .unwrap();
        reconfigured_registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();

        let reconfigured_env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&reconfigured_registry),
            "remote-env",
            "default",
        )
        .unwrap();
        assert!(!reconfigured_env.attempt_isolation());

        // 4. Simulate lease expiry and writer crash recovery
        clock.advance(25.0);
        let report = kernel.expire_leases(false).unwrap();
        assert_eq!(report.suspended, 1);

        // 5. Verify the persisted execution record strictly preserved attempt_isolation = true
        let exec_after = kernel.execution(&execution_id).unwrap();
        assert!(
            exec_after.attempt_isolation,
            "Persisted Execution must retain its creation-time isolation fact regardless of later registry reconfigurations"
        );

        // 6. Verify writer recovery uses persisted execution isolation (fails closed vs unproven quiescence or safe replacement)
        // A nonterminal NACK for this isolated writer must NOT retry without quiescence proof,
        // and when replacing, writer safety predicates use the stored execution attempt_isolation fact.
        let claim_after_expiry = kernel.claim_next_available().unwrap();
        assert!(
            claim_after_expiry.is_none(),
            "Expired isolated writer without quiescent proof must not be blindly re-dispatched"
        );
    }
}
