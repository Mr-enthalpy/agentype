//! Decoders for the canonical JSON documents produced by [`crate::canonical`].
//!
//! The canonical document is the durable representation of an immutable
//! revision. Decoding validates the canonical envelope (`canonical` version and
//! `kind`) and every field the decoder consumes: missing fields, wrong shapes,
//! unknown enum spellings, and an empty locator fail closed. Unknown **extra**
//! fields are ignored here. A caller that treats the document as durability
//! authority MUST also verify the stored content digest — and, at the catalog
//! boundary, that re-encoding the decoded record reproduces the stored canonical
//! bytes (see `agentype-storage-sqlite::catalog`); the decoders alone are not
//! an integrity proof.
//!
//! SpawnSource / SourceConfig / AdapterBindingPolicy carry a mutable disposition
//! that is deliberately not part of the canonical content; the decoders take the
//! disposition as an explicit argument so content and overlay stay separate.

use crate::canonical::CANONICAL_FORMAT_VERSION;
use crate::capability::{
    Assurance, CapabilityClaim, CapabilityDefinition, CapabilityPolarity, CapabilityRef,
    CapabilityValue, MatcherKind, Quantity, SecurityClass,
};
use crate::error::ContractError;
use crate::records::{
    workspace_rank, AdapterBindingPolicy, AdapterPolicyRef, AffinityConstraint, AgentType,
    AgentTypeContract, AgentTypeRef, Budget, ConfigDigest, ConfigStatus, ContinuityMode,
    CredentialRef, LifecycleMode, NetworkPolicy, PhysicalSafety, SandboxPolicyRef,
    SecurityContract, SourceConfig, SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef,
    TaskRequirement,
};
use crate::requirement::{GenerationPolicy, TaskAgentRequirement};
use agentype_core::{InformationFunction, WorkspaceMode};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

fn err(msg: impl Into<String>) -> ContractError {
    ContractError::InvariantViolation(msg.into())
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, ContractError> {
    value
        .as_object()
        .ok_or_else(|| err(format!("{what} must be a JSON object")))
}

fn field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value, ContractError> {
    object
        .get(key)
        .ok_or_else(|| err(format!("missing canonical field {key}")))
}

fn string_of(value: &Value, what: &str) -> Result<String, ContractError> {
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| err(format!("{what} must be a string")))
}

fn u64_of(value: &Value, what: &str) -> Result<u64, ContractError> {
    value
        .as_u64()
        .ok_or_else(|| err(format!("{what} must be an unsigned integer")))
}

fn f64_of(value: &Value, what: &str) -> Result<f64, ContractError> {
    value
        .as_f64()
        .ok_or_else(|| err(format!("{what} must be a number")))
}

fn array<'a>(value: &'a Value, what: &str) -> Result<&'a Vec<Value>, ContractError> {
    value
        .as_array()
        .ok_or_else(|| err(format!("{what} must be an array")))
}

fn opt_string(value: &Value) -> Result<Option<String>, ContractError> {
    match value {
        Value::Null => Ok(None),
        other => Ok(Some(string_of(other, "optional string")?)),
    }
}

/// An optional opaque locator: `null` means absent, a non-empty string is a
/// location. An empty or whitespace-only locator fails closed.
fn optional_locator(value: &Value) -> Result<Option<String>, ContractError> {
    match opt_string(value)? {
        None => Ok(None),
        Some(locator) if locator.trim().is_empty() => {
            Err(err("config_locator must not be empty when present"))
        }
        Some(locator) => Ok(Some(locator)),
    }
}

fn check_envelope(object: &Map<String, Value>, kind: &str) -> Result<(), ContractError> {
    let canonical = string_of(field(object, "canonical")?, "canonical version")?;
    if canonical != CANONICAL_FORMAT_VERSION {
        return Err(err(format!(
            "unsupported canonical format {canonical}, expected {CANONICAL_FORMAT_VERSION}"
        )));
    }
    let actual = string_of(field(object, "kind")?, "canonical kind")?;
    if actual != kind {
        return Err(err(format!("canonical kind {actual} is not {kind}")));
    }
    Ok(())
}

