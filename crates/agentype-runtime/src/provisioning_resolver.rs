//! M6-B.4 internal provisioning resolution.
//!
//! This is the trusted production producer of `ResolvedProvisioningEvidence` and
//! the source candidate enumeration for an admitted typed Task. It is
//! deliberately **not** a control surface: it is called only by the runtime's
//! internal acquisition path, sources enforcement facts from the imported M5
//! binding (never from a catalog claim or a caller), and performs no durable
//! write.
//!
//! Candidate resolution and winner preparation MAY perform **bounded, read-only
//! I/O** via [`SourceConfigIntegration::attest`] / `prepare` (for example
//! resolving and hashing an `ExternalRef` locator), but they perform **no
//! physical side effect at all**: the durably committed winner is only *prepared*
//! into an opaque launch descriptor, and the exact adapter physically materializes
//! it inside `start_execution` (M5 owns the physical lifecycle). Every integration
//! call is bounded by one absolute `AdapterDeadline` re-qualified by the runtime
//! after the call.
//!
//! `DECLARED` never satisfies a security class. A candidate is eligible only
//! when `can_provision_task` holds over the exact candidate tuple, the
//! `AdapterBindingPolicy` is active and its `required_safety` is satisfied by
//! the imported enforceability, and the source's stable `binding_ref` resolves
//! to exactly one installed binding of the policy's `adapter_kind`.

use crate::{AdapterRegistry, ResolvedAdapterBinding};
use agentype_adapter_api::AdapterDeadline;
use agentype_adapter_api::NetworkEnforcement;
use agentype_agent_contract::{
    can_provision_task, AdapterBindingPolicy, AdapterPolicyRef, AgentTypeRef, BindingSnapshot,
    CapabilityRef, CapabilityValue, ConfigStatus, MaterializationDigest, NetworkPolicy,
    PhysicalSafety, ProvisioningBinding, ResolvedProvisioningEvidence, SandboxPolicyRef,
    SourceConfig, SourceConfigRef, SpawnSource, SpawnSourceRef, RESOLVER_VERSION,
};
use agentype_core::{ExecutionId, TaskId, WorkspaceMode};
use agentype_execution_config::ExecutionRegistry;
use agentype_storage_sqlite::Kernel;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// A typed provisioning resolution failure. None of these is an M5 failure
/// class; the acquisition path maps "no eligible candidate" to
/// `RESOURCE_UNAVAILABLE` and treats corruption as fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningResolutionError {
    NotTypedTask,
    AgentNotBound,
    NoEligibleSource,
    AgentTypeMissing(AgentTypeRef),
    MissingRevision(String),
    MutationBearingConfigUnsupported,
    ExactBindingUnresolved(String),
    ExactBindingChanged(String),
    CredentialUnavailable(String),
    ExternalReferenceNotAttested(String),
    SelectionAmbiguous(String),
    MaterializationMismatch(String),
    ProvisioningDeadlineExceeded(String),
    ProvisioningAuthorityExpired(String),
    PostCommitPreparationFailed(String),
    RequiredSafetyUnsatisfied(String),
    SourceOrConfigInactive,
    PolicyKindMismatch {
        policy_kind: String,
        imported_kind: String,
    },
    Evidence(agentype_agent_contract::ContractError),
    Storage(agentype_core::Error),
}

impl std::fmt::Display for ProvisioningResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotTypedTask => write!(f, "task has no typed agent requirement"),
            Self::AgentNotBound => write!(f, "agent has no exact type binding"),
            Self::NoEligibleSource => write!(f, "no eligible source/config candidate"),
            Self::AgentTypeMissing(r) => {
                write!(
                    f,
                    "agent type {}@{} is missing",
                    r.id().as_str(),
                    r.revision()
                )
            }
            Self::MissingRevision(what) => write!(f, "missing catalog revision: {what}"),
            Self::ExactBindingUnresolved(binding_ref) => {
                write!(
                    f,
                    "exact binding_ref '{binding_ref}' is not currently resolvable"
                )
            }
            Self::ExactBindingChanged(binding_ref) => write!(
                f,
                "the exact binding for '{binding_ref}' changed since resolution"
            ),
            Self::CredentialUnavailable(reference) => {
                write!(
                    f,
                    "credential reference '{reference}' is not currently available"
                )
            }
            Self::ExternalReferenceNotAttested(locator) => write!(
                f,
                "external config '{locator}' has no trusted content attestation"
            ),
            Self::SelectionAmbiguous(what) => write!(
                f,
                "more than one eligible candidate is tied and the frozen selection order cannot disambiguate: {what}"
            ),
            Self::MaterializationMismatch(what) => write!(
                f,
                "the resolved source config does not match the committed digest: {what}"
            ),
            Self::ProvisioningDeadlineExceeded(what) => write!(
                f,
                "the provisioning integration exceeded the absolute deadline: {what}"
            ),
            Self::ProvisioningAuthorityExpired(what) => write!(
                f,
                "the provisioning authority expired while preparing: {what}"
            ),
            Self::PostCommitPreparationFailed(what) => write!(
                f,
                "preparation failed after the provisioning authority was committed: {what}"
            ),
            Self::MutationBearingConfigUnsupported => {
                write!(
                    f,
                    "source config body cannot be read through the validated record"
                )
            }
            Self::RequiredSafetyUnsatisfied(what) => {
                write!(
                    f,
                    "imported enforceability does not satisfy required_safety: {what}"
                )
            }
            Self::SourceOrConfigInactive => write!(f, "source or config is not active"),
            Self::PolicyKindMismatch {
                policy_kind,
                imported_kind,
            } => write!(
                f,
                "adapter policy kind '{policy_kind}' does not match imported kind '{imported_kind}'"
            ),
            Self::Evidence(e) => write!(f, "provisioning evidence: {e}"),
            Self::Storage(e) => write!(f, "storage: {e}"),
        }
    }
}

impl std::error::Error for ProvisioningResolutionError {}

impl From<agentype_core::Error> for ProvisioningResolutionError {
    fn from(value: agentype_core::Error) -> Self {
        Self::Storage(value)
    }
}

fn network_policy(enforcement: NetworkEnforcement) -> NetworkPolicy {
    match enforcement {
        NetworkEnforcement::Disabled => NetworkPolicy::Disabled,
        NetworkEnforcement::Restricted => NetworkPolicy::Restricted,
        NetworkEnforcement::Enabled => NetworkPolicy::Enabled,
    }
}

/// Build imported, candidate-bound enforcement evidence from the imported M5
/// binding. Fails closed if the policy's `adapter_kind` does not match the
/// imported kind or if the imported enforceability does not satisfy the policy's
/// `required_safety`.
///
/// Internal to the resolver: the supported surface is `resolve_source_candidates`
/// plus the `acquire_typed_task_*` entry points, never direct evidence
/// construction.
pub(crate) fn produce_evidence(
    imported: &ResolvedAdapterBinding,
    policy: &AdapterBindingPolicy,
    source: &SpawnSource,
    config: &SourceConfig,
) -> Result<ResolvedProvisioningEvidence, ProvisioningResolutionError> {
    if policy.adapter_kind != imported.adapter_kind() {
        return Err(ProvisioningResolutionError::PolicyKindMismatch {
            policy_kind: policy.adapter_kind.clone(),
            imported_kind: imported.adapter_kind().to_string(),
        });
    }
    if policy.status != ConfigStatus::Active
        || source.status != agentype_agent_contract::SourceStatus::Active
        || config.status != ConfigStatus::Active
    {
        return Err(ProvisioningResolutionError::SourceOrConfigInactive);
    }
    let envelope = imported.safety_envelope();
    let required = &policy.required_safety;
    if required.attempt_isolation() && !envelope.attempt_isolation() {
        return Err(ProvisioningResolutionError::RequiredSafetyUnsatisfied(
            "attempt_isolation".into(),
        ));
    }
    for mode in required.enforceable_workspace_modes() {
        if !envelope.enforces_workspace(*mode) {
            return Err(ProvisioningResolutionError::RequiredSafetyUnsatisfied(
                format!("workspace {}", mode.as_sql()),
            ));
        }
    }
    for policy_net in required.enforceable_network_modes() {
        let enforcement = match policy_net {
            NetworkPolicy::Disabled => NetworkEnforcement::Disabled,
            NetworkPolicy::Restricted => NetworkEnforcement::Restricted,
            NetworkPolicy::Enabled => NetworkEnforcement::Enabled,
        };
        if !envelope.enforces_network(enforcement) {
            return Err(ProvisioningResolutionError::RequiredSafetyUnsatisfied(
                "network".into(),
            ));
        }
    }
    let mut workspace_modes: Vec<WorkspaceMode> = envelope.enforceable_workspace().to_vec();
    workspace_modes.sort_by_key(|m| match m {
        WorkspaceMode::ReadOnly => 0u8,
        WorkspaceMode::Write => 1u8,
    });
    workspace_modes.dedup();
    let network_modes: std::collections::BTreeSet<NetworkPolicy> = envelope
        .enforceable_network()
        .iter()
        .map(|e| network_policy(*e))
        .collect();
    let safety = PhysicalSafety::new(envelope.attempt_isolation(), workspace_modes, network_modes)
        .map_err(ProvisioningResolutionError::Evidence)?;
    // B.4 v1: adapters import coarse safety only; enforced sandbox policies and
    // enforced security-class capabilities are a B.5 extension. Empty here means
    // a security-class requirement fails closed in `can_provision`.
    ResolvedProvisioningEvidence::from_imported_binding(
        policy.policy_ref.clone(),
        imported.adapter_kind(),
        imported.adapter_binding_key().as_str(),
        source.source_ref.clone(),
        config.config_ref.clone(),
        config.config_digest.clone(),
        safety,
        Vec::<SandboxPolicyRef>::new(),
        Vec::<(CapabilityRef, CapabilityValue)>::new(),
    )
    .map_err(ProvisioningResolutionError::Evidence)
}

/// One eligible source/config candidate for a typed Task.
///
/// The exact imported binding identity the resolver proved (`adapter_kind` +
/// opaque `adapter_binding_key`) is carried here so the acquisition and the
/// eventual Execution/BindingSnapshot freeze the same key, instead of letting a
/// caller re-supply an arbitrary key after resolution.
#[derive(Clone, Debug)]
pub struct SourceProvisioningCandidate {
    pub agent_type: AgentTypeRef,
    pub spawn_source: SpawnSource,
    pub source_config: SourceConfig,
    pub adapter_policy: AdapterBindingPolicy,
    pub evidence: ResolvedProvisioningEvidence,
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    pub required_workspace: WorkspaceMode,
    pub required_network: NetworkPolicy,
    /// The imported enforceability **capability** (source/adapter `PhysicalSafety`).
    pub effective_security: PhysicalSafety,
    pub adapter_kind: String,
    pub adapter_binding_key: String,
    /// The exact source-integration protocol the winning binding accepts.
    pub provisioning_protocol: String,
    /// Secret-free canonical **attested** digest the pure integration resolved
    /// for the exact config and physical domain (an expected identity, not proof
    /// of materialization).
    pub attested_materialization_digest: MaterializationDigest,
}

/// Source-private config resolution / attestation seam.
///
/// Core selects a `SourceConfig` but MUST NOT interpret it. The source
/// integration consumes the **validated** `SourceConfigRevision` (the only
/// SourceConfig read path) and, bound to the exact physical domain
/// `(adapter_kind, adapter_binding_key)`, resolves the opaque body/locator into a
/// secret-free attested digest plus an opaque launch descriptor. Both operations
/// are pure/read-only: the physical environment is created by the exact adapter's
/// `start_execution` under M5.
/// the secret-free attested content digest (which must match the committed one)
/// and an **opaque, secret-free launch descriptor** describing the environment
/// to materialize. This is a pure, read-only resolution: it performs NO physical
/// side effect. Core persists the descriptor and forwards it to the physical
/// start request; the exact adapter physically materializes the described
/// environment as part of `start_execution`, so M5 owns the physical lifecycle
/// (no second, unmodelled provisioning lifecycle before the Execution).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSource {
    pub digest: String,
    pub descriptor: String,
}

pub trait SourceConfigIntegration: Send + Sync {
    /// The source-integration **protocol identity** this integration prepares
    /// descriptors under. It must equal the exact adapter binding's
    /// `import_provisioning_protocol`, so a descriptor can only ever be launched
    /// against an adapter that accepts its grammar.
    fn protocol(&self) -> &str;

