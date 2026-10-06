//! M6-B.3 operator provisioning surface: bind existing LogicalAgents to exact
//! AgentType revisions and read typed requirements / match results.
//!
//! This is deliberately separate from [`crate::RootSemanticControl`]: binding an
//! agent is operator provisioning authority, not semantic admission authority
//! (`M6A-B9`). It cannot create a Task, admit a proposal, or expand a Generation,
//! and it never starts physical work.

use agentype_agent_contract::{AgentTypeRef, ExistingAgentCandidate, TaskAgentRequirement};
use agentype_core::{Error, LogicalAgentId, TaskId};
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

    /// Rank existing, bound LogicalAgents for a Task's requirement.
    ///
    /// This is **non-authoritative semantic candidate preselection**: it proves
    /// only semantic compatibility (`can_execute`) and M5 placement, never that an
    /// agent's current Incarnation, source, config, credentials, or adapter binding
    /// can physically execute the Task. It provably cannot provision a new agent
    /// (no `SpawnSource` is consulted) and grants no Task/Attempt/Lease/Execution
    /// authority.
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