fn information_function(value: &str) -> Result<InformationFunction, ContractError> {
    InformationFunction::parse_sql(value).map_err(|e| err(e.to_string()))
}

fn workspace_mode(value: &str) -> Result<WorkspaceMode, ContractError> {
    WorkspaceMode::parse_sql(value).map_err(|e| err(e.to_string()))
}

fn network_policy(value: &str) -> Result<NetworkPolicy, ContractError> {
    match value {
        "DISABLED" => Ok(NetworkPolicy::Disabled),
        "RESTRICTED" => Ok(NetworkPolicy::Restricted),
        "ENABLED" => Ok(NetworkPolicy::Enabled),
        other => Err(err(format!("unknown NetworkPolicy {other}"))),
    }
}

fn lifecycle_mode(value: &str) -> Result<LifecycleMode, ContractError> {
    match value {
        "EPHEMERAL" => Ok(LifecycleMode::Ephemeral),
        "RESIDENT" => Ok(LifecycleMode::Resident),
        "REVIVABLE" => Ok(LifecycleMode::Revivable),
        other => Err(err(format!("unknown LifecycleMode {other}"))),
    }
}

fn continuity_mode(value: &str) -> Result<ContinuityMode, ContractError> {
    match value {
        "NONE" => Ok(ContinuityMode::None),
        "LOGICAL" => Ok(ContinuityMode::Logical),
        other => Err(err(format!("unknown ContinuityMode {other}"))),
    }
}

fn matcher_kind(value: &str) -> Result<MatcherKind, ContractError> {
    match value {
        "BOOL" => Ok(MatcherKind::Bool),
        "SET" => Ok(MatcherKind::Set),
        "ORDERED" => Ok(MatcherKind::Ordered),
        "QUANTITY" => Ok(MatcherKind::Quantity),
        "EXACT" => Ok(MatcherKind::Exact),
        other => Err(err(format!("unknown MatcherKind {other}"))),
    }
}

fn security_class(value: &str) -> Result<SecurityClass, ContractError> {
    match value {
        "FUNCTIONAL" => Ok(SecurityClass::Functional),
        "AUTHORITY" => Ok(SecurityClass::Authority),
        "SANDBOX" => Ok(SecurityClass::Sandbox),
        "CONTINUITY" => Ok(SecurityClass::Continuity),
        other => Err(err(format!("unknown SecurityClass {other}"))),
    }
}

fn capability_polarity(value: &str) -> Result<CapabilityPolarity, ContractError> {
    match value {
        "ABILITY" => Ok(CapabilityPolarity::Ability),
        "RESTRICTION" => Ok(CapabilityPolarity::Restriction),
        other => Err(err(format!("unknown CapabilityPolarity {other}"))),
    }
}

fn assurance(value: &str) -> Result<Assurance, ContractError> {
    match value {
        "DECLARED" => Ok(Assurance::Declared),
        "ENFORCED" => Ok(Assurance::Enforced),
        other => Err(err(format!("unknown Assurance {other}"))),
    }
}

fn capability_value(value: &Value) -> Result<CapabilityValue, ContractError> {
    let object = object(value, "capability value")?;
    let kind = string_of(field(object, "kind")?, "capability value kind")?;
    match kind.as_str() {
        "BOOL" => Ok(CapabilityValue::Bool(
            field(object, "value")?
                .as_bool()
                .ok_or_else(|| err("BOOL value must be boolean"))?,
        )),
        "SET" => {
            let mut items = BTreeSet::new();
            for item in array(field(object, "value")?, "SET value")? {
                items.insert(string_of(item, "SET item")?);
            }
            Ok(CapabilityValue::Set(items))
        }
        "ORDERED" => {
            let rank = u64_of(field(object, "rank")?, "ORDERED rank")?;
            let rank = u32::try_from(rank).map_err(|_| err("ORDERED rank out of range"))?;
            Ok(CapabilityValue::Ordered {
                class: string_of(field(object, "class")?, "ORDERED class")?,
                rank,
            })
        }
        "QUANTITY" => Ok(CapabilityValue::Quantity(Quantity::new(f64_of(
            field(object, "value")?,
            "QUANTITY value",
        )?)?)),
        "EXACT" => Ok(CapabilityValue::Exact(field(object, "value")?.clone())),
        other => Err(err(format!("unknown capability value kind {other}"))),
    }
}

