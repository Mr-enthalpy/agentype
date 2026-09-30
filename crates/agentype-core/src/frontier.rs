//! Core domain types, records, and predicates for M6-A Semantic Frontier Kernel.
//!
//! This module MUST NOT depend on SQLite, Tokio, CLI, or filesystem.

use crate::ids::{GenerationId, ProposalId, ResultId, TaskId};
use crate::records::TaskSpec;
use crate::states::{
    GenerationState, InformationFunction, ProposalExpirationReason, ProposalStateKind, TaskState,
};
use crate::UnixTime;
use serde_json::Value;
use std::collections::HashMap;

pub const GENERATION_FROZEN: &str = "GENERATION_FROZEN";
pub const GENERATION_CLOSED: &str = "GENERATION_CLOSED";

/// Immutable content-addressed artifact reference.
///
/// Requires an immutable content digest (e.g. `sha256:...`) in addition to
/// an opaque storage locator so that the evidence snapshot identity cannot drift.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactRef {
    pub locator: String,
    pub digest: String,
}

impl ArtifactRef {
    pub fn new(
        locator: impl Into<String>,
        digest: impl Into<String>,
    ) -> Result<Self, crate::Error> {
        let locator = locator.into();
        let digest = digest.into();
        if locator.trim().is_empty() {
            return Err(crate::Error::invariant("artifact locator cannot be empty"));
        }
        if digest.trim().is_empty() {
            return Err(crate::Error::invariant(
                "artifact digest cannot be empty (provenance requires immutable content digest)",
            ));
        }
        Ok(Self { locator, digest })
    }
}

/// Immutable set of semantic evidence/seed inputs captured at admission time.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SemanticInputSet {
    pub result_ids: Vec<ResultId>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub seed_refs: Vec<String>,
}

impl SemanticInputSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_result(mut self, result_id: ResultId) -> Self {
        self.result_ids.push(result_id);
        self
    }

    pub fn with_artifact(mut self, artifact: ArtifactRef) -> Self {
        self.artifact_refs.push(artifact);
        self
    }

    pub fn with_seed(mut self, seed_ref: impl Into<String>) -> Self {
        self.seed_refs.push(seed_ref.into());
        self
    }
}

/// Unauthoritative semantic suggestion submitted from workers, Root, or external harnesses.
#[derive(Clone, Debug, PartialEq)]
pub struct RawWorkIntent {
    pub raw_intent_key: String,
    pub objective: String,
    pub information_function: InformationFunction,
    pub semantic_input_set: SemanticInputSet,
    pub rationale: Option<String>,
    pub suggested_task_spec: Option<TaskSpec>,
}

impl RawWorkIntent {
    /// Deterministic canonical fingerprint of this intent's semantic content.
    pub fn fingerprint(&self) -> Result<String, crate::Error> {
        let mut sorted_res = self
            .semantic_input_set
            .result_ids
            .iter()
            .map(|r| r.as_str().to_string())
            .collect::<Vec<_>>();
        sorted_res.sort();

        let mut sorted_art = self.semantic_input_set.artifact_refs.clone();
        sorted_art.sort();
        let sorted_art_json = sorted_art
            .iter()
            .map(|a| serde_json::json!({ "locator": &a.locator, "digest": &a.digest }))
            .collect::<Vec<_>>();

        let mut sorted_seed = self.semantic_input_set.seed_refs.clone();
        sorted_seed.sort();

        let spec_val = match self.suggested_task_spec.as_ref() {
            Some(s) => Some(s.canonical_json()?),
            None => None,
        };

        let val = serde_json::json!({
            "key": self.raw_intent_key,
            "obj": self.objective,
            "fn": self.information_function.as_sql(),
            "rat": self.rationale,
            "inputs": {
                "results": sorted_res,
                "artifacts": sorted_art_json,
                "seeds": sorted_seed,
            },
            "spec": spec_val,
        });
        Ok(val.to_string())
    }
}

