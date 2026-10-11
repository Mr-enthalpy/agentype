//! M6-B operator **catalog administration** surface.
//!
//! B.2 froze the durable Agent Contract catalog behind validated, immutable
//! publication transactions on the internal `Kernel`. M6-B.3 promotes typed
//! admission and LogicalAgent type binding onto the supported runtime surface,
//! and those operations require a `PUBLISHED` AgentType (and its capability
//! definitions) to exist. A supported consumer must be able to establish that
//! durable precondition without reaching into the internal storage crate, so this
//! narrow surface delegates exclusively to the B.2 validated publication
//! transactions.
//!
//! It is operator/provisioning authority only. It cannot admit a proposal,
//! create a Task, claim work, renew a Lease, ACK/NACK a worker, start an
//! Execution, or freeze/close a Generation. Source/config/adapter-policy
//! administration remains out of scope until M6-B.4.

use agentype_agent_contract::{
    AdapterBindingPolicy, AdapterPolicyRef, AgentType, AgentTypeRef, CapabilityDefinition,
    CapabilityRef, ConfigStatus, SourceConfig, SourceConfigRef, SourceStatus, SpawnSource,
    SpawnSourceRef,
};
use agentype_core::Error;
use agentype_storage_sqlite::{AgentTypeStatus, Kernel, SourceConfigBody};

/// Operator authority over the durable Agent Contract catalog.
pub struct CatalogAdmin<'a> {
    kernel: &'a Kernel,
}

impl<'a> CatalogAdmin<'a> {
    pub(crate) fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// Publish an immutable capability definition revision (B.2 canonical).
    pub fn publish_capability_definition(
        &self,
        reference: &CapabilityRef,
        definition: &CapabilityDefinition,
    ) -> Result<String, Error> {
        self.kernel
            .publish_capability_definition(reference, definition)
    }

    /// Publish an immutable AgentType revision (B.2 canonical, refinement
    /// validated).
    pub fn publish_agent_type(&self, agent: &AgentType) -> Result<String, Error> {
        self.kernel.publish_agent_type(agent)
    }

    /// Deprecate an AgentType revision. Existing committed pins remain valid;
    /// only new pre-commit selection is affected.
    ///
    /// ```compile_fail
    /// fn _catalog_cannot_admit(admin: &agentype_runtime::CatalogAdmin<'_>) {
    ///     let _ = admin.admit_proposal;
    /// }
    /// ```
    pub fn deprecate_agent_type(&self, reference: &AgentTypeRef) -> Result<(), Error> {
        self.kernel
            .set_agent_type_status(reference, AgentTypeStatus::Deprecated)
    }

    /// Publish an immutable `AdapterBindingPolicy` revision (M6-B.2 canonical).
    pub fn publish_adapter_binding_policy(
        &self,
        policy: &AdapterBindingPolicy,
    ) -> Result<String, Error> {
        self.kernel.publish_adapter_binding_policy(policy)
    }

    /// Publish an immutable `SpawnSource` revision (M6-B.2 canonical).
    pub fn publish_spawn_source(&self, source: &SpawnSource) -> Result<String, Error> {
        self.kernel.publish_spawn_source(source)
    }

    /// Publish an immutable `SourceConfig` revision (M6-B.2 canonical).
    pub fn publish_source_config(
        &self,
        config: &SourceConfig,
        body: &SourceConfigBody,
    ) -> Result<String, Error> {
        self.kernel.publish_source_config(config, body)
    }

    /// Advance a SpawnSource revision's operational disposition.
    pub fn set_spawn_source_status(
        &self,
        reference: &SpawnSourceRef,
        status: SourceStatus,
    ) -> Result<(), Error> {
        self.kernel.set_spawn_source_status(reference, status)
    }

    /// Advance a SourceConfig revision's operational disposition.
    pub fn set_source_config_status(
        &self,
        reference: &SourceConfigRef,
        status: ConfigStatus,
    ) -> Result<(), Error> {
        self.kernel.set_source_config_status(reference, status)
    }

    /// Advance an AdapterBindingPolicy revision's operational disposition.
    pub fn set_adapter_binding_policy_status(
        &self,
        reference: &AdapterPolicyRef,
        status: ConfigStatus,
    ) -> Result<(), Error> {
        self.kernel
            .set_adapter_binding_policy_status(reference, status)
    }
}

impl std::fmt::Debug for CatalogAdmin<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogAdmin").finish_non_exhaustive()
    }
}