fn capability_ref_from(value: &Value) -> Result<CapabilityRef, ContractError> {
    let object = object(value, "capability ref")?;
    CapabilityRef::new(
        string_of(field(object, "capability_id")?, "capability_id")?,
        u64_of(field(object, "revision")?, "revision")?,
    )
}

fn capability_map_from(
    value: &Value,
) -> Result<BTreeMap<CapabilityRef, CapabilityValue>, ContractError> {
    let mut map = BTreeMap::new();
    for item in array(value, "capability map")? {
        let object = object(item, "capability entry")?;
        let reference = capability_ref_from(item)?;
        let value = capability_value(field(object, "value")?)?;
        if map.insert(reference, value).is_some() {
            return Err(err("duplicate capability entry in canonical map"));
        }
    }
    Ok(map)
}

fn claim_from(value: &Value) -> Result<CapabilityClaim, ContractError> {
    let object = object(value, "capability claim")?;
    Ok(CapabilityClaim {
        reference: capability_ref_from(value)?,
        value: capability_value(field(object, "value")?)?,
        assurance: assurance(&string_of(field(object, "assurance")?, "assurance")?)?,
        declaration_provenance_ref: opt_string(field(object, "declaration_provenance_ref")?)?,
    })
}

fn claims_from(value: &Value) -> Result<Vec<CapabilityClaim>, ContractError> {
    array(value, "claims")?.iter().map(claim_from).collect()
}

fn lifecycle_set_from(value: &Value) -> Result<BTreeSet<LifecycleMode>, ContractError> {
    let mut set = BTreeSet::new();
    for item in array(value, "lifecycle modes")? {
        set.insert(lifecycle_mode(&string_of(item, "lifecycle mode")?)?);
    }
    Ok(set)
}

fn continuity_set_from(value: &Value) -> Result<BTreeSet<ContinuityMode>, ContractError> {
    let mut set = BTreeSet::new();
    for item in array(value, "continuity modes")? {
        set.insert(continuity_mode(&string_of(item, "continuity mode")?)?);
    }
    Ok(set)
}

fn affinity_from(value: &Value) -> Result<AffinityConstraint, ContractError> {
    let object = object(value, "affinity")?;
    match string_of(field(object, "kind")?, "affinity kind")?.as_str() {
        "ANY" => Ok(AffinityConstraint::Any),
        "ONLY" => {
            let mut tags = BTreeSet::new();
            for tag in array(field(object, "tags")?, "affinity tags")? {
                tags.insert(string_of(tag, "affinity tag")?);
            }
            Ok(AffinityConstraint::Only(tags))
        }
        other => Err(err(format!("unknown affinity kind {other}"))),
    }
}

fn security_from(value: &Value) -> Result<SecurityContract, ContractError> {
    let object = object(value, "security")?;
    Ok(SecurityContract {
        workspace: workspace_mode(&string_of(field(object, "workspace")?, "workspace")?)?,
        network: network_policy(&string_of(field(object, "network")?, "network")?)?,
        requires_attempt_isolation: field(object, "requires_attempt_isolation")?
            .as_bool()
            .ok_or_else(|| err("requires_attempt_isolation must be boolean"))?,
    })
}

fn sandbox_policy_from(value: &Value) -> Result<Option<SandboxPolicyRef>, ContractError> {
    match value {
        Value::Null => Ok(None),
        other => {
            let object = object(other, "sandbox policy")?;
            Ok(Some(SandboxPolicyRef::new(
                string_of(field(object, "sandbox_policy_id")?, "sandbox_policy_id")?,
                u64_of(field(object, "revision")?, "sandbox policy revision")?,
            )?))
        }
    }
}

