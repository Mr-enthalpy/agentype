//! The four M6-B relations, kept independent as spec 06 requires.
//!
//! - `can_execute(AgentType, TaskRequirement, CapabilityCatalog)`
//! - `can_provision(SpawnSource, SourceConfig, AgentType, evidence, catalog)`
//! - `more_specific_for(A, B, TaskRequirement, catalog)`
//! - `is_valid_refinement(Base, Derived, catalog)`
//!
//! They MUST NOT be collapsed into a single subtype/inheritance operator. The
//! capability-constraint ordering is defined once and shared by refinement and
//! specificity; capability semantics come from the catalog, not from either
//! AgentType.

use crate::capability::{
    value_satisfies, value_within, Assurance, CapabilityCatalog, CapabilityClaim, CapabilityRef,
    CapabilityValue, MatcherKind,
};
use crate::error::ContractError;
use crate::evidence::ResolvedProvisioningEvidence;
use crate::records::{
    network_rank, workspace_rank, AgentType, AgentTypeContract, ConfigStatus, SandboxPolicyRef,
    SourceConfig, SourceStatus, SpawnSource, TaskRequirement,
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
    Ok(matching
        .iter()
        .copied()
        .find(|claim| claim.assurance == Assurance::Enforced)
        .or(Some(first)))
}

