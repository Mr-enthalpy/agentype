//! Proposal-only intent ingress surface.
//!
//! [`IntentIngress`] lets an external harness or managed worker submit a
//! result-backed semantic suggestion without holding any admission, freeze, or
//! close authority. It deliberately exposes exactly one operation and has no
//! method that can mutate the Generation frontier.
//!
//! A caller only supplies the durable `Result` that anchors the suggestion;
//! the source identity is derived and verified by storage, so a worker cannot
//! forge a free-form provenance string.

use agentype_core::{Error, GenerationId, IntentSource, ProposalRecord, RawWorkIntent, ResultId};
use agentype_storage_sqlite::Kernel;

pub struct IntentIngress<'a> {
    kernel: &'a Kernel,
}

impl<'a> IntentIngress<'a> {
    pub(crate) fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// Compile a result-backed intent into a durable `CompiledWorkProposal`.
    ///
    /// The `source_result_id` MUST resolve to a durable Result; otherwise the
    /// compile fails closed. This grants no admission authority: the proposal
    /// remains `PENDING` until Root admits it.
    pub fn compile_from_result(
        &self,
        generation_id: &GenerationId,
        source_result_id: &ResultId,
        intent: RawWorkIntent,
        compiler_version: u32,
    ) -> Result<ProposalRecord, Error> {
        let source = IntentSource::result(source_result_id.clone());
        self.kernel
            .compile_intent(generation_id, intent, source, compiler_version)
    }
}
