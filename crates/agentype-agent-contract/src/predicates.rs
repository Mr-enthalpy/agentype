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
    value_satisfies, value_within, Assurance, CapabilityCatalog, CapabilityClaim,
    CapabilityPolarity, CapabilityRef, CapabilityValue, MatcherKind,
};
use crate::error::ContractError;
use crate::evidence::ResolvedProvisioningEvidence;
use crate::records::{
    network_rank, workspace_rank, AffinityConstraint, AgentType, AgentTypeContract, ConfigStatus,
    ContinuityMode, SandboxPolicyRef, SourceConfig, SourceStatus, SpawnSource, TaskRequirement,
};
use std::collections::{BTreeMap, BTreeSet};

/// Deterministic declaration lookup. Conflicting values at the same exact
/// reference fail closed rather than depending on insertion order.
fn resolve_claim<'a>(
    claims: &'a [CapabilityClaim],
    reference: &CapabilityRef,
) -> Result<Option<&'a CapabilityClaim>, ContractError> {
    let matching: Vec<&CapabilityClaim> = claims
        .iter()
        .filter(|claim| &claim.reference == reference && claim.value.is_present())
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

fn capability_key(reference: &CapabilityRef) -> String {
    reference.capability_id().as_str().to_string()
}

/// Canonical capability lookup: an explicit `Bool(false)` entry denotes absence
/// and is indistinguishable from an omitted entry, so it is never returned.
fn present_cap<'a>(
    map: &'a BTreeMap<CapabilityRef, CapabilityValue>,
    reference: &CapabilityRef,
) -> Option<&'a CapabilityValue> {
    map.get(reference).filter(|value| value.is_present())
}

/// Continuity is a minimum guarantee: a source satisfies a requirement if it
/// advertises any mode at least as strong (e.g. `{Logical}` satisfies `None`).
fn continuity_satisfies(source_modes: &BTreeSet<ContinuityMode>, required: ContinuityMode) -> bool {
    source_modes.iter().any(|mode| *mode >= required)
}

/// Claims must name a catalog capability, match its declared value shape, and
/// must not conflict at the same exact revision — for every claim, Functional
/// or security class.
fn validate_claims(
    claims: &[CapabilityClaim],
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    for claim in claims {
        // Bool(false) is absence: a non-present claim is normalized away.
        if !claim.value.is_present() {
            continue;
        }
        let definition =
            catalog
                .get(&claim.reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: capability_key(&claim.reference),
                })?;
        if definition.matcher_kind != claim.value.matcher_kind() {
            return Err(ContractError::InvariantViolation(format!(
                "claim value shape does not match the definition for {}@{}",
                claim.reference.capability_id().as_str(),
                claim.reference.revision()
            )));
        }
    }
    // Deterministic conflict detection over every distinct reference.
    let mut seen: Vec<&CapabilityRef> = Vec::new();
    for claim in claims {
        if !seen.contains(&&claim.reference) {
            seen.push(&claim.reference);
            let _ = resolve_claim(claims, &claim.reference)?;
        }
    }
    Ok(())
}

/// Whole-record validation for a [`SpawnSource`]: capabilities are catalog
/// known, functional values match their definition shape, and functional
/// declarations stay within the provisionable envelope. Security-class
/// declarations are proven by evidence, not by the envelope.
pub fn validate_spawn_source(
    source: &SpawnSource,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    validate_claims(&source.claims, catalog)?;
    for (reference, value) in &source.functional_envelope {
        // Bool(false) is absence: normalize it away.
        if !value.is_present() {
            continue;
        }
        let definition =
            catalog
                .get(reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: capability_key(reference),
                })?;
        if definition.matcher_kind != value.matcher_kind() {
            return Err(ContractError::InvariantViolation(format!(
                "source envelope value shape does not match the definition for {}@{}",
                reference.capability_id().as_str(),
                reference.revision()
            )));
        }
    }
    for claim in &source.claims {
        if !claim.value.is_present() {
            continue;
        }
        let requires_evidence = catalog
            .get(&claim.reference)
            .map(|d| d.security_class.requires_evidence())
            .unwrap_or(false);
        if requires_evidence {
            continue;
        }
        let ceiling = source
            .functional_envelope
            .get(&claim.reference)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: capability_key(&claim.reference),
            })?;
        if !value_within(ceiling, &claim.value) {
            return Err(ContractError::InvariantViolation(format!(
                "source declaration for {} exceeds its provisionable envelope",
                claim.reference.capability_id().as_str()
            )));
        }
    }
    Ok(())
}

