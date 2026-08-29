//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

#![forbid(unsafe_code)]

use agentype_core::{Claim, Error, ExecutionLaunchSnapshot, ExpireReport, FrozenExecutionSafety};
use agentype_storage_sqlite::Kernel;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Configuration for an execution target (adapter binding + host/endpoint settings).
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionTargetConfig {
    pub name: String,
    pub adapter_kind: String,
    pub attempt_isolation: bool,
    pub options: Value,
}

impl ExecutionTargetConfig {
    pub fn new(
        name: impl Into<String>,
        adapter_kind: impl Into<String>,
        attempt_isolation: bool,
    ) -> Self {
        Self {
            name: name.into(),
            adapter_kind: adapter_kind.into(),
            attempt_isolation,
            options: Value::Null,
        }
    }

    pub fn with_options(mut self, options: Value) -> Self {
        self.options = options;
        self
    }
}

/// Configuration for an execution profile (model settings, timeouts, options, compatibility).
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionProfileConfig {
    pub name: String,
    pub timeout_seconds: Option<f64>,
    pub allowed_targets: Option<HashSet<String>>,
    pub options: Value,
}

impl ExecutionProfileConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            timeout_seconds: None,
            allowed_targets: None,
            options: Value::Null,
        }
    }

    pub fn with_timeout(mut self, timeout_seconds: f64) -> Self {
        self.timeout_seconds = Some(timeout_seconds);
        self
    }

    pub fn with_allowed_targets(
        mut self,
        targets: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.allowed_targets = Some(targets.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_options(mut self, options: Value) -> Self {
        self.options = options;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigurationError {
    DuplicateTarget(String),
    DuplicateProfile(String),
    InvalidName(String),
    InvalidTimeout(String),
}

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTarget(t) => {
                write!(f, "duplicate execution target registration: '{t}'")
            }
            Self::DuplicateProfile(p) => {
                write!(f, "duplicate execution profile registration: '{p}'")
            }
            Self::InvalidName(m) => write!(f, "invalid configuration name: {m}"),
            Self::InvalidTimeout(m) => write!(f, "invalid timeout: {m}"),
        }
    }
}

impl std::error::Error for ConfigurationError {}

/// Authoritative registry of execution targets and profiles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecutionRegistry {
    targets: HashMap<String, ExecutionTargetConfig>,
    profiles: HashMap<String, ExecutionProfileConfig>,
}

impl ExecutionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_target(
        &mut self,
        target: ExecutionTargetConfig,
    ) -> Result<(), ConfigurationError> {
        if target.name.trim().is_empty() {
            return Err(ConfigurationError::InvalidName(
                "target name cannot be empty".into(),
            ));
        }
        if self.targets.contains_key(&target.name) {
            return Err(ConfigurationError::DuplicateTarget(target.name));
        }
        self.targets.insert(target.name.clone(), target);
        Ok(())
    }

    pub fn register_profile(
        &mut self,
        profile: ExecutionProfileConfig,
    ) -> Result<(), ConfigurationError> {
        if profile.name.trim().is_empty() {
            return Err(ConfigurationError::InvalidName(
                "profile name cannot be empty".into(),
            ));
        }
        if let Some(t) = profile.timeout_seconds {
            if !t.is_finite() || t <= 0.0 {
                return Err(ConfigurationError::InvalidTimeout(format!(
                    "timeout must be positive finite seconds, got {t}"
                )));
            }
        }
        if self.profiles.contains_key(&profile.name) {
            return Err(ConfigurationError::DuplicateProfile(profile.name));
        }
        self.profiles.insert(profile.name.clone(), profile);
        Ok(())
    }

    pub fn get_target(&self, name: &str) -> Option<&ExecutionTargetConfig> {
        self.targets.get(name)
    }

    pub fn get_profile(&self, name: &str) -> Option<&ExecutionProfileConfig> {
        self.profiles.get(name)
    }
}

/// Explicit configuration resolution mode for runtime environments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExecutionResolutionMode<'a> {
    /// Authoritative mode against a registered `ExecutionRegistry`.
    /// Missing target/profile or incompatible combination strictly fails closed.
    ///
    /// This is the required resolution mode for production Daemon / Dispatcher loops.
    Authoritative(&'a ExecutionRegistry),
    /// Permissive direct-caller mode for standalone test / single-shot tool operation.
    /// Emits unisolated default parameters.
    DirectUnconfigured,
}

/// Resolved physical execution environment for an authoritative launch.
///
/// Fields are encapsulated as private and readonly to prevent post-resolution
/// tampering with the isolated safety fact.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedExecutionEnvironment {
    target: ExecutionTargetConfig,
    profile: ExecutionProfileConfig,
    attempt_isolation: bool,
}

impl ResolvedExecutionEnvironment {
    pub(crate) fn new(
        target: ExecutionTargetConfig,
        profile: ExecutionProfileConfig,
        attempt_isolation: bool,
    ) -> Self {
        Self {
            target,
            profile,
            attempt_isolation,
        }
    }

    pub fn target(&self) -> &ExecutionTargetConfig {
        &self.target
    }

    pub fn profile(&self) -> &ExecutionProfileConfig {
        &self.profile
    }

    pub fn attempt_isolation(&self) -> bool {
        self.attempt_isolation
    }

    /// Produce the strongly-typed safety proof bound to this resolved environment.
    pub fn safety(&self) -> FrozenExecutionSafety {
        FrozenExecutionSafety::from_resolved_authority(
            &self.target.name,
            &self.profile.name,
            self.attempt_isolation,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    TargetNotFound(String),
    ProfileNotFound(String),
    Incompatible(String),
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetNotFound(t) => write!(f, "execution target not found in registry: '{t}'"),
            Self::ProfileNotFound(p) => {
                write!(f, "execution profile not found in registry: '{p}'")
            }
            Self::Incompatible(m) => write!(f, "target/profile combination incompatible: {m}"),
        }
    }
}

