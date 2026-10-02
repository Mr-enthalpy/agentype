//! Canonical encoding and content digests for M6-B catalog revisions.
//!
//! A durable `(exact ref, content digest)` pair MUST be immutable: two records
//! that denote the same revision content MUST encode to byte-identical canonical
//! JSON, and two records with different content MUST NOT share a digest. This
//! module owns that one definition so `agentype-agent-contract` value types and
//! the schema v6 persistence layer cannot drift apart.
//!
//! Rules enforced before encoding:
//! - set-like sequences (`claims`, `credential_refs`) are sorted and deduped,
//!   so a permutation of the same logical content digests identically;
//! - `Bool(false)` is canonical absence and is normalized to omission only after
//!   the [`CapabilityCatalog`] resolves the exact capability and confirms a
//!   matching `Bool` shape — an unknown or malformed capability fails closed;
//! - at most one value per exact [`CapabilityRef`], with the `ENFORCED` claim
//!   preferred when declarations agree and a conflict rejected otherwise.
//!
//! This module performs **no** I/O and never mints enforcement evidence.

use crate::capability::{
    Assurance, CapabilityCatalog, CapabilityClaim, CapabilityDefinition, CapabilityPolarity,
    CapabilityRef, CapabilityValue, MatcherKind, SecurityClass,
};
use crate::error::ContractError;
use crate::predicates::{validate_source_config, validate_spawn_source};
use crate::records::{
    AdapterBindingPolicy, AffinityConstraint, AgentType, AgentTypeContract, ContinuityMode,
    CredentialRef, LifecycleMode, NetworkPolicy, SourceConfig, SpawnSource,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Version tag embedded in every canonical document. A future encoding change
/// bumps this so old and new digests can never be confused.
pub const CANONICAL_FORMAT_VERSION: &str = "agentype-contract/1";

// =============================================================================
// Normalization (must run before encoding)
// =============================================================================

fn shape_check(
    reference: &CapabilityRef,
    value: &CapabilityValue,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    let definition = catalog
        .get(reference)
        .ok_or_else(|| ContractError::CapabilityMismatch {
            capability: reference.capability_id().as_str().to_string(),
        })?;
    if definition.matcher_kind != value.matcher_kind() {
        return Err(ContractError::InvariantViolation(format!(
            "capability value shape does not match the definition for {}@{}",
            reference.capability_id().as_str(),
            reference.revision()
        )));
    }
    Ok(())
}

/// Sort/dedup a claim list into its canonical form: at most one claim per exact
/// reference, `Bool(false)` (absence) dropped, and conflicting values rejected.
pub fn canonicalize_claims(
    claims: &mut Vec<CapabilityClaim>,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    let mut kept: Vec<CapabilityClaim> = Vec::with_capacity(claims.len());
    for claim in claims.drain(..) {
        // Resolve and shape-check BEFORE absence normalization.
        shape_check(&claim.reference, &claim.value, catalog)?;
        if !claim.value.is_present() {
            continue;
        }
        kept.push(claim);
    }
    kept.sort_by(|a, b| {
        (a.reference.capability_id().as_str(), a.reference.revision())
            .cmp(&(b.reference.capability_id().as_str(), b.reference.revision()))
    });

    let mut canonical: Vec<CapabilityClaim> = Vec::with_capacity(kept.len());
    let mut index = 0;
    while index < kept.len() {
        let mut end = index + 1;
        while end < kept.len() && kept[end].reference == kept[index].reference {
            end += 1;
        }
        let group = &kept[index..end];
        let first = &group[0];
        if group.iter().any(|claim| claim.value != first.value) {
            return Err(ContractError::InvariantViolation(format!(
                "conflicting capability claims for {}@{}",
                first.reference.capability_id().as_str(),
                first.reference.revision()
            )));
        }
        // Deterministic pick: prefer the ENFORCED declaration, else the first.
        let chosen = group
            .iter()
            .find(|claim| claim.assurance == Assurance::Enforced)
            .unwrap_or(first);
        canonical.push(chosen.clone());
        index = end;
    }
    *claims = canonical;
    Ok(())
}

/// Canonicalize a keyed capability map: drop absent (`Bool(false)`) entries and
/// shape-check every remaining value against the catalog.
pub fn canonicalize_capability_map(
    map: &mut BTreeMap<CapabilityRef, CapabilityValue>,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    let mut out = BTreeMap::new();
    for (reference, value) in std::mem::take(map) {
        shape_check(&reference, &value, catalog)?;
        if !value.is_present() {
            continue;
        }
        out.insert(reference, value);
    }
    *map = out;
    Ok(())
}

/// Sort/dedup opaque credential references (set-like content).
pub fn canonicalize_credential_refs(refs: &mut Vec<CredentialRef>) {
    refs.sort();
    refs.dedup();
}

/// Fully canonicalize an AgentType revision record.
pub fn canonicalize_agent_type(
    agent: &mut AgentType,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    agent.contract.validate(catalog)?;
    agent.contract.normalize();
    canonicalize_capability_map(&mut agent.contract.required_capabilities, catalog)
}

/// Fully canonicalize a SpawnSource revision record (disposition excluded).
pub fn canonicalize_spawn_source(
    source: &mut SpawnSource,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    validate_spawn_source(source, catalog)?;
    canonicalize_claims(&mut source.claims, catalog)?;
    canonicalize_capability_map(&mut source.functional_envelope, catalog)
}

/// Fully canonicalize a SourceConfig revision record (disposition excluded).
pub fn canonicalize_source_config(
    config: &mut SourceConfig,
    source: &SpawnSource,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    validate_source_config(config, source, catalog)?;
    canonicalize_claims(&mut config.claims, catalog)?;
    canonicalize_credential_refs(&mut config.credential_refs);
    Ok(())
}

// =============================================================================
// Canonical value construction
// =============================================================================

fn matcher_str(kind: MatcherKind) -> &'static str {
    match kind {
        MatcherKind::Bool => "BOOL",
        MatcherKind::Set => "SET",
        MatcherKind::Ordered => "ORDERED",
        MatcherKind::Quantity => "QUANTITY",
        MatcherKind::Exact => "EXACT",
    }
}