fn contract_from(value: &Value) -> Result<AgentTypeContract, ContractError> {
    let object = object(value, "contract")?;
    let mut allowed = Vec::new();
    for function in array(
        field(object, "allowed_information_functions")?,
        "allowed_information_functions",
    )? {
        allowed.push(information_function(&string_of(
            function,
            "information function",
        )?)?);
    }
    Ok(AgentTypeContract {
        allowed_information_functions: allowed,
        required_capabilities: capability_map_from(field(object, "required_capabilities")?)?,
        affinity: affinity_from(field(object, "affinity")?)?,
        budget_ceiling: Budget::new(f64_of(field(object, "budget_ceiling")?, "budget_ceiling")?)?,
        security: security_from(field(object, "security")?)?,
        lifecycle: lifecycle_set_from(field(object, "lifecycle")?)?,
        continuity: continuity_mode(&string_of(field(object, "continuity")?, "continuity")?)?,
        sandbox_policy: sandbox_policy_from(field(object, "sandbox_policy")?)?,
        anchor_constraint: opt_string(field(object, "anchor_constraint")?)?,
    })
}

fn parse_document(json: &str, kind: &str) -> Result<Value, ContractError> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| err(format!("invalid canonical JSON: {e}")))?;
    let object = object(&value, "canonical document")?;
    check_envelope(object, kind)?;
    Ok(value)
}

pub fn agent_type_from_canonical_json(json: &str) -> Result<AgentType, ContractError> {
    let value = parse_document(json, "AGENT_TYPE")?;
    let document = object(&value, "agent type")?;
    let type_ref = AgentTypeRef::new(
        string_of(field(document, "type_id")?, "type_id")?,
        u64_of(field(document, "revision")?, "revision")?,
    )?;
    let based_on = match field(document, "based_on")? {
        Value::Null => None,
        other => {
            let base = object(other, "based_on")?;
            Some(AgentTypeRef::new(
                string_of(field(base, "type_id")?, "based_on type_id")?,
                u64_of(field(base, "revision")?, "based_on revision")?,
            )?)
        }
    };
    Ok(AgentType {
        type_ref,
        based_on,
        contract: contract_from(field(document, "contract")?)?,
    })
}

pub fn spawn_source_from_canonical_json(
    json: &str,
    status: SourceStatus,
) -> Result<SpawnSource, ContractError> {
    let value = parse_document(json, "SPAWN_SOURCE")?;
    let document = object(&value, "spawn source")?;
    let source_ref = SpawnSourceRef::new(
        string_of(field(document, "source_id")?, "source_id")?,
        u64_of(field(document, "revision")?, "revision")?,
    )?;
    let policy = object(field(document, "adapter_policy")?, "adapter_policy")?;
    let adapter_policy = AdapterPolicyRef::new(
        string_of(field(policy, "adapter_policy_id")?, "adapter_policy_id")?,
        u64_of(field(policy, "revision")?, "adapter policy revision")?,
    )?;
    let lifecycle_modes = lifecycle_set_from(field(document, "lifecycle_modes")?)?;
    let continuity_modes = continuity_set_from(field(document, "continuity_modes")?)?;
    let functional_envelope = capability_map_from(field(document, "functional_envelope")?)?;
    let claims = claims_from(field(document, "claims")?)?;
    Ok(SpawnSource {
        source_ref,
        adapter_policy,
        lifecycle_modes,
        continuity_modes,
        functional_envelope,
        claims,
        status,
    })
}

fn nullable_lifecycle(value: &Value) -> Result<Option<BTreeSet<LifecycleMode>>, ContractError> {
    match value {
        Value::Null => Ok(None),
        other => Ok(Some(lifecycle_set_from(other)?)),
    }
}

fn nullable_continuity(value: &Value) -> Result<Option<BTreeSet<ContinuityMode>>, ContractError> {
    match value {
        Value::Null => Ok(None),
        other => Ok(Some(continuity_set_from(other)?)),
    }
}

