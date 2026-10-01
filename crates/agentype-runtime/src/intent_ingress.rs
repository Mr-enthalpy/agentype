//! Proposal-only intent ingress surface.
//!
//! [`IntentIngress`] lets an external harness or managed worker submit a
//! result-backed semantic suggestion without holding any admission, freeze, or
//! close authority. It deliberately exposes exactly one operation and has no
//! method that can mutate the Generation frontier.
//!
//! A caller names a durable `Result` and the `raw_intent_key` of an intent that
//! Result carries. The intent itself is reconstructed from the immutable Result
//! payload by storage, so a caller cannot inject an unrelated intent and label
//! it with a Result it did not come from.

use agentype_core::{Error, GenerationId, ProposalRecord, ResultId};
use agentype_storage_sqlite::Kernel;

pub struct IntentIngress<'a> {
    kernel: &'a Kernel,
}

impl<'a> IntentIngress<'a> {
    pub(crate) fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// Compile a Result-carried intent selected by `(source_result_id,
    /// raw_intent_key)` into a durable `CompiledWorkProposal`.
    ///
    /// The Result MUST exist and MUST carry the requested intent; otherwise the
    /// compile fails closed. This grants no admission authority: the proposal
    /// remains `PENDING` until Root admits it.
    pub fn compile_from_result(
        &self,
        generation_id: &GenerationId,
        source_result_id: &ResultId,
        raw_intent_key: &str,
        compiler_version: u32,
    ) -> Result<ProposalRecord, Error> {
        self.kernel.compile_result_intent(
            generation_id,
            source_result_id,
            raw_intent_key,
            compiler_version,
        )
    }
}