fn class_str(class: SecurityClass) -> &'static str {
    match class {
        SecurityClass::Functional => "FUNCTIONAL",
        SecurityClass::Authority => "AUTHORITY",
        SecurityClass::Sandbox => "SANDBOX",
        SecurityClass::Continuity => "CONTINUITY",
    }
}

fn polarity_str(polarity: CapabilityPolarity) -> &'static str {
    match polarity {
        CapabilityPolarity::Ability => "ABILITY",
        CapabilityPolarity::Restriction => "RESTRICTION",
    }
}

fn assurance_str(assurance: Assurance) -> &'static str {
    match assurance {
        Assurance::Declared => "DECLARED",
        Assurance::Enforced => "ENFORCED",
    }
}

fn lifecycle_str(mode: LifecycleMode) -> &'static str {
    match mode {
        LifecycleMode::Ephemeral => "EPHEMERAL",
        LifecycleMode::Resident => "RESIDENT",
        LifecycleMode::Revivable => "REVIVABLE",
    }
}

fn continuity_str(mode: ContinuityMode) -> &'static str {
    match mode {
        ContinuityMode::None => "NONE",
        ContinuityMode::Logical => "LOGICAL",
    }
}

fn network_str(policy: NetworkPolicy) -> &'static str {
    match policy {
        NetworkPolicy::Disabled => "DISABLED",
        NetworkPolicy::Restricted => "RESTRICTED",
        NetworkPolicy::Enabled => "ENABLED",
    }
}

