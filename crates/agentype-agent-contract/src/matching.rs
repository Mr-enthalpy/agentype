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
//! Both are hard gates. A candidate that fails either is not returned.
//!
//! Ranking follows the frozen spec 06 matching preference: an exact pin is
//! preferred, then compatible narrower/refinement types, then equivalent or
//! incomparable compatible types, then compatible broader/general types. Within a
//! class, candidate-vs-candidate specificity (`more_specific_for`) is applied as
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
use crate::records::{AgentType, LifecycleMode};
use crate::requirement::TaskAgentRequirement;
use agentype_core::{ContinuityPreference, LogicalAgentId, PartitionId, WorkstreamId};
use std::cmp::Ordering;
use std::collections::BTreeSet;

/// One existing, immediately usable (M5 `READY` and unassigned) LogicalAgent,
/// resolved to its exact bound AgentType and its M5 placement facts.
#[derive(Clone, Debug, PartialEq)]
pub struct ExistingAgentCandidate {
    pub logical_agent_id: LogicalAgentId,
    pub agent_type: AgentType,
    pub lifecycle: LifecycleMode,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Relation {
    Exact,
    Narrower,
    EquivalentOrIncomparable,
    Broader,
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
    Relation::EquivalentOrIncomparable
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

    let eligible: Vec<ExistingAgentCandidate> = candidates
        .iter()
        .filter(|candidate| {
            placement_eligible(candidate, placement)
                && candidate
                    .agent_type
                    .contract
                    .lifecycle
                    .contains(&candidate.lifecycle)
                && can_execute(&candidate.agent_type, &req.hard, catalog).is_ok()
        })
        .cloned()
        .collect();

    let relations: Vec<Relation> = eligible
        .iter()
        .map(|candidate| relation(&candidate.agent_type, required, req, catalog))
        .collect();

    let domination: Vec<usize> = eligible
        .iter()
        .enumerate()
        .map(|(i, candidate)| {
            eligible
                .iter()
                .enumerate()
                .filter(|(j, other)| {
                    relations[*j] == relations[i]
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

    let mut indexed: Vec<(Relation, usize, ExistingAgentCandidate)> = relations
        .into_iter()
        .zip(domination)
        .zip(eligible)
        .map(|((relation, domination), candidate)| (relation, domination, candidate))
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
            .then_with(|| match (a.available_since, b.available_since) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
            .then_with(|| {
                a.created_at
                    .partial_cmp(&b.created_at)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.agent_type.type_ref.cmp(&b.agent_type.type_ref))
            .then_with(|| a.logical_agent_id.cmp(&b.logical_agent_id))
    });

    Ok(indexed
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect())
}
