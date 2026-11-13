//! M6-B.2 Agent Contract catalog persistence (schema v6).
//!
//! Immutable revision content is stored separately from the mutable
//! `ACTIVE`/`DRAINING`/`DISABLED` disposition overlay, so status never enters a
//! content digest. Every publication is idempotent for the same exact
//! `(ref, content digest)` and fails closed (invariant violation) when a
//! different digest is offered for an already-published revision.
//!
//! This module performs no external I/O and never mints enforcement evidence;
//! it only persists the pure `agentype-agent-contract` value types.

use crate::store::{map_sqlite, query_opt};
use agentype_agent_contract::{
    adapter_binding_policy_content_digest, adapter_binding_policy_from_canonical_json,
    agent_type_content_digest, agent_type_from_canonical_json,
    canonical_adapter_binding_policy_bytes, canonical_agent_type_bytes,
    canonical_capability_definition_bytes, canonical_json_body_digest,
    canonical_sandbox_policy_bytes, canonical_source_config_bytes, canonical_spawn_source_bytes,
    canonicalize_agent_type, canonicalize_source_config, canonicalize_spawn_source,
    capability_definition_content_digest, capability_definition_from_canonical_json,
    content_digest, is_valid_refinement, is_valid_refinement_with_policies,
    sandbox_policy_content_digest, sandbox_policy_from_canonical_json,
    source_config_content_digest, source_config_revision_from_canonical_json,
    spawn_source_content_digest, spawn_source_from_canonical_json, validate_agent_type_coarse_fine,
    validate_capability_claims_against_policy, AdapterBindingPolicy, AdapterPolicyRef, AgentType,
    AgentTypeId, AgentTypeLookup, AgentTypeRef, CapabilityCatalog, CapabilityDefinition,
    CapabilityRef, ConfigDigest, ConfigStatus, ContractError, EnforcementProfileRef,
    NormalizedSandboxPolicy, PolicyInput, SandboxPolicyRef, SecurityContractMode, SourceConfig,
    SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef, SpawnSourceSecurityDeclaration,
    SANDBOX_POLICY_SCHEMA_VERSION,
};
use agentype_core::{Error, UnixTime};
use rusqlite::{params, Transaction};
use std::collections::BTreeSet;

/// Publication status of an exact AgentType revision (the catalog-owned overlay).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTypeStatus {
    Published,
    Deprecated,
}

impl AgentTypeStatus {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Published => "PUBLISHED",
            Self::Deprecated => "DEPRECATED",
        }
    }

    fn parse_sql(value: &str) -> Result<Self, Error> {
        match value {
            "PUBLISHED" => Ok(Self::Published),
            "DEPRECATED" => Ok(Self::Deprecated),
            other => Err(Error::invariant(format!("unknown AgentTypeStatus {other}"))),
        }
    }
}

/// The opaque kind of a SourceConfig body. Core never interprets the payload;
/// it only uses it to validate the caller-declared `config_digest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigMode {
    OpaqueJson,
    ExternalRef,
}

impl ConfigMode {
    fn as_sql(self) -> &'static str {
        match self {
            Self::OpaqueJson => "OPAQUE_JSON",
            Self::ExternalRef => "EXTERNAL_REF",
        }
    }

    fn parse_sql(value: &str) -> Result<Self, Error> {
        match value {
            "OPAQUE_JSON" => Ok(Self::OpaqueJson),
            "EXTERNAL_REF" => Ok(Self::ExternalRef),
            other => Err(Error::invariant(format!("unknown ConfigMode {other}"))),
        }
    }
}

/// The source-private configuration body supplied at publication.
///
/// Location and content identity are deliberately separate: an `ExternalRef`
/// freezes **both** an opaque `locator` (where the configuration lives) and the
/// `config_digest` already carried by the `SourceConfig` (which version of the
/// content is behind it). Core stores the locator verbatim and never treats it
/// as, or substitutes it for, a digest.
#[derive(Clone, Debug)]
pub enum SourceConfigBody {
    /// A Core-opaque JSON body. `config_digest` MUST equal
    /// [`canonical_json_body_digest`] of this value; no locator is stored.
    ///
    /// This is durable **non-secret** source-private configuration material.
    /// Provider/vendor secrets (API keys, tokens) MUST NOT be persisted here;
    /// secret-bearing configuration belongs behind `ExternalRef` /
    /// `CredentialRef` (B.5), so the Scheduler database never becomes a
    /// provider-secret store. Core cannot detect vendor secret schemas, so the
    /// enforcing responsibility stays with the source-specific publisher.
    OpaqueJson(serde_json::Value),
    /// A non-empty opaque locator owned entirely outside Core. `config_digest`
    /// is the caller-declared content digest that the source integration
    /// resolves and attests later; B.2 performs no external I/O.
    ExternalRef { locator: String },
}

fn source_status_sql(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Active => "ACTIVE",
        SourceStatus::Draining => "DRAINING",
        SourceStatus::Disabled => "DISABLED",
    }
}

fn source_status_parse(value: &str) -> Result<SourceStatus, Error> {
    match value {
        "ACTIVE" => Ok(SourceStatus::Active),
        "DRAINING" => Ok(SourceStatus::Draining),
        "DISABLED" => Ok(SourceStatus::Disabled),
        other => Err(Error::invariant(format!("unknown SourceStatus {other}"))),
    }
}

fn config_status_sql(status: ConfigStatus) -> &'static str {
    match status {
        ConfigStatus::Active => "ACTIVE",
        ConfigStatus::Draining => "DRAINING",
        ConfigStatus::Disabled => "DISABLED",
    }
}

fn config_status_parse(value: &str) -> Result<ConfigStatus, Error> {
    match value {
        "ACTIVE" => Ok(ConfigStatus::Active),
        "DRAINING" => Ok(ConfigStatus::Draining),
        "DISABLED" => Ok(ConfigStatus::Disabled),
        other => Err(Error::invariant(format!("unknown ConfigStatus {other}"))),
    }
}

fn status_rank(status: &str) -> Result<u8, Error> {
    match status {
        "ACTIVE" => Ok(0),
        "DRAINING" => Ok(1),
        "DISABLED" => Ok(2),
        other => Err(Error::invariant(format!("unknown disposition {other}"))),
    }
}

fn contract_fault(error: ContractError) -> Error {
    Error::invariant(error.to_string())
}

/// Convert a durable unsigned revision/version into the signed integer domain
/// only when it round-trips, so a truncating `as` cast can never make two
/// different versions compare equal.
fn checked_i64(value: u64, what: &str) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| {
        Error::invariant(format!(
            "{what} {value} does not fit in the durable integer domain"
        ))
    })
}

/// Convert a stored signed version into `u32` only when it is exactly
/// representable; a negative or oversized value is corruption, not a version.
fn checked_u32(value: i64, what: &str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| {
        Error::invariant(format!(
            "{what} {value} is outside the supported unsigned 32-bit version domain"
        ))
    })
}

fn canonical_json_string(bytes: Vec<u8>, what: &str) -> Result<String, Error> {
    String::from_utf8(bytes)
        .map_err(|_| Error::invariant(format!("{what} canonical bytes are not UTF-8")))
}

fn existing_digest(
    tx: &Transaction<'_>,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Option<String>, Error> {
    query_opt(tx, sql, args, |row| row.get::<_, String>(0))
}

/// Insert immutable revision content, rejecting a different digest for an
/// already-published exact revision.
fn guard_immutable(existing: Option<String>, digest: &str, what: &str) -> Result<(), Error> {
    match existing {
        None => Ok(()),
        Some(stored) if stored == digest => Ok(()),
        Some(stored) => Err(Error::invariant(format!(
            "immutable {what} revision already published with content digest {stored}; \
             refusing to replace it with {digest}"
        ))),
    }
}

/// Durability integrity check at the catalog authority boundary: the stored
/// canonical document MUST hash to the recorded content digest. A mismatch is
/// corruption, never a silent alternative record.
fn verify_content_digest(json: &str, digest: &str, what: &str) -> Result<(), Error> {
    let recomputed = content_digest(json.as_bytes());
    if recomputed != digest {
        return Err(Error::invariant(format!(
            "catalog {what} content digest {digest} does not match recomputed {recomputed}"
        )));
    }
    Ok(())
}

/// The stored document MUST be exactly the canonical encoding of the record it
/// decodes to; a non-canonical ordering or duplicate set entry fails closed.
fn verify_reencoded(reencoded: &[u8], stored_json: &str, what: &str) -> Result<(), Error> {
    if reencoded != stored_json.as_bytes() {
        return Err(Error::invariant(format!(
            "catalog {what} content is not the canonical encoding; refusing to read"
        )));
    }
    Ok(())
}

