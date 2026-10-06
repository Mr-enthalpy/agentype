//! M6-B.3 pure **semantic candidate preselection** for existing LogicalAgents.
//!
//! This is a non-authoritative read: it produces a ranked list of existing,
//! already-bound agents that are semantically compatible with a Task, and it
//! grants no Task/Attempt/Lease/Execution authority and no physical eligibility.
//! A result here does NOT mean an agent's current Incarnation, source, config, or
//! credentials can actually execute the Task — M6-B.4/M6-B.5 must still prove
//! `can_provision_task` and the remaining physical eligibility before any
//! authority-bearing acquisition. `match_existing_agents(...)` MUST NOT be
//! consumed as "runnable agent".
//!
//! Eligibility composes:
//!
//! 1. the frozen M5 placement rules (exact partition, concrete tag superset,
//!    `Required` continuity workstream gate), and
//! 2. the M6-B AgentType contract (`can_execute`),
//!
//! plus an explicit rule that a different revision of the same `type_id` is never
//! a substitute (`D-TYPE-REV-COMPAT` is deferred). `more_specific_for` is a
//! **preference relation only**; it orders candidates and never decides
//! eligibility, so a semantically compatible type that is neither a strict
//! refinement nor strictly broader is still a candidate (ranked last).
//!
//! A candidate's realized M5 retention is deliberately NOT interpreted against
//! the AgentType `lifecycle` set: that set is a **required source envelope**
//! (spec 06), not a per-instance mode whitelist. Only M5 `READY`, unassigned
//! agents are considered (the storage loader filters to that); the
//! "cold/revivable" tier is deliberately not approximated from non-READY states.

use crate::capability::CapabilityCatalog;
use crate::error::ContractError;
use crate::predicates::{can_execute, more_specific_for};
use crate::records::AgentType;
use crate::requirement::TaskAgentRequirement;
use agentype_core::{
    claim_tiebreak, ContinuityPreference, LogicalAgentId, PartitionId, WorkstreamId,
};
use std::cmp::Ordering;
use std::collections::BTreeSet;

/// One existing, `READY`, unassigned LogicalAgent that is a **non-authoritative
/// semantic candidate**: semantically compatible with the Task and M5-placeable.
/// This is not a runnable/execution-eligible agent; physical eligibility is
/// M6-B.4/M6-B.5.
#[derive(Clone, Debug, PartialEq)]
pub struct ExistingAgentCandidate {
    pub logical_agent_id: LogicalAgentId,
    pub agent_type: AgentType,
    pub partition: PartitionId,
    pub tags: BTreeSet<String>,
    pub workstream_id: Option<WorkstreamId>,
    pub available_since: Option<f64>,
    pub created_at: f64,
}

/// The M5 placement facts a claimable Task demands.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskPlacement {
    pub partition: PartitionId,
    pub required_tags: BTreeSet<String>,
    pub workstream_id: Option<WorkstreamId>,
    pub continuity: ContinuityPreference,
}

/// Ranking class of a candidate's type relative to the requirement's exact pin.
///
/// This is a **preference** class, not an eligibility gate. `OtherCompatible`
/// covers semantically compatible types that are neither a strict refinement nor
/// strictly broader than the pin (including equivalent-authority types with a
/// different `type_id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Relation {
    Exact,
    Narrower,
    Broader,
    OtherCompatible,
}

fn relation(
    candidate: &AgentType,
    required: &AgentType,
    req: &TaskAgentRequirement,
    catalog: &CapabilityCatalog,
) -> Relation {
    if candidate.type_ref == required.type_ref {
        return Relation::Exact;
    }
    if more_specific_for(candidate, required, &req.hard, catalog) {
        return Relation::Narrower;
    }
    if more_specific_for(required, candidate, &req.hard, catalog) {
        return Relation::Broader;
    }
    Relation::OtherCompatible
}

/// The one explicit eligibility exclusion beyond placement + `can_execute`: a
/// different revision of the pinned `type_id` is never a substitute while
/// `D-TYPE-REV-COMPAT` is deferred.
fn is_cross_revision_of_pin(candidate: &AgentType, required: &AgentType) -> bool {
    candidate.type_ref != required.type_ref && candidate.type_ref.id() == required.type_ref.id()
}