    /// Pure, side-effect-free attestation used during candidate eligibility:
    /// verify the exact validated revision is materializable for the exact
    /// physical domain (resolving and hashing an `ExternalRef` locator, or
    /// shape-checking an opaque body) and return the expected, secret-free
    /// materialization digest. MUST NOT mutate anything. The single absolute
    /// `deadline` bounds every Scheduler-facing stage (M5.6 discipline).
    fn attest(
        &self,
        revision: &agentype_storage_sqlite::SourceConfigRevision,
        adapter_kind: &str,
        adapter_binding_key: &str,
        deadline: &AdapterDeadline,
    ) -> Result<String, ProvisioningResolutionError>;

    /// Pure, read-only preparation of the durably committed winner. Resolves the
    /// source-private config into an opaque launch [`PreparedSource`] descriptor
    /// (and re-derives the attested digest, which the caller requires to
    /// exact-match the committed one so an `attest`/`prepare` TOCTOU cannot slip
    /// through). MUST NOT mutate anything or create any physical resource — the
    /// physical materialization happens in the exact adapter's
    /// `start_execution` under M5. Called once, after the provisioning
    /// binding/claim is committed; never during eligibility.
    /// Explicit availability failures settle as RESOURCE_UNAVAILABLE; authority
    /// loss is left to recovery. Storage/corruption and all unrecognized errors
    /// propagate as control-plane faults without a Task-level NACK.
    fn prepare(
        &self,
        revision: &agentype_storage_sqlite::SourceConfigRevision,
        adapter_kind: &str,
        adapter_binding_key: &str,
        deadline: &AdapterDeadline,
    ) -> Result<PreparedSource, ProvisioningResolutionError>;
}

/// Composition-root-owned routing from a durable `SpawnSourceRef` to its
/// **source-local** `SourceConfigIntegration`. Core never interprets the
/// source-private config; the composition root binds each source to the one
/// integration that understands its config grammar. The registry lives outside
/// Core, so source multiplicity (`many AgentType <-> many SpawnSource <-> many
/// config`) is preserved and the candidate universe depends only on the durable
/// catalog plus this explicit routing — never on which single integration a
/// caller happens to pass.
#[derive(Default)]
pub struct SourceIntegrationRegistry {
    by_source: Vec<(SpawnSourceRef, Arc<dyn SourceConfigIntegration>)>,
    uniform: Option<Arc<dyn SourceConfigIntegration>>,
}

impl SourceIntegrationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry that routes any source to one integration (test/legacy use).
    pub fn uniform(integration: Arc<dyn SourceConfigIntegration>) -> Self {
        Self {
            by_source: Vec::new(),
            uniform: Some(integration),
        }
    }

    /// Bind one exact source revision to its integration. A second registration
    /// for the same source is rejected (fail closed).
    pub fn register(
        &mut self,
        source: SpawnSourceRef,
        integration: Arc<dyn SourceConfigIntegration>,
    ) -> Result<(), ProvisioningResolutionError> {
        if self.by_source.iter().any(|(s, _)| s == &source) {
            return Err(ProvisioningResolutionError::MissingRevision(
                "duplicate source integration registration".into(),
            ));
        }
        self.by_source.push((source, integration));
        Ok(())
    }

    pub fn resolve(&self, source: &SpawnSourceRef) -> Option<&dyn SourceConfigIntegration> {
        self.by_source
            .iter()
            .find(|(s, _)| s == source)
            .map(|(_, i)| i.as_ref())
            .or(self.uniform.as_deref())
    }
}

impl SourceProvisioningCandidate {
    pub fn adapter_policy_ref(&self) -> &AdapterPolicyRef {
        &self.adapter_policy.policy_ref
    }

    pub fn source_ref(&self) -> &SpawnSourceRef {
        &self.spawn_source.source_ref
    }

    pub fn config_ref(&self) -> &SourceConfigRef {
        &self.source_config.config_ref
    }

    /// The durable acquisition selection, built from this resolver-produced
    /// candidate. Never built from caller-supplied enforcement facts.
    /// `catalog_frontier_digest` is the frontier the winner was selected against.
    pub fn selection(
        &self,
        catalog_frontier_digest: String,
    ) -> agentype_storage_sqlite::ResolvedProvisioningSelection {
        agentype_storage_sqlite::ResolvedProvisioningSelection {
            spawn_source: self.spawn_source.source_ref.clone(),
            source_config: self.source_config.config_ref.clone(),
            adapter_policy: self.adapter_policy.policy_ref.clone(),
            evidence: self.evidence.clone(),
            adapter_kind: self.adapter_kind.clone(),
            adapter_binding_key: self.adapter_binding_key.clone(),
            provisioning_protocol: self.provisioning_protocol.clone(),
            attested_materialization_digest: self.attested_materialization_digest.clone(),
            catalog_frontier_digest,
        }
    }

    /// Freeze one Execution's immutable `BindingSnapshot`. The exact
    /// `adapter_kind`/`adapter_binding_key` come from the resolver-produced
    /// candidate; the caller supplies only the created Execution id and the
    /// Attempt's resolved target/profile.
    #[allow(clippy::too_many_arguments)]
    pub fn binding_snapshot(
        &self,
        execution_id: &ExecutionId,
        provisioning_binding: &ProvisioningBinding,
        execution_target: &str,
        execution_profile: &str,
        authoritative_isolation: bool,
        credential_refs_digest: Option<String>,
        launch_descriptor: &str,
    ) -> BindingSnapshot {
        // Capability and effective policy are distinct: the snapshot keeps the
        // source/adapter's imported enforceability as a capability set and
        // records the per-execution effective policy separately. M4 owns the
        // authoritative isolation, which the validator requires to equal the
        // frozen Execution.
        BindingSnapshot {
            snapshot_id: format!("snap_{}", Uuid::new_v4().simple()),
            execution_id: execution_id.clone(),
            provisioning_binding_id: provisioning_binding.provisioning_binding_id.clone(),
            adapter_kind: self.adapter_kind.clone(),
            adapter_binding_key: self.adapter_binding_key.clone(),
            spawn_source: self.spawn_source.source_ref.clone(),
            source_config: self.source_config.config_ref.clone(),
            source_config_digest: self.source_config.config_digest.clone(),
            attested_materialization_digest: self.attested_materialization_digest.clone(),
            launch_descriptor: launch_descriptor.to_string(),
            execution_target: execution_target.to_string(),
            execution_profile: execution_profile.to_string(),
            required_capabilities: self.required_capabilities.clone(),
            enforceable_security: self.effective_security.clone(),
            effective_isolation: authoritative_isolation,
            effective_workspace: self.required_workspace,
            effective_network: self.required_network,
            credential_refs_digest,
            resolver_version: RESOLVER_VERSION.to_string(),
        }
    }
}

/// The single authoritative classifier for a `ContractError` raised during
/// candidate eligibility: only these mean "this candidate is ineligible" and
/// may be skipped. Any other `ContractError` (notably `InvariantViolation`) is
/// durable corruption and MUST fail closed.
fn evidence_is_candidate_local(error: &agentype_agent_contract::ContractError) -> bool {
    use agentype_agent_contract::ContractError as C;
    matches!(
        error,
        C::CapabilityMismatch { .. }
            | C::SecurityUnenforceable { .. }
            | C::EvidencePolicyMismatch { .. }
            | C::EvidenceSubjectMismatch { .. }
            | C::SourceConfigInvalid { .. }
    )
}

impl ProvisioningResolutionError {
    /// A candidate-local ineligibility (no binding, inactive source/config,
    /// unsatisfied safety, capability mismatch) skips just that candidate.
    /// Catalog/durable corruption and storage faults fail the whole resolution.
    fn is_candidate_ineligible(&self) -> bool {
        matches!(
            self,
            Self::RequiredSafetyUnsatisfied(_)
                | Self::SourceOrConfigInactive
                | Self::ExternalReferenceNotAttested(_)
                | Self::PolicyKindMismatch { .. }
        ) || matches!(self, Self::Evidence(error) if evidence_is_candidate_local(error))
    }
}

/// Enumerate eligible source/config candidates for an admitted typed Task,
/// evaluating `can_provision_task` over the **Task pin** AgentType. For an
/// already-bound agent use [`resolve_source_candidates_for_agent`], which
/// evaluates over the agent's actual bound AgentType so a source legal only for
/// the bound type is not missed.
///
/// Pure with respect to durability (no writes, no adapter calls). A
/// candidate-local ineligibility is skipped so it cannot hide a later eligible
/// source; catalog/durable corruption fails the whole resolution closed. The
/// result is ordered by `(source_id, revision, config_id, config_revision)`.
pub(crate) fn resolve_source_candidates(
    kernel: &Kernel,
    task_id: &TaskId,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    deadline: &AdapterDeadline,
) -> Result<Vec<SourceProvisioningCandidate>, ProvisioningResolutionError> {
    let requirement = kernel
        .get_task_agent_requirement(task_id)?
        .ok_or(ProvisioningResolutionError::NotTypedTask)?;
    let (agent_type, _status) = kernel
        .get_agent_type(&requirement.required_type)?
        .ok_or_else(|| {
            ProvisioningResolutionError::AgentTypeMissing(requirement.required_type.clone())
        })?;
    enumerate_candidates(
        kernel,
        task_id,
        &requirement,
        &agent_type,
        adapters,
        execution_registry,
        integrations,
        deadline,
    )
}

/// Enumerate eligible source/config candidates for an existing bound agent,
/// evaluating `can_provision_task` over the agent's **actual bound AgentType**
/// (not the Task pin), mirroring the acquisition's re-proof.
pub(crate) fn resolve_source_candidates_for_agent(
    kernel: &Kernel,
    task_id: &TaskId,
    agent_id: &agentype_core::LogicalAgentId,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    deadline: &AdapterDeadline,
) -> Result<Vec<SourceProvisioningCandidate>, ProvisioningResolutionError> {
    let requirement = kernel
        .get_task_agent_requirement(task_id)?
        .ok_or(ProvisioningResolutionError::NotTypedTask)?;
    let bound = kernel
        .get_logical_agent_type_binding(agent_id)?
        .ok_or(ProvisioningResolutionError::AgentNotBound)?;
    let (agent_type, _status) = kernel
        .get_agent_type(&bound)?
        .ok_or_else(|| ProvisioningResolutionError::AgentTypeMissing(bound.clone()))?;
    enumerate_candidates(
        kernel,
        task_id,
        &requirement,
        &agent_type,
        adapters,
        execution_registry,
        integrations,
        deadline,
    )
}

