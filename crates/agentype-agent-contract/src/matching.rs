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
//! 2. the M6-B AgentType contract (`can_execute` for `COARSE_V1`,
//!    `can_execute_security_v1` for `SECURITY_V1`),
//!
//! plus an explicit rule that a different revision of the same `type_id` is never
//! a substitute (`D-TYPE-REV-COMPAT` is deferred). `more_specific_for` is a
//! **preference relation only**; it orders candidates and never decides
//! eligibility, so a semantically compatible type that is neither a strict
//! refinement nor strictly broader is still a candidate (ranked last).
//!
//! M6-B.5 B5.2 adds a mode-aware entry point. A `SECURITY_V1` Task MUST use
//! [`match_existing_agents_security_v1`], which orders by the proven policy
//! narrowing instead of policy reference equality and excludes `COARSE_V1`
//! candidates (ADR-0013 §11). The frozen [`match_existing_agents`] is unchanged
//! and remains the only relation used for `COARSE_V1`.
//!
//! A candidate's realized M5 retention is deliberately NOT interpreted against
//! the AgentType `lifecycle` set: that set is a **required source envelope**
//! (spec 06), not a per-instance mode whitelist. Only M5 `READY`, unassigned
//! agents are considered (the storage loader filters to that); the
//! "cold/revivable" tier is deliberately not approximated from non-READY states.

use crate::authority::{SecurityRevision, TaskSecurityCeiling};
use crate::capability::CapabilityCatalog;
use crate::error::ContractError;
use crate::predicates::{
    can_execute, can_execute_security_v1, more_specific_for, more_specific_for_with_policies,
};
use crate::records::AgentType;
use crate::requirement::TaskAgentRequirement;
use crate::sandbox::{
    validate_agent_type_coarse_fine, NormalizedSandboxPolicy, SecurityContractMode,
};
use agentype_core::{
    claim_placement_eligible, claim_tiebreak, ClaimIntent, ContinuityPreference, LogicalAgentId,
    PartitionId, WorkstreamId,
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

/// A candidate paired with the validated security identity of its bound
/// AgentType revision.
///
/// The pairing exists so preselection cannot compare a candidate's policy with
/// another candidate's mode: both come from the same wrapper, and the wrapper's
/// only constructor validates them together. It borrows the storage-owned
/// candidate; nothing here is authoritative.
#[derive(Clone, Copy, Debug)]
pub struct QualifiedExistingAgentCandidate<'a> {
    candidate: &'a ExistingAgentCandidate,
    revision: SecurityRevision<'a>,
}

impl<'a> QualifiedExistingAgentCandidate<'a> {
    /// Pair one candidate with its declared mode and loaded authored policy.
    ///
    /// A `SECURITY_V1` revision must declare and supply its authored policy, and
    /// that policy MUST project exactly to the revision's coarse
    /// `SecurityContract`. This closes the case where a caller supplies policy
    /// bytes that describe more authority than the durable revision declares.
    pub fn new(
        candidate: &'a ExistingAgentCandidate,
        mode: SecurityContractMode,
        authored_policy: Option<&'a NormalizedSandboxPolicy>,
    ) -> Result<Self, ContractError> {
        let revision = SecurityRevision::new(&candidate.agent_type, mode, authored_policy)?;
        if let Some(policy) = revision.authored_policy() {
            validate_agent_type_coarse_fine(&candidate.agent_type.contract.security, policy).map_err(
                |conflict| {
                    ContractError::InvariantViolation(format!(
                        "candidate {}@{} authored policy does not project to its coarse security contract: {}",
                        candidate.agent_type.type_ref.id().as_str(),
                        candidate.agent_type.type_ref.revision(),
                        conflict
                    ))
                },
            )?;
        }
        Ok(Self {
            candidate,
            revision,
        })
    }

    pub fn candidate(&self) -> &'a ExistingAgentCandidate {
        self.candidate
    }

    pub fn revision(&self) -> SecurityRevision<'a> {
        self.revision
    }
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

