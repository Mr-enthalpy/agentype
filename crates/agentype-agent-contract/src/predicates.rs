//! The four M6-B relations, kept independent as spec 06 requires.
//!
//! - `can_execute(AgentType, TaskRequirement)`
//! - `can_provision(SpawnSource, SourceConfig, AgentType, evidence)`
//! - `more_specific_for(A, B, TaskRequirement)`
//! - `is_valid_refinement(Base, Derived)`
//!
//! They MUST NOT be collapsed into a single subtype/inheritance operator. The
//! capability-constraint ordering is defined once and shared by refinement and
//! specificity.

use crate::capability::{value_satisfies, Assurance, CapabilityClaim, CapabilityRef, MatcherKind};
use crate::error::ContractError;
use crate::evidence::ResolvedProvisioningEvidence;
use crate::records::{
    network_rank, workspace_rank, AgentType, AgentTypeContract, ConfigStatus, SourceConfig,
    SourceStatus, SpawnSource, TaskRequirement,
};

/// Deterministic declaration lookup. Conflicting values at the same exact
/// reference fail closed rather than depending on insertion order.
fn resolve_claim<'a>(
    claims: &'a [CapabilityClaim],
    reference: &CapabilityRef,
) -> Result<Option<&'a CapabilityClaim>, ContractError> {
    let matching: Vec<&CapabilityClaim> = claims
        .iter()
        .filter(|claim| &claim.reference == reference)
        .collect();
    let Some(first) = matching.first() else {
        return Ok(None);
    };
    if matching.iter().any(|claim| claim.value != first.value) {
        return Err(ContractError::InvariantViolation(format!(
            "conflicting capability claims for {}@{}",
            reference.capability_id().as_str(),
            reference.revision()
        )));
    }
    // Prefer an ENFORCED-declared claim, but this is only a declaration tiebreak;
    // security classes still require imported evidence.
    Ok(matching
        .iter()
        .copied()
        .find(|claim| claim.assurance == Assurance::Enforced)
        .or(Some(first)))
}

/// Can this AgentType contract execute the Task requirement?
pub fn can_execute(agent: &AgentType, req: &TaskRequirement) -> Result<(), ContractError> {
    agent.contract.validate()?;

    if !agent
        .contract
        .allowed_information_functions
        .contains(&req.information_function)
    {
        return Err(ContractError::CapabilityMismatch {
            capability: "information_function".into(),
        });
    }

    for (reference, required) in &req.required_capabilities {
        let spec = agent
            .contract
            .capability_specs
            .get(reference)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: reference.capability_id().as_str().to_string(),
            })?;
        let provided = agent
            .contract
            .required_capabilities
            .get(reference)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: reference.capability_id().as_str().to_string(),
            })?;
        if !value_satisfies(spec.matcher_kind, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: reference.capability_id().as_str().to_string(),
            });
        }
    }

    for permission in &req.required_permissions {
        if !agent.contract.permission_ceiling.contains(permission) {
            return Err(ContractError::CapabilityMismatch {
                capability: format!("permission:{permission}"),
            });
        }
    }
    for tool in &req.required_tools {
        if !agent.contract.tools.contains(tool) {
            return Err(ContractError::CapabilityMismatch {
                capability: format!("tool:{tool}"),
            });
        }
    }
    if !req.required_affinity.is_subset(&agent.contract.affinity) {
        return Err(ContractError::CapabilityMismatch {
            capability: "affinity".into(),
        });
    }

    if workspace_rank(agent.contract.security.workspace) < workspace_rank(req.required_workspace) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "agent workspace policy is narrower than the Task requirement".into(),
        });
    }
    if network_rank(agent.contract.security.network) < network_rank(req.required_network) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "agent network policy is narrower than the Task requirement".into(),
        });
    }
    if agent.contract.continuity < req.required_continuity {
        return Err(ContractError::CapabilityMismatch {
            capability: "continuity".into(),
        });
    }
    if let Some(required_anchor) = &req.required_anchor {
        if let Some(constraint) = &agent.contract.anchor_constraint {
            if constraint != required_anchor {
                return Err(ContractError::CapabilityMismatch {
                    capability: "anchor".into(),
                });
            }
        }
    }
    if req.budget.get() > agent.contract.budget_ceiling.get() {
        return Err(ContractError::CapabilityMismatch {
            capability: "budget".into(),
        });
    }

    Ok(())
}