fn capability_value_value(value: &CapabilityValue) -> Value {
    match value {
        CapabilityValue::Bool(present) => json!({"kind": "BOOL", "value": present}),
        CapabilityValue::Set(items) => json!({
            "kind": "SET",
            "value": items.iter().collect::<Vec<_>>(),
        }),
        CapabilityValue::Ordered { class, rank } => {
            json!({"kind": "ORDERED", "class": class, "rank": rank})
        }
        CapabilityValue::Quantity(quantity) => {
            json!({"kind": "QUANTITY", "value": quantity.get()})
        }
        CapabilityValue::Exact(exact) => json!({"kind": "EXACT", "value": exact}),
    }
}

fn claim_value(claim: &CapabilityClaim) -> Value {
    json!({
        "capability_id": claim.reference.capability_id().as_str(),
        "revision": claim.reference.revision(),
        "value": capability_value_value(&claim.value),
        "assurance": assurance_str(claim.assurance),
        "declaration_provenance_ref": claim.declaration_provenance_ref,
    })
}

fn claims_value(claims: &[CapabilityClaim]) -> Value {
    Value::Array(claims.iter().map(claim_value).collect())
}

fn capability_map_value(map: &BTreeMap<CapabilityRef, CapabilityValue>) -> Value {
    Value::Array(
        map.iter()
            .map(|(reference, value)| {
                json!({
                    "capability_id": reference.capability_id().as_str(),
                    "revision": reference.revision(),
                    "value": capability_value_value(value),
                })
            })
            .collect(),
    )
}

fn affinity_value(affinity: &AffinityConstraint) -> Value {
    match affinity {
        AffinityConstraint::Any => json!({"kind": "ANY"}),
        AffinityConstraint::Only(tags) => {
            json!({"kind": "ONLY", "tags": tags.iter().collect::<Vec<_>>()})
        }
    }
}

fn contract_value(contract: &AgentTypeContract) -> Value {
    json!({
        "allowed_information_functions": contract
            .allowed_information_functions
            .iter()
            .map(|f| f.as_sql())
            .collect::<Vec<_>>(),
        "required_capabilities": capability_map_value(&contract.required_capabilities),
        "affinity": affinity_value(&contract.affinity),
        "budget_ceiling": contract.budget_ceiling.get(),
        "security": {
            "workspace": contract.security.workspace.as_sql(),
            "network": network_str(contract.security.network),
            "requires_attempt_isolation": contract.security.requires_attempt_isolation,
        },
        "lifecycle": contract.lifecycle.iter().map(|m| lifecycle_str(*m)).collect::<Vec<_>>(),
        "continuity": continuity_str(contract.continuity),
        "sandbox_policy": contract.sandbox_policy.as_ref().map(|p| json!({
            "sandbox_policy_id": p.id().as_str(),
            "revision": p.revision(),
        })),
        "anchor_constraint": contract.anchor_constraint,
    })
}

fn to_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("canonical JSON is always serializable")
}

// =============================================================================
// Byte encoders (input MUST already be canonicalized)
// =============================================================================

pub fn canonical_agent_type_bytes(agent: &AgentType) -> Vec<u8> {
    to_bytes(json!({
        "canonical": CANONICAL_FORMAT_VERSION,
        "kind": "AGENT_TYPE",
        "type_id": agent.type_ref.id().as_str(),
        "revision": agent.type_ref.revision(),
        "based_on": agent.based_on.as_ref().map(|base| json!({
            "type_id": base.id().as_str(),
            "revision": base.revision(),
        })),
        "contract": contract_value(&agent.contract),
    }))
}

pub fn canonical_spawn_source_bytes(source: &SpawnSource) -> Vec<u8> {
    to_bytes(json!({
        "canonical": CANONICAL_FORMAT_VERSION,
        "kind": "SPAWN_SOURCE",
        "source_id": source.source_ref.id().as_str(),
        "revision": source.source_ref.revision(),
        "adapter_policy": {
            "adapter_policy_id": source.adapter_policy.id().as_str(),
            "revision": source.adapter_policy.revision(),
        },
        "lifecycle_modes": source.lifecycle_modes.iter().map(|m| lifecycle_str(*m)).collect::<Vec<_>>(),
        "continuity_modes": source.continuity_modes.iter().map(|m| continuity_str(*m)).collect::<Vec<_>>(),
        "functional_envelope": capability_map_value(&source.functional_envelope),
        "claims": claims_value(&source.claims),
    }))
}