/// Whole-record validation for a [`SourceConfig`] against its source: claims
/// are catalog known and functional declarations stay within the source
/// envelope, independent of which capabilities the AgentType happens to use.
pub fn validate_source_config(
    config: &SourceConfig,
    source: &SpawnSource,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    if config.config_ref.source() != &source.source_ref {
        return Err(ContractError::SourceConfigInvalid {
            reason: "config does not belong to this exact source revision".into(),
        });
    }
    // A config may narrow the source envelope, never widen it.
    if let Some(modes) = &config.lifecycle_modes {
        if !modes.is_subset(&source.lifecycle_modes) {
            return Err(ContractError::SourceConfigInvalid {
                reason: "config lifecycle modes widen the source envelope".into(),
            });
        }
    }
    if let Some(modes) = &config.continuity_modes {
        // Continuity is an ordered minimum: a config may require a mode that the
        // source can satisfy with a mode at least as strong (e.g. source
        // {Logical} permits a config {None}).
        let satisfiable = modes
            .iter()
            .all(|needed| source.continuity_modes.iter().any(|have| have >= needed));
        if !satisfiable {
            return Err(ContractError::SourceConfigInvalid {
                reason: "config continuity modes exceed the source envelope".into(),
            });
        }
    }
    validate_claims(&config.claims, catalog)?;
    for claim in &config.claims {
        if !claim.value.is_present() {
            continue;
        }
        let requires_evidence = catalog
            .get(&claim.reference)
            .map(|d| d.security_class.requires_evidence())
            .unwrap_or(false);
        if requires_evidence {
            continue;
        }
        let ceiling = source
            .functional_envelope
            .get(&claim.reference)
            .ok_or_else(|| ContractError::CapabilityMismatch {
                capability: capability_key(&claim.reference),
            })?;
        if !value_within(ceiling, &claim.value) {
            return Err(ContractError::InvariantViolation(format!(
                "config declaration for {} exceeds the source envelope",
                claim.reference.capability_id().as_str()
            )));
        }
    }
    Ok(())
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
        // Bool(false) is absence: a Task that expresses "no requirement" for a
        // capability is exactly a Task that omits it.
        if !required.is_present() {
            continue;
        }
        let definition =
            catalog
                .get(reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                })?;
        // Fail closed on a malformed value shape before any polarity handling.
        if definition.matcher_kind != required.matcher_kind() {
            return Err(ContractError::InvariantViolation(format!(
                "Task capability value shape does not match the definition for {}@{}",
                reference.capability_id().as_str(),
                reference.revision()
            )));
        }
        // Restriction capabilities are effective policy the Task may add or
        // strengthen; they are composed with the AgentType and proven in
        // `can_provision_task`, not required to be pre-advertised here.
        if definition.polarity == CapabilityPolarity::Restriction {
            continue;
        }
        let provided =
            present_cap(&agent.contract.required_capabilities, reference).ok_or_else(|| {
                ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                }
            })?;
        if !value_satisfies(definition.matcher_kind, required, provided) {
            return Err(ContractError::CapabilityMismatch {
                capability: reference.capability_id().as_str().to_string(),
            });
        }
    }

    if !affinity_accepts(&agent.contract.affinity, &req.required_affinity) {
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
    // Whole-record validation runs before per-requirement checks, so an illegal
    // declaration (including config/source ownership) fails even when no
    // AgentType requires that capability.
    validate_spawn_source(source, catalog)?;
    validate_source_config(config, source, catalog)?;
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

    if !agent
        .contract
        .lifecycle
        .is_subset(config.effective_lifecycle(source))
    {
        return Err(ContractError::CapabilityMismatch {
            capability: "lifecycle".into(),
        });
    }
    if !continuity_satisfies(
        config.effective_continuity(source),
        agent.contract.continuity,
    ) {
        return Err(ContractError::CapabilityMismatch {
            capability: "continuity".into(),
        });
    }

    for (reference, required) in &agent.contract.required_capabilities {
        // Bool(false) is absence and imposes no provisioning requirement.
        if !required.is_present() {
            continue;
        }
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
        let ceiling = present_cap(&source.functional_envelope, reference).ok_or_else(|| {
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

/// Physical eligibility for a specific Task.
///
/// `can_execute(agent, task) && can_provision(agent, source, config, evidence)`
/// is **necessary but not sufficient**: the imported environment must also be
/// able to enforce the Task's effective (stricter) restrictions, not merely the
/// AgentType ceiling. This is the frozen contract/sandbox eligibility seam: a
/// mandatory conjunct that B.3/B.4 must include, not the complete physical
/// candidate eligibility decision (which also needs adapter/credential checks).
pub fn can_provision_task(
    agent: &AgentType,
    source: &SpawnSource,
    config: &SourceConfig,
    evidence: &ResolvedProvisioningEvidence,
    catalog: &CapabilityCatalog,
    task: &TaskRequirement,
) -> Result<(), ContractError> {
    can_execute(agent, task, catalog)?;
    can_provision(agent, source, config, evidence, catalog)?;

    let safety = evidence.enforceable_safety();
    if !safety.enforces_workspace(task.required_workspace) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "environment cannot enforce the Task workspace mode".into(),
        });
    }
    if !safety.enforces_network(task.required_network) {
        return Err(ContractError::SecurityUnenforceable {
            reason: "environment cannot enforce the Task network policy".into(),
        });
    }
    if let Some(policy) = &task.sandbox_policy {
        if !evidence.enforces_sandbox_policy(policy) {
            return Err(ContractError::SecurityUnenforceable {
                reason: format!(
                    "Task sandbox policy {}@{} is not enforced by imported evidence",
                    policy.id().as_str(),
                    policy.revision()
                ),
            });
        }
    }
    if !continuity_satisfies(
        config.effective_continuity(source),
        task.required_continuity,
    ) {
        return Err(ContractError::CapabilityMismatch {
            capability: "continuity".into(),
        });
    }

    // Task restrictions compose with the AgentType envelope (join). The joined
    // effective restriction is proven by imported evidence for security classes
    // and by the effective source/config functional value otherwise, keeping
    // SecurityClass (proof authority) orthogonal to polarity (composition).
    for (reference, task_value) in &task.required_capabilities {
        // Bool(false) is absence: a Task expressing "no restriction" must not
        // demand an enforcement proof for a capability it does not require.
        if !task_value.is_present() {
            continue;
        }
        let definition =
            catalog
                .get(reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: capability_key(reference),
                })?;
        if definition.polarity != CapabilityPolarity::Restriction {
            continue;
        }
        let effective = match present_cap(&agent.contract.required_capabilities, reference) {
            Some(agent_value) => restriction_join(definition.matcher_kind, agent_value, task_value)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: capability_key(reference),
                })?,
            None => task_value.clone(),
        };

        if definition.security_class.requires_evidence() {
            let provided = evidence.enforced_capability(reference).ok_or_else(|| {
                ContractError::SecurityUnenforceable {
                    reason: format!(
                        "Task restriction {}@{} has no imported enforcement evidence",
                        reference.capability_id().as_str(),
                        reference.revision()
                    ),
                }
            })?;
            if !value_satisfies(definition.matcher_kind, &effective, provided) {
                return Err(ContractError::CapabilityMismatch {
                    capability: capability_key(reference),
                });
            }
        } else {
            let provided = effective_functional_value(source, config, reference)?;
            if !value_satisfies(definition.matcher_kind, &effective, provided) {
                return Err(ContractError::CapabilityMismatch {
                    capability: capability_key(reference),
                });
            }
        }
    }

    Ok(())
}

