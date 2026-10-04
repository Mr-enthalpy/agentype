//! M6-B.3 operator provisioning surface: bind existing LogicalAgents to exact
//! AgentType revisions and read typed requirements / match results.
//!
//! This is deliberately separate from [`crate::RootSemanticControl`]: binding an
//! agent is operator provisioning authority, not semantic admission authority
//! (`M6A-B9`). It cannot create a Task, admit a proposal, or expand a Generation,
//! and it never starts physical work.

use agentype_agent_contract::{
    AgentTypeRef, ExistingAgentCandidate, GenerationPolicy, TaskAgentRequirement,
};
use agentype_core::{Error, GenerationId, LogicalAgentId, TaskId};
use agentype_storage_sqlite::Kernel;

/// Operator authority over durable agent-type bindings and typed reads.
pub struct ProvisioningAdmin<'a> {
    kernel: &'a Kernel,
}

impl<'a> ProvisioningAdmin<'a> {
    pub(crate) fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// Bind an existing LogicalAgent to an exact AgentType revision. Write-once
    /// and no physical provisioning: the agent must already exist.
    ///
    /// ```compile_fail
    /// fn _no_semantic_admit_through_admin(
    ///     admin: &agentype_runtime::ProvisioningAdmin<'_>,
    /// ) {
    ///     let _ = admin.admit_proposal;
    /// }
    /// ```
    pub fn bind_logical_agent_type(
        &self,
        agent_id: &LogicalAgentId,
        type_ref: &AgentTypeRef,
    ) -> Result<(), Error> {
        self.kernel.bind_logical_agent_type(agent_id, type_ref)
    }

    pub fn task_agent_requirement(
        &self,
        task_id: &TaskId,
    ) -> Result<Option<TaskAgentRequirement>, Error> {
        self.kernel.get_task_agent_requirement(task_id)
    }

    pub fn logical_agent_type_binding(
        &self,
        agent_id: &LogicalAgentId,
    ) -> Result<Option<AgentTypeRef>, Error> {
        self.kernel.get_logical_agent_type_binding(agent_id)
    }

    pub fn generation_policy(
        &self,
        generation_id: &GenerationId,
    ) -> Result<Option<GenerationPolicy>, Error> {
        self.kernel.get_generation_policy(generation_id)
    }

    /// Rank existing, bound LogicalAgents for a Task's requirement. Pure read:
    /// it provably cannot provision a new agent (no SpawnSource is consulted).
    pub fn match_existing_agents_for_task(
        &self,
        task_id: &TaskId,
    ) -> Result<Vec<ExistingAgentCandidate>, Error> {
        self.kernel.match_existing_agents_for_task(task_id)
    }
}

impl std::fmt::Debug for ProvisioningAdmin<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisioningAdmin").finish_non_exhaustive()
    }
}