pub fn canonical_source_config_bytes(config: &SourceConfig) -> Vec<u8> {
    to_bytes(json!({
        "canonical": CANONICAL_FORMAT_VERSION,
        "kind": "SOURCE_CONFIG",
        "source_id": config.config_ref.source().id().as_str(),
        "source_revision": config.config_ref.source().revision(),
        "config_id": config.config_ref.config_id().as_str(),
        "revision": config.config_ref.revision(),
        "config_digest": config.config_digest.as_str(),
        "lifecycle_modes": config.lifecycle_modes.as_ref().map(|modes| {
            modes.iter().map(|m| lifecycle_str(*m)).collect::<Vec<_>>()
        }),
        "continuity_modes": config.continuity_modes.as_ref().map(|modes| {
            modes.iter().map(|m| continuity_str(*m)).collect::<Vec<_>>()
        }),
        "credential_refs": config.credential_refs.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        "claims": claims_value(&config.claims),
    }))
}

pub fn canonical_adapter_binding_policy_bytes(policy: &AdapterBindingPolicy) -> Vec<u8> {
    to_bytes(json!({
        "canonical": CANONICAL_FORMAT_VERSION,
        "kind": "ADAPTER_BINDING_POLICY",
        "adapter_policy_id": policy.policy_ref.id().as_str(),
        "revision": policy.policy_ref.revision(),
        "adapter_kind": policy.adapter_kind,
        "binding_ref": policy.binding_ref,
        "required_safety": {
            "attempt_isolation": policy.required_safety.attempt_isolation(),
            "workspace_modes": policy
                .required_safety
                .enforceable_workspace_modes()
                .iter()
                .map(|m| m.as_sql())
                .collect::<Vec<_>>(),
            "network_modes": policy
                .required_safety
                .enforceable_network_modes()
                .iter()
                .map(|m| network_str(*m))
                .collect::<Vec<_>>(),
        },
    }))
}

pub fn canonical_capability_definition_bytes(
    reference: &CapabilityRef,
    definition: &CapabilityDefinition,
) -> Vec<u8> {
    to_bytes(json!({
        "canonical": CANONICAL_FORMAT_VERSION,
        "kind": "CAPABILITY_DEFINITION",
        "capability_id": reference.capability_id().as_str(),
        "revision": reference.revision(),
        "matcher_kind": matcher_str(definition.matcher_kind),
        "security_class": class_str(definition.security_class),
        "polarity": polarity_str(definition.polarity),
    }))
}

// =============================================================================
// Digests
// =============================================================================

/// Content-addressed digest of canonical bytes, `sha256:`-prefixed lowercase hex.
pub fn content_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Digest of an opaque JSON body. `serde_json::Value` objects serialize with
/// sorted keys, so the digest is permutation-stable and can validate a
/// `SourceConfig`'s `OpaqueJson` payload against its declared `config_digest`.
pub fn canonical_json_body_digest(value: &Value) -> String {
    content_digest(&serde_json::to_vec(value).expect("opaque JSON body is serializable"))
}

pub fn agent_type_content_digest(agent: &AgentType) -> String {
    content_digest(&canonical_agent_type_bytes(agent))
}

pub fn spawn_source_content_digest(source: &SpawnSource) -> String {
    content_digest(&canonical_spawn_source_bytes(source))
}

pub fn source_config_content_digest(config: &SourceConfig) -> String {
    content_digest(&canonical_source_config_bytes(config))
}

pub fn adapter_binding_policy_content_digest(policy: &AdapterBindingPolicy) -> String {
    content_digest(&canonical_adapter_binding_policy_bytes(policy))
}

pub fn capability_definition_content_digest(
    reference: &CapabilityRef,
    definition: &CapabilityDefinition,
) -> String {
    content_digest(&canonical_capability_definition_bytes(
        reference, definition,
    ))
}