impl std::error::Error for ResolutionError {}

/// Standardized runtime resolution of execution environment.
///
/// Rules:
/// 1. If `mode` is `ExecutionResolutionMode::Authoritative(reg)`, the registry is strictly authoritative:
///    - Missing target -> `ResolutionError::TargetNotFound` (RESOURCE_UNAVAILABLE).
///    - Missing profile -> `ResolutionError::ProfileNotFound` (RESOURCE_UNAVAILABLE).
///    - Incompatible target/profile -> `ResolutionError::Incompatible` (RESOURCE_UNAVAILABLE).
///    - No silent fallback to adapter defaults.
/// 2. If `mode` is `ExecutionResolutionMode::DirectUnconfigured`, direct-caller mode is used with unisolated defaults.
pub fn resolve_execution_environment(
    mode: ExecutionResolutionMode<'_>,
    target_name: &str,
    profile_name: &str,
) -> Result<ResolvedExecutionEnvironment, ResolutionError> {
    match mode {
        ExecutionResolutionMode::Authoritative(reg) => {
            let target = reg
                .get_target(target_name)
                .ok_or_else(|| ResolutionError::TargetNotFound(target_name.to_string()))?;
            let profile = reg
                .get_profile(profile_name)
                .ok_or_else(|| ResolutionError::ProfileNotFound(profile_name.to_string()))?;

            if let Some(allowed) = &profile.allowed_targets {
                if !allowed.contains(target_name) {
                    return Err(ResolutionError::Incompatible(format!(
                        "profile '{profile_name}' is not compatible with target '{target_name}'"
                    )));
                }
            }

            let attempt_isolation = target.attempt_isolation;
            Ok(ResolvedExecutionEnvironment::new(
                target.clone(),
                profile.clone(),
                attempt_isolation,
            ))
        }
        ExecutionResolutionMode::DirectUnconfigured => Ok(ResolvedExecutionEnvironment::new(
            ExecutionTargetConfig::new(target_name, "default", false),
            ExecutionProfileConfig::new(profile_name),
            false,
        )),
    }
}

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
    fn explicitly_empty_registry_fails_closed() {
        let registry = ExecutionRegistry::new();
        let err = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            "local",
            "default",
        )
        .unwrap_err();
        assert_eq!(err, ResolutionError::TargetNotFound("local".to_string()));
    }

    #[test]
    fn missing_profile_fails_closed() {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        let err = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            "local",
            "isolated",
        )
        .unwrap_err();
        assert_eq!(
            err,
            ResolutionError::ProfileNotFound("isolated".to_string())
        );
    }

    #[test]
    fn incompatible_target_and_profile_fails_closed() {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        registry
            .register_target(ExecutionTargetConfig::new("remote", "container", true))
            .unwrap();
        registry
            .register_profile(
                ExecutionProfileConfig::new("remote-only").with_allowed_targets(["remote"]),
            )
            .unwrap();

        // local + remote-only must fail with Incompatible
        let err = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            "local",
            "remote-only",
        )
        .unwrap_err();
        assert_eq!(
            err,
            ResolutionError::Incompatible(
                "profile 'remote-only' is not compatible with target 'local'".to_string()
            )
        );

        // remote + remote-only must succeed
        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            "remote",
            "remote-only",
        )
        .unwrap();
        assert_eq!(env.target().name, "remote");
        assert_eq!(env.profile().name, "remote-only");
        assert!(env.attempt_isolation());
    }

    #[test]
    fn duplicate_target_or_profile_registration_fails_closed() {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "process", false))
            .unwrap();
        let dup_target = registry
            .register_target(ExecutionTargetConfig::new("local", "process", true))
            .unwrap_err();
        assert_eq!(
            dup_target,
            ConfigurationError::DuplicateTarget("local".into())
        );

        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        let dup_profile = registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap_err();
        assert_eq!(
            dup_profile,
            ConfigurationError::DuplicateProfile("default".into())
        );
    }

    #[test]
    fn invalid_configuration_parameters_fail_closed() {
        let mut registry = ExecutionRegistry::new();
        assert_eq!(
            registry
                .register_target(ExecutionTargetConfig::new("   ", "process", false))
                .unwrap_err(),
            ConfigurationError::InvalidName("target name cannot be empty".into())
        );
        assert_eq!(
            registry
                .register_profile(ExecutionProfileConfig::new("p").with_timeout(-5.0))
                .unwrap_err(),
            ConfigurationError::InvalidTimeout(
                "timeout must be positive finite seconds, got -5".into()
            )
        );
    }

    #[test]
    fn valid_target_and_profile_resolve_isolation() {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local-b", "process", true))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("isolated-writer"))
            .unwrap();

        let env = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            "local-b",
            "isolated-writer",
        )
        .unwrap();
        assert_eq!(env.target().name, "local-b");
        assert_eq!(env.target().adapter_kind, "process");
        assert_eq!(env.profile().name, "isolated-writer");
        assert!(env.attempt_isolation());
        assert_eq!(
            env.safety(),
            FrozenExecutionSafety::from_resolved_authority("local-b", "isolated-writer", true)
        );
    }

    #[test]
    fn direct_unconfigured_mode_returns_unisolated_defaults() {
        let env = resolve_execution_environment(
            ExecutionResolutionMode::DirectUnconfigured,
            "local",
            "default",
        )
        .unwrap();
        assert_eq!(env.target().name, "local");
        assert_eq!(env.profile().name, "default");
        assert!(!env.attempt_isolation());
        assert_eq!(
            env.safety(),
            FrozenExecutionSafety::unisolated("local", "default")
        );
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
}
