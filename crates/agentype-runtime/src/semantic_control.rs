//! Root semantic control surface over one running Scheduler daemon.
//!
//! [`RootSemanticControl`] provides Root with explicit, durable operations
//! for managing the semantic admission frontier: creating generations,
//! compiling intents, admitting proposals, and closing settled generations.
//!
//! Worker acknowledgement, mechanical claims, and lease renewals are deliberately
//! absent from this surface.

use agentype_core::{
    Error, GenerationId, GenerationRecord, GenerationView, ProposalId, ProposalRecord,
    RawWorkIntent, TaskId, TaskSpec,
};
use agentype_storage_sqlite::Kernel;
use serde_json::Value;

pub struct RootSemanticControl<'a> {
    kernel: &'a Kernel,
}

impl<'a> RootSemanticControl<'a> {
    pub(crate) fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    pub fn create_generation(&self, seed_payload: Value) -> Result<GenerationRecord, Error> {
        self.kernel.create_generation(seed_payload)
    }

    /// Compile a Root-originated intent. Root authority is the only path that
    /// may originate an intent outside a durable Result.
    pub fn compile_root_intent(
        &self,
        generation_id: &GenerationId,
        intent: RawWorkIntent,
        command_ref: &str,
        compiler_version: u32,
    ) -> Result<ProposalRecord, Error> {
        self.kernel
            .compile_root_intent(generation_id, intent, command_ref, compiler_version)
    }

    pub fn admit_proposal(
        &self,
        proposal_id: &ProposalId,
        expected_generation_revision: u64,
        override_task_spec: Option<TaskSpec>,
    ) -> Result<TaskId, Error> {
        self.kernel.admit_proposal(
            proposal_id,
            expected_generation_revision,
            override_task_spec,
        )
    }

    /// Admit a proposal with an exact-revision agent requirement (M6-B.3).
    ///
    /// The requirement is created atomically with the Task and its
    /// GenerationTaskBinding. No `SpawnSource` is selected and no physical work
    /// starts; existing-agent matching is a separate, pure read.
    pub fn admit_typed_proposal(
        &self,
        proposal_id: &ProposalId,
        expected_generation_revision: u64,
        override_task_spec: Option<TaskSpec>,
        agent_requirement: agentype_agent_contract::AgentRequirementDraft,
    ) -> Result<TaskId, Error> {
        self.kernel.admit_typed_proposal(
            proposal_id,
            expected_generation_revision,
            override_task_spec,
            agent_requirement,
        )
    }

    /// Create a Generation with an immutable policy ceiling (M6-B.3
    /// `D-GEN-POLICY`). The policy folds into every typed admission as a
    /// generation-wide hard requirement.
    pub fn create_generation_with_policy(
        &self,
        seed_payload: serde_json::Value,
        policy: Option<agentype_agent_contract::GenerationPolicy>,
    ) -> Result<GenerationRecord, Error> {
        self.kernel
            .create_generation_with_policy(seed_payload, policy)
    }

    /// Read the immutable policy a Generation was created with, if any.
    ///
    /// A Generation policy is semantic-frontier state owned by Root, so its read
    /// lives here rather than on the provisioning surface.
    pub fn read_generation_policy(
        &self,
        generation_id: &GenerationId,
    ) -> Result<Option<agentype_agent_contract::GenerationPolicy>, Error> {
        self.kernel.get_generation_policy(generation_id)
    }

    pub fn freeze_generation(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
    ) -> Result<(), Error> {
        self.kernel
            .freeze_generation(generation_id, expected_revision)
    }

    pub fn close_generation(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
    ) -> Result<(), Error> {
        self.kernel
            .close_generation(generation_id, expected_revision)
    }

    pub fn reject_proposal(&self, proposal_id: &ProposalId, reason: &str) -> Result<(), Error> {
        self.kernel.reject_proposal(proposal_id, reason)
    }

    pub fn read_generation_view(
        &self,
        generation_id: &GenerationId,
    ) -> Result<GenerationView, Error> {
        self.kernel.get_generation_view(generation_id)
    }

    pub fn read_proposal(&self, proposal_id: &ProposalId) -> Result<ProposalRecord, Error> {
        self.kernel.get_proposal(proposal_id)
    }
}