/// The effective functional value a source/config can provide: the config
/// declaration, else the source declaration, else the source envelope ceiling.
fn effective_functional_value<'a>(
    source: &'a SpawnSource,
    config: &'a SourceConfig,
    reference: &CapabilityRef,
) -> Result<&'a CapabilityValue, ContractError> {
    let ceiling = present_cap(&source.functional_envelope, reference).ok_or_else(|| {
        ContractError::CapabilityMismatch {
            capability: capability_key(reference),
        }
    })?;
    let source_claim = resolve_claim(&source.claims, reference)?;
    let config_claim = resolve_claim(&config.claims, reference)?;
    Ok(config_claim
        .map(|c| &c.value)
        .or_else(|| source_claim.map(|c| &c.value))
        .unwrap_or(ceiling))
}

/// Deterministic join of two restriction values (the effective, strongest
/// restriction). `None` means the two values are incompatible.
fn restriction_join(
    matcher: MatcherKind,
    a: &CapabilityValue,
    b: &CapabilityValue,
) -> Option<CapabilityValue> {
    match (matcher, a, b) {
        (MatcherKind::Bool, CapabilityValue::Bool(x), CapabilityValue::Bool(y)) => {
            Some(CapabilityValue::Bool(*x || *y))
        }
        (MatcherKind::Set, CapabilityValue::Set(x), CapabilityValue::Set(y)) => {
            Some(CapabilityValue::Set(x.union(y).cloned().collect()))
        }
        (
            MatcherKind::Ordered,
            CapabilityValue::Ordered {
                class: ac,
                rank: ar,
            },
            CapabilityValue::Ordered {
                class: bc,
                rank: br,
            },
        ) if ac == bc => Some(CapabilityValue::Ordered {
            class: ac.clone(),
            rank: (*ar).max(*br),
        }),
        (MatcherKind::Quantity, CapabilityValue::Quantity(x), CapabilityValue::Quantity(y)) => {
            Some(CapabilityValue::Quantity(if x.get() >= y.get() {
                *x
            } else {
                *y
            }))
        }
        (MatcherKind::Exact, x, y) if x == y => Some(x.clone()),
        _ => None,
    }
}