/// Durable compiled candidate proposal validated and normalized by deterministic compiler.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalRecord {
    pub proposal_id: ProposalId,
    pub generation_id: GenerationId,
    pub source_kind: String,
    pub source_ref: String,
    pub raw_intent_key: String,
    pub intent_fingerprint: String,
    pub objective: String,
    pub rationale: Option<String>,
    pub information_function: InformationFunction,
    pub normalized_task_spec: Option<TaskSpec>,
    pub semantic_input_set: SemanticInputSet,
    pub compiler_version: u32,
    pub state: ProposalStateKind,
    pub admitted_task_id: Option<TaskId>,
    pub expiration_reason: Option<ProposalExpirationReason>,
    pub rejection_reason: Option<String>,
    pub created_at: UnixTime,
    pub updated_at: UnixTime,
}

/// Durable semantic admission frontier record.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationRecord {
    pub generation_id: GenerationId,
    pub state: GenerationState,
    pub revision: u64,
    pub admission_seq: u64,
    pub seed_payload: Value,
    pub created_at: UnixTime,
    pub frozen_at: Option<UnixTime>,
    pub closed_at: Option<UnixTime>,
}

/// Durable binding connecting an M6 semantic Task to its unique Generation.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationTaskBindingRecord {
    pub generation_id: GenerationId,
    pub task_id: TaskId,
    pub proposal_id: ProposalId,
    pub information_function: InformationFunction,
    pub admission_seq: u64,
    pub admitted_task_spec: TaskSpec,
    pub semantic_input_set: SemanticInputSet,
    pub created_at: UnixTime,
}

/// Snapshot of a task's terminal disposition for Settled evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSettledSnapshot {
    pub task_id: TaskId,
    pub is_terminal: bool,
}

/// Derived rebuildable view of a Generation's semantic frontier state.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationView {
    pub generation: GenerationRecord,
    pub is_settled: bool,
    pub task_count_by_info_fn: HashMap<InformationFunction, usize>,
    pub task_count_by_state: HashMap<TaskState, usize>,
    pub pending_proposal_ids: Vec<ProposalId>,
    pub admitted_task_ids: Vec<TaskId>,
    pub expired_proposal_ids: Vec<ProposalId>,
    pub rejected_proposal_ids: Vec<ProposalId>,
}

/// Predicate: determines whether a generation in `state` admits a task with `info_fn`.
///
/// INV-A7: No EXPAND task may be admitted after Generation enters FROZEN.
/// INV-A8: No task of any InformationFunction may be admitted after CLOSED.
pub fn generation_allows_admit(state: GenerationState, info_fn: InformationFunction) -> bool {
    match state {
        GenerationState::Open => true,
        GenerationState::Frozen => matches!(
            info_fn,
            InformationFunction::CompressPositive | InformationFunction::CompressNegative
        ),
        GenerationState::Closed => false,
    }
}

/// Predicate: strictly monotonic transitions OPEN -> FROZEN -> CLOSED.
pub fn can_transition_generation(from: GenerationState, to: GenerationState) -> bool {
    matches!(
        (from, to),
        (GenerationState::Open, GenerationState::Frozen)
            | (GenerationState::Frozen, GenerationState::Closed)
    )
}

/// Predicate: GenerationSettled(G).
///
/// A generation is settled iff it is FROZEN and every admitted Task has reached
/// an M5 terminal disposition.
pub fn is_generation_settled(state: GenerationState, tasks: &[TaskSettledSnapshot]) -> bool {
    state == GenerationState::Frozen && tasks.iter().all(|t| t.is_terminal)
}