#[allow(clippy::too_many_arguments)]
fn enumerate_candidates(
    kernel: &Kernel,
    task_id: &TaskId,
    requirement: &agentype_agent_contract::TaskAgentRequirement,
    agent_type: &agentype_agent_contract::AgentType,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    deadline: &AdapterDeadline,
) -> Result<Vec<SourceProvisioningCandidate>, ProvisioningResolutionError> {
    let task = kernel.task(task_id)?;
    let partition = kernel.partition(task.partition.as_str())?;
    let (target, _profile) = agentype_execution_config::resolve_target_profile(
        execution_registry,
        &partition.execution_target,
        &partition.execution_profile,
    )
    .map_err(|err| {
        ProvisioningResolutionError::Storage(agentype_core::Error::invalid_authority(
            err.to_string(),
        ))
    })?;
    let required_lifecycle = agentype_agent_contract::required_lifecycle(partition.retention);
    let required_isolation = agent_type.contract.security.requires_attempt_isolation
        || requirement.hard.required_attempt_isolation;
    let catalog = kernel.load_capability_catalog()?;
    let mut out: Vec<SourceProvisioningCandidate> = Vec::new();
    for source_ref in kernel.list_active_spawn_source_refs()? {
        let source = kernel
            .get_spawn_source(&source_ref)?
            .ok_or_else(|| ProvisioningResolutionError::MissingRevision("spawn_source".into()))?;
        let policy = kernel
            .get_adapter_binding_policy(&source.adapter_policy)?
            .ok_or_else(|| {
                ProvisioningResolutionError::MissingRevision("adapter_binding_policy".into())
            })?;
        // Reject known static ineligibility before any source-private I/O.
        // These filters are re-proved by the authority transaction, not deferred
        // until ranking or handoff after an attestation budget has been spent.
        if policy.status != ConfigStatus::Active || policy.adapter_kind != target.adapter_kind {
            continue;
        }
        // Source-local routing: this exact source must be bound (by the
        // composition root) to the one integration that understands its
        // source-private config grammar. An unrouted source is ineligible; its
        // config is never interpreted by an integration belonging to another
        // source.
        let Some(integration) = integrations.resolve(&source_ref) else {
            continue;
        };
        let imported = match adapters.resolve_binding_ref(&policy.adapter_kind, &policy.binding_ref)
        {
            Ok(imported) => imported,
            // No installed binding for the alias: this source simply has no
            // candidate. It is not a corruption and never falls back to another
            // source of the same kind.
            Err(_) => continue,
        };
        // The source integration's descriptor protocol must be accepted by the
        // exact adapter binding that will physically materialize it. A mismatch,
        // or an adapter that is not provisioning-capable, makes this source
        // ineligible.
        if imported.provisioning_protocol() != Some(integration.protocol()) {
            continue;
        }
        if agentype_execution_config::validate_attempt_isolation(
            target.attempt_isolation,
            required_isolation,
            imported.safety_envelope().attempt_isolation(),
        )
        .is_err()
        {
            continue;
        }
        for config_ref in kernel.list_active_source_config_refs(&source_ref)? {
            let revision = kernel
                .get_source_config_revision(&config_ref)?
                .ok_or_else(|| {
                    ProvisioningResolutionError::MissingRevision("source_config".into())
                })?;
            let config = revision.config().clone();
            if !config.credential_refs.is_empty()
                || !config
                    .effective_lifecycle(&source)
                    .contains(&required_lifecycle)
            {
                continue;
            }
            let evidence = match produce_evidence(&imported, &policy, &source, &config) {
                Ok(evidence) => evidence,
                Err(err) if err.is_candidate_ineligible() => continue,
                Err(err) => return Err(err),
            };
            match can_provision_task(
                agent_type,
                &source,
                &config,
                &evidence,
                &catalog,
                &requirement.hard,
            ) {
                Ok(()) => {}
                Err(err) => {
                    let err = ProvisioningResolutionError::Evidence(err);
                    if err.is_candidate_ineligible() {
                        continue;
                    }
                    return Err(err);
                }
            }
            // Pure, side-effect-free attestation during eligibility. The source
            // integration verifies the exact revision is materializable for this
            // exact physical domain (resolving/hashing an ExternalRef, or
            // shape-checking an opaque body). An empty attestation is ineligible
            // and never enters selection. The same absolute endpoint is
            // re-qualified after the call: attestation evidence obtained after
            // the deadline MUST NOT become candidate evidence.
            if deadline.is_expired() {
                return Err(ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                    "deadline already expired before attest".into(),
                ));
            }
            let attestation = integration.attest(
                &revision,
                imported.adapter_kind(),
                imported.adapter_binding_key().as_str(),
                deadline,
            );
            if deadline.is_expired() {
                return Err(ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                    "deadline expired after attest".into(),
                ));
            }
            let materialization_digest = match attestation {
                // A malformed (non-canonical) digest from the integration is a
                // control-plane fault, not a candidate-local ineligibility: it
                // fails the whole resolution closed rather than being skipped.
                Ok(digest) if !digest.trim().is_empty() => MaterializationDigest::new(digest)
                    .map_err(ProvisioningResolutionError::Evidence)?,
                Ok(_) => continue,
                Err(err) if err.is_candidate_ineligible() => continue,
                Err(err) => return Err(err),
            };
            let effective_security = evidence.enforceable_safety().clone();
            out.push(SourceProvisioningCandidate {
                agent_type: agent_type.type_ref.clone(),
                spawn_source: source.clone(),
                source_config: config,
                adapter_policy: policy.clone(),
                evidence,
                required_capabilities: requirement.hard.required_capabilities.clone(),
                required_workspace: requirement.hard.required_workspace,
                required_network: requirement.hard.required_network,
                effective_security,
                adapter_kind: imported.adapter_kind().to_string(),
                adapter_binding_key: imported.adapter_binding_key().as_str().to_string(),
                provisioning_protocol: imported
                    .provisioning_protocol()
                    .unwrap_or_default()
                    .to_string(),
                attested_materialization_digest: materialization_digest,
            });
        }
    }
    Ok(out)
}

/// Select the single deterministic candidate using the frozen spec 07 order.
///
/// The hard filters (correctness, sandbox enforceability, AgentType
/// compatibility) are already applied by `can_provision_task`. The next frozen
/// dimension is **continuity value**: the strongest continuity candidate wins.
/// The remaining frozen dimensions (availability, cost/resource policy) have no
/// durable model yet, so when more than one candidate is tied on continuity the
/// selection is **unresolved and fails closed** rather than inventing a
/// tie-break. Deterministic identity ordering is NOT the selection rule; it is
/// only a deterministic enumeration order
/// ([ADR-0010](../../../decisions/0010-m6b-selection-order-scope.md)).
pub(crate) fn select_candidate(
    candidates: &[SourceProvisioningCandidate],
) -> Result<&SourceProvisioningCandidate, ProvisioningResolutionError> {
    let best = candidates
        .iter()
        .map(continuity_rank)
        .max()
        .ok_or(ProvisioningResolutionError::NoEligibleSource)?;
    let mut top = candidates
        .iter()
        .filter(|candidate| continuity_rank(candidate) == best);
    let winner = top
        .next()
        .ok_or(ProvisioningResolutionError::NoEligibleSource)?;
    match top.next() {
        None => Ok(winner),
        Some(other) => Err(ProvisioningResolutionError::SelectionAmbiguous(format!(
            "{}@{} / {}@{} vs {}@{} / {}@{}",
            winner.spawn_source.source_ref.id().as_str(),
            winner.spawn_source.source_ref.revision(),
            winner.source_config.config_ref.config_id().as_str(),
            winner.source_config.config_ref.revision(),
            other.spawn_source.source_ref.id().as_str(),
            other.spawn_source.source_ref.revision(),
            other.source_config.config_ref.config_id().as_str(),
            other.source_config.config_ref.revision(),
        ))),
    }
}

fn continuity_rank(candidate: &SourceProvisioningCandidate) -> u8 {
    let effective = candidate
        .source_config
        .effective_continuity(&candidate.spawn_source);
    if effective.contains(&agentype_agent_contract::ContinuityMode::Logical) {
        1
    } else {
        0
    }
}

/// Re-prove, immediately before the authority transaction, the process-local
/// facts that can drift after resolution: the Scheduler derives the selection
/// from a fresh enumeration, the winner's exact binding is still resolvable, and
/// the config carries no credential references (B.4 fails closed for those; the
/// availability/attestation authority is B.5). The committed selection is built
/// from the freshly resolved winner, never from a caller-supplied candidate.
fn prepare_selection(
    kernel: &Kernel,
    task_id: &TaskId,
    agent_id: Option<&agentype_core::LogicalAgentId>,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    deadline: &AdapterDeadline,
) -> Result<
    (
        agentype_storage_sqlite::ResolvedProvisioningSelection,
        SourceProvisioningCandidate,
    ),
    ProvisioningResolutionError,
> {
    // Capture the active candidate frontier the winner is selected against; the
    // authority transaction recomputes it and rejects a concurrent publish.
    let frontier = kernel.catalog_frontier_digest()?;
    let eligible = match agent_id {
        Some(agent) => resolve_source_candidates_for_agent(
            kernel,
            task_id,
            agent,
            adapters,
            execution_registry,
            integrations,
            deadline,
        )?,
        None => resolve_source_candidates(
            kernel,
            task_id,
            adapters,
            execution_registry,
            integrations,
            deadline,
        )?,
    };
    // Enumeration completes all known hard filters before attestation and
    // returns only physically eligible candidates. Ranking adds no eligibility.
    let winner = select_candidate(&eligible)?;

    // The winner's exact binding must still be resolvable in the live runtime.
    let policy = kernel
        .get_adapter_binding_policy(&winner.adapter_policy.policy_ref)?
        .ok_or_else(|| {
            ProvisioningResolutionError::MissingRevision("adapter_binding_policy".into())
        })?;
    let resolved = adapters
        .resolve_binding_ref(&policy.adapter_kind, &policy.binding_ref)
        .map_err(|_| {
            ProvisioningResolutionError::ExactBindingUnresolved(policy.binding_ref.clone())
        })?;
    if resolved.adapter_kind() != winner.adapter_kind
        || resolved.adapter_binding_key().as_str() != winner.adapter_binding_key
        || resolved.provisioning_protocol() != Some(winner.provisioning_protocol.as_str())
    {
        return Err(ProvisioningResolutionError::ExactBindingChanged(
            policy.binding_ref.clone(),
        ));
    }

    // Credential-bearing configs were filtered during enumeration (B.4 has no
    // credential authority), so they never reach selection; the Kernel also
    // fails closed for them as defense in depth.
    //
    // No physical side effect exists before the Execution here: the winner is
    // only *prepared* (pure/read-only) after the authority transaction freezes
    // its provisioning identity, and the exact adapter physically materializes
    // the descriptor during `start_execution` (M5 owns the physical lifecycle).
    // Build the durable selection from the freshly resolved winner, never from
    // caller-supplied enforcement facts.
    Ok((winner.selection(frontier), winner.clone()))
}

/// A committed typed acquisition plus the exact winner candidate it was built
/// from. The candidate is not persisted by the kernel; carrying it out of the
/// facade is what lets the Execution handoff construct the `BindingSnapshot`
/// from the same evidence the authority transaction proved.
#[derive(Debug)]
pub(crate) struct TypedAcquisitionOutcome {
    pub acquisition: agentype_storage_sqlite::TypedAcquisition,
    pub candidate: SourceProvisioningCandidate,
    /// Opaque launch descriptor for the environment the exact adapter will
    /// physically materialize during `start_execution`, populated by
    /// `finalize_preparation`.
    pub launch_descriptor: String,
}

/// The single, Scheduler-owned authority-bearing typed acquisition.
///
/// It applies the frozen spec 06 existing-first order: the B.3 ranked READY
/// candidates are tried in order, each with a full B.4 physical eligibility
/// proof, and only when none can be acquired does the Scheduler provision a new
/// LogicalAgent from an eligible SpawnSource. The `existing` vs `new` decision is
/// therefore a deterministic Scheduler control, not an integration choice.
///
/// Daemon-internal: owned by the (not-yet-wired) ControlLoopService. It is
/// `pub(crate)` and currently unused outside tests until that integration lands.
#[allow(clippy::too_many_arguments, dead_code)]
pub(crate) fn acquire_typed_task(
    kernel: &Kernel,
    task_id: &TaskId,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    attest_deadline: &AdapterDeadline,
    prepare_deadline: &AdapterDeadline,
) -> Result<TypedAcquisitionOutcome, ProvisioningResolutionError> {
    let existing = kernel.match_existing_agents_for_task(task_id)?;
    let mut last_error: Option<ProvisioningResolutionError> = None;
    for candidate in existing {
        match try_acquire_existing(
            kernel,
            task_id,
            &candidate.logical_agent_id,
            adapters,
            execution_registry,
            integrations,
            attest_deadline,
        ) {
            Ok(acquisition) => {
                return finalize_preparation(kernel, integrations, acquisition, prepare_deadline)
            }
            // Only an explicitly enumerated candidate-local ineligibility permits
            // falling through to the next agent/source. Anything else
            // (corruption, storage failure, recovery-required, unsupported
            // resolver state) is a control-plane fault and MUST terminate.
            Err(err) if !is_candidate_local(&err) => return Err(err),
            Err(err) => last_error = Some(err),
        }
    }
    match acquire_new_agent(
        kernel,
        task_id,
        adapters,
        execution_registry,
        integrations,
        attest_deadline,
    ) {
        Ok(acquisition) => {
            finalize_preparation(kernel, integrations, acquisition, prepare_deadline)
        }
        Err(err) if !is_candidate_local(&err) => Err(err),
        Err(err) => Err(last_error.unwrap_or(err)),
    }
}

