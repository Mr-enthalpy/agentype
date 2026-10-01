//! The four M6-B relations, kept independent as spec 06 requires.
//!
//! - `can_execute(AgentType, TaskRequirement)`
//! - `can_provision(SpawnSource, SourceConfig, AgentType, PhysicalSafety, exact binding)`
//! - `more_specific_for(A, B, TaskRequirement)`
//! - `is_valid_refinement(Base, Derived)`
//!
//! They MUST NOT be collapsed into a single subtype/inheritance operator.

use crate::capability::{value_satisfies, Assurance, CapabilityValue};
use crate::error::ContractError;
use crate::records::{
    network_rank, workspace_rank, AgentType, ConfigStatus, PhysicalSafety, SourceConfig,
    SourceStatus, SpawnSource, TaskRequirement,
};

fn capability_matcher(
    agent: &AgentType,
    capability_id: &crate::ids::CapabilityId,
    value: &CapabilityValue,
) -> crate::capability::MatcherKind {
    agent
        .contract
        .capability_specs
        .get(capability_id)
        .map(|s| s.matcher_kind)
        .unwrap_or_else(|| value.matcher_kind())
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

    for (id, required) in &req.required_capabilities {
        let provided = agent
            .contract
            .required_capabilities
            .get(id)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: id.as_str().to_string(),
            })?;
        let matcher = capability_matcher(agent, id, required);
        if !value_satisfies(matcher, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: id.as_str().to_string(),
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
    if req.budget > agent.contract.budget_ceiling {
        return Err(ContractError::CapabilityMismatch {
            capability: "budget".into(),
        });
    }

    Ok(())
}

fn enforced_bool(source: &SpawnSource, capability_id: &str) -> bool {
    source.claims.iter().any(|claim| {
        claim.capability_id.as_str() == capability_id
            && matches!(claim.value, CapabilityValue::Bool(true))
            && claim.assurance == Assurance::Enforced
    })
}

/// Can this source + config provision an environment that realizes the
/// AgentType contract, given the imported physical safety and exact binding?
pub fn can_provision(
    agent: &AgentType,
    source: &SpawnSource,
    config: &SourceConfig,
    physical: &PhysicalSafety,
    exact_binding_available: bool,
) -> Result<(), ContractError> {
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

    for (id, required) in &agent.contract.required_capabilities {
        let provided = source.functional_envelope.get(id).or_else(|| {
            source
                .claims
                .iter()
                .find(|c| &c.capability_id == id)
                .map(|c| &c.value)
        });
        let provided = match provided {
            Some(v) => v,
            None => {
                return Err(ContractError::CapabilityMismatch {
                    capability: id.as_str().to_string(),
                })
            }
        };
        let matcher = capability_matcher(agent, id, required);
        if !value_satisfies(matcher, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: id.as_str().to_string(),
            });
        }
    }

    if agent.contract.security.requires_attempt_isolation
        && (!physical.attempt_isolation || !enforced_bool(source, "attempt_isolation"))
    {
        return Err(ContractError::SecurityUnenforceable {
            reason: "attempt isolation is required but not enforced".into(),
        });
    }
    if workspace_rank(physical.workspace) < workspace_rank(agent.contract.security.workspace) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "physical workspace cannot realize the agent workspace policy".into(),
        });
    }
    if network_rank(physical.network) < network_rank(agent.contract.security.network) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "physical network cannot realize the agent network policy".into(),
        });
    }

    if !exact_binding_available {
        return Err(ContractError::AdapterBindingMissing);
    }

    Ok(())
}

/// Is A strictly more specific than B for this Task? Only meaningful once both
/// are known to be executable; never ranked by nominal inheritance depth.
pub fn more_specific_for(a: &AgentType, b: &AgentType, req: &TaskRequirement) -> bool {
    if a.type_ref == b.type_ref {
        return false;
    }
    if can_execute(a, req).is_err() || can_execute(b, req).is_err() {
        return false;
    }
    let a = &a.contract;
    let b = &b.contract;
    a.permission_ceiling.is_subset(&b.permission_ceiling)
        && a.visibility.is_subset(&b.visibility)
        && a.tools.is_subset(&b.tools)
        && a.roots.is_subset(&b.roots)
        && a.budget_ceiling <= b.budget_ceiling
}

/// Spec 06 refinement monotonicity: the derived type MUST NOT enlarge authority.
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
    if derived_c.continuity > base_c.continuity {
        return invalid("derived continuity exceeds base continuity");
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

    Ok(())
}
