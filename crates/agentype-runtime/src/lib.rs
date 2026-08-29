//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

pub use agentype_execution_config::*;

use agentype_adapter_api::ExecutionRequest;
use agentype_core::{Claim, Error, ExpireReport, WorkspaceMode};
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
/// to the Kernel without caller tampering, then assembles the worker request with the
/// runtime-rendered worker prompt (never the Task label).
pub fn prepare_execution_launch(
    kernel: &Kernel,
    claim: &Claim,
    environment: &ResolvedExecutionEnvironment,
) -> Result<PreparedExecutionLaunch, Error> {
    let snapshot = kernel.create_execution(claim, environment.safety())?;
    let prompt = render_worker_prompt(&snapshot);
    let request = ExecutionRequest::from_launch(&snapshot, prompt);
    Ok(PreparedExecutionLaunch { snapshot, request })
}

/// Render the provider-neutral worker prompt from an authoritative launch snapshot.
///
/// Replicates the V0.1 worker protocol exactly (Python oracle
/// `Dispatcher._render_prompt`): one section per fact, joined by blank lines,
/// with writer recovery rules appended only for WRITE tasks. The durable Task
/// label (`task_name`) is deliberately not part of the protocol; the objective
/// is the task payload. Adapters MUST NOT compose scheduler semantics themselves.
pub fn render_worker_prompt(launch: &ExecutionLaunchSnapshot) -> String {
    let mut sections = vec![
        "LOCAL AGENT SCHEDULER TASK".to_string(),
        format!("TASK_ID\n{}", launch.task_id().as_str()),
        format!("ATTEMPT_ID\n{}", launch.attempt_id().as_str()),
        format!("LEASE_EPOCH\n{}", launch.lease_epoch()),
        format!(
            "WORKSTREAM\n{}",
            match launch.workstream_id() {
                Some(w) => w.as_str().to_string(),
                None => "none".to_string(),
            }
        ),
        format!("OBJECTIVE\n{}", python_canonical_json(launch.payload())),
        format!("ACCEPTANCE\n{}", python_canonical_json(launch.acceptance())),
        format!(
            "COMMITTED CONTINUITY\n{}",
            python_canonical_json(launch.continuity().capsule())
        ),
    ];
    if matches!(launch.workspace_mode(), WorkspaceMode::Write) {
        sections.push(
            "WRITER RECOVERY RULES\n\
             The current workspace is authoritative. Inspect assignment-scoped state and diff \
             before writing; continue idempotently; do not revert unrelated work."
                .to_string(),
        );
    }
    sections.push(
        "RETURN\nReturn the authoritative result only when acceptance is satisfied. \
         Do not claim Scheduler ACK; the Scheduler validates the current lease separately."
            .to_string(),
    );
    sections.join("\n\n")
}

/// Canonical JSON rendering matching the V0.1 oracle's
/// `json.dumps(value, ensure_ascii=False, sort_keys=True)` (sorted object
/// keys, `", "` / `": "` separators, non-ASCII kept literal).
fn python_canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(true) => "true".to_string(),
        serde_json::Value::Bool(false) => "false".to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => python_json_string(s),
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(python_canonical_json).collect();
            format!("[{}]", parts.join(", "))
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}: {}",
                        python_json_string(k),
                        python_canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn python_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
        assert!(launch.snapshot().attempt_isolation());
        let execution_id = launch.snapshot().execution_id().clone();

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
