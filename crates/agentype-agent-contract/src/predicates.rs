//! The four M6-B relations, kept independent as spec 06 requires.
//!
//! - `can_execute(AgentType, TaskRequirement)`
//! - `can_provision(SpawnSource, SourceConfig, AgentType, PhysicalSafety, exact binding)`
//! - `more_specific_for(A, B, TaskRequirement)`
//! - `is_valid_refinement(Base, Derived)`
//!
//! They MUST NOT be collapsed into a single subtype/inheritance operator.

use crate::capability::{value_satisfies, Assurance, CapabilityClaim, CapabilityRef};
use crate::error::ContractError;
use crate::records::{
    network_rank, workspace_rank, AgentType, AgentTypeContract, ConfigStatus, PhysicalSafety,
    SourceConfig, SourceStatus, SpawnSource, TaskRequirement,
};

/// Deterministic claim lookup. Conflicting values at the same exact reference
/// fail closed rather than depending on insertion order.
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
            reference.capability_id.as_str(),
            reference.revision
        )));
    }
    Ok(matching
        .iter()
        .copied()
        .find(|claim| claim.assurance == Assurance::Enforced)
        .or(Some(first)))
}

/// Can this AgentType contract execute the Task requirement?
pub fn can_execute(agent: &AgentType, req: &TaskRequirement) -> Result<(), ContractError> {
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
                capability: reference.capability_id.as_str().to_string(),
            })?;
        let provided = agent
            .contract
            .required_capabilities
            .get(reference)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: reference.capability_id.as_str().to_string(),
            })?;
        if !value_satisfies(spec.matcher_kind, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: reference.capability_id.as_str().to_string(),
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
/// AgentType contract, given the imported enforceable physical facts and exact
/// binding availability?
pub fn can_provision(
    agent: &AgentType,
    source: &SpawnSource,
    config: &SourceConfig,
    physical: &PhysicalSafety,
    exact_binding_available: bool,
) -> Result<(), ContractError> {
    if config.config_ref.source != source.source_ref {
        return Err(ContractError::SourceConfigInvalid {
            reason: "config does not belong to this exact source revision".into(),
        });
    }
    if source.status != SourceStatus::Active || config.status != ConfigStatus::Active {
        return Err(ContractError::SourceConfigInvalid {
            reason: "source or config is not active".into(),
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
                capability: reference.capability_id.as_str().to_string(),
            })?;

        // Config-specific claims take precedence over source-wide claims.
        let claim = match resolve_claim(&config.claims, reference)? {
            Some(claim) => Some(claim),
            None => resolve_claim(&source.claims, reference)?,
        };
        let envelope = source.functional_envelope.get(reference);
        let (provided, assurance) = match (claim, envelope) {
            (Some(claim), _) => (&claim.value, Some(claim.assurance)),
            (None, Some(value)) => (value, None),
            (None, None) => {
                return Err(ContractError::CapabilityMismatch {
                    capability: reference.capability_id.as_str().to_string(),
                })
            }
        };

        if spec.security_class.requires_enforced() && assurance != Some(Assurance::Enforced) {
            return Err(ContractError::SecurityUnenforceable {
                reason: format!(
                    "capability {}@{} requires an ENFORCED claim",
                    reference.capability_id.as_str(),
                    reference.revision
                ),
            });
        }
        if !value_satisfies(spec.matcher_kind, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: reference.capability_id.as_str().to_string(),
            });
        }
    }

    // Restrictions must be mechanically enforceable, never derived from a
    // stronger capability.
    if !physical
        .enforceable_workspace_modes
        .contains(&agent.contract.security.workspace)
    {
        return Err(ContractError::SecurityUnenforceable {
            reason: "physical environment cannot enforce the required workspace mode".into(),
        });
    }
    if !physical
        .enforceable_network_modes
        .contains(&agent.contract.security.network)
    {
        return Err(ContractError::SecurityUnenforceable {
            reason: "physical environment cannot enforce the required network policy".into(),
        });
    }
    if agent.contract.security.requires_attempt_isolation && !physical.attempt_isolation {
        return Err(ContractError::SecurityUnenforceable {
            reason: "attempt isolation is required but not enforceable".into(),
        });
    }

    if !exact_binding_available {
        return Err(ContractError::AdapterBindingMissing);
    }

    Ok(())
}

fn anchor_no_wider(a: &Option<String>, b: &Option<String>) -> bool {
    match b {
        None => true,
        Some(_) => a == b,
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
        && a.budget_ceiling <= b.budget_ceiling
        && workspace_rank(a.security.workspace) <= workspace_rank(b.security.workspace)
        && network_rank(a.security.network) <= network_rank(b.security.network)
        && a.security.tool_roots.is_subset(&b.security.tool_roots)
        && (!b.security.requires_attempt_isolation || a.security.requires_attempt_isolation)
        && a.continuity >= b.continuity
        && anchor_no_wider(&a.anchor_constraint, &b.anchor_constraint)
        && b.required_capabilities
            .keys()
            .all(|k| a.required_capabilities.contains_key(k))
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
/// and MUST NOT weaken a base guarantee (lifecycle, continuity, security).
pub fn is_valid_refinement(base: &AgentType, derived: &AgentType) -> Result<(), ContractError> {
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

    Ok(())
}
