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

    pub fn compile_intent(
        &self,
        generation_id: &GenerationId,
        intent: RawWorkIntent,
        source_kind: &str,
        source_ref: &str,
        compiler_version: u32,
    ) -> Result<ProposalRecord, Error> {
        self.kernel.compile_intent(
            generation_id,
            intent,
            source_kind,
            source_ref,
            compiler_version,
        )
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
}
