//! M5 runtime configuration boundary and M4 recovery orchestration.
//! Dispatcher/heartbeat/notifier loops belong to subsequent M5 tasks.

use agentype_core::{Error, ExpireReport};
use agentype_storage_sqlite::Kernel;
use serde_json::Value;
use std::collections::HashMap;

/// Configuration for an execution target (adapter binding + host/endpoint settings).
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionTargetConfig {
    pub name: String,
    pub adapter_kind: String,
    pub options: Value,
}

impl ExecutionTargetConfig {
    pub fn new(name: impl Into<String>, adapter_kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            adapter_kind: adapter_kind.into(),
            options: Value::Null,
        }
    }

    pub fn with_options(mut self, options: Value) -> Self {
        self.options = options;
        self
    }
}

/// Configuration for an execution profile (model settings, attempt isolation, timeouts).
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionProfileConfig {
    pub name: String,
    pub attempt_isolation: bool,
    pub timeout_seconds: Option<f64>,
    pub options: Value,
}

impl ExecutionProfileConfig {
    pub fn new(name: impl Into<String>, attempt_isolation: bool) -> Self {
        Self {
            name: name.into(),
            attempt_isolation,
            timeout_seconds: None,
            options: Value::Null,
        }
    }

    pub fn with_timeout(mut self, timeout_seconds: f64) -> Self {
        self.timeout_seconds = Some(timeout_seconds);
        self
    }

    pub fn with_options(mut self, options: Value) -> Self {
        self.options = options;
        self
    }
}

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

    pub fn register_target(&mut self, target: ExecutionTargetConfig) {
        self.targets.insert(target.name.clone(), target);
    }

    pub fn register_profile(&mut self, profile: ExecutionProfileConfig) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    pub fn get_target(&self, name: &str) -> Option<&ExecutionTargetConfig> {
        self.targets.get(name)
    }

    pub fn get_profile(&self, name: &str) -> Option<&ExecutionProfileConfig> {
        self.profiles.get(name)
    }
}

/// Resolved physical execution environment for an authoritative launch.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedExecutionEnvironment {
    pub target: ExecutionTargetConfig,
    pub profile: ExecutionProfileConfig,
    pub attempt_isolation: bool,
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
            Self::Incompatible(msg) => {
                write!(f, "incompatible target/profile configuration: {msg}")
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

/// Standardized runtime resolution of execution environment.
///
/// Rules:
/// 1. If `registry` is `Some`, the registry is strictly authoritative:
///    - Missing target -> `ResolutionError::TargetNotFound` (RESOURCE_UNAVAILABLE).
///    - Missing profile -> `ResolutionError::ProfileNotFound` (RESOURCE_UNAVAILABLE).
///    - No silent fallback to adapter defaults.
/// 2. If `registry` is `None`, explicit direct-caller mode is used with default values.
pub fn resolve_execution_environment(
    registry: Option<&ExecutionRegistry>,
    target_name: &str,
    profile_name: &str,
) -> Result<ResolvedExecutionEnvironment, ResolutionError> {
    match registry {
        Some(reg) => {
            let target = reg
                .get_target(target_name)
                .ok_or_else(|| ResolutionError::TargetNotFound(target_name.to_string()))?;
            let profile = reg
                .get_profile(profile_name)
                .ok_or_else(|| ResolutionError::ProfileNotFound(profile_name.to_string()))?;
            Ok(ResolvedExecutionEnvironment {
                target: target.clone(),
                profile: profile.clone(),
                attempt_isolation: profile.attempt_isolation,
            })
        }
        None => Ok(ResolvedExecutionEnvironment {
            target: ExecutionTargetConfig::new(target_name, "default"),
            profile: ExecutionProfileConfig::new(profile_name, false),
            attempt_isolation: false,
        }),
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
    use agentype_core::{Clock, ManualClock, PartitionSpec, Retention};
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
    fn explicitly_empty_registry_fails_closed() {
        let registry = ExecutionRegistry::new();
        let err = resolve_execution_environment(Some(&registry), "local", "default").unwrap_err();
        assert_eq!(err, ResolutionError::TargetNotFound("local".to_string()));
    }

    #[test]
    fn missing_profile_fails_closed() {
        let mut registry = ExecutionRegistry::new();
        registry.register_target(ExecutionTargetConfig::new("local", "process"));
        let err = resolve_execution_environment(Some(&registry), "local", "isolated").unwrap_err();
        assert_eq!(
            err,
            ResolutionError::ProfileNotFound("isolated".to_string())
        );
    }

    #[test]
    fn valid_target_and_profile_resolve_isolation() {
        let mut registry = ExecutionRegistry::new();
        registry.register_target(ExecutionTargetConfig::new("local-b", "codex"));
        registry.register_profile(ExecutionProfileConfig::new("isolated-writer", true));

        let env =
            resolve_execution_environment(Some(&registry), "local-b", "isolated-writer").unwrap();
        assert_eq!(env.target.name, "local-b");
        assert_eq!(env.target.adapter_kind, "codex");
        assert_eq!(env.profile.name, "isolated-writer");
        assert!(env.attempt_isolation);
    }

    #[test]
    fn unsupplied_registry_returns_direct_caller_mode() {
        let env = resolve_execution_environment(None, "local", "default").unwrap();
        assert_eq!(env.target.name, "local");
        assert_eq!(env.profile.name, "default");
        assert!(!env.attempt_isolation);
    }
}
