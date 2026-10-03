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
    canonical_source_config_bytes, canonical_spawn_source_bytes, canonicalize_agent_type,
    canonicalize_source_config, canonicalize_spawn_source, capability_definition_content_digest,
    capability_definition_from_canonical_json, content_digest, is_valid_refinement,
    source_config_content_digest, source_config_revision_from_canonical_json,
    spawn_source_content_digest, spawn_source_from_canonical_json, AdapterBindingPolicy,
    AdapterPolicyRef, AgentType, AgentTypeId, AgentTypeLookup, AgentTypeRef, CapabilityCatalog,
    CapabilityDefinition, CapabilityRef, ConfigDigest, ConfigStatus, ContractError, SourceConfig,
    SourceConfigRef, SourceStatus, SpawnSource, SpawnSourceRef,
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
                    content_json, content_digest
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
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(map_sqlite)?;
    let mut catalog = CapabilityCatalog::new();
    for row in rows {
        let (capability_id, revision, matcher, class, polarity, content_json, content_digest) =
            row.map_err(map_sqlite)?;
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
        verify_reencoded(
            &canonical_capability_definition_bytes(&canonical_ref, &definition),
            &content_json,
            "capability definition",
        )?;
        catalog
            .define(
                reference,
                definition.matcher_kind,
                definition.security_class,
                definition.polarity,
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
    let had_existing = existing.is_some();
    guard_immutable(existing, &digest, "capability definition")?;
    tx.execute(
        "INSERT OR IGNORE INTO capability_definitions(
             capability_id, revision, matcher_kind, security_class, polarity,
             content_json, content_digest, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            reference.capability_id().as_str(),
            reference.revision(),
            matcher_sql(definition),
            class_sql(definition),
            polarity_sql(definition),
            content_json,
            digest,
            now,
        ],
    )
    .map_err(map_sqlite)?;
    if had_existing {
        // An idempotent republish of an existing exact revision MUST only succeed
        // if that stored revision still passes the catalog integrity boundary.
        get_capability_definition(tx, reference)?
            .ok_or_else(|| Error::invariant("published capability definition is missing"))?;
    }
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
    let had_existing = existing.is_some();
    guard_immutable(existing, &digest, "adapter binding policy")?;
    tx.execute(
        "INSERT OR IGNORE INTO adapter_binding_policies(
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
    // The value type carries an operator disposition; persist it separately.
    tx.execute(
        "INSERT OR IGNORE INTO adapter_binding_policy_dispositions(
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
    if had_existing {
        get_adapter_binding_policy(tx, &policy.policy_ref)?
            .ok_or_else(|| Error::invariant("published adapter binding policy is missing"))?;
    }
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
    let mut canonical = agent.clone();
    let catalog = load_capability_catalog(tx)?;
    canonicalize_agent_type(&mut canonical, &catalog).map_err(contract_fault)?;

    // based_on provenance: the base must exist and the derived contract must be
    // a monotonic narrowing of it (spec 06). A missing base fails closed rather
    // than trusting the field.
    if let Some(base_ref) = &canonical.based_on {
        let base = load_agent_type(tx, base_ref)?.ok_or_else(|| {
            Error::not_found(format!(
                "based_on agent type {}@{} does not exist",
                base_ref.id().as_str(),
                base_ref.revision()
            ))
        })?;
        is_valid_refinement(&base, &canonical, &catalog).map_err(contract_fault)?;
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
    let had_existing = existing.is_some();
    if existing.is_none() {
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
    }
    guard_immutable(existing, &digest, "agent type")?;
    tx.execute(
        "INSERT OR IGNORE INTO agent_type_dispositions(
             type_id, revision, status, deprecated_at, updated_at)
         VALUES(?1,?2,'PUBLISHED',NULL,?3)",
        params![
            canonical.type_ref.id().as_str(),
            canonical.type_ref.revision(),
            now
        ],
    )
    .map_err(map_sqlite)?;
    if had_existing {
        load_agent_type(tx, &canonical.type_ref)?
            .ok_or_else(|| Error::invariant("published agent type is missing"))?;
    }
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
    let mut canonical = source.clone();
    let catalog = load_capability_catalog(tx)?;
    canonicalize_spawn_source(&mut canonical, &catalog).map_err(contract_fault)?;

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
    let had_existing = existing.is_some();
    if existing.is_none() {
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
    }
    guard_immutable(existing, &digest, "spawn source")?;
    // A fresh revision persists the caller's initial disposition; re-publishing
    // an existing exact revision is content-idempotent and leaves the live
    // disposition untouched (drive later changes through set_spawn_source_status).
    tx.execute(
        "INSERT OR IGNORE INTO spawn_source_dispositions(
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
    if had_existing {
        load_spawn_source(tx, &canonical.source_ref)?
            .ok_or_else(|| Error::invariant("published spawn source is missing"))?;
    }
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
    let had_existing = existing.is_some();
    if existing.is_none() {
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
    }
    guard_immutable(existing, &digest, "source config")?;
    // A fresh revision persists the caller's initial disposition; re-publishing
    // an existing exact revision is content-idempotent and leaves the live
    // disposition untouched (drive later changes through set_source_config_status).
    tx.execute(
        "INSERT OR IGNORE INTO source_config_dispositions(
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
    if had_existing {
        load_source_config_revision(tx, &config.config_ref)?
            .ok_or_else(|| Error::invariant("published source config is missing"))?;
    }
    Ok(digest)
}

// =============================================================================
// Reads
// =============================================================================

fn load_agent_type(
    tx: &Transaction<'_>,
    reference: &AgentTypeRef,
) -> Result<Option<AgentType>, Error> {
    let row = query_opt(
        tx,
        "SELECT content_json, content_digest, based_on_type_id, based_on_revision
         FROM agent_types WHERE type_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((json, digest, base_id, base_revision)) => {
            verify_content_digest(&json, &digest, "agent type")?;
            let mut agent = agent_type_from_canonical_json(&json).map_err(contract_fault)?;
            cross_check(agent.type_ref == *reference, "agent type")?;
            let canonical_base = agent
                .based_on
                .as_ref()
                .map(|base| (base.id().as_str().to_string(), base.revision() as i64));
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
            // The stored document MUST be the unique canonical encoding: decode,
            // re-canonicalize against the catalog, then require byte equality.
            let catalog = load_capability_catalog(tx)?;
            canonicalize_agent_type(&mut agent, &catalog).map_err(contract_fault)?;
            verify_reencoded(&canonical_agent_type_bytes(&agent), &json, "agent type")?;
            Ok(Some(agent))
        }
    }
}

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
            let canonical_base = agent
                .based_on
                .as_ref()
                .map(|base| (base.id().as_str().to_string(), base.revision() as i64));
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

fn load_spawn_source(
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
                    && source.adapter_policy.revision() as i64 == policy_revision,
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

/// The single validated authority for one SourceConfig revision: the decoded
/// config, its opaque body mode, and its `ExternalRef` locator (if any). Every
/// SourceConfig getter derives from this record, and the duplicated
/// body/mode/locator/digest columns MUST agree with the canonical document, so a
/// drifted column can never become a second source of truth.
#[derive(Clone, Debug)]
pub struct SourceConfigRevision {
    pub config: SourceConfig,
    pub mode: ConfigMode,
    pub locator: Option<String>,
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
    match mode {
        ConfigMode::ExternalRef => {
            cross_check(payload_json.is_none(), "source config")?;
            cross_check(locator.is_some(), "source config")?;
            cross_check(locator == canonical_locator, "source config")?;
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
        }
    }
    Ok(Some(SourceConfigRevision {
        config,
        mode,
        locator: canonical_locator,
    }))
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
                content_json, content_digest
         FROM capability_definitions WHERE capability_id=?1 AND revision=?2",
        params![reference.capability_id().as_str(), reference.revision()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        },
    )?;
    match row {
        None => Ok(None),
        Some((capability_id, revision, matcher, class, polarity, json, digest)) => {
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
    let current = query_opt(
        tx,
        "SELECT status FROM agent_type_dispositions WHERE type_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| {
        Error::not_found(format!(
            "agent type {}@{} is not published",
            reference.id().as_str(),
            reference.revision()
        ))
    })?;
    if current == "DEPRECATED" && status == AgentTypeStatus::Published {
        return Err(Error::invalid_transition(
            "a deprecated AgentType revision cannot be republished",
        ));
    }
    let deprecated_at = (status == AgentTypeStatus::Deprecated).then_some(now);
    tx.execute(
        "UPDATE agent_type_dispositions
         SET status=?1, deprecated_at=?2, updated_at=?3
         WHERE type_id=?4 AND revision=?5",
        params![
            status.as_sql(),
            deprecated_at,
            now,
            reference.id().as_str(),
            reference.revision()
        ],
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

pub fn set_spawn_source_status(
    tx: &Transaction<'_>,
    now: UnixTime,
    reference: &SpawnSourceRef,
    status: SourceStatus,
) -> Result<(), Error> {
    let current = query_opt(
        tx,
        "SELECT status FROM spawn_source_dispositions WHERE source_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::not_found("spawn source disposition missing"))?;
    let next = source_status_sql(status);
    check_monotonic(&current, next)?;
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
    let current = query_opt(
        tx,
        "SELECT status FROM source_config_dispositions
         WHERE source_id=?1 AND source_revision=?2 AND config_id=?3 AND config_revision=?4",
        params![id, source_revision, config_id, config_revision],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::not_found("source config disposition missing"))?;
    let next = config_status_sql(status);
    check_monotonic(&current, next)?;
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
    let current = query_opt(
        tx,
        "SELECT status FROM adapter_binding_policy_dispositions
         WHERE policy_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )?
    .ok_or_else(|| Error::not_found("adapter binding policy disposition missing"))?;
    let next = config_status_sql(status);
    check_monotonic(&current, next)?;
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
/// A published ref is only selectable if its exact revision passes the same
/// durability boundary as `get_agent_type` (digest, canonical re-encoding,
/// relational mirror). Any corruption fails the whole lookup closed rather than
/// silently dropping a revision or falling back to an older one, so the lookup
/// is not a second, weaker authority.
pub fn load_agent_type_lookup(tx: &Transaction<'_>) -> Result<DurableAgentTypeLookup, Error> {
    let mut statement = tx
        .prepare(
            "SELECT at.type_id, at.revision
             FROM agent_types at
             JOIN agent_type_dispositions d
               ON d.type_id=at.type_id AND d.revision=at.revision
             WHERE d.status='PUBLISHED'",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(map_sqlite)?;
    let mut published = BTreeSet::new();
    for row in rows {
        let (type_id, revision) = row.map_err(map_sqlite)?;
        let reference = AgentTypeRef::new(type_id, revision as u64).map_err(contract_fault)?;
        load_agent_type(tx, &reference)?
            .ok_or_else(|| Error::invariant("published agent type is missing"))?;
        published.insert(reference);
    }
    Ok(DurableAgentTypeLookup { published })
}