/// Predicate: terminal projection for the `GenerationView.is_settled` read model.
///
/// `is_generation_settled` is the *close gate*: it is deliberately false once a
/// generation has advanced to `CLOSED`. The read model instead reports the
/// natural-language property "this frontier no longer has unsettled admitted
/// work", which is true for a settled `FROZEN` generation and remains true for
/// its terminal `CLOSED` successor.
pub fn is_generation_view_settled(state: GenerationState, tasks: &[TaskSettledSnapshot]) -> bool {
    state == GenerationState::Closed || is_generation_settled(state, tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_admit_predicate() {
        // OPEN allows all three information functions
        assert!(generation_allows_admit(
            GenerationState::Open,
            InformationFunction::Expand
        ));
        assert!(generation_allows_admit(
            GenerationState::Open,
            InformationFunction::CompressPositive
        ));
        assert!(generation_allows_admit(
            GenerationState::Open,
            InformationFunction::CompressNegative
        ));

        // FROZEN forbids EXPAND, allows COMPRESS_*
        assert!(!generation_allows_admit(
            GenerationState::Frozen,
            InformationFunction::Expand
        ));
        assert!(generation_allows_admit(
            GenerationState::Frozen,
            InformationFunction::CompressPositive
        ));
        assert!(generation_allows_admit(
            GenerationState::Frozen,
            InformationFunction::CompressNegative
        ));

        // CLOSED forbids all
        assert!(!generation_allows_admit(
            GenerationState::Closed,
            InformationFunction::Expand
        ));
        assert!(!generation_allows_admit(
            GenerationState::Closed,
            InformationFunction::CompressPositive
        ));
        assert!(!generation_allows_admit(
            GenerationState::Closed,
            InformationFunction::CompressNegative
        ));
    }

    #[test]
    fn test_monotonic_generation_transitions() {
        assert!(can_transition_generation(
            GenerationState::Open,
            GenerationState::Frozen
        ));
        assert!(can_transition_generation(
            GenerationState::Frozen,
            GenerationState::Closed
        ));

        assert!(!can_transition_generation(
            GenerationState::Open,
            GenerationState::Closed
        ));
        assert!(!can_transition_generation(
            GenerationState::Frozen,
            GenerationState::Open
        ));
        assert!(!can_transition_generation(
            GenerationState::Closed,
            GenerationState::Frozen
        ));
    }

    #[test]
    fn test_generation_settled_predicate() {
        let t1 = TaskSettledSnapshot {
            task_id: TaskId::new(),
            is_terminal: true,
        };
        let t2 = TaskSettledSnapshot {
            task_id: TaskId::new(),
            is_terminal: false,
        };

        // If OPEN, never settled
        assert!(!is_generation_settled(
            GenerationState::Open,
            std::slice::from_ref(&t1)
        ));

        // If FROZEN with non-terminal tasks, not settled
        assert!(!is_generation_settled(
            GenerationState::Frozen,
            &[t1.clone(), t2.clone()]
        ));

        // If FROZEN with all terminal tasks, settled
        assert!(is_generation_settled(
            GenerationState::Frozen,
            std::slice::from_ref(&t1)
        ));

        // Empty task set when FROZEN is settled
        assert!(is_generation_settled(GenerationState::Frozen, &[]));
    }

    #[test]
    fn test_generation_view_settled_projection() {
        let terminal = TaskSettledSnapshot {
            task_id: TaskId::new(),
            is_terminal: true,
        };
        let open_task = TaskSettledSnapshot {
            task_id: TaskId::new(),
            is_terminal: false,
        };

        // A settled FROZEN generation is reported settled in the view.
        assert!(is_generation_view_settled(
            GenerationState::Frozen,
            std::slice::from_ref(&terminal)
        ));
        // An unsettled FROZEN generation stays unsettled.
        assert!(!is_generation_view_settled(
            GenerationState::Frozen,
            std::slice::from_ref(&open_task)
        ));
        // CLOSED is terminal and always reports settled, even though the close
        // gate predicate itself is false there.
        assert!(is_generation_view_settled(GenerationState::Closed, &[]));
        assert!(is_generation_view_settled(
            GenerationState::Closed,
            std::slice::from_ref(&terminal)
        ));
        // OPEN is never settled.
        assert!(!is_generation_view_settled(GenerationState::Open, &[]));
    }
}
