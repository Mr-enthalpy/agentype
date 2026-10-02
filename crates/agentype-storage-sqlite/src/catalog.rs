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
    capability_definition_from_canonical_json, is_valid_refinement,
    source_config_from_canonical_json, spawn_source_content_digest,
    spawn_source_from_canonical_json, AdapterBindingPolicy, AdapterPolicyRef, AgentType,
    AgentTypeId, AgentTypeLookup, AgentTypeRef, CapabilityCatalog, CapabilityDefinition,
    CapabilityRef, ConfigStatus, ContractError, SourceConfig, SourceConfigRef, SourceStatus,
    SpawnSource, SpawnSourceRef,
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
#[derive(Clone, Debug)]
pub enum SourceConfigBody {
    /// A Core-opaque JSON body. `config_digest` MUST equal
    /// [`canonical_json_body_digest`] of this value.
    OpaqueJson(serde_json::Value),
    /// A locator owned entirely outside Core; no body is stored.
    ExternalRef,
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

// =============================================================================
// CapabilityCatalog
// =============================================================================

pub fn load_capability_catalog(tx: &Transaction<'_>) -> Result<CapabilityCatalog, Error> {
    let mut statement = tx
        .prepare(
            "SELECT capability_id, revision, content_json FROM capability_definitions
             ORDER BY capability_id, revision",
        )
        .map_err(map_sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(map_sqlite)?;
    let mut catalog = CapabilityCatalog::new();
    for row in rows {
        let (capability_id, revision, content_json) = row.map_err(map_sqlite)?;
        let definition =
            capability_definition_from_canonical_json(&content_json).map_err(contract_fault)?;
        let reference =
            CapabilityRef::new(capability_id, revision as u64).map_err(contract_fault)?;
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
    tx.execute(
        "INSERT OR IGNORE INTO spawn_source_dispositions(
             source_id, revision, status, updated_at)
         VALUES(?1,?2,'ACTIVE',?3)",
        params![
            canonical.source_ref.id().as_str(),
            canonical.source_ref.revision(),
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

    let (mode, payload_json) = match body {
        SourceConfigBody::OpaqueJson(payload) => {
            let expected = canonical_json_body_digest(payload);
            if config.config_digest.as_str() != expected {
                return Err(Error::invariant(format!(
                    "config digest {} does not match the canonical opaque JSON body digest {expected}",
                    config.config_digest.as_str()
                )));
            }
            (
                ConfigMode::OpaqueJson,
                Some(canonical_json_string(
                    serde_json::to_vec(payload)
                        .map_err(|e| Error::invariant(format!("config body: {e}")))?,
                    "config body",
                )?),
            )
        }
        SourceConfigBody::ExternalRef => (ConfigMode::ExternalRef, None),
    };

    let mut canonical = config.clone();
    let catalog = load_capability_catalog(tx)?;
    canonicalize_source_config(&mut canonical, &source, &catalog).map_err(contract_fault)?;

    let digest = agentype_agent_contract::source_config_content_digest(&canonical);
    let content_json =
        canonical_json_string(canonical_source_config_bytes(&canonical), "source config")?;
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
    if existing.is_none() {
        tx.execute(
            "INSERT INTO source_configs(
                 source_id, source_revision, config_id, config_revision,
                 config_mode, config_payload_json, config_digest, content_json,
                 content_digest, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                config.config_ref.source().id().as_str(),
                config.config_ref.source().revision(),
                config.config_ref.config_id().as_str(),
                config.config_ref.revision(),
                mode.as_sql(),
                payload_json,
                canonical.config_digest.as_str(),
                content_json,
                digest,
                now,
            ],
        )
        .map_err(map_sqlite)?;
    }
    guard_immutable(existing, &digest, "source config")?;
    tx.execute(
        "INSERT OR IGNORE INTO source_config_dispositions(
             source_id, source_revision, config_id, config_revision, status, updated_at)
         VALUES(?1,?2,?3,?4,'ACTIVE',?5)",
        params![
            config.config_ref.source().id().as_str(),
            config.config_ref.source().revision(),
            config.config_ref.config_id().as_str(),
            config.config_ref.revision(),
            now,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(digest)
}

// =============================================================================
// Reads
// =============================================================================

fn load_agent_type(
    tx: &Transaction<'_>,
    reference: &AgentTypeRef,
) -> Result<Option<AgentType>, Error> {
    let content = query_opt(
        tx,
        "SELECT content_json FROM agent_types WHERE type_id=?1 AND revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )?;
    content
        .map(|json| agent_type_from_canonical_json(&json).map_err(contract_fault))
        .transpose()
}

pub fn get_agent_type(
    tx: &Transaction<'_>,
    reference: &AgentTypeRef,
) -> Result<Option<(AgentType, AgentTypeStatus)>, Error> {
    let row = query_opt(
        tx,
        "SELECT at.content_json, d.status
         FROM agent_types at
         JOIN agent_type_dispositions d
           ON d.type_id=at.type_id AND d.revision=at.revision
         WHERE at.type_id=?1 AND at.revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    match row {
        None => Ok(None),
        Some((json, status)) => Ok(Some((
            agent_type_from_canonical_json(&json).map_err(contract_fault)?,
            AgentTypeStatus::parse_sql(&status)?,
        ))),
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
        "SELECT s.content_json, d.status
         FROM spawn_sources s
         JOIN spawn_source_dispositions d
           ON d.source_id=s.source_id AND d.revision=s.revision
         WHERE s.source_id=?1 AND s.revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    match row {
        None => Ok(None),
        Some((json, status)) => Ok(Some(
            spawn_source_from_canonical_json(&json, source_status_parse(&status)?)
                .map_err(contract_fault)?,
        )),
    }
}

pub fn get_source_config(
    tx: &Transaction<'_>,
    reference: &SourceConfigRef,
) -> Result<Option<SourceConfig>, Error> {
    let row = query_opt(
        tx,
        "SELECT c.content_json, d.status
         FROM source_configs c
         JOIN source_config_dispositions d
           ON d.source_id=c.source_id AND d.source_revision=c.source_revision
          AND d.config_id=c.config_id AND d.config_revision=c.config_revision
         WHERE c.source_id=?1 AND c.source_revision=?2 AND c.config_id=?3 AND c.config_revision=?4",
        params![
            reference.source().id().as_str(),
            reference.source().revision(),
            reference.config_id().as_str(),
            reference.revision()
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    match row {
        None => Ok(None),
        Some((json, status)) => Ok(Some(
            source_config_from_canonical_json(&json, config_status_parse(&status)?)
                .map_err(contract_fault)?,
        )),
    }
}

/// The opaque body kind a published SourceConfig revision was stored with.
pub fn get_source_config_mode(
    tx: &Transaction<'_>,
    reference: &SourceConfigRef,
) -> Result<Option<ConfigMode>, Error> {
    let mode = query_opt(
        tx,
        "SELECT config_mode FROM source_configs
         WHERE source_id=?1 AND source_revision=?2 AND config_id=?3 AND config_revision=?4",
        params![
            reference.source().id().as_str(),
            reference.source().revision(),
            reference.config_id().as_str(),
            reference.revision()
        ],
        |row| row.get::<_, String>(0),
    )?;
    mode.map(|value| ConfigMode::parse_sql(&value)).transpose()
}

pub fn get_adapter_binding_policy(
    tx: &Transaction<'_>,
    reference: &AdapterPolicyRef,
) -> Result<Option<AdapterBindingPolicy>, Error> {
    let row = query_opt(
        tx,
        "SELECT p.content_json, d.status
         FROM adapter_binding_policies p
         JOIN adapter_binding_policy_dispositions d
           ON d.policy_id=p.policy_id AND d.revision=p.revision
         WHERE p.policy_id=?1 AND p.revision=?2",
        params![reference.id().as_str(), reference.revision()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    match row {
        None => Ok(None),
        Some((json, status)) => Ok(Some(
            adapter_binding_policy_from_canonical_json(&json, config_status_parse(&status)?)
                .map_err(contract_fault)?,
        )),
    }
}

pub fn get_capability_definition(
    tx: &Transaction<'_>,
    reference: &CapabilityRef,
) -> Result<Option<CapabilityDefinition>, Error> {
    let content = query_opt(
        tx,
        "SELECT content_json FROM capability_definitions
         WHERE capability_id=?1 AND revision=?2",
        params![reference.capability_id().as_str(), reference.revision()],
        |row| row.get::<_, String>(0),
    )?;
    content
        .map(|json| capability_definition_from_canonical_json(&json).map_err(contract_fault))
        .transpose()
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
        published.insert(AgentTypeRef::new(type_id, revision as u64).map_err(contract_fault)?);
    }
    Ok(DurableAgentTypeLookup { published })
}