/// `Ability` order: `derived` advertises no more than `base`. A smaller/equal
/// value executes a subset of the Tasks the base executes.
fn ability_value_no_wider(
    matcher: MatcherKind,
    base: &CapabilityValue,
    derived: &CapabilityValue,
) -> bool {
    match (matcher, base, derived) {
        // Presence: `derived` may advertise no more than `base`
        // (`derived implies base`).
        (MatcherKind::Bool, CapabilityValue::Bool(b), CapabilityValue::Bool(d)) => !*d || *b,
        (MatcherKind::Exact, b, d) => b == d,
        (MatcherKind::Set, CapabilityValue::Set(b), CapabilityValue::Set(d)) => d.is_subset(b),
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
        ) => bc == dc && dr <= br,
        (MatcherKind::Quantity, CapabilityValue::Quantity(b), CapabilityValue::Quantity(d)) => {
            d.get() <= b.get()
        }
        _ => false,
    }
}

/// `Restriction` order: `derived` is at least as restrictive as `base`.
fn restriction_value_no_wider(
    matcher: MatcherKind,
    base: &CapabilityValue,
    derived: &CapabilityValue,
) -> bool {
    match (matcher, base, derived) {
        // Presence-derived order: `derived` at least as restrictive as `base`
        // (`base implies derived`). base=false -> derived=true is a new
        // restriction (narrower); base=true -> derived=false weakens it.
        (MatcherKind::Bool, CapabilityValue::Bool(b), CapabilityValue::Bool(d)) => !*b || *d,
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

/// `derived` must not widen. Whether a capability may be added/dropped or
/// raised/lowered depends on the catalog-owned polarity: abilities narrow by
/// shrinking, restrictions narrow by growing.
fn capability_provision_no_wider(
    derived: &AgentTypeContract,
    base: &AgentTypeContract,
    catalog: &CapabilityCatalog,
) -> bool {
    // A capability `derived` adds that `base` lacks. `Bool(false)` is absence
    // on both sides, so a missing entry and a `Bool(false)` entry are the same
    // state and are never treated as an addition or a drop.
    for (reference, derived_value) in &derived.required_capabilities {
        if !derived_value.is_present() {
            continue;
        }
        if present_cap(&base.required_capabilities, reference).is_some() {
            continue;
        }
        let Some(definition) = catalog.get(reference) else {
            return false;
        };
        if definition.polarity == CapabilityPolarity::Ability {
            // Adding an ability widens.
            return false;
        }
        // Adding a restriction narrows: allowed.
    }

    for (reference, base_value) in &base.required_capabilities {
        if !base_value.is_present() {
            continue;
        }
        let Some(definition) = catalog.get(reference) else {
            return false;
        };
        let Some(derived_value) = present_cap(&derived.required_capabilities, reference) else {
            // Dropping an ability narrows (allowed); dropping a restriction
            // weakens the guarantee (rejected).
            if definition.polarity == CapabilityPolarity::Restriction {
                return false;
            }
            continue;
        };
        let no_wider = match definition.polarity {
            CapabilityPolarity::Ability => {
                ability_value_no_wider(definition.matcher_kind, base_value, derived_value)
            }
            CapabilityPolarity::Restriction => {
                restriction_value_no_wider(definition.matcher_kind, base_value, derived_value)
            }
        };
        if !no_wider {
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

/// Does an affinity constraint accept a Task's required tags? `Any` accepts
/// every Task; `Only(S)` accepts a subset of `S`.
fn affinity_accepts(
    constraint: &AffinityConstraint,
    required: &std::collections::BTreeSet<String>,
) -> bool {
    match constraint {
        AffinityConstraint::Any => true,
        AffinityConstraint::Only(allowed) => required.is_subset(allowed),
    }
}

/// `a` grants no broader affinity than `b` (`Any` is the top element).
fn affinity_no_wider(a: &AffinityConstraint, b: &AffinityConstraint) -> bool {
    match (a, b) {
        (AffinityConstraint::Any, AffinityConstraint::Any) => true,
        (AffinityConstraint::Any, AffinityConstraint::Only(_)) => false,
        (AffinityConstraint::Only(_), AffinityConstraint::Any) => true,
        (AffinityConstraint::Only(x), AffinityConstraint::Only(y)) => x.is_subset(y),
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

/// A grants no more authority/semantic scope than B on every relevant dimension.
///
/// `continuity` is deliberately **not** part of this order: it is a
/// provisioning-guarantee dimension (a stronger guarantee lets a type execute
/// more continuity-requiring Tasks), so it belongs to candidate ranking, not to
/// semantic specificity. Two types differing only in continuity are incomparable
/// for `more_specific_for`.
fn authority_no_wider(
    a: &AgentTypeContract,
    b: &AgentTypeContract,
    catalog: &CapabilityCatalog,
) -> bool {
    a.allowed_information_functions
        .iter()
        .all(|f| b.allowed_information_functions.contains(f))
        && affinity_no_wider(&a.affinity, &b.affinity)
        && a.budget_ceiling <= b.budget_ceiling
        && a.lifecycle.is_subset(&b.lifecycle)
        && workspace_rank(a.security.workspace) <= workspace_rank(b.security.workspace)
        && network_rank(a.security.network) <= network_rank(b.security.network)
        && (!b.security.requires_attempt_isolation || a.security.requires_attempt_isolation)
        && sandbox_policy_no_wider(&a.sandbox_policy, &b.sandbox_policy)
        && anchor_no_wider(&a.anchor_constraint, &b.anchor_constraint)
        && capability_provision_no_wider(a, b, catalog)
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

    // Affinity is an allowed-tag ceiling: a derived type may only narrow, so
    // refinement never enlarges the executable Task set.
    if !affinity_no_wider(&derived_c.affinity, &base_c.affinity) {
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
    if !capability_provision_no_wider(derived_c, base_c, catalog) {
        return invalid("derived capability constraints are wider than base");
    }

    Ok(())
}