/// Can this AgentType contract execute the Task requirement?
pub fn can_execute(
    agent: &AgentType,
    req: &TaskRequirement,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    agent.contract.validate(catalog)?;

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
        let definition =
            catalog
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
        if !value_satisfies(definition.matcher_kind, required, provided) {
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
    if !sandbox_policy_within(&req.sandbox_policy, &agent.contract.sandbox_policy) {
        return Err(ContractError::CapabilityMismatch {
            capability: "sandbox_policy".into(),
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
/// AgentType contract, given imported, policy-bound enforcement evidence?
pub fn can_provision(
    agent: &AgentType,
    source: &SpawnSource,
    config: &SourceConfig,
    evidence: &ResolvedProvisioningEvidence,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    agent.contract.validate(catalog)?;

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
            expected: source.adapter_policy.id().as_str().to_string(),
            actual: evidence.adapter_policy().id().as_str().to_string(),
        });
    }
    // Evidence is bound to the exact candidate it was resolved for: evidence
    // for config A must never authorize config B under the same source.
    if evidence.source_ref() != &source.source_ref
        || evidence.source_config_ref() != &config.config_ref
        || evidence.config_digest() != &config.config_digest
    {
        return Err(ContractError::EvidenceSubjectMismatch {
            reason: "evidence was not resolved for this exact source config digest".into(),
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
        let definition =
            catalog
                .get(reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                })?;
        let capability_name = reference.capability_id().as_str().to_string();

        if definition.security_class.requires_evidence() {
            let provided = evidence.enforced_capability(reference).ok_or_else(|| {
                ContractError::SecurityUnenforceable {
                    reason: format!(
                        "capability {}@{} has no imported enforcement evidence",
                        capability_name,
                        reference.revision()
                    ),
                }
            })?;
            if !value_satisfies(definition.matcher_kind, required, provided) {
                return Err(ContractError::CapabilityMismatch {
                    capability: capability_name,
                });
            }
            continue;
        }

        // Functional: the source envelope is the provisionable ceiling; neither
        // the source nor the config may declare a value beyond it.
        let ceiling = source.functional_envelope.get(reference).ok_or_else(|| {
            ContractError::CapabilityMismatch {
                capability: capability_name.clone(),
            }
        })?;
        let source_claim = resolve_claim(&source.claims, reference)?;
        if let Some(claim) = source_claim {
            if !value_within(ceiling, &claim.value) {
                return Err(ContractError::InvariantViolation(format!(
                    "source declaration for {capability_name} exceeds its provisionable envelope"
                )));
            }
        }
        let config_claim = resolve_claim(&config.claims, reference)?;
        if let Some(claim) = config_claim {
            if !value_within(ceiling, &claim.value) {
                return Err(ContractError::InvariantViolation(format!(
                    "config declaration for {capability_name} exceeds the source envelope"
                )));
            }
        }
        let provided: &CapabilityValue = config_claim
            .map(|c| &c.value)
            .or_else(|| source_claim.map(|c| &c.value))
            .unwrap_or(ceiling);
        if !value_satisfies(definition.matcher_kind, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: capability_name,
            });
        }
    }

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
    if let Some(policy) = &agent.contract.sandbox_policy {
        if !evidence.enforces_sandbox_policy(policy) {
            return Err(ContractError::SecurityUnenforceable {
                reason: format!(
                    "sandbox policy {}@{} is not enforced by imported evidence",
                    policy.id().as_str(),
                    policy.revision()
                ),
            });
        }
    }

    Ok(())
}

/// Is `derived` at least as restrictive as `base` for one capability revision?
fn requirement_no_wider(
    matcher: MatcherKind,
    base: &CapabilityValue,
    derived: &CapabilityValue,
) -> bool {
    match (matcher, base, derived) {
        (MatcherKind::Bool, CapabilityValue::Bool(b), CapabilityValue::Bool(d)) => b == d,
        (MatcherKind::Exact, b, d) => b == d,
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
/// no requirement disappears and each is at least as restrictive.
fn capability_constraints_no_wider(
    derived: &AgentTypeContract,
    base: &AgentTypeContract,
    catalog: &CapabilityCatalog,
) -> bool {
    for (reference, base_value) in &base.required_capabilities {
        let Some(derived_value) = derived.required_capabilities.get(reference) else {
            return false;
        };
        let Some(definition) = catalog.get(reference) else {
            return false;
        };
        if !requirement_no_wider(definition.matcher_kind, base_value, derived_value) {
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

/// `derived` no wider than `base` for the full sandbox policy reference.
fn sandbox_policy_no_wider(
    derived: &Option<SandboxPolicyRef>,
    base: &Option<SandboxPolicyRef>,
) -> bool {
    match base {
        None => true,
        Some(_) => derived == base,
    }
}

/// Whether a Task sandbox requirement is allowed by an agent sandbox policy.
///
/// `None` on the allowance side is unconstrained (consistent with the
/// refinement order, where `None -> Some(P)` is a narrowing).
fn sandbox_policy_within(
    requirement: &Option<SandboxPolicyRef>,
    allowance: &Option<SandboxPolicyRef>,
) -> bool {
    match (requirement, allowance) {
        (_, None) => true,
        (None, _) => true,
        (Some(r), Some(a)) => r == a,
    }
}

/// A grants no more authority than B on every relevant dimension.
fn authority_no_wider(
    a: &AgentTypeContract,
    b: &AgentTypeContract,
    catalog: &CapabilityCatalog,
) -> bool {
    a.allowed_information_functions
        .iter()
        .all(|f| b.allowed_information_functions.contains(f))
        && a.permission_ceiling.is_subset(&b.permission_ceiling)
        && a.visibility.is_subset(&b.visibility)
        && a.tools.is_subset(&b.tools)
        && a.roots.is_subset(&b.roots)
        && a.affinity.is_subset(&b.affinity)
        && a.budget_ceiling <= b.budget_ceiling
        && a.lifecycle.is_subset(&b.lifecycle)
        && a.continuity >= b.continuity
        && workspace_rank(a.security.workspace) <= workspace_rank(b.security.workspace)
        && network_rank(a.security.network) <= network_rank(b.security.network)
        && a.security.tool_roots.is_subset(&b.security.tool_roots)
        && (!b.security.requires_attempt_isolation || a.security.requires_attempt_isolation)
        && sandbox_policy_no_wider(&a.sandbox_policy, &b.sandbox_policy)
        && anchor_no_wider(&a.anchor_constraint, &b.anchor_constraint)
        && capability_constraints_no_wider(a, b, catalog)
}

/// Is A strictly more specific than B for this Task? Only defined once both are
/// executable; never ranked by nominal inheritance depth. Two types with
/// equivalent authority are incomparable, not mutually more specific.
pub fn more_specific_for(
    a: &AgentType,
    b: &AgentType,
    req: &TaskRequirement,
    catalog: &CapabilityCatalog,
) -> bool {
    if a.type_ref == b.type_ref {
        return false;
    }
    if can_execute(a, req, catalog).is_err() || can_execute(b, req, catalog).is_err() {
        return false;
    }
    authority_no_wider(&a.contract, &b.contract, catalog)
        && !authority_no_wider(&b.contract, &a.contract, catalog)
}

/// Spec 06 refinement monotonicity: the derived type MUST NOT enlarge authority
/// and MUST NOT weaken a base guarantee (lifecycle, continuity, security,
/// capability constraints).
pub fn is_valid_refinement(
    base: &AgentType,
    derived: &AgentType,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    base.contract.validate(catalog)?;
    derived.contract.validate(catalog)?;

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
    // Affinity is an allowed-tag ceiling like permission/tools: a derived type
    // may only support a subset, so refinement never enlarges the executable
    // Task set.
    if !derived_c.affinity.is_subset(&base_c.affinity) {
        return invalid("derived affinity broadens base affinity");
    }
    if derived_c.budget_ceiling > base_c.budget_ceiling {
        return invalid("derived budget exceeds base budget");
    }
    if !derived_c.lifecycle.is_subset(&base_c.lifecycle) {
        return invalid("derived lifecycle widens base lifecycle");
    }
    if derived_c.continuity < base_c.continuity {
        return invalid("derived continuity weakens base continuity");
    }
    if !sandbox_policy_no_wider(&derived_c.sandbox_policy, &base_c.sandbox_policy) {
        return invalid("derived sandbox policy is wider than base sandbox policy");
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
    if !capability_constraints_no_wider(derived_c, base_c, catalog) {
        return invalid("derived capability constraints are wider than base");
    }

    Ok(())
}
