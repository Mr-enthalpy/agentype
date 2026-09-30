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

/// Immutable set of semantic evidence/seed inputs captured at admission time.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SemanticInputSet {
    pub result_ids: Vec<ResultId>,
    pub artifact_refs: Vec<String>,
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

    pub fn with_artifact(mut self, artifact_ref: impl Into<String>) -> Self {
        self.artifact_refs.push(artifact_ref.into());
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

/// Durable compiled candidate proposal validated and normalized by deterministic compiler.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalRecord {
    pub proposal_id: ProposalId,
    pub generation_id: GenerationId,
    pub source_kind: String,
    pub source_ref: String,
    pub raw_intent_key: String,
    pub information_function: InformationFunction,
    pub normalized_task_spec: TaskSpec,
    pub semantic_input_set: SemanticInputSet,
    pub compiler_version: u32,
    pub state: ProposalStateKind,
    pub admitted_task_id: Option<TaskId>,
    pub expiration_reason: Option<ProposalExpirationReason>,
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
}