/// Prepare (pure, read-only) the durably committed winner under its frozen
/// provisioning identity. This runs AFTER the authority transaction, so losers
/// are never prepared and no physical side effect precedes the durable
/// commitment. On success the outcome gains the opaque launch descriptor that
/// the Execution handoff freezes into the `BindingSnapshot` and forwards to the
/// adapter, which physically materializes the described environment during
/// `start_execution` (M5 owns the physical lifecycle).
///
/// Preparation is provably side-effect-free, so there is no writer-unknown
/// outcome:
/// * `Unavailable` (deadline spent, or the pure descriptor cannot be resolved)
///   follows the frozen pre-start `RESOURCE_UNAVAILABLE` semantics;
/// * `AuthorityLost` does NOT fabricate a Task-level configuration failure —
///   recovery/expire settles the claim;
/// * `Fatal` durable corruption / persistence is propagated as-is.
fn finalize_preparation(
    kernel: &Kernel,
    integrations: &SourceIntegrationRegistry,
    mut outcome: TypedAcquisitionOutcome,
    deadline: &AdapterDeadline,
) -> Result<TypedAcquisitionOutcome, ProvisioningResolutionError> {
    match prepare_winner(kernel, integrations, &outcome.acquisition, deadline) {
        Ok(descriptor) => {
            outcome.launch_descriptor = descriptor;
            Ok(outcome)
        }
        Err(PreparationFailure::Unavailable(err)) => {
            settle_configuration_unavailable(kernel, &outcome.acquisition.claim, &err.to_string())?;
            Err(ProvisioningResolutionError::PostCommitPreparationFailed(
                err.to_string(),
            ))
        }
        Err(PreparationFailure::AuthorityLost(err)) => {
            // Authority already gone before any (pure) work; not a Task-level
            // configuration failure. Recovery/expire settles the claim.
            Err(ProvisioningResolutionError::PostCommitPreparationFailed(
                err.to_string(),
            ))
        }
        Err(PreparationFailure::Fatal(err)) => Err(err),
    }
}

/// Preparation failure classification. Preparation is pure/read-only, so there
/// is no physical outcome to be unknown; persistence/corruption faults are fatal
/// and are never disguised as a Task failure class.
enum PreparationFailure {
    /// Static/liveness failure -> `RESOURCE_UNAVAILABLE`.
    Unavailable(ProvisioningResolutionError),
    /// Authority loss before any work -> no Task-level config failure.
    AuthorityLost(ProvisioningResolutionError),
    /// Durable corruption / persistence fault -> fatal.
    Fatal(ProvisioningResolutionError),
}

impl PreparationFailure {
    /// Classify the integration boundary by fault kind, not by where the error
    /// originated. Unknown/control-plane errors fail closed; candidate skipping
    /// is no longer applicable after authority committed.
    fn from_integration(error: ProvisioningResolutionError) -> Self {
        use agentype_core::Error;
        use ProvisioningResolutionError as P;
        match &error {
            P::ProvisioningAuthorityExpired(_)
            | P::Storage(Error::StaleAuthority(_) | Error::InvalidAuthority(_)) => {
                Self::AuthorityLost(error)
            }
            P::ExternalReferenceNotAttested(_)
            | P::CredentialUnavailable(_)
            | P::SourceOrConfigInactive
            | P::ExactBindingUnresolved(_)
            | P::ExactBindingChanged(_)
            | P::ProvisioningDeadlineExceeded(_)
            | P::MaterializationMismatch(_)
            | P::Storage(Error::ConfigurationUnavailable(_)) => Self::Unavailable(error),
            P::Evidence(contract_error) if evidence_is_candidate_local(contract_error) => {
                Self::Unavailable(error)
            }
            _ => Self::Fatal(error),
        }
    }
}

fn prepare_winner(
    kernel: &Kernel,
    integrations: &SourceIntegrationRegistry,
    acquisition: &agentype_storage_sqlite::TypedAcquisition,
    deadline: &AdapterDeadline,
) -> Result<String, PreparationFailure> {
    if deadline.is_expired() {
        return Err(PreparationFailure::Unavailable(
            ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                "deadline already expired before prepare".into(),
            ),
        ));
    }
    // Full committed-claim authority fence: the same AuthoritySnapshot validation
    // every authority-bearing transaction uses (Attempt/Lease ACTIVE, epoch,
    // Task.current_attempt_id, fencing_epoch, unexpired) plus Claim-identity
    // coherence.
    match kernel.claim_authority_is_current(&acquisition.claim) {
        Ok(true) => {}
        Ok(false) => {
            return Err(PreparationFailure::AuthorityLost(
                ProvisioningResolutionError::ProvisioningAuthorityExpired(
                    "lease authority was not current before prepare".into(),
                ),
            ))
        }
        Err(err) => {
            return Err(PreparationFailure::Fatal(
                ProvisioningResolutionError::Storage(err),
            ))
        }
    }
    let binding = &acquisition.provisioning_binding;
    // Re-resolve the SAME source-local integration that produced the committed
    // binding; its config is never interpreted by another source's integration.
    let integration = integrations.resolve(&binding.spawn_source).ok_or_else(|| {
        PreparationFailure::Fatal(ProvisioningResolutionError::MissingRevision(
            "source integration".into(),
        ))
    })?;
    let revision = match kernel.get_source_config_revision(&binding.source_config) {
        Ok(Some(revision)) => revision,
        // A committed immutable revision that has vanished is durable
        // corruption, not a candidate-local ineligibility.
        Ok(None) => {
            return Err(PreparationFailure::Fatal(
                ProvisioningResolutionError::MissingRevision("source_config".into()),
            ))
        }
        Err(err) => {
            return Err(PreparationFailure::Fatal(
                ProvisioningResolutionError::Storage(err),
            ))
        }
    };
    // Pure, read-only preparation: no physical side effect.
    let prepared = integration
        .prepare(
            &revision,
            &binding.adapter_kind,
            &binding.adapter_binding_key,
            deadline,
        )
        .map_err(PreparationFailure::from_integration)?;
    if deadline.is_expired() {
        return Err(PreparationFailure::Unavailable(
            ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                "deadline expired after prepare".into(),
            ),
        ));
    }
    let actual = MaterializationDigest::new(prepared.digest).map_err(|err| {
        PreparationFailure::Unavailable(ProvisioningResolutionError::Evidence(err))
    })?;
    if let Err(err) =
        require_materialization_receipt(&binding.attested_materialization_digest, &actual)
    {
        return Err(PreparationFailure::Unavailable(err));
    }
    if prepared.descriptor.trim().is_empty() {
        return Err(PreparationFailure::Unavailable(
            ProvisioningResolutionError::Evidence(
                agentype_agent_contract::ContractError::InvalidRef {
                    reason: "prepare returned an empty launch descriptor".into(),
                },
            ),
        ));
    }
    // Re-validate the full committed-claim authority before reporting success.
    match kernel.claim_authority_is_current(&acquisition.claim) {
        Ok(true) => Ok(prepared.descriptor),
        Ok(false) => Err(PreparationFailure::AuthorityLost(
            ProvisioningResolutionError::ProvisioningAuthorityExpired(
                "lease authority was no longer current after prepare".into(),
            ),
        )),
        Err(err) => Err(PreparationFailure::Fatal(
            ProvisioningResolutionError::Storage(err),
        )),
    }
}

/// Deterministically settle a durably committed acquisition as
/// `RESOURCE_UNAVAILABLE` (the frozen pre-start composition-failure semantics).
fn settle_configuration_unavailable(
    kernel: &Kernel,
    claim: &agentype_core::Claim,
    detail: &str,
) -> Result<(), ProvisioningResolutionError> {
    match kernel.report_configuration_unavailable(&claim.attempt_id, claim.lease_epoch, detail) {
        Ok(_) => Ok(()),
        // Authority already expired: nothing to settle; recovery cleans up.
        Err(agentype_core::Error::StaleAuthority(_))
        | Err(agentype_core::Error::InvalidAuthority(_)) => Ok(()),
        Err(err) => Err(ProvisioningResolutionError::Storage(err)),
    }
}

/// The source-integration attested digest MUST match the committed one. A
/// mismatch (e.g. external state changed between `attest` and `prepare`) rejects
/// preparation and MUST NOT produce an Execution. The message is redacted:
/// it never echoes a source-produced value.
fn require_materialization_receipt(
    committed: &MaterializationDigest,
    actual: &MaterializationDigest,
) -> Result<(), ProvisioningResolutionError> {
    if actual != committed {
        return Err(ProvisioningResolutionError::MaterializationMismatch(
            "the resolved source config does not match the committed digest".into(),
        ));
    }
    Ok(())
}