/// The canonical document is the single authority. Any duplicate relational
/// column that a future code path might trust MUST agree with it, so a drifted
/// mirror cannot become a second source of truth.
fn cross_check(condition: bool, what: &str) -> Result<(), Error> {
    if condition {
        Ok(())
    } else {
        Err(Error::invariant(format!(
            "catalog {what} relational mirror does not match its canonical document"
        )))
    }
}

fn require_canonical_digest(digest: &ConfigDigest, what: &str) -> Result<(), Error> {
    digest.validate_canonical().map_err(|error| {
        Error::invariant(format!("catalog {what} is not a canonical digest: {error}"))
    })
}

// =============================================================================
// CapabilityCatalog
// =============================================================================

pub fn load_capability_catalog(tx: &Transaction<'_>) -> Result<CapabilityCatalog, Error> {
    let mut statement = tx
        .prepare(
            "SELECT capability_id, revision, matcher_kind, security_class, polarity,
                    canonical_version, content_json, content_digest
             FROM capability_definitions ORDER BY capability_id, revision",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(map_sqlite)?;
    let mut catalog = CapabilityCatalog::new();
    for row in rows {
        let (
            capability_id,
            revision,
            matcher,
            class,
            polarity,
            canonical_version,
            content_json,
            content_digest,
        ) = row.map_err(map_sqlite)?;
        verify_content_digest(&content_json, &content_digest, "capability definition")?;
        let (canonical_ref, definition) =
            capability_definition_from_canonical_json(&content_json).map_err(contract_fault)?;
        let reference =
            CapabilityRef::new(capability_id, revision as u64).map_err(contract_fault)?;
        cross_check(canonical_ref == reference, "capability definition")?;
        cross_check(
            matcher == matcher_sql(&definition)
                && class == class_sql(&definition)
                && polarity == polarity_sql(&definition),
            "capability definition",
        )?;
        // The row's version column is the authority for how the content is
        // read; a document whose encoding disagrees with it must fail closed
        // rather than be reinterpreted under the other version.
        cross_check(
            checked_u32(canonical_version, "capability definition canonical_version")?
                == definition.canonical_version(),
            "capability definition canonical_version",
        )?;
        verify_reencoded(
            &canonical_capability_definition_bytes(&canonical_ref, &definition),
            &content_json,
            "capability definition",
        )?;
        catalog
            .define_with_projection(
                reference,
                definition.matcher_kind,
                definition.security_class,
                definition.polarity,
                definition.policy_projection.clone(),
            )
            .map_err(contract_fault)?;
    }
    Ok(catalog)
}

pub fn publish_capability_definition(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &CapabilityRef,
    definition: &CapabilityDefinition,
) -> Result<String, Error> {
    // The publisher is a command boundary: a hand-built definition can bypass the
    // `PolicyProjection` constructor, so re-run the whole-definition invariant
    // before any digest or insert. Otherwise an invalid row would be durable and
    // every later catalog read would fail closed on it.
    definition
        .validate_projection()
        .map_err(|error| Error::invalid_authority(error.to_string()))?;
    let digest = capability_definition_content_digest(reference, definition);
    let content_json = canonical_json_string(
        canonical_capability_definition_bytes(reference, definition),
        "capability definition",
    )?;
    let existing = existing_digest(
        tx,
        "SELECT content_digest FROM capability_definitions
         WHERE capability_id=?1 AND revision=?2",
        params![reference.capability_id().as_str(), reference.revision()],
    )?;
    if let Some(stored) = existing {
        // An existing exact revision is never repaired: it MUST already be a
        // complete, valid catalog record, and the offered content MUST match.
        guard_immutable(Some(stored), &digest, "capability definition")?;
        get_capability_definition(tx, reference)?
            .ok_or_else(|| Error::invariant("published capability definition is missing"))?;
        return Ok(digest);
    }
    tx.execute(
        "INSERT INTO capability_definitions(
             capability_id, revision, matcher_kind, security_class, polarity,
             canonical_version, content_json, content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            reference.capability_id().as_str(),
            reference.revision(),
            matcher_sql(definition),
            class_sql(definition),
            polarity_sql(definition),
            definition.canonical_version(),
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

fn matcher_sql(definition: &CapabilityDefinition) -> &'static str {
    use agentype_agent_contract::MatcherKind;
    match definition.matcher_kind {
        MatcherKind::Bool => "BOOL",
        MatcherKind::Set => "SET",
        MatcherKind::Ordered => "ORDERED",
        MatcherKind::Quantity => "QUANTITY",
        MatcherKind::Exact => "EXACT",
    }
}

fn class_sql(definition: &CapabilityDefinition) -> &'static str {
    use agentype_agent_contract::SecurityClass;
    match definition.security_class {
        SecurityClass::Functional => "FUNCTIONAL",
        SecurityClass::Authority => "AUTHORITY",
        SecurityClass::Sandbox => "SANDBOX",
        SecurityClass::Continuity => "CONTINUITY",
    }
}

fn polarity_sql(definition: &CapabilityDefinition) -> &'static str {
    use agentype_agent_contract::CapabilityPolarity;
    match definition.polarity {
        CapabilityPolarity::Ability => "ABILITY",
        CapabilityPolarity::Restriction => "RESTRICTION",
    }
}

// =============================================================================
// AdapterBindingPolicy
// =============================================================================