/// Can this source + config provision an environment that realizes the
/// AgentType contract, given the imported, policy-bound enforcement evidence?
pub fn can_provision(
    agent: &AgentType,
    source: &SpawnSource,
    config: &SourceConfig,
    evidence: &ResolvedProvisioningEvidence,
) -> Result<(), ContractError> {
    agent.contract.validate()?;

    if config.config_ref.source() != &source.source_ref {
        return Err(ContractError::SourceConfigInvalid {
            reason: "config does not belong to this exact source revision".into(),
        });
    }
    if source.status != SourceStatus::Active || config.status != ConfigStatus::Active {
        return Err(ContractError::SourceConfigInvalid {
            reason: "source or config is not active".into(),
        });
    }
    if evidence.adapter_policy() != &source.adapter_policy {
        return Err(ContractError::EvidencePolicyMismatch {
            expected: source.adapter_policy.policy_id().as_str().to_string(),
            actual: evidence.adapter_policy().policy_id().as_str().to_string(),
        });
    }

    if !agent.contract.lifecycle.is_subset(&source.lifecycle_modes) {
        return Err(ContractError::CapabilityMismatch {
            capability: "lifecycle".into(),
        });
    }
    if !source.continuity_modes.contains(&agent.contract.continuity) {
        return Err(ContractError::CapabilityMismatch {
            capability: "continuity".into(),
        });
    }

    for (reference, required) in &agent.contract.required_capabilities {
        let spec = agent
            .contract
            .capability_specs
            .get(reference)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: reference.capability_id().as_str().to_string(),
            })?;

        if spec.security_class.requires_evidence() {
            // Only imported enforcement evidence satisfies a security class.
            let provided = evidence.enforced_capability(reference).ok_or_else(|| {
                ContractError::SecurityUnenforceable {
                    reason: format!(
                        "capability {}@{} has no imported enforcement evidence",
                        reference.capability_id().as_str(),
                        reference.revision()
                    ),
                }
            })?;
            if !value_satisfies(spec.matcher_kind, required, provided) {
                return Err(ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                });
            }
            continue;
        }

        // Functional: config declarations take precedence over source declarations.
        let claim = match resolve_claim(&config.claims, reference)? {
            Some(claim) => Some(claim),
            None => resolve_claim(&source.claims, reference)?,
        };
        let envelope = source.functional_envelope.get(reference);
        let provided = match (claim, envelope) {
            (Some(claim), _) => &claim.value,
            (None, Some(value)) => value,
            (None, None) => {
                return Err(ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                })
            }
        };
        if !value_satisfies(spec.matcher_kind, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: reference.capability_id().as_str().to_string(),
            });
        }
    }

    // Restrictions must be mechanically enforceable, never inferred.
    let safety = evidence.enforceable_safety();
    if !safety.enforces_workspace(agent.contract.security.workspace) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "physical environment cannot enforce the required workspace mode".into(),
        });
    }
    if !safety.enforces_network(agent.contract.security.network) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "physical environment cannot enforce the required network policy".into(),
        });
    }
    if agent.contract.security.requires_attempt_isolation && !safety.attempt_isolation() {
        return Err(ContractError::SecurityUnenforceable {
            reason: "attempt isolation is required but not enforceable".into(),
        });
    }

    Ok(())
}

/// Is `derived` at least as restrictive as `base` for one capability revision?
fn requirement_no_wider(
    matcher: MatcherKind,
    base: &crate::capability::CapabilityValue,
    derived: &crate::capability::CapabilityValue,
) -> bool {
    use crate::capability::CapabilityValue;
    match (matcher, base, derived) {
        // Equality matchers admit no ordering; a change is not monotone.
        (MatcherKind::Bool, CapabilityValue::Bool(b), CapabilityValue::Bool(d)) => b == d,
        (MatcherKind::Exact, b, d) => b == d,
        // Derived demanding a superset is at least as restrictive.
        (MatcherKind::Set, CapabilityValue::Set(b), CapabilityValue::Set(d)) => b.is_subset(d),
        (
            MatcherKind::Ordered,
            CapabilityValue::Ordered {
                class: bc,
                rank: br,
            },
            CapabilityValue::Ordered {
                class: dc,
                rank: dr,
            },
        ) => bc == dc && dr >= br,
        (MatcherKind::Quantity, CapabilityValue::Quantity(b), CapabilityValue::Quantity(d)) => {
            d.get() >= b.get()
        }
        _ => false,
    }
}

/// `derived`'s capability constraints are at least as restrictive as `base`'s:
/// no requirement disappears, and no matcher or security class is downgraded.
fn capability_constraints_no_wider(derived: &AgentTypeContract, base: &AgentTypeContract) -> bool {
    for (reference, base_value) in &base.required_capabilities {
        let Some(derived_value) = derived.required_capabilities.get(reference) else {
            return false;
        };
        let (Some(base_spec), Some(derived_spec)) = (
            base.capability_specs.get(reference),
            derived.capability_specs.get(reference),
        ) else {
            return false;
        };
        if derived_spec.matcher_kind != base_spec.matcher_kind {
            return false;
        }
        if derived_spec.security_class != base_spec.security_class {
            return false;
        }
        if !requirement_no_wider(base_spec.matcher_kind, base_value, derived_value) {
            return false;
        }
    }
    true
}