/// The same classification under the `SECURITY_V1` policy order.
///
/// Both sides are `SecurityRevision`s, so the comparison is impossible to make
/// against the unvalidated authored policy alone.
fn security_relation(
    candidate: &SecurityRevision<'_>,
    required: &SecurityRevision<'_>,
    req: &TaskAgentRequirement,
    ceiling: &TaskSecurityCeiling,
    catalog: &CapabilityCatalog,
) -> Relation {
    if candidate.agent_type().type_ref == required.agent_type().type_ref {
        return Relation::Exact;
    }
    if more_specific_for_with_policies(candidate, required, &req.hard, ceiling, catalog) {
        return Relation::Narrower;
    }
    if more_specific_for_with_policies(required, candidate, &req.hard, ceiling, catalog) {
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

fn security_strictly_more_specific(
    a: &QualifiedExistingAgentCandidate<'_>,
    b: &QualifiedExistingAgentCandidate<'_>,
    req: &TaskAgentRequirement,
    ceiling: &TaskSecurityCeiling,
    catalog: &CapabilityCatalog,
) -> bool {
    a.candidate().agent_type.type_ref != b.candidate().agent_type.type_ref
        && more_specific_for_with_policies(
            &a.revision(),
            &b.revision(),
            &req.hard,
            ceiling,
            catalog,
        )
        && !more_specific_for_with_policies(
            &b.revision(),
            &a.revision(),
            &req.hard,
            ceiling,
            catalog,
        )
}

fn same_workstream(candidate: &ExistingAgentCandidate, placement: &TaskPlacement) -> bool {
    placement.workstream_id.is_some() && candidate.workstream_id == placement.workstream_id
}

/// Use the same M5 decision as legacy selection and typed authority commitment.
fn placement_eligible(candidate: &ExistingAgentCandidate, placement: &TaskPlacement) -> bool {
    let required_tags: Vec<String> = placement.required_tags.iter().cloned().collect();
    let actual_tags: Vec<String> = candidate.tags.iter().cloned().collect();
    claim_placement_eligible(
        &ClaimIntent {
            partition: placement.partition.as_str(),
            required_tags: &required_tags,
            workstream_id: placement.workstream_id.as_ref().map(|id| id.as_str()),
            continuity: placement.continuity,
        },
        candidate.partition.as_str(),
        &actual_tags,
        candidate.workstream_id.as_ref().map(|id| id.as_str()),
    )
}

/// The single domination/sort implementation shared by both modes.
///
/// Sharing it is deliberate: mode-aware preselection MUST NOT acquire a
/// different tie-break order, only a different eligibility predicate and
/// policy-order dominance test. No partial selection happens here — every
/// eligible item is ranked.
fn finish_ranking<T>(
    eligible: Vec<(Relation, T)>,
    placement: &TaskPlacement,
    strictly_more_specific: impl Fn(&T, &T) -> bool,
    candidate_of: impl Fn(&T) -> &ExistingAgentCandidate,
) -> Vec<ExistingAgentCandidate> {
    let domination: Vec<usize> = eligible
        .iter()
        .map(|(relation, candidate)| {
            eligible
                .iter()
                .filter(|(other_relation, other)| {
                    other_relation == relation && strictly_more_specific(other, candidate)
                })
                .count()
        })
        .collect();

    let mut indexed: Vec<(Relation, usize, T)> = eligible
        .into_iter()
        .zip(domination)
        .map(|((relation, candidate), domination)| (relation, domination, candidate))
        .collect();

    indexed.sort_by(|(a_rel, a_dom, a_item), (b_rel, b_dom, b_item)| {
        let a = candidate_of(a_item);
        let b = candidate_of(b_item);
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

    indexed
        .into_iter()
        .map(|(_, _, item)| candidate_of(&item).clone())
        .collect()
}

/// Rank existing bound, `READY`, unassigned agents for one Task, best first.
///
/// The result is non-authoritative semantic preselection: physical eligibility
/// (SpawnSource, config, adapter binding, credentials, enforcement evidence) is
/// M6-B.4/M6-B.5 and is NOT decided here.
///
/// This is the frozen `COARSE_V1` relation. `SECURITY_V1` MUST use
/// [`match_existing_agents_security_v1`].
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

    Ok(finish_ranking(
        eligible,
        placement,
        |a, b| strictly_more_specific(&a.agent_type, &b.agent_type, req, catalog),
        |candidate| candidate,
    ))
}

/// Rank existing candidates for a `SECURITY_V1` Task over the policy order.
///
/// Eligibility is M5 placement + [`can_execute_security_v1`] against the Task
/// security ceiling + the explicit cross-revision exclusion. A `COARSE_V1`
/// candidate is excluded: the coarse contract carries no authored policy, so it
/// cannot bound a `SECURITY_V1` Task (ADR-0013 §11). A `SECURITY_V1` candidate
/// whose policy reference differs from the pinned type's but whose authored
/// policy provably bounds the Task ceiling is retained — reference equality is
/// not the order.
///
/// The result remains non-authoritative: physical eligibility and enforcement
/// qualification are decided later.
pub fn match_existing_agents_security_v1(
    required: &SecurityRevision<'_>,
    ceiling: &TaskSecurityCeiling,
    req: &TaskAgentRequirement,
    placement: &TaskPlacement,
    candidates: &[QualifiedExistingAgentCandidate<'_>],
    catalog: &CapabilityCatalog,
) -> Result<Vec<ExistingAgentCandidate>, ContractError> {
    if !required.is_security_v1() {
        return Err(ContractError::InvariantViolation(
            "SECURITY_V1 preselection requires a SECURITY_V1 pinned agent type".into(),
        ));
    }
    if req.required_type != required.agent_type().type_ref {
        return Err(ContractError::InvariantViolation(format!(
            "match target {}@{} does not match the requirement's pinned type",
            required.agent_type().type_ref.id().as_str(),
            required.agent_type().type_ref.revision()
        )));
    }
    if ceiling.required_workspace() != req.hard.required_workspace
        || ceiling.required_network() != req.hard.required_network
    {
        return Err(ContractError::InvariantViolation(
            "task security ceiling was derived from a different coarse requirement".into(),
        ));
    }

    let eligible: Vec<(Relation, QualifiedExistingAgentCandidate<'_>)> = candidates
        .iter()
        .filter(|candidate| {
            let revision = candidate.revision();
            if !revision.is_security_v1() {
                return false;
            }
            // `SecurityRevision::new` guarantees this for SECURITY_V1; the
            // fallback keeps the filter total rather than panicking.
            let Some(authored_policy) = revision.authored_policy() else {
                return false;
            };
            placement_eligible(candidate.candidate(), placement)
                && can_execute_security_v1(
                    revision.agent_type(),
                    &req.hard,
                    ceiling.policy(),
                    authored_policy,
                    catalog,
                )
                .is_ok()
                && !is_cross_revision_of_pin(
                    &candidate.candidate().agent_type,
                    required.agent_type(),
                )
        })
        .map(|candidate| {
            (
                security_relation(&candidate.revision(), required, req, ceiling, catalog),
                *candidate,
            )
        })
        .collect();

    Ok(finish_ranking(
        eligible,
        placement,
        |a, b| security_strictly_more_specific(a, b, req, ceiling, catalog),
        |candidate| candidate.candidate(),
    ))
}