/// Decode a SourceConfig revision. The opaque `config_locator` (present only
/// for an `ExternalRef` body) is returned separately from the `config_digest`:
/// location and content identity are distinct and MUST NOT be conflated.
pub fn source_config_revision_from_canonical_json(
    json: &str,
    status: ConfigStatus,
) -> Result<(SourceConfig, Option<String>), ContractError> {
    let value = parse_document(json, "SOURCE_CONFIG")?;
    let object = object(&value, "source config")?;
    let source = SpawnSourceRef::new(
        string_of(field(object, "source_id")?, "source_id")?,
        u64_of(field(object, "source_revision")?, "source_revision")?,
    )?;
    let config_ref = SourceConfigRef::new(
        source,
        string_of(field(object, "config_id")?, "config_id")?,
        u64_of(field(object, "revision")?, "revision")?,
    )?;
    let mut credential_refs = Vec::new();
    for item in array(field(object, "credential_refs")?, "credential_refs")? {
        credential_refs.push(CredentialRef::new(string_of(item, "credential ref")?)?);
    }
    let config_locator = optional_locator(field(object, "config_locator")?)?;
    let config = SourceConfig {
        config_ref,
        config_digest: ConfigDigest::new(string_of(
            field(object, "config_digest")?,
            "config_digest",
        )?)?,
        lifecycle_modes: nullable_lifecycle(field(object, "lifecycle_modes")?)?,
        continuity_modes: nullable_continuity(field(object, "continuity_modes")?)?,
        credential_refs,
        claims: claims_from(field(object, "claims")?)?,
        status,
    };
    Ok((config, config_locator))
}

pub fn adapter_binding_policy_from_canonical_json(
    json: &str,
    status: ConfigStatus,
) -> Result<AdapterBindingPolicy, ContractError> {
    let value = parse_document(json, "ADAPTER_BINDING_POLICY")?;
    let document = object(&value, "adapter binding policy")?;
    let policy_ref = AdapterPolicyRef::new(
        string_of(field(document, "adapter_policy_id")?, "adapter_policy_id")?,
        u64_of(field(document, "revision")?, "revision")?,
    )?;
    let safety = object(field(document, "required_safety")?, "required_safety")?;
    let attempt_isolation = field(safety, "attempt_isolation")?
        .as_bool()
        .ok_or_else(|| err("attempt_isolation must be boolean"))?;
    let mut workspace_modes = Vec::new();
    for item in array(field(safety, "workspace_modes")?, "workspace_modes")? {
        workspace_modes.push(workspace_mode(&string_of(item, "workspace mode")?)?);
    }
    let mut network_modes = BTreeSet::new();
    for item in array(field(safety, "network_modes")?, "network_modes")? {
        network_modes.insert(network_policy(&string_of(item, "network mode")?)?);
    }
    workspace_modes.sort_by_key(|m| workspace_rank(*m));
    workspace_modes.dedup();
    let policy = AdapterBindingPolicy {
        policy_ref,
        adapter_kind: string_of(field(document, "adapter_kind")?, "adapter_kind")?,
        binding_ref: string_of(field(document, "binding_ref")?, "binding_ref")?,
        required_safety: PhysicalSafety::new(attempt_isolation, workspace_modes, network_modes)?,
        status,
    };
    policy.validate()?;
    Ok(policy)
}

pub fn capability_definition_from_canonical_json(
    json: &str,
) -> Result<(CapabilityRef, CapabilityDefinition), ContractError> {
    let value = parse_document(json, "CAPABILITY_DEFINITION")?;
    let object = object(&value, "capability definition")?;
    let reference = CapabilityRef::new(
        string_of(field(object, "capability_id")?, "capability_id")?,
        u64_of(field(object, "revision")?, "revision")?,
    )?;
    let definition = CapabilityDefinition {
        matcher_kind: matcher_kind(&string_of(field(object, "matcher_kind")?, "matcher_kind")?)?,
        security_class: security_class(&string_of(
            field(object, "security_class")?,
            "security_class",
        )?)?,
        polarity: capability_polarity(&string_of(field(object, "polarity")?, "polarity")?)?,
    };
    Ok((reference, definition))
}