fn anchor_no_wider(derived: &Option<String>, base: &Option<String>) -> bool {
    match base {
        None => true,
        Some(_) => derived == base,
    }
}

/// A grants no more authority than B on every relevant dimension.
fn authority_no_wider(a: &AgentTypeContract, b: &AgentTypeContract) -> bool {
    a.allowed_information_functions
        .iter()
        .all(|f| b.allowed_information_functions.contains(f))
        && a.permission_ceiling.is_subset(&b.permission_ceiling)
        && a.visibility.is_subset(&b.visibility)
        && a.tools.is_subset(&b.tools)
        && a.roots.is_subset(&b.roots)
        && a.affinity.is_superset(&b.affinity)
        && a.budget_ceiling <= b.budget_ceiling
        && workspace_rank(a.security.workspace) <= workspace_rank(b.security.workspace)
        && network_rank(a.security.network) <= network_rank(b.security.network)
        && a.security.tool_roots.is_subset(&b.security.tool_roots)
        && (!b.security.requires_attempt_isolation || a.security.requires_attempt_isolation)
        && a.continuity >= b.continuity
        && anchor_no_wider(&a.anchor_constraint, &b.anchor_constraint)
        && capability_constraints_no_wider(a, b)
}

/// Is A strictly more specific than B for this Task? Only defined once both are
/// executable; never ranked by nominal inheritance depth. Two types with
/// equivalent authority are incomparable, not mutually more specific.
pub fn more_specific_for(a: &AgentType, b: &AgentType, req: &TaskRequirement) -> bool {
    if a.type_ref == b.type_ref {
        return false;
    }
    if can_execute(a, req).is_err() || can_execute(b, req).is_err() {
        return false;
    }
    authority_no_wider(&a.contract, &b.contract) && !authority_no_wider(&b.contract, &a.contract)
}

/// Spec 06 refinement monotonicity: the derived type MUST NOT enlarge authority
/// and MUST NOT weaken a base guarantee (lifecycle, continuity, security,
/// capability constraints).
pub fn is_valid_refinement(base: &AgentType, derived: &AgentType) -> Result<(), ContractError> {
    base.contract.validate()?;
    derived.contract.validate()?;

    let base_c = &base.contract;
    let derived_c = &derived.contract;
    let invalid = |reason: &str| {
        Err(ContractError::InvalidRefinement {
            reason: reason.to_string(),
        })
    };

    if !derived_c
        .permission_ceiling
        .is_subset(&base_c.permission_ceiling)
    {
        return invalid("derived permission widens base permission");
    }
    if !derived_c.visibility.is_subset(&base_c.visibility) {
        return invalid("derived visibility widens base visibility");
    }
    if !derived_c.tools.is_subset(&base_c.tools) {
        return invalid("derived tools widen base tools");
    }
    if !derived_c.roots.is_subset(&base_c.roots) {
        return invalid("derived roots widen base roots");
    }
    if !derived_c.affinity.is_superset(&base_c.affinity) {
        return invalid("derived affinity broadens base affinity");
    }
    if derived_c.budget_ceiling > base_c.budget_ceiling {
        return invalid("derived budget exceeds base budget");
    }
    if !derived_c.lifecycle.is_subset(&base_c.lifecycle) {
        return invalid("derived lifecycle widens base lifecycle");
    }
    // Continuity is a guarantee: a derived type may strengthen it, never weaken it.
    if derived_c.continuity < base_c.continuity {
        return invalid("derived continuity weakens base continuity");
    }
    if let Some(constraint) = &base_c.anchor_constraint {
        if derived_c.anchor_constraint.as_ref() != Some(constraint) {
            return invalid("derived anchor does not satisfy base anchor constraint");
        }
    }
    if workspace_rank(derived_c.security.workspace) > workspace_rank(base_c.security.workspace) {
        return invalid("derived workspace policy widens base workspace policy");
    }
    if network_rank(derived_c.security.network) > network_rank(base_c.security.network) {
        return invalid("derived network policy widens base network policy");
    }
    if !derived_c
        .security
        .tool_roots
        .is_subset(&base_c.security.tool_roots)
    {
        return invalid("derived tool roots widen base tool roots");
    }
    if base_c.security.requires_attempt_isolation && !derived_c.security.requires_attempt_isolation
    {
        return invalid("derived type weakens required attempt isolation");
    }
    if !derived_c
        .allowed_information_functions
        .iter()
        .all(|f| base_c.allowed_information_functions.contains(f))
    {
        return invalid("derived type adds an information function not allowed by base");
    }
    if !capability_constraints_no_wider(derived_c, base_c) {
        return invalid("derived capability constraints are wider than base");
    }

    Ok(())
}