pub fn publish_adapter_binding_policy(
    tx: &Transaction<'_>,
    now: UnixTime,
    policy: &AdapterBindingPolicy,
) -> Result<String, Error> {
    // The value type's fields are public, so the durable boundary MUST re-check
    // the whole-record invariant rather than trusting the caller's constructor.
    policy.validate().map_err(contract_fault)?;
    let digest = adapter_binding_policy_content_digest(policy);
    let content_json = canonical_json_string(
        canonical_adapter_binding_policy_bytes(policy),
        "adapter binding policy",
    )?;
    let existing = existing_digest(
        tx,
        "SELECT content_digest FROM adapter_binding_policies
         WHERE policy_id=?1 AND revision=?2",
        params![
            policy.policy_ref.id().as_str(),
            policy.policy_ref.revision()
        ],
    )?;
    if let Some(stored) = existing {
        guard_immutable(Some(stored), &digest, "adapter binding policy")?;
        get_adapter_binding_policy(tx, &policy.policy_ref)?
            .ok_or_else(|| Error::invariant("published adapter binding policy is missing"))?;
        return Ok(digest);
    }
    tx.execute(
        "INSERT INTO adapter_binding_policies(
             policy_id, revision, adapter_kind, binding_ref, content_json,
             content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            policy.policy_ref.id().as_str(),
            policy.policy_ref.revision(),
            policy.adapter_kind,
            policy.binding_ref,
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    // The value type carries an operator disposition; persist it separately, for
    // a fresh revision only. An existing revision's overlay is never repaired.
    tx.execute(
        "INSERT INTO adapter_binding_policy_dispositions(
             policy_id, revision, status, updated_at)
         VALUES(?1,?2,?3,?4)",
        params![
            policy.policy_ref.id().as_str(),
            policy.policy_ref.revision(),
            config_status_sql(policy.status),
            now,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

// =============================================================================
// AgentType
// =============================================================================

pub fn publish_agent_type(
    tx: &Transaction<'_>,
    now: UnixTime,
    agent: &AgentType,
) -> Result<String, Error> {
    publish_agent_type_with_security(tx, now, agent, SecurityContractMode::CoarseV1)
}

/// Publish an AgentType revision together with its immutable security mode.
///
/// The mode is part of the revision: a `SECURITY_V1` revision's declaration row
/// is written in the same transaction as the immutable content, so there is no
/// window or API to attach a security fact after publication. Absence of a row
/// is the frozen `COARSE_V1` contract.
///
/// A `SECURITY_V1` revision MUST author a sandbox policy whose projection is
/// exactly its coarse `SecurityContract`, and every mapped capability claim it
/// requires MUST be allowed by that same policy (ADR-0013 §3, §6).
pub fn publish_agent_type_with_security(
    tx: &Transaction<'_>,
    now: UnixTime,
    agent: &AgentType,
    security_mode: SecurityContractMode,
) -> Result<String, Error> {
    let mut canonical = agent.clone();
    let catalog = load_capability_catalog(tx)?;
    canonicalize_agent_type(&mut canonical, &catalog).map_err(contract_fault)?;

    // based_on provenance: the base must be a complete, valid catalog record
    // (validated canonical content AND a present disposition overlay) before it
    // can be used as authority for a new immutable revision. A base that the
    // catalog already treats as corrupt must not propagate into new durable
    // facts. The base may be PUBLISHED or DEPRECATED; only a missing overlay
    // fails closed.
    if let Some(base_ref) = &canonical.based_on {
        let (base, _status) = get_agent_type(tx, base_ref)?.ok_or_else(|| {
            Error::not_found(format!(
                "based_on agent type {}@{} does not exist",
                base_ref.id().as_str(),
                base_ref.revision()
            ))
        })?;
        // The security contract mode is part of the authority a revision carries.
        // A refinement MUST NOT silently drop a physical enforcement contract, so
        // `SECURITY_V1 -> COARSE_V1` is rejected even when every coarse field is
        // unchanged. Within `SECURITY_V1` the authored policies are ordered by the
        // real policy algebra; a `COARSE_V1` base keeps the frozen reference
        // relation.
        let base_mode = raw_agent_type_security_mode(tx, base_ref)?;
        match (base_mode, security_mode) {
            (SecurityContractMode::SecurityV1, SecurityContractMode::CoarseV1) => {
                return Err(Error::invalid_authority(format!(
                    "agent type {}@{} is a COARSE_V1 refinement of SECURITY_V1 base {}@{}: a refinement must not drop the physical enforcement contract",
                    canonical.type_ref.id().as_str(),
                    canonical.type_ref.revision(),
                    base_ref.id().as_str(),
                    base_ref.revision()
                )));
            }
            (SecurityContractMode::SecurityV1, SecurityContractMode::SecurityV1) => {
                let base_policy = load_optional_policy(tx, base.contract.sandbox_policy.as_ref())?;
                let derived_policy =
                    load_optional_policy(tx, canonical.contract.sandbox_policy.as_ref())?;
                is_valid_refinement_with_policies(
                    &base,
                    &canonical,
                    base_policy.as_ref(),
                    derived_policy.as_ref(),
                    &catalog,
                )
                .map_err(contract_fault)?;
            }
            (SecurityContractMode::CoarseV1, _) => {
                // A coarse base carries no fine policy ceiling, so the frozen
                // exact-reference monotonicity remains the whole contract. A
                // derived `SECURITY_V1` revision may narrow by pinning a policy
                // reference but cannot relax the base's coarse reference.
                is_valid_refinement(&base, &canonical, &catalog).map_err(contract_fault)?;
            }
        }
    }

    // P1-3: a `SECURITY_V1` AgentType is only valid when its authored policy is
    // the single authority its coarse contract and mapped claims project from.
    if security_mode == SecurityContractMode::SecurityV1 {
        let reference = canonical.contract.sandbox_policy.as_ref().ok_or_else(|| {
            Error::invalid_authority("a SECURITY_V1 agent type must author a sandbox policy")
        })?;
        let authored = get_sandbox_policy(tx, reference)?.ok_or_else(|| {
            Error::not_found(format!(
                "sandbox policy {}@{} does not exist",
                reference.id().as_str(),
                reference.revision()
            ))
        })?;
        validate_agent_type_coarse_fine(&canonical.contract.security, &authored).map_err(
            |error| {
                Error::invalid_authority(format!(
                    "SECURITY_V1 agent type contradicts its authored policy: {error}"
                ))
            },
        )?;
        validate_capability_claims_against_policy(
            &canonical.contract.required_capabilities,
            &catalog,
            &authored,
        )
        .map_err(|error| {
            Error::invalid_authority(format!(
                "SECURITY_V1 agent type capability claims contradict its authored policy: {error}"
            ))
        })?;
    }

    let digest = agent_type_content_digest(&canonical);
    let content_json = canonical_json_string(canonical_agent_type_bytes(&canonical), "agent type")?;
    let existing = existing_digest(
        tx,
        "SELECT content_digest FROM agent_types WHERE type_id=?1 AND revision=?2",
        params![
            canonical.type_ref.id().as_str(),
            canonical.type_ref.revision()
        ],
    )?;
    if let Some(stored) = existing {
        // Never repair an existing revision: validate the complete stored record
        // (including its disposition overlay) and the offered content identity.
        guard_immutable(Some(stored), &digest, "agent type")?;
        // The security mode is part of the immutable revision identity. An
        // idempotent republish must request the same mode; changing it would
        // reinterpret an existing revision and requires a new revision instead.
        let stored_mode = raw_agent_type_security_mode(tx, &canonical.type_ref)?;
        if stored_mode != security_mode {
            return Err(Error::invariant(format!(
                "agent type {}@{} is {} but was republished as {}; a mode change requires a new revision",
                canonical.type_ref.id().as_str(),
                canonical.type_ref.revision(),
                stored_mode.as_str(),
                security_mode.as_str()
            )));
        }
        get_agent_type(tx, &canonical.type_ref)?
            .ok_or_else(|| Error::invariant("published agent type is missing"))?;
        return Ok(digest);
    }
    tx.execute(
        "INSERT INTO agent_types(
             type_id, revision, based_on_type_id, based_on_revision,
             content_json, content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            canonical.type_ref.id().as_str(),
            canonical.type_ref.revision(),
            canonical.based_on.as_ref().map(|b| b.id().as_str()),
            canonical.based_on.as_ref().map(|b| b.revision()),
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    // The declaration is written before the disposition/published marker and in
    // the same transaction as the revision; there is no separate
    // post-publication declaration API. Absence is `COARSE_V1`. The schema
    // publication-closure trigger rejects a late declaration once the
    // disposition exists.
    if security_mode == SecurityContractMode::SecurityV1 {
        tx.execute(
            "INSERT INTO agent_type_security_declarations(
                 type_id, revision, security_contract_mode, created_at)
             VALUES(?1,?2,?3,?4)",
            params![
                canonical.type_ref.id().as_str(),
                canonical.type_ref.revision(),
                SecurityContractMode::SecurityV1.as_str(),
                now
            ],
        )
        .map_err(map_sqlite)?;
    }
    tx.execute(
        "INSERT INTO agent_type_dispositions(
             type_id, revision, status, deprecated_at, updated_at)
         VALUES(?1,?2,'PUBLISHED',NULL,?3)",
        params![
            canonical.type_ref.id().as_str(),
            canonical.type_ref.revision(),
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

// =============================================================================
// SpawnSource
// =============================================================================

pub fn publish_spawn_source(
    tx: &Transaction<'_>,
    now: UnixTime,
    source: &SpawnSource,
) -> Result<String, Error> {
    publish_spawn_source_with_security(tx, now, source, None)
}

/// Publish a SpawnSource revision together with its immutable security
/// declaration, if any (ADR-0013 §11).
///
/// A declaration makes the revision `SECURITY_V1` whether or not it carries an
/// authored `P_source` ceiling. It is written in the same transaction as the
/// immutable source content, so a source revision can never be selected as
/// `COARSE_V1` and later "upgraded" in place; changing the declaration requires
/// a new revision.
pub fn publish_spawn_source_with_security(
    tx: &Transaction<'_>,
    now: UnixTime,
    source: &SpawnSource,
    declaration: Option<&SpawnSourceSecurityDeclaration>,
) -> Result<String, Error> {
    let mut canonical = source.clone();
    let catalog = load_capability_catalog(tx)?;
    canonicalize_spawn_source(&mut canonical, &catalog).map_err(contract_fault)?;

    // The referenced adapter binding policy MUST be a complete, valid catalog
    // record (validated canonical content AND a present disposition overlay).
    // The SQL FK only proves the immutable row exists; it cannot prove the row
    // is not corrupt, so a corrupt dependency must not be referenced by a new
    // immutable revision. A DRAINING/DISABLED policy is still referenceable.
    get_adapter_binding_policy(tx, &canonical.adapter_policy)?.ok_or_else(|| {
        Error::not_found(format!(
            "adapter binding policy {}@{} does not exist",
            canonical.adapter_policy.id().as_str(),
            canonical.adapter_policy.revision()
        ))
    })?;

    // Validate the optional declaration before any write, including its
    // authored `P_source` reference and content digest.
    if let Some(declaration) = declaration {
        validated_source_declaration(tx, declaration)?;
    }

    let digest = spawn_source_content_digest(&canonical);
    let content_json =
        canonical_json_string(canonical_spawn_source_bytes(&canonical), "spawn source")?;
    let existing = existing_digest(
        tx,
        "SELECT content_digest FROM spawn_sources WHERE source_id=?1 AND revision=?2",
        params![
            canonical.source_ref.id().as_str(),
            canonical.source_ref.revision()
        ],
    )?;
    if let Some(stored) = existing {
        guard_immutable(Some(stored), &digest, "spawn source")?;
        // The security declaration is part of the immutable revision identity.
        // An idempotent republish must carry exactly the same declaration; any
        // change requires a new source revision.
        let stored_declaration = spawn_source_security_declaration(tx, &canonical.source_ref)?;
        if stored_declaration.as_ref() != declaration {
            return Err(Error::invariant(format!(
                "spawn source {}@{} security declaration differs from its existing immutable revision; a change requires a new revision",
                canonical.source_ref.id().as_str(),
                canonical.source_ref.revision()
            )));
        }
        get_spawn_source(tx, &canonical.source_ref)?
            .ok_or_else(|| Error::invariant("published spawn source is missing"))?;
        return Ok(digest);
    }
    tx.execute(
        "INSERT INTO spawn_sources(
             source_id, revision, adapter_policy_id, adapter_policy_revision,
             content_json, content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            canonical.source_ref.id().as_str(),
            canonical.source_ref.revision(),
            canonical.adapter_policy.id().as_str(),
            canonical.adapter_policy.revision(),
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    // The declaration is written before the disposition/published marker so the
    // schema publication-closure trigger can prove it belongs to the initial
    // publication, not to a later upgrade of an existing COARSE_V1 revision.
    if let Some(declaration) = declaration {
        insert_spawn_source_declaration(tx, now, &canonical.source_ref, declaration)?;
    }
    // A fresh revision persists the caller's initial disposition; an existing
    // revision's overlay is never (re)created or repaired here.
    tx.execute(
        "INSERT INTO spawn_source_dispositions(
             source_id, revision, status, updated_at)
         VALUES(?1,?2,?3,?4)",
        params![
            canonical.source_ref.id().as_str(),
            canonical.source_ref.revision(),
            source_status_sql(canonical.status),
            now
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

// =============================================================================
// SourceConfig
// =============================================================================

pub fn publish_source_config(
    tx: &Transaction<'_>,
    now: UnixTime,
    config: &SourceConfig,
    body: &SourceConfigBody,
) -> Result<String, Error> {
    let source = load_spawn_source(tx, config.config_ref.source())?.ok_or_else(|| {
        Error::not_found(format!(
            "source {}@{} does not exist",
            config.config_ref.source().id().as_str(),
            config.config_ref.source().revision()
        ))
    })?;

    // The declared content digest MUST have the frozen canonical grammar before
    // it enters a durable revision, for both body modes.
    require_canonical_digest(&config.config_digest, "source config")?;

    let (mode, payload_json, config_locator) = match body {
        SourceConfigBody::OpaqueJson(payload) => {
            let expected = canonical_json_body_digest(payload);
            if config.config_digest.as_str() != expected {
                return Err(Error::invariant(format!(
                    "config digest {} does not match the canonical opaque JSON body digest {expected}",
                    config.config_digest.as_str()
                )));
            }
            let payload_json = canonical_json_string(
                serde_json::to_vec(payload)
                    .map_err(|e| Error::invariant(format!("config body: {e}")))?,
                "config body",
            )?;
            (ConfigMode::OpaqueJson, Some(payload_json), None)
        }
        SourceConfigBody::ExternalRef { locator } => {
            // Structural validation only: reject an all-whitespace locator, but
            // store the locator exactly as supplied. Trimming is a source-specific
            // normalization Core MUST NOT perform on an opaque identity.
            //
            // The locator and the declared content digest are two distinct
            // fields (`config_locator` vs `config_digest`), which is the whole
            // structural guarantee. Their *values* are source-private and may
            // legitimately coincide (e.g. a content-addressed locator that is
            // both where the content lives and its digest), so Core MUST NOT
            // interpret a value equality as a conflation.
            if locator.trim().is_empty() {
                return Err(Error::invariant(
                    "an ExternalRef config locator must not be empty",
                ));
            }
            (ConfigMode::ExternalRef, None, Some(locator.clone()))
        }
    };

    let mut canonical = config.clone();
    let catalog = load_capability_catalog(tx)?;
    canonicalize_source_config(&mut canonical, &source, &catalog).map_err(contract_fault)?;

    let digest = source_config_content_digest(&canonical, config_locator.as_deref());
    let content_json = canonical_json_string(
        canonical_source_config_bytes(&canonical, config_locator.as_deref()),
        "source config",
    )?;
    let existing = existing_digest(
        tx,
        "SELECT content_digest FROM source_configs
         WHERE source_id=?1 AND source_revision=?2 AND config_id=?3 AND config_revision=?4",
        params![
            config.config_ref.source().id().as_str(),
            config.config_ref.source().revision(),
            config.config_ref.config_id().as_str(),
            config.config_ref.revision()
        ],
    )?;
    if let Some(stored) = existing {
        guard_immutable(Some(stored), &digest, "source config")?;
        load_source_config_revision(tx, &config.config_ref)?
            .ok_or_else(|| Error::invariant("published source config is missing"))?;
        return Ok(digest);
    }
    tx.execute(
        "INSERT INTO source_configs(
             source_id, source_revision, config_id, config_revision,
             config_mode, config_payload_json, config_locator, config_digest,
             content_json, content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            config.config_ref.source().id().as_str(),
            config.config_ref.source().revision(),
            config.config_ref.config_id().as_str(),
            config.config_ref.revision(),
            mode.as_sql(),
            payload_json,
            config_locator,
            canonical.config_digest.as_str(),
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    // A fresh revision persists the caller's initial disposition; an existing
    // revision's overlay is never (re)created or repaired here.
    tx.execute(
        "INSERT INTO source_config_dispositions(
             source_id, source_revision, config_id, config_revision, status, updated_at)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            config.config_ref.source().id().as_str(),
            config.config_ref.source().revision(),
            config.config_ref.config_id().as_str(),
            config.config_ref.revision(),
            config_status_sql(canonical.status),
            now,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

// =============================================================================
// Reads
// =============================================================================

pub fn get_agent_type(
    tx: &Transaction<'_>,
    reference: &AgentTypeRef,
) -> Result<Option<(AgentType, AgentTypeStatus)>, Error> {
    let row = query_opt(
        tx,
        "SELECT at.content_json, at.content_digest, at.based_on_type_id, at.based_on_revision,
                d.status
         FROM agent_types at
         LEFT JOIN agent_type_dispositions d
           ON d.type_id=at.type_id AND d.revision=at.revision
         WHERE at.type_id=?1 AND at.revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((json, digest, base_id, base_revision, status)) => {
            // A revision exists but its disposition overlay is missing: that is
            // durable corruption, not "not found".
            let status = status.ok_or_else(|| {
                Error::invariant("catalog agent type is missing its disposition overlay")
            })?;
            verify_content_digest(&json, &digest, "agent type")?;
            let mut agent = agent_type_from_canonical_json(&json).map_err(contract_fault)?;
            cross_check(agent.type_ref == *reference, "agent type")?;
            let canonical_base = match agent.based_on.as_ref() {
                Some(base) => Some((
                    base.id().as_str().to_string(),
                    checked_i64(base.revision(), "agent type revision")?,
                )),
                None => None,
            };
            let row_base = match (base_id, base_revision) {
                (Some(id), Some(revision)) => Some((id, revision)),
                (None, None) => None,
                _ => {
                    return Err(Error::invariant(
                        "catalog agent type based_on mirror is half-populated",
                    ))
                }
            };
            cross_check(canonical_base == row_base, "agent type")?;
            let catalog = load_capability_catalog(tx)?;
            canonicalize_agent_type(&mut agent, &catalog).map_err(contract_fault)?;
            verify_reencoded(&canonical_agent_type_bytes(&agent), &json, "agent type")?;
            validate_agent_type_security_read(tx, &agent, &catalog)?;
            Ok(Some((agent, AgentTypeStatus::parse_sql(&status)?)))
        }
    }
}

pub fn get_spawn_source(
    tx: &Transaction<'_>,
    reference: &SpawnSourceRef,
) -> Result<Option<SpawnSource>, Error> {
    load_spawn_source(tx, reference)
}

pub(crate) fn load_spawn_source(
    tx: &Transaction<'_>,
    reference: &SpawnSourceRef,
) -> Result<Option<SpawnSource>, Error> {
    let row = query_opt(
        tx,
        "SELECT s.content_json, s.content_digest, s.adapter_policy_id,
                s.adapter_policy_revision, d.status
         FROM spawn_sources s
         LEFT JOIN spawn_source_dispositions d
           ON d.source_id=s.source_id AND d.revision=s.revision
         WHERE s.source_id=?1 AND s.revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((json, digest, policy_id, policy_revision, status)) => {
            let status = status.ok_or_else(|| {
                Error::invariant("catalog spawn source is missing its disposition overlay")
            })?;
            verify_content_digest(&json, &digest, "spawn source")?;
            let mut source = spawn_source_from_canonical_json(&json, source_status_parse(&status)?)
                .map_err(contract_fault)?;
            cross_check(source.source_ref == *reference, "spawn source")?;
            cross_check(
                source.adapter_policy.id().as_str() == policy_id
                    && checked_i64(source.adapter_policy.revision(), "adapter policy revision")?
                        == policy_revision,
                "spawn source",
            )?;
            // Decode, re-canonicalize against the catalog, then require the
            // stored document to be the unique canonical encoding.
            let catalog = load_capability_catalog(tx)?;
            canonicalize_spawn_source(&mut source, &catalog).map_err(contract_fault)?;
            verify_reencoded(
                &canonical_spawn_source_bytes(&source),
                &json,
                "spawn source",
            )?;
            Ok(Some(source))
        }
    }
}

/// The single validated authority for one SourceConfig revision.
///
/// This is an **unforgeable validated fact**: its fields are private and there
/// is no public constructor, so a workspace sibling crate (a future B.4
/// resolver) cannot mint one that merely *looks* validated. The only production
/// constructor is [`load_source_config_revision`], which performs the content
/// digest check, canonical decode/re-encode, relational-mirror cross-checks,
/// source/config compatibility validation, and the opaque body digest check.
///
/// The `mode`/`body` invariant is fixed by that constructor:
/// `OpaqueJson` has a payload and no locator; `ExternalRef` has a locator and no
/// payload. [`SourceConfigRevision::body`] dispatches on `mode`, so a consumer
/// never guesses the body kind from which fields happen to be present.
///
/// # Unforgeable
///
/// ```compile_fail
/// use agentype_storage_sqlite::SourceConfigRevision;
/// // Private fields and no public constructor: this cannot compile outside the
/// // validated read path.
/// let _forged = SourceConfigRevision {
///     config: unreachable!(),
///     mode: unreachable!(),
///     locator: unreachable!(),
///     payload: unreachable!(),
/// };
/// ```
#[derive(Clone, Debug)]
pub struct SourceConfigRevision {
    config: SourceConfig,
    mode: ConfigMode,
    locator: Option<String>,
    payload: Option<serde_json::Value>,
}

/// A borrowed view of a validated `SourceConfigRevision` body.
#[derive(Clone, Copy, Debug)]
pub enum SourceConfigBodyView<'a> {
    OpaqueJson(&'a serde_json::Value),
    ExternalRef(&'a str),
}

impl SourceConfigRevision {
    /// The only constructor. Private: callers obtain a revision exclusively
    /// through [`load_source_config_revision`].
    fn validated(
        config: SourceConfig,
        mode: ConfigMode,
        locator: Option<String>,
        payload: Option<serde_json::Value>,
    ) -> Self {
        debug_assert!(
            matches!(
                (mode, locator.is_some(), payload.is_some()),
                (ConfigMode::OpaqueJson, false, true) | (ConfigMode::ExternalRef, true, false)
            ),
            "validated SourceConfigRevision must be mode/body consistent"
        );
        Self {
            config,
            mode,
            locator,
            payload,
        }
    }

    pub fn config(&self) -> &SourceConfig {
        &self.config
    }

    pub fn mode(&self) -> ConfigMode {
        self.mode
    }

    pub fn locator(&self) -> Option<&str> {
        self.locator.as_deref()
    }

    /// The complete immutable revision-content identity: config-selection
    /// metadata **plus** the opaque body mode and `ExternalRef` locator, which
    /// are part of the canonical content and its digest. Two revisions that
    /// agree on `SourceConfig` metadata but differ in locator are different
    /// revisions.
    pub fn same_revision_content(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.locator == other.locator
            && self.config.same_config_contract_content(&other.config)
    }

    /// The validated opaque body, dispatched on the validated `mode`. The
    /// `OpaqueJson`/`ExternalRef` invariant is guaranteed by the only
    /// constructor, so an impossible state is a hard failure, never a guess
    /// based on which fields happen to be present.
    pub fn body(&self) -> SourceConfigBodyView<'_> {
        match self.mode {
            ConfigMode::OpaqueJson => SourceConfigBodyView::OpaqueJson(
                self.payload
                    .as_ref()
                    .expect("validated OpaqueJson revision has a payload"),
            ),
            ConfigMode::ExternalRef => SourceConfigBodyView::ExternalRef(
                self.locator
                    .as_deref()
                    .expect("validated ExternalRef revision has a locator"),
            ),
        }
    }
}

pub fn load_source_config_revision(
    tx: &Transaction<'_>,
    reference: &SourceConfigRef,
) -> Result<Option<SourceConfigRevision>, Error> {
    let row = query_opt(
        tx,
        "SELECT c.content_json, c.content_digest, c.config_mode, c.config_payload_json,
                c.config_locator, c.config_digest, d.status
         FROM source_configs c
         LEFT JOIN source_config_dispositions d
           ON d.source_id=c.source_id AND d.source_revision=c.source_revision
          AND d.config_id=c.config_id AND d.config_revision=c.config_revision
         WHERE c.source_id=?1 AND c.source_revision=?2 AND c.config_id=?3 AND c.config_revision=?4",
        params![
            reference.source().id().as_str(),
            reference.source().revision(),
            reference.config_id().as_str(),
            reference.revision()
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        },
    )?;
    let Some((json, digest, mode, payload_json, locator, config_digest, status)) = row else {
        return Ok(None);
    };
    let status = status.ok_or_else(|| {
        Error::invariant("catalog source config is missing its disposition overlay")
    })?;
    verify_content_digest(&json, &digest, "source config")?;
    let (mut config, canonical_locator) =
        source_config_revision_from_canonical_json(&json, config_status_parse(&status)?)
            .map_err(contract_fault)?;
    cross_check(config.config_ref == *reference, "source config")?;
    require_canonical_digest(&config.config_digest, "source config")?;
    // The canonical `config_digest` and the duplicated column MUST agree.
    cross_check(
        config.config_digest.as_str() == config_digest,
        "source config",
    )?;
    // The stored document MUST be the unique canonical encoding: decode against
    // the exact source, re-canonicalize against the catalog, then compare bytes.
    let source = load_spawn_source(tx, config.config_ref.source())?.ok_or_else(|| {
        Error::invariant("catalog source config references a missing source revision")
    })?;
    let catalog = load_capability_catalog(tx)?;
    canonicalize_source_config(&mut config, &source, &catalog).map_err(contract_fault)?;
    verify_reencoded(
        &canonical_source_config_bytes(&config, canonical_locator.as_deref()),
        &json,
        "source config",
    )?;
    let mode = ConfigMode::parse_sql(&mode)?;
    let payload = match mode {
        ConfigMode::ExternalRef => {
            cross_check(payload_json.is_none(), "source config")?;
            cross_check(locator.is_some(), "source config")?;
            cross_check(locator == canonical_locator, "source config")?;
            None
        }
        ConfigMode::OpaqueJson => {
            cross_check(locator.is_none(), "source config")?;
            cross_check(canonical_locator.is_none(), "source config")?;
            let payload_json = payload_json
                .ok_or_else(|| Error::invariant("catalog source config body is missing"))?;
            let payload: serde_json::Value = serde_json::from_str(&payload_json)
                .map_err(|e| Error::invariant(format!("catalog source config body: {e}")))?;
            cross_check(
                canonical_json_body_digest(&payload) == config.config_digest.as_str(),
                "source config",
            )?;
            Some(payload)
        }
    };
    Ok(Some(SourceConfigRevision::validated(
        config,
        mode,
        canonical_locator,
        payload,
    )))
}

pub fn get_source_config(
    tx: &Transaction<'_>,
    reference: &SourceConfigRef,
) -> Result<Option<SourceConfig>, Error> {
    Ok(load_source_config_revision(tx, reference)?.map(|revision| revision.config))
}

/// The opaque `ExternalRef` locator a published SourceConfig revision was stored
/// with, or `None` for an `OpaqueJson` body. Distinct from `config_digest`.
pub fn get_source_config_locator(
    tx: &Transaction<'_>,
    reference: &SourceConfigRef,
) -> Result<Option<String>, Error> {
    Ok(load_source_config_revision(tx, reference)?.and_then(|revision| revision.locator))
}

/// The opaque body kind a published SourceConfig revision was stored with.
pub fn get_source_config_mode(
    tx: &Transaction<'_>,
    reference: &SourceConfigRef,
) -> Result<Option<ConfigMode>, Error> {
    Ok(load_source_config_revision(tx, reference)?.map(|revision| revision.mode))
}

pub fn get_adapter_binding_policy(
    tx: &Transaction<'_>,
    reference: &AdapterPolicyRef,
) -> Result<Option<AdapterBindingPolicy>, Error> {
    let row = query_opt(
        tx,
        "SELECT p.content_json, p.content_digest, p.adapter_kind, p.binding_ref, d.status
         FROM adapter_binding_policies p
         LEFT JOIN adapter_binding_policy_dispositions d
           ON d.policy_id=p.policy_id AND d.revision=p.revision
         WHERE p.policy_id=?1 AND p.revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((json, digest, adapter_kind, binding_ref, status)) => {
            let status = status.ok_or_else(|| {
                Error::invariant(
                    "catalog adapter binding policy is missing its disposition overlay",
                )
            })?;
            verify_content_digest(&json, &digest, "adapter binding policy")?;
            let policy =
                adapter_binding_policy_from_canonical_json(&json, config_status_parse(&status)?)
                    .map_err(contract_fault)?;
            cross_check(policy.policy_ref == *reference, "adapter binding policy")?;
            cross_check(
                policy.adapter_kind == adapter_kind && policy.binding_ref == binding_ref,
                "adapter binding policy",
            )?;
            verify_reencoded(
                &canonical_adapter_binding_policy_bytes(&policy),
                &json,
                "adapter binding policy",
            )?;
            Ok(Some(policy))
        }
    }
}

pub fn get_capability_definition(
    tx: &Transaction<'_>,
    reference: &CapabilityRef,
) -> Result<Option<CapabilityDefinition>, Error> {
    let row = query_opt(
        tx,
        "SELECT capability_id, revision, matcher_kind, security_class, polarity,
                canonical_version, content_json, content_digest
         FROM capability_definitions WHERE capability_id=?1 AND revision=?2",
        params![reference.capability_id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((
            capability_id,
            revision,
            matcher,
            class,
            polarity,
            canonical_version,
            json,
            digest,
        )) => {
            verify_content_digest(&json, &digest, "capability definition")?;
            let (canonical_ref, definition) =
                capability_definition_from_canonical_json(&json).map_err(contract_fault)?;
            let row_ref =
                CapabilityRef::new(capability_id, revision as u64).map_err(contract_fault)?;
            cross_check(canonical_ref == row_ref, "capability definition")?;
            cross_check(
                matcher == matcher_sql(&definition)
                    && class == class_sql(&definition)
                    && polarity == polarity_sql(&definition),
                "capability definition",
            )?;
            cross_check(
                checked_u32(canonical_version, "capability definition canonical_version")?
                    == definition.canonical_version(),
                "capability definition canonical_version",
            )?;
            verify_reencoded(
                &canonical_capability_definition_bytes(&canonical_ref, &definition),
                &json,
                "capability definition",
            )?;
            Ok(Some(definition))
        }
    }
}

// =============================================================================
// Dispositions (monotonic)
// =============================================================================

pub fn set_agent_type_status(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &AgentTypeRef,
    status: AgentTypeStatus,
) -> Result<(), Error> {
    let current = match query_opt(
        tx,
        "SELECT status FROM agent_type_dispositions WHERE type_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )? {
        Some(status) => status,
        None => {
            let exists = revision_exists(
                tx,
                "SELECT 1 FROM agent_types WHERE type_id=?1 AND revision=?2",
                params![reference.id().as_str(), reference.revision()],
            )?;
            return Err(absent_disposition(exists, "agent type"));
        }
    };
    // Idempotent: repeating the current disposition is a no-op that preserves
    // the original transition timestamp (`deprecated_at`).
    if current == status.as_sql() {
        return Ok(());
    }
    if current == "DEPRECATED" && status == AgentTypeStatus::Published {
        return Err(Error::invalid_transition(
            "a deprecated AgentType revision cannot be republished",
        ));
    }
    // The only remaining transition is PUBLISHED -> DEPRECATED.
    tx.execute(
        "UPDATE agent_type_dispositions
         SET status='DEPRECATED', deprecated_at=?1, updated_at=?1
         WHERE type_id=?2 AND revision=?3",
        params![now, reference.id().as_str(), reference.revision()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

fn check_monotonic(current: &str, status: &str) -> Result<(), Error> {
    if status_rank(status)? < status_rank(current)? {
        return Err(Error::invalid_transition(format!(
            "disposition {current} -> {status} is not monotonic"
        )));
    }
    Ok(())
}

/// Classify a missing disposition overlay: a revision that does not exist is
/// `NotFound`; a revision that exists without its overlay is corruption.
fn absent_disposition(revision_exists: bool, what: &str) -> Error {
    if revision_exists {
        Error::invariant(format!("catalog {what} is missing its disposition overlay"))
    } else {
        Error::not_found(format!("{what} revision does not exist"))
    }
}

fn revision_exists(
    tx: &Transaction<'_>,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<bool, Error> {
    Ok(query_opt(tx, sql, args, |row| row.get::<_, i64>(0))?.is_some())
}

pub fn set_spawn_source_status(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &SpawnSourceRef,
    status: SourceStatus,
) -> Result<(), Error> {
    let current = match query_opt(
        tx,
        "SELECT status FROM spawn_source_dispositions WHERE source_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )? {
        Some(status) => status,
        None => {
            let exists = revision_exists(
                tx,
                "SELECT 1 FROM spawn_sources WHERE source_id=?1 AND revision=?2",
                params![reference.id().as_str(), reference.revision()],
            )?;
            return Err(absent_disposition(exists, "spawn source"));
        }
    };
    let next = source_status_sql(status);
    check_monotonic(&current, next)?;
    // Repeating the current disposition is a durable no-op that preserves the
    // original transition timestamp (`updated_at`).
    if current == next {
        return Ok(());
    }
    tx.execute(
        "UPDATE spawn_source_dispositions SET status=?1, updated_at=?2
         WHERE source_id=?3 AND revision=?4",
        params![next, now, reference.id().as_str(), reference.revision()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

pub fn set_source_config_status(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &SourceConfigRef,
    status: ConfigStatus,
) -> Result<(), Error> {
    let id = reference.source().id().as_str();
    let source_revision = reference.source().revision();
    let config_id = reference.config_id().as_str();
    let config_revision = reference.revision();
    let current = match query_opt(
        tx,
        "SELECT status FROM source_config_dispositions
         WHERE source_id=?1 AND source_revision=?2 AND config_id=?3 AND config_revision=?4",
        params![id, source_revision, config_id, config_revision],
        |row| row.get::<_, String>(0),
    )? {
        Some(status) => status,
        None => {
            let exists = revision_exists(
                tx,
                "SELECT 1 FROM source_configs
                 WHERE source_id=?1 AND source_revision=?2 AND config_id=?3 AND config_revision=?4",
                params![id, source_revision, config_id, config_revision],
            )?;
            return Err(absent_disposition(exists, "source config"));
        }
    };
    let next = config_status_sql(status);
    check_monotonic(&current, next)?;
    if current == next {
        return Ok(());
    }
    tx.execute(
        "UPDATE source_config_dispositions SET status=?1, updated_at=?2
         WHERE source_id=?3 AND source_revision=?4 AND config_id=?5 AND config_revision=?6",
        params![next, now, id, source_revision, config_id, config_revision],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

pub fn set_adapter_binding_policy_status(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &AdapterPolicyRef,
    status: ConfigStatus,
) -> Result<(), Error> {
    let current = match query_opt(
        tx,
        "SELECT status FROM adapter_binding_policy_dispositions
         WHERE policy_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )? {
        Some(status) => status,
        None => {
            let exists = revision_exists(
                tx,
                "SELECT 1 FROM adapter_binding_policies WHERE policy_id=?1 AND revision=?2",
                params![reference.id().as_str(), reference.revision()],
            )?;
            return Err(absent_disposition(exists, "adapter binding policy"));
        }
    };
    let next = config_status_sql(status);
    check_monotonic(&current, next)?;
    if current == next {
        return Ok(());
    }
    tx.execute(
        "UPDATE adapter_binding_policy_dispositions SET status=?1, updated_at=?2
         WHERE policy_id=?3 AND revision=?4",
        params![next, now, reference.id().as_str(), reference.revision()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

// =============================================================================
// Selector lookup
// =============================================================================

/// Snapshot of published AgentType revisions, for pre-commit selector resolution.
pub struct DurableAgentTypeLookup {
    published: BTreeSet<AgentTypeRef>,
}

impl AgentTypeLookup for DurableAgentTypeLookup {
    fn is_published(&self, reference: &AgentTypeRef) -> bool {
        self.published.contains(reference)
    }

    fn latest_revision(&self, type_id: &AgentTypeId) -> Option<u64> {
        self.published
            .iter()
            .filter(|reference| reference.id() == type_id)
            .map(|reference| reference.revision())
            .max()
    }
}

/// Build the pre-commit selector lookup from the **validated** canonical read.
///
/// Every immutable AgentType row is validated (digest, canonical re-encoding,
/// relational mirror) and MUST have a disposition overlay; a missing overlay is
/// corruption that fails the whole lookup closed. Only then are `PUBLISHED`
/// revisions admitted. This means a corrupt revision cannot be silently dropped
/// and cannot cause `Latest` to fall back to an older revision, so the lookup is
/// not a second, weaker authority.
pub fn load_agent_type_lookup(tx: &Transaction<'_>) -> Result<DurableAgentTypeLookup, Error> {
    let references: Vec<AgentTypeRef> = {
        let mut statement = tx
            .prepare("SELECT type_id, revision FROM agent_types")
            .map_err(map_sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(map_sqlite)?;
        let mut refs = Vec::new();
        for row in rows {
            let (type_id, revision) = row.map_err(map_sqlite)?;
            refs.push(AgentTypeRef::new(type_id, revision as u64).map_err(contract_fault)?);
        }
        refs
    };
    let mut published = BTreeSet::new();
    for reference in references {
        // `get_agent_type` validates content, mirror, and requires the overlay.
        let (_, status) = get_agent_type(tx, &reference)?
            .ok_or_else(|| Error::invariant("published agent type is missing"))?;
        if status == AgentTypeStatus::Published {
            published.insert(reference);
        }
    }
    Ok(DurableAgentTypeLookup { published })
}

// =============================================================================
// M6-B.5 authored sandbox policies and security declarations (schema v9)
// =============================================================================

/// Publish one immutable authored sandbox policy revision.
///
/// The canonical bytes and digest are computed here from the pure value form;
/// a caller cannot declare either. Republishing the same exact revision is
/// content-idempotent and re-validates the stored record.
pub fn publish_sandbox_policy(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &SandboxPolicyRef,
    policy: &NormalizedSandboxPolicy,
) -> Result<String, Error> {
    let digest = sandbox_policy_content_digest(policy);
    let content_json =
        canonical_json_string(canonical_sandbox_policy_bytes(policy), "sandbox policy")?;
    let existing = existing_digest(
        tx,
        "SELECT content_digest FROM sandbox_policy_revisions
         WHERE policy_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
    )?;
    if let Some(stored) = existing {
        guard_immutable(Some(stored), &digest, "sandbox policy")?;
        get_sandbox_policy(tx, reference)?
            .ok_or_else(|| Error::invariant("published sandbox policy is missing"))?;
        return Ok(digest);
    }
    tx.execute(
        "INSERT INTO sandbox_policy_revisions(
             policy_id, revision, canonical_version, content_json, content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            reference.id().as_str(),
            reference.revision(),
            SANDBOX_POLICY_SCHEMA_VERSION,
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

/// Load the authored policy of an optional AgentType contract reference.
///
/// Absence is not corruption (a coarse contract may pin no fine policy), but a
/// present reference that names a missing revision is: the immutable AgentType
/// content is then unreadable authority.
fn load_optional_policy(
    tx: &Transaction<'_>,
    reference: Option<&SandboxPolicyRef>,
) -> Result<Option<NormalizedSandboxPolicy>, Error> {
    match reference {
        None => Ok(None),
        Some(reference) => get_sandbox_policy(tx, reference)?.map(Some).ok_or_else(|| {
            Error::not_found(format!(
                "sandbox policy {}@{} does not exist",
                reference.id().as_str(),
                reference.revision()
            ))
        }),
    }
}

/// The validated authoritative read of one authored policy revision.
pub fn get_sandbox_policy(
    tx: &Transaction<'_>,
    reference: &SandboxPolicyRef,
) -> Result<Option<NormalizedSandboxPolicy>, Error> {
    let row = query_opt(
        tx,
        "SELECT canonical_version, content_json, content_digest
         FROM sandbox_policy_revisions WHERE policy_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((canonical_version, json, digest)) => {
            verify_content_digest(&json, &digest, "sandbox policy")?;
            let policy = sandbox_policy_from_canonical_json(&json).map_err(contract_fault)?;
            // Policy rows store the canonical schema version only; the meet
            // algorithm version belongs to derivation/binding rows. A row this
            // build does not implement fails closed instead of being decoded
            // under current semantics.
            cross_check(
                checked_u32(canonical_version, "sandbox policy canonical_version")?
                    == SANDBOX_POLICY_SCHEMA_VERSION,
                "sandbox policy canonical_version",
            )?;
            verify_reencoded(
                &canonical_sandbox_policy_bytes(&policy),
                &json,
                "sandbox policy",
            )?;
            Ok(Some(policy))
        }
    }
}

/// The raw declared mode of an exact AgentType revision; absence is `COARSE_V1`.
///
/// This is deliberately **not** a publication API: the declaration is written
/// only together with the immutable revision by
/// [`publish_agent_type_with_security`]. Keeping the raw read separate avoids a
/// recursion through the validated [`get_agent_type`] read.
fn raw_agent_type_security_mode(
    tx: &Transaction<'_>,
    reference: &AgentTypeRef,
) -> Result<SecurityContractMode, Error> {
    let stored = query_opt(
        tx,
        "SELECT security_contract_mode FROM agent_type_security_declarations
         WHERE type_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )?;
    match stored {
        None => Ok(SecurityContractMode::CoarseV1),
        Some(mode) => SecurityContractMode::parse(&mode)
            .filter(|mode| *mode == SecurityContractMode::SecurityV1)
            .ok_or_else(|| {
                Error::invariant(format!(
                    "agent type security declaration records unsupported mode {mode:?}"
                ))
            }),
    }
}

/// The declared mode of an exact AgentType revision; absence is `COARSE_V1`.
pub fn agent_type_security_mode(
    tx: &Transaction<'_>,
    reference: &AgentTypeRef,
) -> Result<SecurityContractMode, Error> {
    get_agent_type(tx, reference)?.ok_or_else(|| {
        Error::not_found(format!(
            "agent type {}@{} does not exist",
            reference.id().as_str(),
            reference.revision()
        ))
    })?;
    raw_agent_type_security_mode(tx, reference)
}

/// Re-prove the §3 AgentType projection and §6 capability consistency on the
/// validated catalog read.
///
/// A `SECURITY_V1` revision whose authored policy no longer projects to its
/// coarse contract, or which requires a mapped capability its own policy denies,
/// is catalog corruption and MUST NOT become an agent's identity.
fn validate_agent_type_security_read(
    tx: &Transaction<'_>,
    agent: &AgentType,
    catalog: &CapabilityCatalog,
) -> Result<(), Error> {
    if raw_agent_type_security_mode(tx, &agent.type_ref)? != SecurityContractMode::SecurityV1 {
        return Ok(());
    }
    let reference = agent.contract.sandbox_policy.as_ref().ok_or_else(|| {
        Error::invariant("a SECURITY_V1 agent type is missing its authored sandbox policy")
    })?;
    let authored = get_sandbox_policy(tx, reference)?.ok_or_else(|| {
        Error::invariant(format!(
            "SECURITY_V1 agent type pins a missing sandbox policy {}@{}",
            reference.id().as_str(),
            reference.revision()
        ))
    })?;
    validate_agent_type_coarse_fine(&agent.contract.security, &authored).map_err(|error| {
        Error::invariant(format!(
            "SECURITY_V1 agent type contradicts its authored policy: {error}"
        ))
    })?;
    validate_capability_claims_against_policy(
        &agent.contract.required_capabilities,
        catalog,
        &authored,
    )
    .map_err(|error| {
        Error::invariant(format!(
            "SECURITY_V1 agent type capability claims contradict its authored policy: {error}"
        ))
    })?;
    Ok(())
}

/// Validate a SpawnSource security declaration against the authored policy
/// catalog and return its durable reference tuple.
fn validated_source_declaration(
    tx: &Transaction<'_>,
    declaration: &SpawnSourceSecurityDeclaration,
) -> Result<(Option<String>, Option<i64>, Option<String>), Error> {
    match declaration.source_policy() {
        None => Ok((None, None, None)),
        Some(input) => {
            let authored = input.reference().ok_or_else(|| {
                Error::invariant(
                    "a SpawnSource P_source declaration must reference an authored policy revision",
                )
            })?;
            let policy = get_sandbox_policy(tx, authored)?.ok_or_else(|| {
                Error::not_found(format!(
                    "sandbox policy {}@{} does not exist",
                    authored.id().as_str(),
                    authored.revision()
                ))
            })?;
            let recomputed = sandbox_policy_content_digest(&policy);
            if input.policy_digest() != recomputed {
                return Err(Error::invariant(format!(
                    "spawn source P_source digest {} does not match authored policy {recomputed}",
                    input.policy_digest()
                )));
            }
            // A SpawnSource revision has no isolation guarantee field, so its
            // policy MUST NOT introduce an unattributed guarantee. Isolation is
            // carried only by the AgentType/Task/SourceRequirement fields that
            // `I_required` composes; otherwise `P_eff.attempt_isolation = true`
            // would have no requirement asking the execution to request it.
            if policy.attempt_isolation() {
                return Err(Error::invariant(
                    "spawn source P_source policy cannot require attempt isolation; the requirement's guarantee is the only authority",
                ));
            }
            Ok((
                Some(authored.id().as_str().to_string()),
                Some(checked_i64(authored.revision(), "sandbox policy revision")?),
                Some(recomputed),
            ))
        }
    }
}

/// Insert the immutable security declaration row of one exact source revision.
fn insert_spawn_source_declaration(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &SpawnSourceRef,
    declaration: &SpawnSourceSecurityDeclaration,
) -> Result<(), Error> {
    let (policy_id, policy_revision, policy_digest) =
        validated_source_declaration(tx, declaration)?;
    let profile = declaration.enforcement_profile();
    let profile_version = checked_i64(u64::from(profile.version()), "enforcement profile version")?;

    let existing = query_opt(
        tx,
        "SELECT source_policy_id, source_policy_revision, source_policy_digest,
                enforcement_protocol, enforcement_profile, enforcement_profile_version
         FROM spawn_source_security_declarations WHERE source_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    )?;
    if let Some((
        stored_id,
        stored_revision,
        stored_digest,
        stored_protocol,
        stored_profile,
        stored_version,
    )) = existing
    {
        let identical = stored_id == policy_id
            && stored_revision == policy_revision
            && stored_digest == policy_digest
            && stored_protocol == profile.protocol()
            && stored_profile == profile.profile()
            && stored_version == profile_version;
        if !identical {
            return Err(Error::invariant(
                "spawn source security declaration for this exact revision already exists \
                 with different content; a change requires a new source revision",
            ));
        }
        return Ok(());
    }

    tx.execute(
        "INSERT INTO spawn_source_security_declarations(
             source_id, revision, security_contract_mode,
             source_policy_id, source_policy_revision, source_policy_digest,
             enforcement_protocol, enforcement_profile, enforcement_profile_version,
             created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            reference.id().as_str(),
            reference.revision(),
            SecurityContractMode::SecurityV1.as_str(),
            policy_id,
            policy_revision,
            policy_digest,
            profile.protocol(),
            profile.profile(),
            profile_version,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// The validated security declaration of one exact SpawnSource revision.
pub fn spawn_source_security_declaration(
    tx: &Transaction<'_>,
    reference: &SpawnSourceRef,
) -> Result<Option<SpawnSourceSecurityDeclaration>, Error> {
    let row = query_opt(
        tx,
        "SELECT security_contract_mode, source_policy_id, source_policy_revision,
                source_policy_digest, enforcement_protocol, enforcement_profile,
                enforcement_profile_version
         FROM spawn_source_security_declarations WHERE source_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        },
    )?;
    let Some((mode, policy_id, policy_revision, policy_digest, protocol, profile, profile_version)) =
        row
    else {
        return Ok(None);
    };
    cross_check(
        mode == SecurityContractMode::SecurityV1.as_str(),
        "spawn source security declaration",
    )?;
    let enforcement_profile = EnforcementProfileRef::new(
        protocol,
        profile,
        checked_u32(profile_version, "enforcement profile version")?,
    )
    .map_err(contract_fault)?;
    let source_policy = match (policy_id, policy_revision, policy_digest) {
        (Some(id), Some(revision), Some(declared_digest)) => {
            let revision = u64::try_from(revision).map_err(|_| {
                Error::invariant("spawn source declaration sandbox policy revision is negative")
            })?;
            let reference = SandboxPolicyRef::new(id, revision).map_err(contract_fault)?;
            let policy = get_sandbox_policy(tx, &reference)?.ok_or_else(|| {
                Error::invariant(format!(
                    "spawn source declaration references missing sandbox policy {}@{}",
                    reference.id().as_str(),
                    reference.revision()
                ))
            })?;
            let recomputed = sandbox_policy_content_digest(&policy);
            if recomputed != declared_digest {
                return Err(Error::invariant(format!(
                    "spawn source declaration digest {declared_digest} does not match \
                     authored policy {recomputed}"
                )));
            }
            Some(PolicyInput::authored(reference, &policy))
        }
        (None, None, None) => None,
        _ => {
            return Err(Error::invariant(
                "spawn source declaration policy reference is partially absent",
            ))
        }
    };
    Ok(Some(SpawnSourceSecurityDeclaration::new(
        source_policy,
        enforcement_profile,
    )))
}

/// The declared mode of an exact SpawnSource revision; absence is `COARSE_V1`.
///
/// The declaration is read through the validated reader, so a corrupt
/// declaration fails closed instead of being reported as coarse.
pub fn spawn_source_security_mode(
    tx: &Transaction<'_>,
    reference: &SpawnSourceRef,
) -> Result<SecurityContractMode, Error> {
    get_spawn_source(tx, reference)?.ok_or_else(|| {
        Error::not_found(format!(
            "spawn source {}@{} does not exist",
            reference.id().as_str(),
            reference.revision()
        ))
    })?;
    match spawn_source_security_declaration(tx, reference)? {
        None => Ok(SecurityContractMode::CoarseV1),
        Some(declaration) => Ok(declaration.mode()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SourceConfig {
        SourceConfig {
            config_ref: SourceConfigRef::new(
                SpawnSourceRef::new("codex-local", 1).unwrap(),
                "deep",
                1,
            )
            .unwrap(),
            config_digest: ConfigDigest::new(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .unwrap(),
            lifecycle_modes: None,
            continuity_modes: None,
            credential_refs: Vec::new(),
            claims: Vec::new(),
            status: ConfigStatus::Active,
        }
    }

    #[test]
    fn revision_identity_includes_locator_and_body_matches_mode() {
        let a = SourceConfigRevision::validated(
            config(),
            ConfigMode::ExternalRef,
            Some("file:///a".into()),
            None,
        );
        let b = SourceConfigRevision::validated(
            config(),
            ConfigMode::ExternalRef,
            Some("file:///b".into()),
            None,
        );
        let same = SourceConfigRevision::validated(
            config(),
            ConfigMode::ExternalRef,
            Some("file:///a".into()),
            None,
        );
        // The metadata-only relation says equal, but the complete durable
        // revision identity (which includes the locator) does not.
        assert!(a.config().same_config_contract_content(b.config()));
        assert!(!a.same_revision_content(&b));
        assert!(a.same_revision_content(&same));
        assert_eq!(a.mode(), ConfigMode::ExternalRef);
        assert_eq!(a.locator(), Some("file:///a"));
        match a.body() {
            SourceConfigBodyView::ExternalRef(locator) => assert_eq!(locator, "file:///a"),
            SourceConfigBodyView::OpaqueJson(_) => panic!("ExternalRef dispatched as opaque"),
        }

        let payload = serde_json::json!({"model": "x"});
        let opaque = SourceConfigRevision::validated(
            config(),
            ConfigMode::OpaqueJson,
            None,
            Some(payload.clone()),
        );
        assert_eq!(opaque.mode(), ConfigMode::OpaqueJson);
        assert!(opaque.locator().is_none());
        match opaque.body() {
            SourceConfigBodyView::OpaqueJson(value) => assert_eq!(*value, payload),
            SourceConfigBodyView::ExternalRef(_) => panic!("OpaqueJson dispatched as external"),
        }
    }
}
