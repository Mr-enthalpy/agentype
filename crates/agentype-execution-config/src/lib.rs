//! Pure execution configuration registry, resolution authority, and unforgeable
//! isolated-safety facts.
//!
//! This crate owns the authoritative execution configuration models and produces
//! strongly-typed `FrozenExecutionSafety` facts. Outside of this crate, `FrozenExecutionSafety`
//! carrying `attempt_isolation = true` cannot be forged by any safe or unsafe public constructor;
//! it can be obtained exclusively through `ResolvedExecutionEnvironment::safety()`.
//! (`ExecutionLaunchSnapshot`'s raw constructor is a trusted unchecked path —
//! see its safety documentation; the snapshot boundary is procedural, not
//! memory-safety-enforced.)

use agentype_core::{
    AttemptId, AuthoritativeExecutionBinding, BatchId, CommittedContinuitySnapshot, ExecutionId,
    IncarnationId, LeaseEpoch, LeaseId, LogicalAgentId, RequestId, TaskId, UnixTime, WorkspaceMode,
    WorkstreamId,
};
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
    InvalidAdapterKind(String),
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
            Self::InvalidAdapterKind(m) => write!(f, "invalid adapter kind: {m}"),
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
        if target.adapter_kind.trim().is_empty() {
            return Err(ConfigurationError::InvalidAdapterKind(
                "adapter kind cannot be empty: a target must name its adapter binding".into(),
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

/// Strongly-typed safety fact produced exclusively through authoritative
/// configuration resolution, and bound to the Attempt identity it was
/// resolved for.
///
/// Ensures that the isolation guarantee cannot be decoupled from the target
/// and profile for which configuration resolution was performed, nor from the
/// Attempt/lease epoch whose durable binding keyed the resolution: the Kernel
/// rejects a proof whose `attempt_id` / `lease_epoch` do not match the
/// attempt being launched, so a proof can never be replayed across attempts
/// even when the target and profile names coincide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenExecutionSafety {
    attempt_id: AttemptId,
    lease_epoch: LeaseEpoch,
    execution_target: String,
    execution_profile: String,
    attempt_isolation: bool,
}

impl FrozenExecutionSafety {
    /// Safe constructor for unisolated execution environments
    /// (no-isolation-assumption fail-safe). The proof is still Attempt-bound.
    pub fn unisolated(binding: AuthoritativeExecutionBinding) -> Self {
        Self {
            attempt_id: binding.attempt_id,
            lease_epoch: binding.lease_epoch,
            execution_target: binding.execution_target,
            execution_profile: binding.execution_profile,
            attempt_isolation: false,
        }
    }

    /// Internal constructor used exclusively by `ResolvedExecutionEnvironment`.
    pub(crate) fn new(binding: AuthoritativeExecutionBinding, attempt_isolation: bool) -> Self {
        Self {
            attempt_id: binding.attempt_id,
            lease_epoch: binding.lease_epoch,
            execution_target: binding.execution_target,
            execution_profile: binding.execution_profile,
            attempt_isolation,
        }
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn lease_epoch(&self) -> LeaseEpoch {
        self.lease_epoch
    }

    pub fn execution_target(&self) -> &str {
        &self.execution_target
    }

    pub fn execution_profile(&self) -> &str {
        &self.execution_profile
    }

    pub fn attempt_isolation(&self) -> bool {
        self.attempt_isolation
    }
}

/// Resolved physical execution environment for an authoritative launch.
///
/// Fields are encapsulated as private and readonly to prevent post-resolution
/// tampering with the isolated safety fact. The environment carries the
/// durable `AuthoritativeExecutionBinding` it was resolved for, so the minted
/// safety proof is Attempt-bound.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedExecutionEnvironment {
    target: ExecutionTargetConfig,
    profile: ExecutionProfileConfig,
    attempt_isolation: bool,
    binding: AuthoritativeExecutionBinding,
}

impl ResolvedExecutionEnvironment {
    pub(crate) fn new(
        target: ExecutionTargetConfig,
        profile: ExecutionProfileConfig,
        attempt_isolation: bool,
        binding: AuthoritativeExecutionBinding,
    ) -> Self {
        Self {
            target,
            profile,
            attempt_isolation,
            binding,
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

    /// The durable authority binding this environment was resolved for.
    pub fn binding(&self) -> &AuthoritativeExecutionBinding {
        &self.binding
    }

    /// Produce the strongly-typed safety proof bound to this resolved
    /// environment AND to the Attempt identity of its binding.
    ///
    /// This is the sole mechanism across all crates to obtain a
    /// `FrozenExecutionSafety` carrying `attempt_isolation = true`.
    pub fn safety(&self) -> FrozenExecutionSafety {
        FrozenExecutionSafety::new(self.binding.clone(), self.attempt_isolation)
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
/// The resolution key comes from the durable `AuthoritativeExecutionBinding`
/// (Attempt-frozen target/profile), never from a Claim DTO. Rules:
/// 1. If `mode` is `ExecutionResolutionMode::Authoritative(reg)`, the registry is strictly authoritative:
///    - Missing target -> `ResolutionError::TargetNotFound` (RESOURCE_UNAVAILABLE).
///    - Missing profile -> `ResolutionError::ProfileNotFound` (RESOURCE_UNAVAILABLE).
///    - Incompatible target/profile -> `ResolutionError::Incompatible` (RESOURCE_UNAVAILABLE).
///    - No silent fallback to adapter defaults.
/// 2. If `mode` is `ExecutionResolutionMode::DirectUnconfigured`, direct-caller mode is used with unisolated defaults.
///
/// The resolved environment carries the binding, so the minted
/// `FrozenExecutionSafety` is Attempt-bound: `Kernel::create_execution`
/// rejects a proof whose attempt/lease epoch do not match the attempt being
/// launched, closing cross-attempt proof replay.
pub fn resolve_execution_environment(
    mode: ExecutionResolutionMode<'_>,
    binding: &AuthoritativeExecutionBinding,
) -> Result<ResolvedExecutionEnvironment, ResolutionError> {
    let target_name = binding.execution_target.as_str();
    let profile_name = binding.execution_profile.as_str();
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
                binding.clone(),
            ))
        }
        ExecutionResolutionMode::DirectUnconfigured => Ok(ResolvedExecutionEnvironment::new(
            ExecutionTargetConfig::new(target_name, "default", false),
            ExecutionProfileConfig::new(profile_name),
            false,
            binding.clone(),
        )),
    }
}

/// Authoritative launch snapshot reconstructed from durable Scheduler state.
///
/// Encapsulates all execution parameters as private, readonly fields. The
/// canonical production path is the Kernel execution-creation transaction;
/// the raw constructor below is a **trusted unchecked constructor**, not a
/// memory-safety-enforced boundary (see its safety documentation).
///
/// `task_name` is the durable human-readable Task label (a scheduling/display
/// fact). It is NOT the worker prompt: the worker-facing prompt is a derived
/// representation rendered by the runtime from the full launch protocol
/// (IDs, epoch, payload, acceptance, continuity, workspace mode).
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionLaunchSnapshot {
    execution_id: ExecutionId,
    request_id: RequestId,
    task_id: TaskId,
    batch_id: BatchId,
    attempt_id: AttemptId,
    attempt_number: u32,
    lease_id: LeaseId,
    lease_epoch: LeaseEpoch,
    lease_expires_at: UnixTime,
    logical_agent_id: LogicalAgentId,
    incarnation_id: IncarnationId,
    incarnation_runtime_handle: Value,
    execution_target: String,
    execution_profile: String,
    workspace_mode: WorkspaceMode,
    task_name: String,
    payload: Value,
    acceptance: Value,
    workstream_id: Option<WorkstreamId>,
    continuity: CommittedContinuitySnapshot,
    safety: FrozenExecutionSafety,
}

impl ExecutionLaunchSnapshot {
    /// Trusted internal unchecked constructor for the Kernel execution-creation
    /// transaction.
    ///
    /// # Safety
    ///
    /// The caller MUST be the fenced Kernel execution-creation transaction,
    /// which has atomically validated the Attempt, Lease, Task, Agent, and
    /// Incarnation records from durable storage, so that every field reflects
    /// durable authority.
    ///
    /// This `unsafe` marker is a **procedural contract, not an access-control
    /// mechanism**: Rust memory safety does not enforce the kernel-only
    /// construction invariant, and any crate can call this function inside an
    /// `unsafe` block. Constructing a snapshot here does not by itself confer
    /// SQLite authority, but callers outside the Kernel transaction bypass the
    /// validation guarantees and violate the crate's trust contract.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn from_persisted_kernel_authority(
        execution_id: ExecutionId,
        request_id: RequestId,
        task_id: TaskId,
        batch_id: BatchId,
        attempt_id: AttemptId,
        attempt_number: u32,
        lease_id: LeaseId,
        lease_epoch: LeaseEpoch,
        lease_expires_at: UnixTime,
        logical_agent_id: LogicalAgentId,
        incarnation_id: IncarnationId,
        incarnation_runtime_handle: Value,
        execution_target: String,
        execution_profile: String,
        workspace_mode: WorkspaceMode,
        task_name: String,
        payload: Value,
        acceptance: Value,
        workstream_id: Option<WorkstreamId>,
        continuity: CommittedContinuitySnapshot,
        safety: FrozenExecutionSafety,
    ) -> Self {
        Self {
            execution_id,
            request_id,
            task_id,
            batch_id,
            attempt_id,
            attempt_number,
            lease_id,
            lease_epoch,
            lease_expires_at,
            logical_agent_id,
            incarnation_id,
            incarnation_runtime_handle,
            execution_target,
            execution_profile,
            workspace_mode,
            task_name,
            payload,
            acceptance,
            workstream_id,
            continuity,
            safety,
        }
    }

    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn batch_id(&self) -> &BatchId {
        &self.batch_id
    }

    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }

    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }

    pub fn lease_id(&self) -> &LeaseId {
        &self.lease_id
    }

    pub fn lease_epoch(&self) -> LeaseEpoch {
        self.lease_epoch
    }

    pub fn lease_expires_at(&self) -> UnixTime {
        self.lease_expires_at
    }

    pub fn logical_agent_id(&self) -> &LogicalAgentId {
        &self.logical_agent_id
    }

    pub fn incarnation_id(&self) -> &IncarnationId {
        &self.incarnation_id
    }

    pub fn incarnation_runtime_handle(&self) -> &Value {
        &self.incarnation_runtime_handle
    }

    pub fn execution_target(&self) -> &str {
        &self.execution_target
    }

    pub fn execution_profile(&self) -> &str {
        &self.execution_profile
    }

    pub fn workspace_mode(&self) -> WorkspaceMode {
        self.workspace_mode
    }

    /// Durable human-readable Task label. Never send this to a worker as the
    /// prompt; use the runtime-rendered worker protocol instead.
    pub fn task_name(&self) -> &str {
        &self.task_name
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }

    pub fn acceptance(&self) -> &Value {
        &self.acceptance
    }

    pub fn workstream_id(&self) -> Option<&WorkstreamId> {
        self.workstream_id.as_ref()
    }

    pub fn continuity(&self) -> &CommittedContinuitySnapshot {
        &self.continuity
    }

    pub fn safety(&self) -> &FrozenExecutionSafety {
        &self.safety
    }

    pub fn attempt_isolation(&self) -> bool {
        self.safety.attempt_isolation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(target: &str, profile: &str) -> AuthoritativeExecutionBinding {
        AuthoritativeExecutionBinding {
            attempt_id: AttemptId::new(),
            lease_epoch: LeaseEpoch(1),
            execution_target: target.to_string(),
            execution_profile: profile.to_string(),
        }
    }

    #[test]
    fn explicitly_empty_registry_fails_closed() {
        let registry = ExecutionRegistry::new();
        let err = resolve_execution_environment(
            ExecutionResolutionMode::Authoritative(&registry),
            &binding("local", "default"),
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
            &binding("local", "isolated"),
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
            &binding("local", "remote-only"),
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
            &binding("remote", "remote-only"),
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
        // An empty or whitespace adapter_kind is an empty binding: rejected.
        assert_eq!(
            registry
                .register_target(ExecutionTargetConfig::new("local", "", false))
                .unwrap_err(),
            ConfigurationError::InvalidAdapterKind(
                "adapter kind cannot be empty: a target must name its adapter binding".into()
            )
        );
        assert_eq!(
            registry
                .register_target(ExecutionTargetConfig::new("local", "   ", false))
                .unwrap_err(),
            ConfigurationError::InvalidAdapterKind(
                "adapter kind cannot be empty: a target must name its adapter binding".into()
            )
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

        let b = binding("local-b", "isolated-writer");
        let env =
            resolve_execution_environment(ExecutionResolutionMode::Authoritative(&registry), &b)
                .unwrap();
        assert_eq!(env.target().name, "local-b");
        assert_eq!(env.target().adapter_kind, "process");
        assert_eq!(env.profile().name, "isolated-writer");
        assert!(env.attempt_isolation());
        assert_eq!(env.binding(), &b);
        assert_eq!(env.safety(), FrozenExecutionSafety::new(b.clone(), true));
    }

    #[test]
    fn direct_unconfigured_mode_returns_unisolated_defaults() {
        let b = binding("local", "default");
        let env =
            resolve_execution_environment(ExecutionResolutionMode::DirectUnconfigured, &b).unwrap();
        assert_eq!(env.target().name, "local");
        assert_eq!(env.profile().name, "default");
        assert!(!env.attempt_isolation());
        assert_eq!(env.safety(), FrozenExecutionSafety::unisolated(b));
    }
}