/// Whether an error is a candidate-local ineligibility that may be skipped while
/// trying the next route. The default (unknown errors) is NOT local: fail closed.
fn is_candidate_local(error: &ProvisioningResolutionError) -> bool {
    match error {
        ProvisioningResolutionError::NoEligibleSource
        | ProvisioningResolutionError::AgentNotBound
        | ProvisioningResolutionError::RequiredSafetyUnsatisfied(_)
        | ProvisioningResolutionError::SourceOrConfigInactive
        | ProvisioningResolutionError::ExternalReferenceNotAttested(_)
        | ProvisioningResolutionError::PolicyKindMismatch { .. }
        | ProvisioningResolutionError::ExactBindingUnresolved(_)
        | ProvisioningResolutionError::ExactBindingChanged(_)
        | ProvisioningResolutionError::CredentialUnavailable(_) => true,
        // Only the narrow candidate-local `ContractError`s are skippable; a
        // `SelectionAmbiguous` is a task-terminal resolution failure (ADR-0010),
        // never an agent/source fallback.
        ProvisioningResolutionError::Evidence(error) => evidence_is_candidate_local(error),
        // Per-candidate storage rejections from the acquisition re-proof (agent
        // not placeable, policy/kind/type mismatch, agent not READY) are local.
        // A `StaleAuthority` is a global authority-snapshot invalidation (the
        // catalog frontier changed, or the claim authority expired): the whole
        // acquisition must be re-resolved from the top of the B.3 ranking, not
        // fall through to a lower-ranked agent, so it is NOT local.
        // Only an explicit per-candidate authority rejection is candidate-local.
        // `NotFound` (a vanished Task/partition) and `InvalidTransition` (a
        // global authority change) are NOT "this candidate is unsuitable"; they
        // must fail the whole acquisition closed rather than fall through to a
        // lower-ranked agent.
        ProvisioningResolutionError::Storage(err) => matches!(
            err,
            agentype_core::Error::InvalidAuthority(_)
                | agentype_core::Error::ConfigurationUnavailable(_)
        ),
        // MissingRevision / AgentTypeMissing / MutationBearingConfigUnsupported /
        // Storage(InvariantViolation | StaleAuthority | Conflict | RecoveryRequired
        // | StorageFailure) / NotTypedTask are control-plane/corruption/global
        // failures: never local.
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn try_acquire_existing(
    kernel: &Kernel,
    task_id: &TaskId,
    agent_id: &agentype_core::LogicalAgentId,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    deadline: &AdapterDeadline,
) -> Result<TypedAcquisitionOutcome, ProvisioningResolutionError> {
    let (selection, candidate) = prepare_selection(
        kernel,
        task_id,
        Some(agent_id),
        adapters,
        execution_registry,
        integrations,
        deadline,
    )?;
    let acquisition =
        kernel.acquire_typed_task_existing(task_id, agent_id, &selection, execution_registry)?;
    Ok(TypedAcquisitionOutcome {
        acquisition,
        candidate,
        launch_descriptor: String::new(),
    })
}

fn acquire_new_agent(
    kernel: &Kernel,
    task_id: &TaskId,
    adapters: &AdapterRegistry,
    execution_registry: &ExecutionRegistry,
    integrations: &SourceIntegrationRegistry,
    deadline: &AdapterDeadline,
) -> Result<TypedAcquisitionOutcome, ProvisioningResolutionError> {
    let (selection, candidate) = prepare_selection(
        kernel,
        task_id,
        None,
        adapters,
        execution_registry,
        integrations,
        deadline,
    )?;
    let acquisition =
        kernel.acquire_typed_task_new_agent(task_id, &selection, execution_registry)?;
    Ok(TypedAcquisitionOutcome {
        acquisition,
        candidate,
        launch_descriptor: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdapterDeadlinePolicy, AdapterSafetyEnvelope};
    use agentype_adapter_api::{FakeAdapter, NetworkEnforcement};
    use agentype_agent_contract::{
        canonical_json_body_digest, AffinityConstraint, AgentRequirementDraft, AgentType,
        AgentTypeContract, AgentTypeSelector, Budget, ConfigDigest, ContinuityMode, CredentialRef,
        LifecycleMode, SecurityContract, SourceStatus,
    };
    use agentype_core::{
        Clock, FailureClass, LogicalAgentState, ManualClock, PartitionSpec, RawWorkIntent,
        Retention, SemanticInputSet, TaskId, TaskSpec, TaskState,
    };
    use agentype_execution_config::{
        AdapterBindingKey, ExecutionProfileConfig, ExecutionRegistry, ExecutionTargetConfig,
    };
    use agentype_storage_sqlite::{Kernel, SourceConfigBody};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    const MAX_BYTES: usize = 16_384;

    // A fatal attestation error makes accidental evaluation visible: known
    // ineligibility must never spend a deadline or abort later valid candidates.
    struct StaticFilterProbe;

    impl SourceConfigIntegration for StaticFilterProbe {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            kind: &str,
            key: &str,
            deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            if !revision.config().credential_refs.is_empty()
                || kind == "other"
                || key == "k1"
                    && revision.config().config_ref.source().id().as_str() == "unisolated"
            {
                return Err(ProvisioningResolutionError::MissingRevision(
                    "statically ineligible candidate reached attest".into(),
                ));
            }
            EchoMaterializer.attest(revision, kind, key, deadline)
        }

        fn prepare(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            kind: &str,
            key: &str,
            deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            EchoMaterializer.prepare(revision, kind, key, deadline)
        }
    }

    #[test]
    fn credential_config_is_filtered_before_attestation_and_cannot_hide_valid_config() {
        let kernel = kernel();
        let pin = publish(&kernel);
        let body = json!({"model": "credential-first"});
        let config = SourceConfig {
            config_ref: SourceConfigRef::new(
                SpawnSourceRef::new("source", 1).unwrap(),
                "000-credential",
                1,
            )
            .unwrap(),
            config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: vec![CredentialRef::new("secret").unwrap()],
            claims: Vec::new(),
            status: ConfigStatus::Active,
        };
        kernel
            .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
            .unwrap();
        let task = typed_task(&kernel, pin);
        let result = acquire_typed_task(
            &kernel,
            &task,
            &adapters_with_binding(),
            &execution_registry(),
            &si(StaticFilterProbe),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        assert_eq!(
            result
                .candidate
                .source_config
                .config_ref
                .config_id()
                .as_str(),
            "config"
        );
        assert_eq!(kernel.attempt_count_for_task(&task).unwrap(), 1);
    }

    #[test]
    fn wrong_target_kind_is_filtered_before_attestation() {
        let kernel = kernel();
        let pin = publish(&kernel);
        publish_wrong_kind_logical_source(&kernel);
        let task = typed_task(&kernel, pin);
        let result = acquire_typed_task(
            &kernel,
            &task,
            &adapters_with_binding_and_other(),
            &execution_registry(),
            &si(StaticFilterProbe),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        assert_eq!(result.candidate.adapter_kind, "default");
    }

    #[test]
    fn target_isolation_filters_incapable_source_before_attest_and_selection() {
        let kernel = kernel();
        let pin = publish(&kernel);
        // Disable the original source; add an incapable and a capable source
        // sharing the target kind and continuity rank. Without the target gate
        // the resolver would call the incapable source and/or report ambiguity.
        kernel
            .set_spawn_source_status(
                &SpawnSourceRef::new("source", 1).unwrap(),
                SourceStatus::Disabled,
            )
            .unwrap();
        for (name, binding_ref) in [("unisolated", "primary"), ("isolated", "isolated")] {
            let policy = AdapterBindingPolicy {
                policy_ref: AdapterPolicyRef::new(name, 1).unwrap(),
                adapter_kind: "default".into(),
                binding_ref: binding_ref.into(),
                required_safety: safety(),
                status: ConfigStatus::Active,
            };
            kernel.publish_adapter_binding_policy(&policy).unwrap();
            let source = SpawnSource {
                source_ref: SpawnSourceRef::new(name, 1).unwrap(),
                adapter_policy: policy.policy_ref,
                lifecycle_modes: [LifecycleMode::Ephemeral].into_iter().collect(),
                continuity_modes: [ContinuityMode::None].into_iter().collect(),
                functional_envelope: BTreeMap::new(),
                claims: Vec::new(),
                status: SourceStatus::Active,
            };
            kernel.publish_spawn_source(&source).unwrap();
            let body = json!({"model": name});
            let config = SourceConfig {
                config_ref: SourceConfigRef::new(source.source_ref, "config", 1).unwrap(),
                config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
                lifecycle_modes: None,
                continuity_modes: None,
                credential_refs: Vec::new(),
                claims: Vec::new(),
                status: ConfigStatus::Active,
            };
            kernel
                .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
                .unwrap();
        }
        let mut adapters = adapters_with_binding();
        adapters
            .register_with_safety_and_ref(
                "default",
                "isolated",
                AdapterBindingKey::new("k2").unwrap(),
                Arc::new(FakeAdapter::new()),
                AdapterDeadlinePolicy::uniform(Duration::from_secs(1)).unwrap(),
                AdapterSafetyEnvelope::unenforceable()
                    .with_attempt_isolation(true)
                    .with_enforceable_workspace([WorkspaceMode::ReadOnly])
                    .with_enforceable_network([NetworkEnforcement::Disabled]),
            )
            .unwrap();
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "default", true))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        let task = typed_task(&kernel, pin);
        let result = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(StaticFilterProbe),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        assert_eq!(result.candidate.adapter_binding_key, "k2");
        assert_eq!(kernel.attempt_count_for_task(&task).unwrap(), 1);
        let launch =
            crate::prepare_typed_execution_launch(&kernel, result, &registry, &adapters).unwrap();
        assert!(launch.snapshot().attempt_isolation());
    }

    struct PrepareError(ProvisioningResolutionError);

    impl SourceConfigIntegration for PrepareError {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            kind: &str,
            key: &str,
            deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            EchoMaterializer.attest(revision, kind, key, deadline)
        }

        fn prepare(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _kind: &str,
            _key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            Err(self.0.clone())
        }
    }

    fn preparation_kernel() -> (Kernel, rusqlite::Connection) {
        let path = std::env::temp_dir().join(format!("b4-preparation-{}.db", Uuid::new_v4()));
        let kernel =
            Kernel::open(&path, Arc::new(ManualClock::new(1_000.0)), 10.0, MAX_BYTES).unwrap();
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
        let raw = rusqlite::Connection::open(path).unwrap();
        (kernel, raw)
    }

    #[test]
    fn integration_fatal_preparation_errors_preserve_authority_and_fail_closed() {
        use agentype_core::Error;
        for err in [
            ProvisioningResolutionError::Storage(Error::invariant("integration invariant")),
            ProvisioningResolutionError::Storage(Error::storage_failure("integration storage")),
            ProvisioningResolutionError::Storage(Error::recovery_required("integration recovery")),
            ProvisioningResolutionError::Storage(Error::not_found("integration missing row")),
            ProvisioningResolutionError::Storage(Error::invalid_transition(
                "integration transition",
            )),
            ProvisioningResolutionError::Storage(Error::conflict("integration conflict")),
            ProvisioningResolutionError::MissingRevision("source_config".into()),
            ProvisioningResolutionError::NotTypedTask,
        ] {
            let (kernel, raw) = preparation_kernel();
            let pin = publish_with_lifecycle(&kernel, LifecycleMode::Resident);
            let task = typed_task(&kernel, pin.clone());
            let agent = kernel.ready_agent("general").unwrap();
            kernel.bind_logical_agent_type(&agent, &pin).unwrap();
            let (adapters, start_probe) = adapters_with_start_probe();
            let outcome = try_acquire_existing(
                &kernel,
                &task,
                &agent,
                &adapters,
                &execution_registry(),
                &si(EchoMaterializer),
                &deadline(),
            )
            .unwrap();
            let claim = outcome.acquisition.claim.clone();
            let binding = outcome.acquisition.provisioning_binding.clone();
            let result = finalize_preparation(
                &kernel,
                &si(PrepareError(err.clone())),
                outcome,
                &deadline(),
            )
            .unwrap_err();
            assert_eq!(result, err);
            assert!(kernel.claim_authority_is_current(&claim).unwrap());
            assert_eq!(kernel.task(&task).unwrap().state, TaskState::Leased);
            assert_eq!(
                kernel.attempt(&claim.attempt_id).unwrap().state,
                agentype_core::AttemptState::Active
            );
            assert_eq!(
                kernel.lease_for_attempt(&claim.attempt_id).unwrap().state,
                agentype_core::LeaseState::Active
            );
            assert_eq!(
                kernel
                    .get_provisioning_binding(&binding.incarnation_id)
                    .unwrap(),
                Some(binding)
            );
            assert_eq!(
                raw.query_row("SELECT COUNT(*) FROM failures", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                raw.query_row("SELECT COUNT(*) FROM executions", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(start_probe.start_call_count(), 0);
        }
    }

    #[test]
    fn integration_availability_preparation_errors_settle_resource_unavailable() {
        for err in [
            ProvisioningResolutionError::ExternalReferenceNotAttested("config unavailable".into()),
            ProvisioningResolutionError::ProvisioningDeadlineExceeded("read deadline".into()),
            ProvisioningResolutionError::Storage(agentype_core::Error::configuration_unavailable(
                "integration unavailable",
            )),
        ] {
            let (kernel, raw) = preparation_kernel();
            let pin = publish_with_lifecycle(&kernel, LifecycleMode::Resident);
            let task = typed_task(&kernel, pin.clone());
            let agent = kernel.ready_agent("general").unwrap();
            kernel.bind_logical_agent_type(&agent, &pin).unwrap();
            let (adapters, start_probe) = adapters_with_start_probe();
            let outcome = try_acquire_existing(
                &kernel,
                &task,
                &agent,
                &adapters,
                &execution_registry(),
                &si(EchoMaterializer),
                &deadline(),
            )
            .unwrap();
            let claim = outcome.acquisition.claim.clone();
            let result =
                finalize_preparation(&kernel, &si(PrepareError(err)), outcome, &deadline())
                    .unwrap_err();
            assert!(matches!(
                result,
                ProvisioningResolutionError::PostCommitPreparationFailed(_)
            ));
            assert!(!kernel.claim_authority_is_current(&claim).unwrap());
            assert_eq!(
                kernel.attempt(&claim.attempt_id).unwrap().state,
                agentype_core::AttemptState::Failed
            );
            assert_eq!(
                kernel.logical_agent(&agent).unwrap().state,
                LogicalAgentState::Ready
            );
            assert_eq!(
                raw.query_row(
                    "SELECT failure_class FROM failures WHERE attempt_id=?1",
                    [claim.attempt_id.as_str()],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "RESOURCE_UNAVAILABLE"
            );
            assert_eq!(
                raw.query_row("SELECT COUNT(*) FROM executions", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(start_probe.start_call_count(), 0);
        }
    }

    #[test]
    fn integration_authority_loss_does_not_nack_the_task() {
        for err in [
            ProvisioningResolutionError::ProvisioningAuthorityExpired("lost".into()),
            ProvisioningResolutionError::Storage(agentype_core::Error::stale("lost")),
            ProvisioningResolutionError::Storage(agentype_core::Error::invalid_authority("lost")),
        ] {
            let (kernel, raw) = preparation_kernel();
            let pin = publish_with_lifecycle(&kernel, LifecycleMode::Resident);
            let task = typed_task(&kernel, pin.clone());
            let agent = kernel.ready_agent("general").unwrap();
            kernel.bind_logical_agent_type(&agent, &pin).unwrap();
            let outcome = try_acquire_existing(
                &kernel,
                &task,
                &agent,
                &adapters_with_binding(),
                &execution_registry(),
                &si(EchoMaterializer),
                &deadline(),
            )
            .unwrap();
            assert!(
                finalize_preparation(&kernel, &si(PrepareError(err)), outcome, &deadline())
                    .is_err()
            );
            assert_eq!(kernel.task(&task).unwrap().state, TaskState::Leased);
            assert_eq!(
                raw.query_row("SELECT COUNT(*) FROM failures", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn pure_preparation_closure_can_roll_over_to_a_new_config_or_source() {
        for cancel in [false, true] {
            for same_source in [false, true] {
                let (kernel, raw) = preparation_kernel();
                let pin = publish_with_lifecycle(&kernel, LifecycleMode::Resident);
                let agent = kernel.ready_agent("general").unwrap();
                kernel.bind_logical_agent_type(&agent, &pin).unwrap();
                let task = typed_task_with_retry(
                    &kernel,
                    pin.clone(),
                    agentype_core::RetryPolicy {
                        max_attempts: 3,
                        retry_classes: vec![FailureClass::ResourceUnavailable],
                        base_backoff_seconds: 0.0,
                        max_backoff_seconds: 0.0,
                    },
                );
                let (adapters, start_probe) = adapters_with_start_probe();
                let registry = execution_registry();
                let outcome = try_acquire_existing(
                    &kernel,
                    &task,
                    &agent,
                    &adapters,
                    &registry,
                    &si(EchoMaterializer),
                    &deadline(),
                )
                .unwrap();
                let old = outcome.acquisition.provisioning_binding.clone();
                let next_task = if cancel {
                    finalize_preparation(&kernel, &si(EchoMaterializer), outcome, &deadline())
                        .unwrap();
                    kernel.cancel_task(&task, false).unwrap();
                    typed_task(&kernel, pin)
                } else {
                    assert!(finalize_preparation(
                        &kernel,
                        &si(PrepareError(
                            ProvisioningResolutionError::ExternalReferenceNotAttested(
                                "A unavailable".into()
                            )
                        )),
                        outcome,
                        &deadline()
                    )
                    .is_err());
                    assert_eq!(kernel.task(&task).unwrap().state, TaskState::RetryWait);
                    assert_eq!(kernel.promote_retry_wait().unwrap(), 1);
                    task
                };
                kernel
                    .set_source_config_status(&old.source_config, ConfigStatus::Disabled)
                    .unwrap();
                let mut source = kernel.get_spawn_source(&old.spawn_source).unwrap().unwrap();
                if !same_source {
                    kernel
                        .set_spawn_source_status(&old.spawn_source, SourceStatus::Disabled)
                        .unwrap();
                    source.source_ref = SpawnSourceRef::new("source-b", 1).unwrap();
                    kernel.publish_spawn_source(&source).unwrap();
                }
                let body = json!({"model": "opaque-b"});
                let config = SourceConfig {
                    config_ref: SourceConfigRef::new(source.source_ref.clone(), "config-b", 1)
                        .unwrap(),
                    config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
                    lifecycle_modes: None,
                    continuity_modes: None,
                    credential_refs: Vec::new(),
                    claims: Vec::new(),
                    status: ConfigStatus::Active,
                };
                kernel
                    .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
                    .unwrap();
                let next = acquire_typed_task(
                    &kernel,
                    &next_task,
                    &adapters,
                    &registry,
                    &si(EchoMaterializer),
                    &deadline(),
                    &deadline(),
                )
                .unwrap();
                assert_eq!(next.acquisition.claim.logical_agent_id, agent);
                assert_ne!(
                    next.acquisition.provisioning_binding.incarnation_id,
                    old.incarnation_id
                );
                assert_eq!(
                    next.acquisition.provisioning_binding.source_config,
                    config.config_ref
                );
                assert_eq!(
                    kernel.incarnation(&old.incarnation_id).unwrap().state,
                    agentype_core::IncarnationState::Lost
                );
                assert_eq!(
                    kernel
                        .get_provisioning_binding(&old.incarnation_id)
                        .unwrap(),
                    Some(old)
                );
                assert_eq!(
                    raw.query_row("SELECT COUNT(*) FROM executions", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
                assert_eq!(
                    raw.query_row(
                        "SELECT COUNT(*) FROM attempts WHERE state='ACTIVE'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1
                );
                crate::prepare_typed_execution_launch(&kernel, next, &registry, &adapters).unwrap();
                assert_eq!(start_probe.start_call_count(), 0);
            }
        }
    }

    struct PrepareUnavailable;

    impl SourceConfigIntegration for PrepareUnavailable {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }
        fn attest(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            kind: &str,
            key: &str,
            deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            EchoMaterializer.attest(revision, kind, key, deadline)
        }
        fn prepare(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            kind: &str,
            key: &str,
            deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            FailingMaterializeMaterializer.prepare(revision, kind, key, deadline)
        }
    }

    #[test]
    fn pure_prepare_failure_cannot_invalidate_the_previous_warm_host() {
        let kernel = partitioned_kernel_with_retention(Retention::Resident);
        let pin = publish_with_lifecycle(&kernel, LifecycleMode::Resident);
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let task_a = typed_task(&kernel, pin.clone());
        let adapters = adapters_with_binding();
        let registry = execution_registry();
        let first = acquire_typed_task(
            &kernel,
            &task_a,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        let pb = first.acquisition.provisioning_binding.clone();
        let claim = first.acquisition.claim.clone();
        let launch =
            crate::prepare_typed_execution_launch(&kernel, first, &registry, &adapters).unwrap();
        kernel
            .ack_success(
                &claim.attempt_id,
                claim.lease_epoch,
                Some(launch.snapshot().execution_id()),
                &json!({}),
                None,
                true,
                true,
            )
            .unwrap()
            .unwrap();
        let task_b = typed_task(&kernel, pin);
        assert!(acquire_typed_task(
            &kernel,
            &task_b,
            &adapters,
            &registry,
            &si(PrepareUnavailable),
            &deadline(),
            &deadline()
        )
        .is_err());
        assert_eq!(
            kernel.incarnation(&pb.incarnation_id).unwrap().state,
            agentype_core::IncarnationState::Warm
        );
        assert_eq!(
            kernel.logical_agent(&agent).unwrap().state,
            LogicalAgentState::Ready
        );
        assert_eq!(
            kernel.get_provisioning_binding(&pb.incarnation_id).unwrap(),
            Some(pb)
        );
    }

    /// A canonical `sha256:<64 hex>` materialization digest for test doubles
    /// whose attestation does not come from a real config digest.
    const TEST_DIGEST: &str =
        "sha256:3333333333333333333333333333333333333333333333333333333333333333";

    /// Test materializer: echoes the validated (canonical) config digest as the
    /// materialization attestation.
    struct EchoMaterializer;

    fn deadline() -> AdapterDeadline {
        AdapterDeadline::after(Duration::from_secs(30)).unwrap()
    }

    /// A source-local integration registry that routes every source to the given
    /// integration (test/legacy use).
    fn si(integration: impl SourceConfigIntegration + 'static) -> SourceIntegrationRegistry {
        SourceIntegrationRegistry::uniform(Arc::new(integration))
    }

    impl SourceConfigIntegration for EchoMaterializer {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            Ok(revision.config().config_digest.as_str().to_string())
        }

        fn prepare(
            &self,
            revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            // Echo the attested digest so a committed binding matches.
            let digest = revision.config().config_digest.as_str().to_string();
            Ok(PreparedSource {
                descriptor: format!("env:{digest}"),
                digest,
            })
        }
    }

    fn kernel() -> Kernel {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                0,
                Retention::Ephemeral,
                "local",
                "default",
            ))
            .unwrap();
        kernel
    }

    fn safety() -> PhysicalSafety {
        PhysicalSafety::new(
            false,
            vec![WorkspaceMode::ReadOnly],
            [NetworkPolicy::Disabled].into_iter().collect(),
        )
        .unwrap()
    }

    fn publish(kernel: &Kernel) -> AgentTypeRef {
        publish_with_lifecycle(kernel, LifecycleMode::Ephemeral)
    }

    fn publish_with_lifecycle(kernel: &Kernel, lifecycle: LifecycleMode) -> AgentTypeRef {
        let agent = AgentType {
            type_ref: AgentTypeRef::new("reviewer", 1).unwrap(),
            based_on: None,
            contract: AgentTypeContract {
                allowed_information_functions: vec![agentype_core::InformationFunction::Expand],
                required_capabilities: BTreeMap::new(),
                affinity: AffinityConstraint::Any,
                budget_ceiling: Budget::new(100.0).unwrap(),
                security: SecurityContract {
                    workspace: WorkspaceMode::ReadOnly,
                    network: NetworkPolicy::Disabled,
                    requires_attempt_isolation: false,
                },
                lifecycle: [lifecycle].into_iter().collect(),
                continuity: ContinuityMode::None,
                sandbox_policy: None,
                anchor_constraint: None,
            },
        };
        kernel.publish_agent_type(&agent).unwrap();

        let policy = AdapterBindingPolicy {
            policy_ref: AdapterPolicyRef::new("policy", 1).unwrap(),
            adapter_kind: "default".into(),
            binding_ref: "primary".into(),
            required_safety: safety(),
            status: ConfigStatus::Active,
        };
        kernel.publish_adapter_binding_policy(&policy).unwrap();

        let source = SpawnSource {
            source_ref: SpawnSourceRef::new("source", 1).unwrap(),
            adapter_policy: policy.policy_ref.clone(),
            lifecycle_modes: [lifecycle].into_iter().collect(),
            continuity_modes: [ContinuityMode::None].into_iter().collect(),
            functional_envelope: BTreeMap::new(),
            claims: Vec::new(),
            status: SourceStatus::Active,
        };
        kernel.publish_spawn_source(&source).unwrap();

        let body = json!({"model": "opaque"});
        let config = SourceConfig {
            config_ref: SourceConfigRef::new(source.source_ref.clone(), "config", 1).unwrap(),
            config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: Vec::new(),
            claims: Vec::new(),
            status: ConfigStatus::Active,
        };
        kernel
            .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
            .unwrap();
        agent.type_ref
    }

    fn typed_task(kernel: &Kernel, pin: AgentTypeRef) -> TaskId {
        typed_task_with_retry(kernel, pin, agentype_core::RetryPolicy::default())
    }

    fn typed_task_with_retry(
        kernel: &Kernel,
        pin: AgentTypeRef,
        retry: agentype_core::RetryPolicy,
    ) -> TaskId {
        let gen = kernel.create_generation(json!({})).unwrap();
        let intent = RawWorkIntent {
            raw_intent_key: "work".into(),
            objective: "work".into(),
            information_function: agentype_core::InformationFunction::Expand,
            semantic_input_set: SemanticInputSet::new(),
            rationale: None,
            suggested_task_spec: Some(
                TaskSpec::new("audit", json!({}))
                    .partition("general")
                    .retry(retry),
            ),
        };
        let proposal = kernel
            .compile_root_intent(&gen.generation_id, intent, "session", 1)
            .unwrap();
        kernel
            .admit_typed_proposal(
                &proposal.proposal_id,
                0,
                None,
                AgentRequirementDraft {
                    required_type: AgentTypeSelector::Exact(pin),
                    required_capabilities: BTreeMap::new(),
                    required_network: NetworkPolicy::Disabled,
                    required_attempt_isolation: false,
                    sandbox_policy: None,
                    required_anchor: None,
                    budget: Budget::new(50.0).unwrap(),
                },
            )
            .unwrap()
    }

    fn adapters_with_binding() -> AdapterRegistry {
        adapters_with_start_probe().0
    }

    fn adapters_with_start_probe() -> (AdapterRegistry, Arc<FakeAdapter>) {
        let probe = Arc::new(FakeAdapter::new());
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_with_safety_and_ref(
                "default",
                "primary",
                AdapterBindingKey::new("k1").unwrap(),
                probe.clone(),
                AdapterDeadlinePolicy::uniform(Duration::from_secs(1)).unwrap(),
                AdapterSafetyEnvelope::unenforceable()
                    .with_enforceable_workspace([WorkspaceMode::ReadOnly])
                    .with_enforceable_network([NetworkEnforcement::Disabled]),
            )
            .unwrap();
        (adapters, probe)
    }

    /// Kernel with a capacity-1 population so a READY agent can be bound.
    fn partitioned_kernel() -> Kernel {
        partitioned_kernel_with_retention(Retention::Ephemeral)
    }

    fn partitioned_kernel_with_retention(retention: Retention) -> Kernel {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general", 1, retention, "local", "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        kernel
    }

    fn execution_registry() -> ExecutionRegistry {
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "default", false))
            .unwrap();
        registry
            .register_profile(ExecutionProfileConfig::new("default"))
            .unwrap();
        registry
    }

    /// Register (under the same `default` policy) a second source/config that
    /// carries a credential ref and a stronger continuity, so it would win the
    /// ranking if credential-bearing configs were not filtered before selection.
    fn publish_credential_bearing_logical_source(kernel: &Kernel) {
        let policy_ref = AdapterPolicyRef::new("policy", 1).unwrap();
        let source = SpawnSource {
            source_ref: SpawnSourceRef::new("source-cred", 1).unwrap(),
            adapter_policy: policy_ref,
            lifecycle_modes: [LifecycleMode::Ephemeral].into_iter().collect(),
            continuity_modes: [ContinuityMode::Logical].into_iter().collect(),
            functional_envelope: BTreeMap::new(),
            claims: Vec::new(),
            status: SourceStatus::Active,
        };
        kernel.publish_spawn_source(&source).unwrap();
        let body = json!({"model": "opaque"});
        let config = SourceConfig {
            config_ref: SourceConfigRef::new(source.source_ref.clone(), "config", 1).unwrap(),
            config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: vec![CredentialRef::new("secret").unwrap()],
            claims: Vec::new(),
            status: ConfigStatus::Active,
        };
        kernel
            .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
            .unwrap();
    }

    /// Register a continuity-stronger source whose `adapter_kind` (`other`) the
    /// partition's target (`default`) cannot run, so it must be filtered by target
    /// compatibility before ranking.
    fn publish_wrong_kind_logical_source(kernel: &Kernel) {
        let policy = AdapterBindingPolicy {
            policy_ref: AdapterPolicyRef::new("policy-other", 1).unwrap(),
            adapter_kind: "other".into(),
            binding_ref: "primary".into(),
            required_safety: safety(),
            status: ConfigStatus::Active,
        };
        kernel.publish_adapter_binding_policy(&policy).unwrap();
        let source = SpawnSource {
            source_ref: SpawnSourceRef::new("source-other", 1).unwrap(),
            adapter_policy: policy.policy_ref.clone(),
            lifecycle_modes: [LifecycleMode::Ephemeral].into_iter().collect(),
            continuity_modes: [ContinuityMode::Logical].into_iter().collect(),
            functional_envelope: BTreeMap::new(),
            claims: Vec::new(),
            status: SourceStatus::Active,
        };
        kernel.publish_spawn_source(&source).unwrap();
        let body = json!({"model": "opaque"});
        let config = SourceConfig {
            config_ref: SourceConfigRef::new(source.source_ref.clone(), "config", 1).unwrap(),
            config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: Vec::new(),
            claims: Vec::new(),
            status: ConfigStatus::Active,
        };
        kernel
            .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
            .unwrap();
    }

    /// `adapters_with_binding` plus a resolvable `other` binding (so the
    /// wrong-kind candidate is enumerated, not skipped for an unresolvable alias).
    fn adapters_with_binding_and_other() -> AdapterRegistry {
        let mut adapters = adapters_with_binding();
        adapters
            .register_with_safety_and_ref(
                "other",
                "primary",
                AdapterBindingKey::new("k-other").unwrap(),
                Arc::new(FakeAdapter::new()),
                AdapterDeadlinePolicy::uniform(Duration::from_secs(1)).unwrap(),
                AdapterSafetyEnvelope::unenforceable()
                    .with_enforceable_workspace([WorkspaceMode::ReadOnly])
                    .with_enforceable_network([NetworkEnforcement::Disabled]),
            )
            .unwrap();
        adapters
    }

    /// Attests a canonically valid digest but fails the physical materialization
    /// with an error after the invocation has been entered.
    struct FailingMaterializeMaterializer;

    impl SourceConfigIntegration for FailingMaterializeMaterializer {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            Ok(TEST_DIGEST.to_string())
        }

        fn prepare(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            Err(ProvisioningResolutionError::ExternalReferenceNotAttested(
                "materialize boom".into(),
            ))
        }
    }

    #[test]
    fn post_commit_prepare_failure_settles_unavailable() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let err = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(FailingMaterializeMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap_err();
        // A post-commit prepare failure is terminal, never candidate-local.
        assert!(!is_candidate_local(&err));
        // Preparation is pure (no physical side effect), so the committed
        // authority is settled with the frozen pre-start RESOURCE_UNAVAILABLE
        // semantics: the agent is released (an Ephemeral pool member retires
        // when released) and the Task is not left leased.
        assert!(matches!(
            kernel.logical_agent(&agent).unwrap().state,
            LogicalAgentState::Ready | LogicalAgentState::Retired
        ));
        assert!(matches!(
            kernel.task(&task).unwrap().state,
            TaskState::Suspended | TaskState::RetryWait
        ));
        assert!(kernel.claim_next_available().unwrap().is_none());
    }

    /// A materializer that records that the physical side effect happened, then
    /// returns an error (a lost acknowledgement, not an absence).
    struct SideEffectProbe {
        observed: Arc<std::sync::atomic::AtomicBool>,
        fail: bool,
    }

    impl SourceConfigIntegration for SideEffectProbe {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            Ok(TEST_DIGEST.to_string())
        }

        fn prepare(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            self.observed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if self.fail {
                Err(ProvisioningResolutionError::ExternalReferenceNotAttested(
                    "acknowledgement lost after side effect".into(),
                ))
            } else {
                Ok(PreparedSource {
                    digest: TEST_DIGEST.to_string(),
                    descriptor: "env:test".to_string(),
                })
            }
        }
    }

    /// A deadline that expired BEFORE the side-effectful invocation is provably
    /// side-effect-free, so the integration is never called and the frozen
    /// pre-start (`RESOURCE_UNAVAILABLE`) boundary applies.
    #[test]
    fn materialize_deadline_before_invocation_is_provably_side_effect_free() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = SideEffectProbe {
            observed: observed.clone(),
            fail: false,
        };
        let expired = AdapterDeadline::from_instant(std::time::Instant::now());
        let result = prepare_winner(&kernel, &si(probe), &outcome.acquisition, &expired);
        assert!(matches!(result, Err(PreparationFailure::Unavailable(_))));
        assert!(
            !observed.load(std::sync::atomic::Ordering::SeqCst),
            "materialize must not be invoked when the deadline is already spent"
        );
    }

    /// A static, purely-configuration failure (missing execution profile) MUST be
    /// rejected BEFORE any authority commit or physical materialization, so the
    /// Task stays QUEUED and no Attempt/Lease/ProvisioningBinding is created.
    #[test]
    fn missing_execution_profile_fails_before_materialization() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                1,
                Retention::Ephemeral,
                "local",
                "missing-profile",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry(); // registers only profile "default"
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = SideEffectProbe {
            observed: observed.clone(),
            fail: false,
        };

        let _ = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(probe),
            &deadline(),
            &deadline(),
        )
        .unwrap_err();
        assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
        assert!(
            !observed.load(std::sync::atomic::Ordering::SeqCst),
            "materialize must never run for a static configuration failure"
        );
        assert!(kernel.claim_next_available().unwrap().is_none());
    }

    /// A profile whose `allowed_targets` excludes the partition target is a
    /// static configuration failure and must likewise precede materialization.
    #[test]
    fn incompatible_profile_allowed_targets_fails_before_materialization() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let mut registry = ExecutionRegistry::new();
        registry
            .register_target(ExecutionTargetConfig::new("local", "default", false))
            .unwrap();
        registry
            .register_profile(
                ExecutionProfileConfig::new("default").with_allowed_targets(["somewhere-else"]),
            )
            .unwrap();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = SideEffectProbe {
            observed: observed.clone(),
            fail: false,
        };

        let _ = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(probe),
            &deadline(),
            &deadline(),
        )
        .unwrap_err();
        assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
        assert!(
            !observed.load(std::sync::atomic::Ordering::SeqCst),
            "materialize must never run for a static configuration failure"
        );
    }

    /// Preparation is pure, so a process death after the authority COMMIT and
    /// before the Execution commitment leaves NO physical side effect: recovery
    /// treats the interrupted acquisition as an ordinary M5 orphan
    /// (`ExecutionLost`), never a `WRITER_QUIESCENCE_UNKNOWN` obligation.
    #[test]
    fn interrupted_acquisition_before_execution_recovers_as_execution_lost() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        // Commit exactly the durable authority (Attempt/Lease/ProvisioningBinding)
        // and stop before any Execution: no physical side effect exists.
        let (selection, _candidate) = prepare_selection(
            &kernel,
            &task,
            None,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
        )
        .unwrap();
        let _acquisition = kernel
            .acquire_typed_task_new_agent(&task, &selection, &registry)
            .unwrap();
        assert!(kernel.claim_next_available().unwrap().is_none());

        kernel.recover_authority().unwrap();
        let escalation = kernel.open_escalation_for_task(&task).unwrap();
        assert_eq!(escalation.failure_class, FailureClass::ExecutionLost);
        assert!(matches!(
            kernel.task(&task).unwrap().state,
            TaskState::Suspended | TaskState::RetryWait
        ));
        assert!(kernel.claim_next_available().unwrap().is_none());
    }

    /// P1-2: a credential-bearing (stronger continuity) candidate must be filtered
    /// before ranking, so a credential-free weaker candidate wins instead of the
    /// acquisition failing on the winner's credential check.
    #[test]
    fn credential_bearing_candidate_is_filtered_before_selection() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        // Add a second, continuity-Logical (stronger) source/config that carries a
        // credential ref: B.4-ineligible, so it must never enter the ranking set.
        publish_credential_bearing_logical_source(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let acquisition = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        // The weaker, credential-free source won; the stronger credential-bearing
        // source was never selected.
        assert_eq!(
            acquisition
                .acquisition
                .provisioning_binding
                .source_config
                .config_id()
                .as_str(),
            "config"
        );
        assert!(acquisition
            .candidate
            .source_config
            .credential_refs
            .is_empty());
    }

    /// P1-2: a wrong-target-kind (stronger continuity) candidate must be filtered
    /// before ranking, so a correct-target-kind weaker candidate wins instead of
    /// the transaction rejecting the wrong-kind winner.
    #[test]
    fn wrong_target_kind_candidate_is_filtered_before_selection() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        // A stronger (continuity-Logical) source whose adapter kind the partition
        // target ("default") cannot run.
        publish_wrong_kind_logical_source(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding_and_other();
        let registry = execution_registry();

        let acquisition = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        assert_eq!(
            acquisition.acquisition.provisioning_binding.adapter_kind,
            "default"
        );
        assert_eq!(acquisition.candidate.adapter_kind, "default");
        assert_eq!(
            acquisition
                .acquisition
                .provisioning_binding
                .source_config
                .config_id()
                .as_str(),
            "config"
        );
    }

    /// Handoff contract: the winner candidate carried out of acquisition is what
    /// freezes the snapshot-bearing Execution, using the exact physical domain.
    #[test]
    fn typed_acquisition_handoff_commits_snapshot_bearing_execution() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        let expected_kind = outcome.candidate.adapter_kind.clone();
        let expected_key = outcome.candidate.adapter_binding_key.clone();
        let expected_digest = outcome.candidate.attested_materialization_digest.clone();
        let expected_handle = outcome.launch_descriptor.clone();
        assert!(!expected_handle.trim().is_empty());

        let prepared =
            crate::prepare_typed_execution_launch(&kernel, outcome, &registry, &adapters).unwrap();

        let execution_id = prepared.snapshot().execution_id().clone();
        let execution = kernel.execution(&execution_id).unwrap();
        assert_eq!(execution.state, agentype_core::ExecutionState::Starting);

        let snapshot = kernel.get_binding_snapshot(&execution_id).unwrap().unwrap();
        assert_eq!(snapshot.adapter_kind, expected_kind);
        assert_eq!(snapshot.adapter_binding_key, expected_key);
        assert_eq!(snapshot.attested_materialization_digest, expected_digest);
        // The materialized-environment handle is the causal link: it is frozen in
        // the BindingSnapshot AND delivered to the physical start request.
        assert_eq!(snapshot.launch_descriptor, expected_handle);
        assert_eq!(prepared.request().launch_descriptor(), expected_handle);
        assert_eq!(snapshot.effective_workspace, WorkspaceMode::ReadOnly);
        assert!(!snapshot.effective_isolation);

        assert_eq!(prepared.request().workspace_mode(), WorkspaceMode::ReadOnly);
        assert_eq!(
            prepared.request().network_policy(),
            NetworkEnforcement::Disabled
        );
        assert!(!prepared.request().attempt_isolation());
    }

    /// A source whose lifecycle cannot realize the member's actual M4 retention
    /// is ineligible: acquisition must fail before any authority/physical work.
    #[test]
    fn resident_partition_without_resident_lifecycle_is_ineligible() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open_memory(clock, 10.0, MAX_BYTES).unwrap();
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
        let pin = publish(&kernel); // SpawnSource lifecycle = {Ephemeral} only
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = SideEffectProbe {
            observed: observed.clone(),
            fail: false,
        };

        let _ = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(probe),
            &deadline(),
            &deadline(),
        )
        .unwrap_err();
        assert_eq!(kernel.task(&task).unwrap().state, TaskState::Queued);
        assert!(
            !observed.load(std::sync::atomic::Ordering::SeqCst),
            "materialize must not run for a lifecycle-ineligible member"
        );
    }

    /// The full committed-claim fence rejects a stale authority BEFORE invoking
    /// the side effect, and does so without nacking a Task-level failure.
    #[test]
    fn stale_authority_before_materialize_is_authority_loss_not_config_failure() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        // Expire the committed lease so the claim authority is no longer current.
        kernel.expire_leases(true).unwrap();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = SideEffectProbe {
            observed: observed.clone(),
            fail: false,
        };
        let result = prepare_winner(&kernel, &si(probe), &outcome.acquisition, &deadline());
        assert!(matches!(result, Err(PreparationFailure::AuthorityLost(_))));
        assert!(
            !observed.load(std::sync::atomic::Ordering::SeqCst),
            "the side effect must not start on a stale claim"
        );
    }

    /// A committed immutable revision that has vanished is a fatal persistence
    /// fault, never a Task-level `RESOURCE_UNAVAILABLE`, and the side effect is
    /// not entered.
    #[test]
    fn missing_committed_revision_is_fatal_not_unavailable() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let mut outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        // Point the committed binding at a config revision that does not exist.
        outcome.acquisition.provisioning_binding.source_config =
            agentype_agent_contract::SourceConfigRef::new(
                outcome
                    .acquisition
                    .provisioning_binding
                    .spawn_source
                    .clone(),
                "missing-config",
                1,
            )
            .unwrap();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = SideEffectProbe {
            observed: observed.clone(),
            fail: false,
        };
        let result = prepare_winner(&kernel, &si(probe), &outcome.acquisition, &deadline());
        assert!(matches!(
            result,
            Err(PreparationFailure::Fatal(
                ProvisioningResolutionError::MissingRevision(_)
            ))
        ));
        assert!(!observed.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// A candidate that disagrees with the authoritative partition target is
    /// durable inconsistency, not a candidate choice, and fails closed.
    #[test]
    fn typed_handoff_rejects_candidate_target_kind_mismatch() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let mut outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        outcome.candidate.adapter_kind = "not-the-target".into();
        let err = crate::prepare_typed_execution_launch(&kernel, outcome, &registry, &adapters)
            .unwrap_err();
        assert!(matches!(err, crate::TypedLaunchError::Inconsistent(_)));
    }

    /// The exact acquired binding must still be installed at handoff time.
    #[test]
    fn typed_handoff_requires_the_exact_installed_binding() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding();
        let registry = execution_registry();

        let mut outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();
        outcome.candidate.adapter_binding_key = "not-installed".into();
        let err = crate::prepare_typed_execution_launch(&kernel, outcome, &registry, &adapters)
            .unwrap_err();
        assert!(matches!(
            err,
            crate::TypedLaunchError::Adapter(crate::AdapterUnavailable::Missing { .. })
        ));
    }

    /// The handoff re-qualifies the CURRENT installed enforceability against the
    /// capability the acquisition froze: an importer that keeps the exact
    /// `(kind, key)` but weakens its workspace/network/isolation capability must
    /// not launch. Preparation is pure, so this is a pre-start failure settled as
    /// `RESOURCE_UNAVAILABLE`, not a writer-unknown fence.
    #[test]
    fn typed_handoff_rejects_weakened_install_enforceability() {
        let kernel = partitioned_kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin.clone());
        let agent = kernel.ready_agent("general").unwrap();
        kernel.bind_logical_agent_type(&agent, &pin).unwrap();
        let adapters = adapters_with_binding(); // imports ReadOnly + Disabled
        let registry = execution_registry();

        let outcome = acquire_typed_task(
            &kernel,
            &task,
            &adapters,
            &registry,
            &si(EchoMaterializer),
            &deadline(),
            &deadline(),
        )
        .unwrap();

        // Same exact (kind, key), but no imported enforceability: the committed
        // capability can no longer be honoured.
        let mut weak = AdapterRegistry::new();
        weak.register_with_safety_and_ref(
            "default",
            "primary",
            AdapterBindingKey::new("k1").unwrap(),
            Arc::new(FakeAdapter::new()),
            AdapterDeadlinePolicy::uniform(Duration::from_secs(1)).unwrap(),
            AdapterSafetyEnvelope::unenforceable(),
        )
        .unwrap();

        let err =
            crate::prepare_typed_execution_launch(&kernel, outcome, &registry, &weak).unwrap_err();
        assert!(matches!(err, crate::TypedLaunchError::Weakened(_)));
        // Pre-start RESOURCE_UNAVAILABLE settlement: the agent is released (an
        // Ephemeral member retires) and the Task is not left leased; no
        // writer-unknown obligation is raised.
        assert!(matches!(
            kernel.logical_agent(&agent).unwrap().state,
            LogicalAgentState::Ready | LogicalAgentState::Retired
        ));
        assert!(matches!(
            kernel.task(&task).unwrap().state,
            TaskState::Suspended | TaskState::RetryWait
        ));
        assert_ne!(
            kernel
                .open_escalation_for_task(&task)
                .unwrap()
                .failure_class,
            FailureClass::WriterQuiescenceUnknown
        );
    }

    /// Source-local routing: an active source that the composition root has NOT
    /// routed to an integration is ineligible, so its opaque config is never
    /// interpreted by another source's integration.
    #[test]
    fn unrouted_source_is_ineligible_under_source_local_routing() {
        let kernel = kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin);
        let adapters = adapters_with_binding();

        // Route a different source id, so the only active source is unrouted.
        let mut routed = SourceIntegrationRegistry::new();
        routed
            .register(
                SpawnSourceRef::new("some-other-source", 1).unwrap(),
                Arc::new(EchoMaterializer),
            )
            .unwrap();
        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &routed,
            &deadline(),
        )
        .unwrap();
        assert!(candidates.is_empty());

        // The uniform registry routes every source and yields the candidate.
        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &si(EchoMaterializer),
            &deadline(),
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn resolves_an_eligible_source_config_candidate() {
        let kernel = kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin);
        let adapters = adapters_with_binding();

        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &si(EchoMaterializer),
            &deadline(),
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.agent_type.id().as_str(), "reviewer");
        assert_eq!(candidate.adapter_policy.binding_ref, "primary");
        assert!(candidate
            .evidence
            .enforceable_safety()
            .enforces_workspace(WorkspaceMode::ReadOnly));
    }

    /// Materializer that fails, as an uninstalled source integration would.
    struct RejectingMaterializer;

    impl SourceConfigIntegration for RejectingMaterializer {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            Err(ProvisioningResolutionError::ExternalReferenceNotAttested(
                "no integration".into(),
            ))
        }

        fn prepare(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            Err(ProvisioningResolutionError::ExternalReferenceNotAttested(
                "no integration".into(),
            ))
        }
    }

    /// Materializer that returns an empty attestation.
    struct EmptyMaterializer;

    impl SourceConfigIntegration for EmptyMaterializer {
        fn protocol(&self) -> &str {
            crate::TEST_PROVISIONING_PROTOCOL
        }

        fn attest(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<String, ProvisioningResolutionError> {
            Ok(String::new())
        }

        fn prepare(
            &self,
            _revision: &agentype_storage_sqlite::SourceConfigRevision,
            _adapter_kind: &str,
            _adapter_binding_key: &str,
            _deadline: &AdapterDeadline,
        ) -> Result<PreparedSource, ProvisioningResolutionError> {
            Ok(PreparedSource {
                digest: String::new(),
                descriptor: String::new(),
            })
        }
    }

    #[test]
    fn materialization_receipt_must_match_committed_digest() {
        let d1 = MaterializationDigest::new(
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let d2 = MaterializationDigest::new(
            "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap();
        assert!(require_materialization_receipt(&d1, &d1).is_ok());
        // A TOCTOU between attest and materialize (committed d1, materialized d2)
        // must be rejected, so no Execution can be created, and the message must
        // not echo a source-produced value.
        let err = require_materialization_receipt(&d1, &d2).unwrap_err();
        let message = err.to_string();
        assert!(!message.contains("1111111111111111"));
        assert!(!message.contains("2222222222222222"));
        assert!(matches!(
            err,
            ProvisioningResolutionError::MaterializationMismatch(_)
        ));
    }

    #[test]
    fn fallback_classifier_is_fail_closed() {
        use agentype_agent_contract::ContractError;
        // Durable corruption surfaced through evidence is NOT candidate-local.
        assert!(!is_candidate_local(&ProvisioningResolutionError::Evidence(
            ContractError::InvariantViolation("corrupt".into())
        )));
        // A concrete capability mismatch IS candidate-local.
        assert!(is_candidate_local(&ProvisioningResolutionError::Evidence(
            ContractError::CapabilityMismatch {
                capability: "x".into()
            }
        )));
        // An ambiguous selection is a task-terminal resolution failure, never an
        // agent/source fallback.
        assert!(!is_candidate_local(
            &ProvisioningResolutionError::SelectionAmbiguous("a vs b".into())
        ));
        // Control-plane storage faults are fatal.
        assert!(!is_candidate_local(&ProvisioningResolutionError::Storage(
            agentype_core::Error::StorageFailure("disk".into())
        )));
        assert!(!is_candidate_local(&ProvisioningResolutionError::Storage(
            agentype_core::Error::RecoveryRequired("recover".into())
        )));
        // A global authority-snapshot invalidation (catalog frontier changed, or
        // claim authority expired) must re-resolve the whole acquisition, never
        // fall through to a lower-ranked agent.
        assert!(!is_candidate_local(&ProvisioningResolutionError::Storage(
            agentype_core::Error::StaleAuthority("frontier changed".into())
        )));
        // A per-candidate storage rejection (agent not placeable / not READY) is
        // local.
        assert!(is_candidate_local(&ProvisioningResolutionError::Storage(
            agentype_core::Error::InvalidAuthority("not placeable".into())
        )));
    }

    #[test]
    fn empty_attestation_is_ineligible_before_selection() {
        let kernel = kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin);
        let adapters = adapters_with_binding();
        // An empty attestation is rejected during eligibility, so it never
        // reaches selection and cannot mask a later eligible candidate.
        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &si(EmptyMaterializer),
            &deadline(),
        )
        .unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn materialization_is_required_for_eligibility() {
        let kernel = kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin);
        let adapters = adapters_with_binding();
        // No source integration materializes the config: the candidate is
        // ineligible, so no typed acquisition can proceed.
        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &si(RejectingMaterializer),
            &deadline(),
        )
        .unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn ambiguous_selection_fails_closed() {
        let kernel = kernel();
        let pin = publish(&kernel);
        // Publish a second active config under the same source: two candidates
        // with the same continuity, and no durability model for availability or
        // cost, so the frozen selection order cannot pick a winner.
        let body = json!({"model": "other"});
        let source = SpawnSourceRef::new("source", 1).unwrap();
        let config = SourceConfig {
            config_ref: SourceConfigRef::new(source, "config-2", 1).unwrap(),
            config_digest: ConfigDigest::new(canonical_json_body_digest(&body)).unwrap(),
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: Vec::new(),
            claims: Vec::new(),
            status: ConfigStatus::Active,
        };
        kernel
            .publish_source_config(&config, &SourceConfigBody::OpaqueJson(body))
            .unwrap();
        let task = typed_task(&kernel, pin);
        let adapters = adapters_with_binding();
        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &si(EchoMaterializer),
            &deadline(),
        )
        .unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(matches!(
            select_candidate(&candidates),
            Err(ProvisioningResolutionError::SelectionAmbiguous(_))
        ));
    }

    #[test]
    fn missing_binding_ref_yields_no_candidate() {
        let kernel = kernel();
        let pin = publish(&kernel);
        let task = typed_task(&kernel, pin);
        // Register the kind without the operator alias: no candidate.
        let mut adapters = AdapterRegistry::new();
        adapters
            .register_with_safety(
                "default",
                AdapterBindingKey::new("k1").unwrap(),
                Arc::new(FakeAdapter::new()),
                AdapterDeadlinePolicy::uniform(Duration::from_secs(1)).unwrap(),
                AdapterSafetyEnvelope::unenforceable()
                    .with_enforceable_workspace([WorkspaceMode::ReadOnly])
                    .with_enforceable_network([NetworkEnforcement::Disabled]),
            )
            .unwrap();
        let candidates = resolve_source_candidates(
            &kernel,
            &task,
            &adapters,
            &execution_registry(),
            &si(EchoMaterializer),
            &deadline(),
        )
        .unwrap();
        assert!(candidates.is_empty());
    }
}