fn strictly_more_specific(
    a: &AgentType,
    b: &AgentType,
    req: &TaskAgentRequirement,
    catalog: &CapabilityCatalog,
) -> bool {
    a.type_ref != b.type_ref
        && more_specific_for(a, b, &req.hard, catalog)
        && !more_specific_for(b, a, &req.hard, catalog)
}

fn same_workstream(candidate: &ExistingAgentCandidate, placement: &TaskPlacement) -> bool {
    placement.workstream_id.is_some() && candidate.workstream_id == placement.workstream_id
}

/// Frozen M5 placement eligibility, mirrored from `core::select_claim_agent`.
fn placement_eligible(candidate: &ExistingAgentCandidate, placement: &TaskPlacement) -> bool {
    if candidate.partition != placement.partition {
        return false;
    }
    if !placement
        .required_tags
        .iter()
        .all(|tag| candidate.tags.contains(tag))
    {
        return false;
    }
    placement.continuity != ContinuityPreference::Required || same_workstream(candidate, placement)
}

/// Rank existing bound, `READY`, unassigned agents for one Task, best first.
///
/// The result is non-authoritative semantic preselection: physical eligibility
/// (SpawnSource, config, adapter binding, credentials, enforcement evidence) is
/// M6-B.4/M6-B.5 and is NOT decided here.
pub fn match_existing_agents(
    required: &AgentType,
    req: &TaskAgentRequirement,
    placement: &TaskPlacement,
    candidates: &[ExistingAgentCandidate],
    catalog: &CapabilityCatalog,
) -> Result<Vec<ExistingAgentCandidate>, ContractError> {
    if req.required_type != required.type_ref {
        return Err(ContractError::InvariantViolation(format!(
            "match target {}@{} does not match the requirement's pinned type",
            required.type_ref.id().as_str(),
            required.type_ref.revision()
        )));
    }

    // Eligibility is M5 placement + `can_execute` + the explicit cross-revision
    // exclusion. `more_specific_for` never removes a compatible candidate.
    let eligible: Vec<(Relation, ExistingAgentCandidate)> = candidates
        .iter()
        .filter(|candidate| {
            placement_eligible(candidate, placement)
                && can_execute(&candidate.agent_type, &req.hard, catalog).is_ok()
                && !is_cross_revision_of_pin(&candidate.agent_type, required)
        })
        .map(|candidate| {
            (
                relation(&candidate.agent_type, required, req, catalog),
                candidate.clone(),
            )
        })
        .collect();

    let domination: Vec<usize> = eligible
        .iter()
        .map(|(relation, candidate)| {
            eligible
                .iter()
                .filter(|(other_relation, other)| {
                    other_relation == relation
                        && strictly_more_specific(
                            &other.agent_type,
                            &candidate.agent_type,
                            req,
                            catalog,
                        )
                })
                .count()
        })
        .collect();

    let mut indexed: Vec<(Relation, usize, ExistingAgentCandidate)> = eligible
        .into_iter()
        .zip(domination)
        .map(|((relation, candidate), domination)| (relation, domination, candidate))
        .collect();

    indexed.sort_by(|(a_rel, a_dom, a), (b_rel, b_dom, b)| {
        a_rel
            .cmp(b_rel)
            .then_with(|| a_dom.cmp(b_dom))
            // `Preferred` (and `Required`, already gated) prefers the same
            // workstream, mirroring `core::claim_selection_rank`.
            .then_with(|| {
                let rank = |c: &ExistingAgentCandidate| -> u8 {
                    if placement.continuity != ContinuityPreference::None
                        && !same_workstream(c, placement)
                    {
                        1
                    } else {
                        0
                    }
                };
                rank(a).cmp(&rank(b))
            })
            // Stronger continuity guarantee first.
            .then_with(|| {
                b.agent_type
                    .contract
                    .continuity
                    .cmp(&a.agent_type.contract.continuity)
            })
            // Frozen M5 availability/identity tie-break parity: effective
            // availability (`available_since` else `created_at`) then lowest
            // LogicalAgent id. Reused directly, not re-derived.
            .then_with(|| {
                let (a_avail, a_id) =
                    claim_tiebreak(a.available_since, a.created_at, a.logical_agent_id.as_str());
                let (b_avail, b_id) =
                    claim_tiebreak(b.available_since, b.created_at, b.logical_agent_id.as_str());
                a_avail
                    .partial_cmp(&b_avail)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a_id.cmp(b_id))
            })
    });

    Ok(indexed
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect())
}
