//! M6-B.3 pure matching of **already-bound** LogicalAgents to a Task's agent
//! requirement.
//!
//! This stage does not provision anything: a new `LogicalAgent` is materialized
//! only by M6-B.4. Matching therefore answers "which existing, immediately usable
//! typed agent may execute this Task", applying the spec 06 semantic preference
//! order.
//!
//! A pinned requirement (`required_type = Some`) is satisfied only by the exact
//! type or a **refinement** of it (`more_specific_for(candidate, required)`). A
//! broader or semantically incomparable type has more authority than the pinned
//! contract and is NOT an eligible substitute; broader/general compatibility
//! belongs to future unpinned selection, not to a pin. Among eligible candidates,
//! candidate-vs-candidate specificity (`more_specific_for`) MUST dominate soft and
//! deterministic tie-breaks, so a strictly more-specific agent always ranks before
//! a less-specific one. This is a partial order, so it is applied as
//! dominance-count layers rather than a naive comparator. Nominal inheritance
//! depth is never consulted.
//!
//! Only M5 `READY`, unassigned agents are candidates (the storage loader filters
//! to that); tier 4 "cold/revivable" is deliberately not approximated from
//! non-READY M5 states.

use crate::capability::CapabilityCatalog;
use crate::error::ContractError;
use crate::predicates::{can_execute, more_specific_for};
use crate::records::{AgentType, LifecycleMode};
use crate::requirement::TaskAgentRequirement;
use agentype_core::LogicalAgentId;
use std::cmp::Ordering;

/// One existing, immediately usable (M5 `READY` and unassigned) LogicalAgent,
/// already resolved to its exact bound AgentType and its physical lifecycle
/// class. `available_since` and `created_at` are diagnostics used only for
/// deterministic tie-breaking.
#[derive(Clone, Debug, PartialEq)]
pub struct ExistingAgentCandidate {
    pub logical_agent_id: LogicalAgentId,
    pub agent_type: AgentType,
    pub lifecycle: LifecycleMode,
    pub available_since: Option<f64>,
    pub created_at: f64,
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

/// Filter and rank existing bound, immediately usable agents for one Task
/// requirement, best first.
///
/// A requirement with `required_type == None` is a **typed** Task (it may still
/// carry hard capability/security constraints); returning no candidate here is a
/// deliberate B.3 limitation (nominal selection is deferred), never an
/// instruction to hand the Task back to the legacy partition path. The
/// distinction is the durable `TaskAgentRequirement` row itself.
///
/// Physical eligibility (adapter binding, credentials) is M6-B.4 and is NOT
/// decided here.
pub fn match_existing_agents(
    required: &AgentType,
    req: &TaskAgentRequirement,
    candidates: &[ExistingAgentCandidate],
    catalog: &CapabilityCatalog,
) -> Result<Vec<ExistingAgentCandidate>, ContractError> {
    let Some(pinned) = &req.required_type else {
        return Ok(Vec::new());
    };
    if pinned != &required.type_ref {
        return Err(ContractError::InvariantViolation(format!(
            "match target {}@{} does not match the requirement's pinned type",
            required.type_ref.id().as_str(),
            required.type_ref.revision()
        )));
    }

    // Eligible = the exact type or a refinement of it, and able to execute the
    // Task's hard requirement. Broader/incomparable types are not substitutes.
    let mut eligible: Vec<ExistingAgentCandidate> = candidates
        .iter()
        .filter(|candidate| {
            can_execute(&candidate.agent_type, &req.hard, catalog).is_ok()
                && candidate
                    .agent_type
                    .contract
                    .lifecycle
                    .contains(&candidate.lifecycle)
                && (candidate.agent_type.type_ref == required.type_ref
                    || more_specific_for(&candidate.agent_type, required, &req.hard, catalog))
        })
        .cloned()
        .collect();

    // Dominance layers: how many other eligible candidates are strictly more
    // specific. Lower is better, and a strictly more-specific candidate always
    // has a strictly lower count than the candidate it dominates.
    let domination: Vec<usize> = eligible
        .iter()
        .map(|candidate| {
            eligible
                .iter()
                .filter(|other| {
                    strictly_more_specific(&other.agent_type, &candidate.agent_type, req, catalog)
                })
                .count()
        })
        .collect();

    let mut indexed: Vec<(usize, ExistingAgentCandidate)> =
        domination.into_iter().zip(eligible.drain(..)).collect();

    indexed.sort_by(|(a_dom, a), (b_dom, b)| {
        let exact = |c: &ExistingAgentCandidate| c.agent_type.type_ref == required.type_ref;
        (if exact(a) { 0u8 } else { 1u8 })
            .cmp(&if exact(b) { 0u8 } else { 1u8 })
            .then_with(|| a_dom.cmp(b_dom))
            // Stronger continuity guarantee first (a ranking dimension applied
            // after compatibility, never part of semantic specificity).
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
            // Stable, collision-free deterministic tie-break.
            .then_with(|| a.agent_type.type_ref.cmp(&b.agent_type.type_ref))
            .then_with(|| a.logical_agent_id.cmp(&b.logical_agent_id))
    });

    Ok(indexed
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect())
}
