//! M6-B.3 pure matching of **already-bound** LogicalAgents to a Task's agent
//! requirement.
//!
//! This stage does not provision anything: a new `LogicalAgent` is materialized
//! only by M6-B.4. Matching answers "which existing, immediately usable typed
//! agent may execute this Task", composing two independent eligibility layers:
//!
//! 1. the frozen M5 placement rules (exact partition, concrete tag superset,
//!    `Required` continuity workstream gate), and
//! 2. the M6-B AgentType contract (`can_execute`).
//!
//! Both are hard gates. A candidate that fails either is not returned. A
//! candidate's realized M5 retention is deliberately NOT interpreted against the
//! AgentType `lifecycle` set: that set is a **required source envelope** (spec
//! 06), not a per-instance mode whitelist, so matching it against a realized
//! retention would be both unnecessary and insufficient. Realized-lifecycle
//! eligibility, if ever needed, is a B.4 provisioning concern.
//!
//! Ranking follows the frozen spec 06 matching preference: an exact pin is
//! preferred, then compatible narrower/refinement types, then compatible
//! broader/general types. A different revision of the same `type_id` is never a
//! substitute (`D-TYPE-REV-COMPAT` is deferred), and a type that is neither a
//! strict refinement nor strictly broader is not eligible. Within a class,
//! candidate-vs-candidate specificity (`more_specific_for`) is applied as
//! dominance-count layers, then the `Preferred` workstream placement, continuity
//! strength, availability, and a stable identity tie-break. Nominal inheritance
//! depth is never consulted.
//!
//! Only M5 `READY`, unassigned agents are candidates (the storage loader filters
//! to that); the "cold/revivable" tier is deliberately not approximated from
//! non-READY M5 states.

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

/// One existing, immediately usable (M5 `READY` and unassigned) LogicalAgent,
/// resolved to its exact bound AgentType and its M5 placement facts.
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

/// Relation of a candidate's type to the requirement's exact pin.
///
/// `None` means the candidate is not a valid substitute: a different revision of
/// the pinned `type_id` (`D-TYPE-REV-COMPAT` is deferred, so there is no
/// automatic cross-revision compatibility), or a type that is neither a strict
/// refinement nor strictly broader. Only strict dominance or the exact pin is a
/// defined direction; genuinely incomparable/equivalent types are not eligible in
/// v1 rather than collapsed into an invented ranking tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Relation {
    Exact,
    Narrower,
    Broader,
}

fn relation(
    candidate: &AgentType,
    required: &AgentType,
    req: &TaskAgentRequirement,
    catalog: &CapabilityCatalog,
) -> Option<Relation> {
    if candidate.type_ref == required.type_ref {
        return Some(Relation::Exact);
    }
    if candidate.type_ref.id() == required.type_ref.id() {
        return None;
    }
    if more_specific_for(candidate, required, &req.hard, catalog) {
        return Some(Relation::Narrower);
    }
    if more_specific_for(required, candidate, &req.hard, catalog) {
        return Some(Relation::Broader);
    }
    None
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

/// Filter and rank existing bound, immediately usable agents for one Task,
/// best first. Physical eligibility (adapter binding, credentials) is M6-B.4 and
/// is NOT decided here.
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

    let eligible: Vec<(Relation, ExistingAgentCandidate)> = candidates
        .iter()
        .filter(|candidate| {
            placement_eligible(candidate, placement)
                && can_execute(&candidate.agent_type, &req.hard, catalog).is_ok()
        })
        .filter_map(|candidate| {
            relation(&candidate.agent_type, required, req, catalog)
                .map(|relation| (relation, candidate.clone()))
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
