//! M6-B.3 pure matching of **already-bound** LogicalAgents to a Task's agent
//! requirement.
//!
//! This stage does not provision anything: a new `LogicalAgent` is materialized
//! only by M6-B.4. Matching therefore answers "which existing, immediately usable
//! typed agent may execute this Task", applying the spec 06 semantic preference
//! order:
//!
//! 1. exact / most-specific compatible resident agent
//! 2. compatible narrower anchored type
//! 3. compatible broader/general type
//! 4. cold/revivable compatible logical agent
//!
//! Only tier 1-3 are implemented in M6-B.3. A candidate here is necessarily M5
//! `READY` and unassigned (the storage loader filters to that), so "cold/revivable"
//! (tier 4) is deliberately **not** approximated from non-READY M5 states: M6
//! revival/continuity is a later seam. Ranking MUST NOT use nominal inheritance
//! depth.
//!
//! An unbound LogicalAgent (one with no exact AgentType binding) can never be
//! returned for a typed requirement: there is no contract to prove `can_execute`.

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

fn specificity_tier(
    candidate: &ExistingAgentCandidate,
    required: &AgentType,
    req: &TaskAgentRequirement,
    catalog: &CapabilityCatalog,
) -> u8 {
    if candidate.agent_type.type_ref == required.type_ref {
        return 0;
    }
    if more_specific_for(&candidate.agent_type, required, &req.hard, catalog) {
        return 1;
    }
    if more_specific_for(required, &candidate.agent_type, &req.hard, catalog) {
        return 3;
    }
    2
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

    let mut eligible: Vec<ExistingAgentCandidate> = Vec::new();
    for candidate in candidates {
        if can_execute(&candidate.agent_type, &req.hard, catalog).is_err() {
            continue;
        }
        if !candidate
            .agent_type
            .contract
            .lifecycle
            .contains(&candidate.lifecycle)
        {
            continue;
        }
        eligible.push(candidate.clone());
    }

    eligible.sort_by(|a, b| {
        specificity_tier(a, required, req, catalog)
            .cmp(&specificity_tier(b, required, req, catalog))
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

    Ok(eligible)
}