fn string_set_from(value: &Value, what: &str) -> Result<BTreeSet<String>, ContractError> {
    let mut set = BTreeSet::new();
    for item in array(value, what)? {
        set.insert(string_of(item, what)?);
    }
    Ok(set)
}

fn task_requirement_from(value: &Value) -> Result<TaskRequirement, ContractError> {
    let object = object(value, "task requirement")?;
    Ok(TaskRequirement {
        information_function: information_function(&string_of(
            field(object, "information_function")?,
            "information_function",
        )?)?,
        required_capabilities: capability_map_from(field(object, "required_capabilities")?)?,
        required_affinity: string_set_from(
            field(object, "required_affinity")?,
            "required_affinity",
        )?,
        required_workspace: workspace_mode(&string_of(
            field(object, "required_workspace")?,
            "required_workspace",
        )?)?,
        required_network: network_policy(&string_of(
            field(object, "required_network")?,
            "required_network",
        )?)?,
        required_attempt_isolation: field(object, "required_attempt_isolation")?
            .as_bool()
            .ok_or_else(|| err("required_attempt_isolation must be boolean"))?,
        required_continuity: continuity_mode(&string_of(
            field(object, "required_continuity")?,
            "required_continuity",
        )?)?,
        sandbox_policy: sandbox_policy_from(field(object, "sandbox_policy")?)?,
        required_anchor: opt_string(field(object, "required_anchor")?)?,
        budget: Budget::new(f64_of(field(object, "budget")?, "budget")?)?,
    })
}

/// Decode a Task agent requirement. The caller MUST also verify the stored
/// content digest and canonical byte equality at the durable boundary.
pub fn task_agent_requirement_from_canonical_json(
    json: &str,
) -> Result<TaskAgentRequirement, ContractError> {
    let value = parse_document(json, "TASK_AGENT_REQUIREMENT")?;
    let document = object(&value, "task agent requirement")?;
    let required_type = match field(document, "required_type")? {
        Value::Null => None,
        other => {
            let reference = object(other, "required_type")?;
            Some(AgentTypeRef::new(
                string_of(field(reference, "type_id")?, "required type_id")?,
                u64_of(field(reference, "revision")?, "required revision")?,
            )?)
        }
    };
    let hard = task_requirement_from(field(document, "hard")?)?;
    Ok(TaskAgentRequirement {
        required_type,
        hard,
    })
}

/// Decode a Generation policy. The caller MUST also verify the stored content
/// digest and canonical byte equality at the durable boundary.
pub fn generation_policy_from_canonical_json(
    json: &str,
) -> Result<GenerationPolicy, ContractError> {
    let value = parse_document(json, "GENERATION_POLICY")?;
    let document = object(&value, "generation policy")?;
    let policy = object(field(document, "policy")?, "generation policy body")?;
    let mut allowed_information_functions = Vec::new();
    for function in array(
        field(policy, "allowed_information_functions")?,
        "allowed_information_functions",
    )? {
        allowed_information_functions.push(information_function(&string_of(
            function,
            "information function",
        )?)?);
    }
    let allowed_affinity = match field(policy, "allowed_affinity")? {
        Value::Null => None,
        other => Some(string_set_from(other, "allowed_affinity")?),
    };
    Ok(GenerationPolicy {
        allowed_information_functions,
        max_workspace: workspace_mode(&string_of(
            field(policy, "max_workspace")?,
            "max_workspace",
        )?)?,
        max_network: network_policy(&string_of(field(policy, "max_network")?, "max_network")?)?,
        requires_attempt_isolation: field(policy, "requires_attempt_isolation")?
            .as_bool()
            .ok_or_else(|| err("requires_attempt_isolation must be boolean"))?,
        min_continuity: continuity_mode(&string_of(
            field(policy, "min_continuity")?,
            "min_continuity",
        )?)?,
        sandbox_policy: sandbox_policy_from(field(policy, "sandbox_policy")?)?,
        budget_ceiling: Budget::new(f64_of(field(policy, "budget_ceiling")?, "budget_ceiling")?)?,
        allowed_affinity,
        anchor_constraint: opt_string(field(policy, "anchor_constraint")?)?,
    })
}
