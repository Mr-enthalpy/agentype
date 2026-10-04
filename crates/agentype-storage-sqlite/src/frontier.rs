//! M6-A Semantic Frontier Kernel persistence and atomic transactions.
//!
//! Enforces:
//! - INV-A1: Every M6 semantic Task belongs to exactly one Generation.
//! - INV-A5: Proposal admission atomically creates M5 Task and GenerationTaskBinding.
//! - INV-A6: A proposal can admit at most one Task (idempotent duplicate rejection).
//! - INV-A7: No EXPAND Task admitted after Generation enters FROZEN.
//! - INV-A8: No Task admitted after Generation enters CLOSED.
//! - INV-A9: Generation close never manufactures Task terminality (settled check required).
//! - INV-A19: Generation state changes and wakeup outbox insertion are atomic.

use crate::store::{json_dump, json_load, map_sqlite, query_opt};
use crate::txutil::required_partition;
use agentype_agent_contract::{task_agent_requirement_content_digest, AgentRequirementDraft};
use agentype_core::{
    generation_allows_admit, is_generation_settled, is_generation_view_settled, ArtifactRef,
    BatchId, ContinuityPreference, Error, FailureClass, GenerationId, GenerationRecord,
    GenerationState, GenerationView, InformationFunction, IntentSource, OutboxEventId, PartitionId,
    ProposalExpirationReason, ProposalId, ProposalRecord, ProposalStateKind, RawWorkIntent,
    ResultId, RetryPolicy, SemanticInputSet, TaskId, TaskSettledSnapshot, TaskSpec, TaskState,
    UnixTime, WorkstreamId, GENERATION_CLOSED, GENERATION_FROZEN,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn semantic_input_set_to_json(set: &SemanticInputSet) -> Value {
    let results: Vec<Value> = set
        .result_ids
        .iter()
        .map(|r| Value::String(r.as_str().to_string()))
        .collect();
    let artifacts: Vec<Value> = set
        .artifact_refs
        .iter()
        .map(|a| {
            let mut m = serde_json::Map::new();
            m.insert("locator".into(), Value::String(a.locator.clone()));
            m.insert("digest".into(), Value::String(a.digest.clone()));
            Value::Object(m)
        })
        .collect();
    let seeds: Vec<Value> = set
        .seed_refs
        .iter()
        .map(|s| Value::String(s.clone()))
        .collect();

    let mut map = serde_json::Map::new();
    map.insert("result_ids".into(), Value::Array(results));
    map.insert("artifact_refs".into(), Value::Array(artifacts));
    map.insert("seed_refs".into(), Value::Array(seeds));
    Value::Object(map)
}

pub fn semantic_input_set_from_json(val: &Value) -> Result<SemanticInputSet, Error> {
    let obj = val
        .as_object()
        .ok_or_else(|| Error::invariant("semantic_input_set must be a JSON object"))?;
    for k in obj.keys() {
        if k != "result_ids" && k != "artifact_refs" && k != "seed_refs" {
            return Err(Error::invariant(format!(
                "unknown field in semantic_input_set: {k}"
            )));
        }
    }
    let mut set = SemanticInputSet::new();
    if let Some(arr_val) = obj.get("result_ids") {
        let arr = arr_val
            .as_array()
            .ok_or_else(|| Error::invariant("result_ids must be an array of strings"))?;
        for v in arr {
            let s = v
                .as_str()
                .ok_or_else(|| Error::invariant("result_ids element must be a string"))?;
            set.result_ids.push(ResultId::from_string(s));
        }
    }
    if let Some(arr_val) = obj.get("artifact_refs") {
        let arr = arr_val
            .as_array()
            .ok_or_else(|| Error::invariant("artifact_refs must be an array of objects"))?;
        for v in arr {
            let art_obj = v
                .as_object()
                .ok_or_else(|| Error::invariant("artifact_refs element must be an object"))?;
            let locator = art_obj
                .get("locator")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::invariant("artifact_ref missing locator"))?;
            let digest = art_obj
                .get("digest")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::invariant("artifact_ref missing digest"))?;
            set.artifact_refs.push(ArtifactRef::new(locator, digest)?);
        }
    }
    if let Some(arr_val) = obj.get("seed_refs") {
        let arr = arr_val
            .as_array()
            .ok_or_else(|| Error::invariant("seed_refs must be an array of strings"))?;
        for v in arr {
            let s = v
                .as_str()
                .ok_or_else(|| Error::invariant("seed_refs element must be a string"))?;
            set.seed_refs.push(s.to_string());
        }
    }
    Ok(set)
}

/// Reject repeated references so a `SemanticInputSet` has true set semantics
/// (and therefore one canonical fingerprint per evidence set).
fn reject_duplicate_refs<T>(refs: &[T], kind: &str) -> Result<(), Error>
where
    T: std::hash::Hash + Eq,
{
    let mut seen = HashSet::with_capacity(refs.len());
    for r in refs {
        if !seen.insert(r) {
            return Err(Error::invariant(format!(
                "duplicate {kind} reference in semantic input set"
            )));
        }
    }
    Ok(())
}

pub fn validate_semantic_input_set(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
    set: &SemanticInputSet,
) -> Result<(), Error> {
    reject_duplicate_refs(&set.result_ids, "result")?;
    reject_duplicate_refs(&set.artifact_refs, "artifact")?;
    reject_duplicate_refs(&set.seed_refs, "seed")?;

    for rid in &set.result_ids {
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM results WHERE id = ?1",
                params![rid.as_str()],
                |_| Ok(true),
            )
            .optional()
            .map_err(map_sqlite)?
            .unwrap_or(false);
        if !exists {
            return Err(Error::not_found(format!(
                "provenance result_id {:?} does not exist",
                rid.as_str()
            )));
        }
    }
    for art in &set.artifact_refs {
        if art.locator.trim().is_empty() {
            return Err(Error::invariant("artifact locator cannot be empty"));
        }
        if art.digest.trim().is_empty() {
            return Err(Error::invariant(
                "artifact digest cannot be empty (provenance requires immutable content digest)",
            ));
        }
    }
    if !set.seed_refs.is_empty() {
        let seed_payload_str: String = tx
            .query_row(
                "SELECT seed_payload_json FROM generations WHERE generation_id = ?1",
                params![generation_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite)?
            .ok_or_else(|| Error::not_found(format!("generation {:?}", generation_id.as_str())))?;
        let seed_payload_val: Value = json_load(&seed_payload_str)?;
        let obj = seed_payload_val.as_object().ok_or_else(|| {
            Error::invalid_authority(
                "generation seed_payload must be a JSON object to resolve seed_refs",
            )
        })?;
        for sref in &set.seed_refs {
            if !obj.contains_key(sref) {
                return Err(Error::not_found(format!(
                    "seed_ref {:?} not found in generation {:?} seed_payload",
                    sref,
                    generation_id.as_str()
                )));
            }
        }
    }
    Ok(())
}

pub fn task_spec_to_json(spec: &TaskSpec) -> Result<Value, Error> {
    spec.canonical_json()
}

pub fn task_spec_from_json(val: &Value) -> Result<TaskSpec, Error> {
    let obj = val
        .as_object()
        .ok_or_else(|| Error::invariant("task_spec must be a JSON object"))?;

    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invariant("missing or invalid field: name"))?
        .to_string();

    let payload = obj
        .get("payload")
        .ok_or_else(|| Error::invariant("missing field: payload"))?
        .clone();

    let acceptance = obj
        .get("acceptance")
        .ok_or_else(|| Error::invariant("missing field: acceptance"))?
        .clone();

    let part_str = obj
        .get("partition")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invariant("missing or invalid field: partition"))?;
    if part_str.trim().is_empty() {
        return Err(Error::invariant("partition cannot be empty"));
    }
    let partition = PartitionId::new(part_str);

    let workstream_id = match obj.get("workstream_id") {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) => Some(WorkstreamId::from_string(s)),
        Some(_) => return Err(Error::invariant("workstream_id must be a string or null")),
    };

    let continuity_str = obj
        .get("continuity")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invariant("missing or invalid field: continuity"))?;
    let continuity = ContinuityPreference::parse_sql(continuity_str)?;

    let tags_arr = obj
        .get("affinity_tags")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invariant("missing or invalid field: affinity_tags"))?;
    let mut affinity_tags = Vec::with_capacity(tags_arr.len());
    for item in tags_arr {
        let t = item
            .as_str()
            .ok_or_else(|| Error::invariant("affinity_tags element must be a string"))?;
        affinity_tags.push(t.to_string());
    }

    let mode_str = obj
        .get("workspace_mode")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invariant("missing or invalid field: workspace_mode"))?;
    let workspace_mode = agentype_core::WorkspaceMode::parse_sql(mode_str)?;

    let deps_arr = obj
        .get("dependencies")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invariant("missing or invalid field: dependencies"))?;
    let mut dependencies = Vec::with_capacity(deps_arr.len());
    for item in deps_arr {
        let d = item
            .as_str()
            .ok_or_else(|| Error::invariant("dependencies element must be a string"))?;
        dependencies.push(d.to_string());
    }

    let priority = obj
        .get("priority")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::invariant("missing or invalid field: priority"))?;

    let max_attempts = obj
        .get("max_attempts")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invariant("missing or invalid field: max_attempts"))?
        as u32;
    if max_attempts == 0 {
        return Err(Error::invariant("max_attempts must be >= 1"));
    }

    let rc_arr = obj
        .get("retry_classes")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invariant("missing or invalid field: retry_classes"))?;
    let mut retry_classes = Vec::with_capacity(rc_arr.len());
    for item in rc_arr {
        let c_str = item
            .as_str()
            .ok_or_else(|| Error::invariant("retry_classes element must be a string"))?;
        retry_classes.push(FailureClass::parse_sql(c_str)?);
    }

    let base_backoff_seconds = obj
        .get("base_backoff_seconds")
        .and_then(Value::as_f64)
        .ok_or_else(|| Error::invariant("missing or invalid field: base_backoff_seconds"))?;
    if !base_backoff_seconds.is_finite() || base_backoff_seconds < 0.0 {
        return Err(Error::invariant(
            "base_backoff_seconds must be a finite non-negative number",
        ));
    }

    let max_backoff_seconds = obj
        .get("max_backoff_seconds")
        .and_then(Value::as_f64)
        .ok_or_else(|| Error::invariant("missing or invalid field: max_backoff_seconds"))?;
    if !max_backoff_seconds.is_finite() || max_backoff_seconds < base_backoff_seconds {
        return Err(Error::invariant(
            "max_backoff_seconds must be a finite number >= base_backoff_seconds",
        ));
    }

    let supersedes_task_id = match obj.get("supersedes_task_id") {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) => Some(TaskId::from_string(s)),
        Some(_) => {
            return Err(Error::invariant(
                "supersedes_task_id must be a string or null",
            ))
        }
    };

    let task_id = match obj.get("task_id") {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) => Some(TaskId::from_string(s)),
        Some(_) => return Err(Error::invariant("task_id must be a string or null")),
    };

    Ok(TaskSpec {
        name,
        payload,
        acceptance,
        partition,
        workstream_id,
        continuity,
        affinity_tags,
        workspace_mode,
        dependencies,
        priority,
        retry_policy: RetryPolicy {
            max_attempts,
            retry_classes,
            base_backoff_seconds,
            max_backoff_seconds,
        },
        supersedes_task_id,
        task_id,
    })
}

/// Create a new semantic admission frontier.
pub fn create_generation(
    tx: &Transaction<'_>,
    now: UnixTime,
    seed_payload: Value,
) -> Result<GenerationRecord, Error> {
    create_generation_with_policy(tx, now, seed_payload, None)
}

/// Create a new semantic admission frontier with an optional immutable
/// Generation policy (M6-B.3 `D-GEN-POLICY`). The policy is written in the same
/// transaction as the Generation; it is never edited afterwards.
pub fn create_generation_with_policy(
    tx: &Transaction<'_>,
    now: UnixTime,
    seed_payload: Value,
    policy: Option<agentype_agent_contract::GenerationPolicy>,
) -> Result<GenerationRecord, Error> {
    let generation_id = GenerationId::new();
    let seed_json = json_dump(&seed_payload);

    tx.execute(
        "INSERT INTO generations(generation_id, state, revision, admission_seq, seed_payload_json, created_at)
         VALUES(?1, 'OPEN', 0, 0, ?2, ?3)",
        params![generation_id.as_str(), seed_json, now],
    )
    .map_err(map_sqlite)?;

    if let Some(policy) = &policy {
        crate::requirement::insert_generation_policy(tx, now, &generation_id, policy)?;
    }

    Ok(GenerationRecord {
        generation_id,
        state: GenerationState::Open,
        revision: 0,
        admission_seq: 0,
        seed_payload,
        created_at: now,
        frozen_at: None,
        closed_at: None,
    })
}

/// Resolve a typed intent source into the persisted `(source_kind, source_ref)`
/// identity, mechanically anchoring a result-backed intent to a durable Result.
pub fn resolve_intent_source(
    tx: &Transaction<'_>,
    source: &IntentSource,
) -> Result<(String, String), Error> {
    match source {
        IntentSource::Root { command_ref } => {
            if command_ref.trim().is_empty() {
                return Err(Error::invalid_authority(
                    "root intent command_ref cannot be empty",
                ));
            }
            Ok(("root".to_string(), command_ref.clone()))
        }
        IntentSource::Result { result_id } => {
            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM results WHERE id = ?1",
                    params![result_id.as_str()],
                    |_| Ok(true),
                )
                .optional()
                .map_err(map_sqlite)?
                .unwrap_or(false);
            if !exists {
                return Err(Error::not_found(format!(
                    "intent source result {:?} does not exist",
                    result_id.as_str()
                )));
            }
            Ok(("result".to_string(), result_id.as_str().to_string()))
        }
    }
}

/// Shared compilation core once an intent and its resolved source identity are
/// known. Idempotent on
/// `(generation_id, source_kind, source_ref, raw_intent_key, compiler_version)`.
fn compile_intent_inner(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &GenerationId,
    intent: RawWorkIntent,
    source_kind: &str,
    source_ref: &str,
    compiler_version: u32,
) -> Result<ProposalRecord, Error> {
    // 1. Compute the canonical content fingerprint up front. Current frontier
    // state governs whether a *new* semantic commitment may be created; it does
    // not govern replay/observation of a commitment that already durably exists.
    let fingerprint = intent.fingerprint()?;

    // 2. Check for existing proposal with same (generation_id, source_kind, source_ref, raw_intent_key, compiler_version)
    if let Some(existing) = query_opt(
        tx,
        "SELECT proposal_id, generation_id, source_kind, source_ref, raw_intent_key,
                intent_fingerprint, objective, rationale, information_function, normalized_task_spec_json,
                semantic_input_set_json, compiler_version, state, admitted_task_id, expiration_reason,
                rejection_reason, created_at, updated_at
         FROM compiled_work_proposals
         WHERE generation_id=?1 AND source_kind=?2 AND source_ref=?3 AND raw_intent_key=?4 AND compiler_version=?5",
        params![
            generation_id.as_str(),
            source_kind,
            source_ref,
            intent.raw_intent_key,
            compiler_version
        ],
        |r| {
            let pid: String = r.get(0)?;
            let gid: String = r.get(1)?;
            let sk: String = r.get(2)?;
            let sr: String = r.get(3)?;
            let rik: String = r.get(4)?;
            let ifp: String = r.get(5)?;
            let obj: String = r.get(6)?;
            let rat: Option<String> = r.get(7)?;
            let if_str: String = r.get(8)?;
            let spec_str: Option<String> = r.get(9)?;
            let set_str: String = r.get(10)?;
            let cv: u32 = r.get(11)?;
            let st_str: String = r.get(12)?;
            let at_str: Option<String> = r.get(13)?;
            let er_str: Option<String> = r.get(14)?;
            let rej_str: Option<String> = r.get(15)?;
            let cat: f64 = r.get(16)?;
            let uat: f64 = r.get(17)?;

            Ok((
                pid, gid, sk, sr, rik, ifp, obj, rat, if_str, spec_str, set_str, cv, st_str, at_str, er_str,
                rej_str, cat, uat,
            ))
        },
    )? {
        let (
            pid,
            gid,
            sk,
            sr,
            rik,
            ifp,
            obj,
            rat,
            if_str,
            spec_str,
            set_str,
            cv,
            st_str,
            at_str,
            er_str,
            rej_str,
            cat,
            uat,
        ) = existing;

        if ifp != fingerprint {
            return Err(Error::conflict(format!(
                "compiled proposal already exists with different intent content for key {:?}",
                intent.raw_intent_key
            )));
        }

        let info_fn = InformationFunction::parse_sql(&if_str)?;
        let norm_spec = match spec_str {
            Some(s) => {
                let spec_val = json_load(&s)?;
                Some(task_spec_from_json(&spec_val)?)
            }
            None => None,
        };
        let set_val = json_load(&set_str)?;
        let input_set = semantic_input_set_from_json(&set_val)?;
        let state = ProposalStateKind::parse_sql(&st_str)?;
        let exp_reason = er_str
            .as_deref()
            .map(ProposalExpirationReason::parse_sql)
            .transpose()?;

        return Ok(ProposalRecord {
            proposal_id: ProposalId::from_string(pid),
            generation_id: GenerationId::from_string(gid),
            source_kind: sk,
            source_ref: sr,
            raw_intent_key: rik,
            intent_fingerprint: ifp,
            objective: obj,
            rationale: rat,
            information_function: info_fn,
            normalized_task_spec: norm_spec,
            semantic_input_set: input_set,
            compiler_version: cv,
            state,
            admitted_task_id: at_str.map(TaskId::from_string),
            expiration_reason: exp_reason,
            rejection_reason: rej_str,
            created_at: cat,
            updated_at: uat,
        });
    }

    // 3. No durable proposal exists yet, so this is a new semantic commitment.
    // Verify the generation exists and its frontier admits a *new* compilation.
    let gen_state_str: String = tx
        .query_row(
            "SELECT state FROM generations WHERE generation_id=?1",
            params![generation_id.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(|| Error::not_found(format!("generation {:?}", generation_id.as_str())))?;

    let gen_state = GenerationState::parse_sql(&gen_state_str)?;
    if gen_state == GenerationState::Closed {
        return Err(Error::invalid_transition(format!(
            "cannot compile intent for CLOSED generation {:?}",
            generation_id.as_str()
        )));
    }
    if gen_state == GenerationState::Frozen
        && intent.information_function == InformationFunction::Expand
    {
        return Err(Error::invalid_transition(format!(
            "cannot compile EXPAND intent for FROZEN generation {:?}",
            generation_id.as_str()
        )));
    }

    // 4. Validate semantic input set provenance
    validate_semantic_input_set(tx, generation_id, &intent.semantic_input_set)?;

    // TaskSpec dependencies must be empty in M6-A
    if let Some(ref spec) = intent.suggested_task_spec {
        if !spec.dependencies.is_empty() {
            return Err(Error::invalid_authority(
                "TaskSpec dependencies must be empty in M6-A",
            ));
        }
    }

    // 5. Build proposal record and insert
    let norm_spec = intent.suggested_task_spec;
    let spec_json_opt = match norm_spec.as_ref() {
        Some(s) => Some(json_dump(&task_spec_to_json(s)?)),
        None => None,
    };
    let proposal_id = ProposalId::new();
    let set_json = json_dump(&semantic_input_set_to_json(&intent.semantic_input_set));

    tx.execute(
        "INSERT INTO compiled_work_proposals(
            proposal_id, generation_id, source_kind, source_ref, raw_intent_key,
            intent_fingerprint, objective, rationale, information_function, normalized_task_spec_json, semantic_input_set_json,
            compiler_version, state, created_at, updated_at
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'PENDING', ?13, ?13)",
        params![
            proposal_id.as_str(),
            generation_id.as_str(),
            source_kind,
            source_ref,
            intent.raw_intent_key,
            fingerprint,
            intent.objective,
            intent.rationale,
            intent.information_function.as_sql(),
            spec_json_opt,
            set_json,
            compiler_version,
            now
        ],
    )
    .map_err(map_sqlite)?;

    // Insert wakeup notification for Root
    let event_id = OutboxEventId::new();
    let mut payload = serde_json::Map::new();
    payload.insert(
        "generation_id".into(),
        Value::String(generation_id.as_str().to_string()),
    );
    payload.insert(
        "proposal_id".into(),
        Value::String(proposal_id.as_str().to_string()),
    );
    payload.insert(
        "information_function".into(),
        Value::String(intent.information_function.as_sql().to_string()),
    );
    tx.execute(
        "INSERT INTO notification_outbox(
            id, event_type, aggregate_type, aggregate_id, payload_json,
            state, delivery_attempts, next_delivery_at, created_at
         )
         VALUES(?1, 'PROPOSAL_AVAILABLE', 'generation', ?2, ?3, 'PENDING', 0, ?4, ?4)",
        params![
            event_id.as_str(),
            generation_id.as_str(),
            json_dump(&Value::Object(payload)),
            now
        ],
    )
    .map_err(map_sqlite)?;

    Ok(ProposalRecord {
        proposal_id,
        generation_id: generation_id.clone(),
        source_kind: source_kind.to_string(),
        source_ref: source_ref.to_string(),
        raw_intent_key: intent.raw_intent_key,
        intent_fingerprint: fingerprint,
        objective: intent.objective,
        rationale: intent.rationale,
        information_function: intent.information_function,
        normalized_task_spec: norm_spec,
        semantic_input_set: intent.semantic_input_set,
        compiler_version,
        state: ProposalStateKind::Pending,
        admitted_task_id: None,
        expiration_reason: None,
        rejection_reason: None,
        created_at: now,
        updated_at: now,
    })
}

/// Reserved payload namespace for M6-owned semantic ingress data.
pub const RESULT_INTENT_ENVELOPE_KEY: &str = "_agentype";
/// Reserved key under [`RESULT_INTENT_ENVELOPE_KEY`] holding carried intents.
pub const RESULT_INTENT_MAP_KEY: &str = "raw_work_intents";

const RESULT_INTENT_FIELDS: [&str; 5] = [
    "objective",
    "information_function",
    "rationale",
    "semantic_input_set",
    "suggested_task_spec",
];

/// Encode one intent as an envelope entry (the map key is the `raw_intent_key`,
/// so the entry itself must not carry it).
pub fn raw_work_intent_to_json(intent: &RawWorkIntent) -> Result<Value, Error> {
    let spec_val = match intent.suggested_task_spec.as_ref() {
        Some(spec) => task_spec_to_json(spec)?,
        None => Value::Null,
    };
    let mut map = serde_json::Map::new();
    map.insert("objective".into(), Value::String(intent.objective.clone()));
    map.insert(
        "information_function".into(),
        Value::String(intent.information_function.as_sql().to_string()),
    );
    map.insert(
        "rationale".into(),
        intent
            .rationale
            .as_ref()
            .map(|r| Value::String(r.clone()))
            .unwrap_or(Value::Null),
    );
    map.insert(
        "semantic_input_set".into(),
        semantic_input_set_to_json(&intent.semantic_input_set),
    );
    map.insert("suggested_task_spec".into(), spec_val);
    Ok(Value::Object(map))
}

/// Strictly decode one envelope entry into an intent under the given key.
pub fn raw_work_intent_from_json(
    raw_intent_key: &str,
    val: &Value,
) -> Result<RawWorkIntent, Error> {
    let obj = val
        .as_object()
        .ok_or_else(|| Error::invariant("raw_work_intent entry must be an object"))?;
    for k in obj.keys() {
        if !RESULT_INTENT_FIELDS.contains(&k.as_str()) {
            return Err(Error::invariant(format!(
                "unknown field in raw_work_intent entry: {k}"
            )));
        }
    }

    let objective = obj
        .get("objective")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invariant("missing or invalid field: objective"))?
        .to_string();

    let if_str = obj
        .get("information_function")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invariant("missing or invalid field: information_function"))?;
    let information_function = InformationFunction::parse_sql(if_str)?;

    let rationale = match obj.get("rationale") {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(Error::invariant("rationale must be a string or null")),
    };

    let set_val = obj
        .get("semantic_input_set")
        .ok_or_else(|| Error::invariant("missing field: semantic_input_set"))?;
    let semantic_input_set = semantic_input_set_from_json(set_val)?;

    let suggested_task_spec = match obj.get("suggested_task_spec") {
        Some(Value::Null) | None => None,
        Some(v) => Some(task_spec_from_json(v)?),
    };

    Ok(RawWorkIntent {
        raw_intent_key: raw_intent_key.to_string(),
        objective,
        information_function,
        semantic_input_set,
        rationale,
        suggested_task_spec,
    })
}

/// Reconstruct a worker-originated intent from the immutable Result that carried
/// it. Fails closed: a missing Result, a Result without the reserved envelope,
/// a missing key, or a malformed entry each produce an error and no commitment.
pub fn load_result_carried_intent(
    tx: &Transaction<'_>,
    result_id: &ResultId,
    raw_intent_key: &str,
) -> Result<RawWorkIntent, Error> {
    if raw_intent_key.trim().is_empty() {
        return Err(Error::invalid_authority("raw_intent_key cannot be empty"));
    }

    let payload_str: String = tx
        .query_row(
            "SELECT payload_json FROM results WHERE id = ?1",
            params![result_id.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(|| Error::not_found(format!("result {:?}", result_id.as_str())))?;
    let payload = json_load(&payload_str)?;

    let no_intent = || {
        Error::not_found(format!(
            "result {:?} does not carry a raw work intent for key {:?}",
            result_id.as_str(),
            raw_intent_key
        ))
    };

    let envelope = match payload.get(RESULT_INTENT_ENVELOPE_KEY) {
        Some(v) => v,
        None => return Err(no_intent()),
    };
    let envelope_obj = envelope
        .as_object()
        .ok_or_else(|| Error::invariant("result intent envelope (_agentype) must be an object"))?;
    let intents = match envelope_obj.get(RESULT_INTENT_MAP_KEY) {
        Some(v) => v,
        None => return Err(no_intent()),
    };
    let intents_obj = intents
        .as_object()
        .ok_or_else(|| Error::invariant("raw_work_intents envelope must be an object"))?;
    let entry = match intents_obj.get(raw_intent_key) {
        Some(v) => v,
        None => return Err(no_intent()),
    };

    raw_work_intent_from_json(raw_intent_key, entry)
}

/// Compile a Root-originated intent, anchored to a Root command reference.
pub fn compile_root_intent(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &GenerationId,
    intent: RawWorkIntent,
    command_ref: &str,
    compiler_version: u32,
) -> Result<ProposalRecord, Error> {
    let (source_kind, source_ref) = resolve_intent_source(tx, &IntentSource::root(command_ref)?)?;
    compile_intent_inner(
        tx,
        now,
        generation_id,
        intent,
        &source_kind,
        &source_ref,
        compiler_version,
    )
}

/// Compile a worker/harness-originated intent that is reconstructable from the
/// immutable Result payload named by `result_id`. The caller cannot supply the
/// intent: it is loaded from the Result, so provenance cannot be forged.
pub fn compile_result_intent(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &GenerationId,
    result_id: &ResultId,
    raw_intent_key: &str,
    compiler_version: u32,
) -> Result<ProposalRecord, Error> {
    let (source_kind, source_ref) =
        resolve_intent_source(tx, &IntentSource::result(result_id.clone()))?;
    let intent = load_result_carried_intent(tx, result_id, raw_intent_key)?;
    compile_intent_inner(
        tx,
        now,
        generation_id,
        intent,
        &source_kind,
        &source_ref,
        compiler_version,
    )
}

/// Read a ProposalRecord by proposal_id.
pub fn get_proposal(
    tx: &Transaction<'_>,
    proposal_id: &ProposalId,
) -> Result<ProposalRecord, Error> {
    let existing = query_opt(
        tx,
        "SELECT proposal_id, generation_id, source_kind, source_ref, raw_intent_key,
                intent_fingerprint, objective, rationale, information_function, normalized_task_spec_json,
                semantic_input_set_json, compiler_version, state, admitted_task_id, expiration_reason,
                rejection_reason, created_at, updated_at
         FROM compiled_work_proposals
         WHERE proposal_id=?1",
        params![proposal_id.as_str()],
        |r| {
            let pid: String = r.get(0)?;
            let gid: String = r.get(1)?;
            let sk: String = r.get(2)?;
            let sr: String = r.get(3)?;
            let rik: String = r.get(4)?;
            let ifp: String = r.get(5)?;
            let obj: String = r.get(6)?;
            let rat: Option<String> = r.get(7)?;
            let if_str: String = r.get(8)?;
            let spec_str: Option<String> = r.get(9)?;
            let set_str: String = r.get(10)?;
            let cv: u32 = r.get(11)?;
            let st_str: String = r.get(12)?;
            let at_str: Option<String> = r.get(13)?;
            let er_str: Option<String> = r.get(14)?;
            let rej_str: Option<String> = r.get(15)?;
            let cat: f64 = r.get(16)?;
            let uat: f64 = r.get(17)?;

            Ok((
                pid, gid, sk, sr, rik, ifp, obj, rat, if_str, spec_str, set_str, cv, st_str, at_str, er_str,
                rej_str, cat, uat,
            ))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("proposal {:?}", proposal_id.as_str())))?;

    let (
        pid,
        gid,
        sk,
        sr,
        rik,
        ifp,
        obj,
        rat,
        if_str,
        spec_str,
        set_str,
        cv,
        st_str,
        at_str,
        er_str,
        rej_str,
        cat,
        uat,
    ) = existing;

    let info_fn = InformationFunction::parse_sql(&if_str)?;
    let norm_spec = match spec_str {
        Some(s) => {
            let spec_val = json_load(&s)?;
            Some(task_spec_from_json(&spec_val)?)
        }
        None => None,
    };
    let set_val = json_load(&set_str)?;
    let input_set = semantic_input_set_from_json(&set_val)?;
    let state = ProposalStateKind::parse_sql(&st_str)?;
    let exp_reason = er_str
        .as_deref()
        .map(ProposalExpirationReason::parse_sql)
        .transpose()?;

    Ok(ProposalRecord {
        proposal_id: ProposalId::from_string(pid),
        generation_id: GenerationId::from_string(gid),
        source_kind: sk,
        source_ref: sr,
        raw_intent_key: rik,
        intent_fingerprint: ifp,
        objective: obj,
        rationale: rat,
        information_function: info_fn,
        normalized_task_spec: norm_spec,
        semantic_input_set: input_set,
        compiler_version: cv,
        state,
        admitted_task_id: at_str.map(TaskId::from_string),
        expiration_reason: exp_reason,
        rejection_reason: rej_str,
        created_at: cat,
        updated_at: uat,
    })
}

/// Atomically admit a CompiledWorkProposal into a Generation.
///
/// Creates M5 Task + GenerationTaskBinding and transitions proposal to ADMITTED.
/// This legacy path carries no typed agent requirement.
pub fn admit_proposal(
    tx: &Transaction<'_>,
    now: UnixTime,
    proposal_id: &ProposalId,
    expected_generation_revision: u64,
    override_task_spec: Option<TaskSpec>,
) -> Result<TaskId, Error> {
    admit_proposal_core(
        tx,
        now,
        proposal_id,
        expected_generation_revision,
        override_task_spec,
        None,
    )
}

/// Atomically admit a CompiledWorkProposal with a typed agent requirement
/// (M6-B.3).
///
/// The requirement is created in the SAME SQLite transaction as the Task and the
/// GenerationTaskBinding, so a failure leaves none of the three. This resolves
/// the exact AgentType pin and folds the Generation policy, but it selects no
/// SpawnSource and performs no external I/O.
pub fn admit_typed_proposal(
    tx: &Transaction<'_>,
    now: UnixTime,
    proposal_id: &ProposalId,
    expected_generation_revision: u64,
    override_task_spec: Option<TaskSpec>,
    agent_requirement: AgentRequirementDraft,
) -> Result<TaskId, Error> {
    admit_proposal_core(
        tx,
        now,
        proposal_id,
        expected_generation_revision,
        override_task_spec,
        Some(agent_requirement),
    )
}

fn admit_proposal_core(
    tx: &Transaction<'_>,
    now: UnixTime,
    proposal_id: &ProposalId,
    expected_generation_revision: u64,
    override_task_spec: Option<TaskSpec>,
    typed_draft: Option<AgentRequirementDraft>,
) -> Result<TaskId, Error> {
    // 1. Fetch proposal row
    let proposal_row = query_opt(
        tx,
        "SELECT generation_id, information_function, normalized_task_spec_json,
                semantic_input_set_json, state, admitted_task_id
         FROM compiled_work_proposals WHERE proposal_id=?1",
        params![proposal_id.as_str()],
        |r| {
            let gid: String = r.get(0)?;
            let if_str: String = r.get(1)?;
            let spec_str: Option<String> = r.get(2)?;
            let set_str: String = r.get(3)?;
            let st_str: String = r.get(4)?;
            let at_str: Option<String> = r.get(5)?;
            Ok((gid, if_str, spec_str, set_str, st_str, at_str))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("proposal {:?}", proposal_id.as_str())))?;

    let (gid, if_str, spec_str_opt, set_str, st_str, admitted_tid) = proposal_row;
    let prop_state = ProposalStateKind::parse_sql(&st_str)?;
    let info_fn = InformationFunction::parse_sql(&if_str)?;

    // Crash-retry idempotency (P1-3): if already ADMITTED, return existing admitted TaskId.
    if prop_state == ProposalStateKind::Admitted {
        if let Some(existing_tid) = admitted_tid {
            if let Some(override_spec) = override_task_spec {
                let admitted_spec_json: Option<String> = tx
                    .query_row(
                        "SELECT admitted_task_spec_json FROM generation_task_bindings WHERE proposal_id=?1",
                        params![proposal_id.as_str()],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(map_sqlite)?;
                if let Some(spec_str) = admitted_spec_json {
                    let admitted_spec = task_spec_from_json(&json_load(&spec_str)?)?;
                    if !admitted_spec.equivalent(&override_spec)? {
                        return Err(Error::conflict(
                            "override_task_spec conflicts with previously admitted task_spec",
                        ));
                    }
                }
            }
            if let Some(draft) = typed_draft.clone() {
                // A typed replay must describe the same committed requirement;
                // otherwise it is a conflicting command, not a replay.
                let admitted_spec_str: String = tx
                    .query_row(
                        "SELECT admitted_task_spec_json FROM generation_task_bindings WHERE proposal_id=?1",
                        params![proposal_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(map_sqlite)?;
                let admitted_spec = task_spec_from_json(&json_load(&admitted_spec_str)?)?;
                let generation_id = GenerationId::from_string(gid.clone());
                let expected = crate::requirement::build_task_agent_requirement(
                    tx,
                    &generation_id,
                    draft,
                    info_fn,
                    &admitted_spec,
                )?;
                let existing_task = TaskId::from_string(existing_tid.clone());
                let committed = crate::requirement::get_task_agent_requirement(tx, &existing_task)?
                    .ok_or_else(|| {
                        Error::conflict(
                            "typed replay of an admission that has no agent requirement",
                        )
                    })?;
                if task_agent_requirement_content_digest(&committed)
                    != task_agent_requirement_content_digest(&expected)
                {
                    return Err(Error::conflict(
                        "agent requirement conflicts with previously admitted requirement",
                    ));
                }
            }
            return Ok(TaskId::from_string(existing_tid));
        }
    }
    if prop_state != ProposalStateKind::Pending {
        return Err(Error::invalid_transition(format!(
            "cannot admit proposal {:?} in state {st_str}",
            proposal_id.as_str()
        )));
    }

    // 2. Fetch Generation row
    let gen_row = query_opt(
        tx,
        "SELECT state, revision, admission_seq FROM generations WHERE generation_id=?1",
        params![gid],
        |r| {
            let s: String = r.get(0)?;
            let rev: u64 = r.get(1)?;
            let seq: u64 = r.get(2)?;
            Ok((s, rev, seq))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("generation {gid}")))?;

    let (gen_state_str, gen_rev, gen_seq) = gen_row;
    let gen_state = GenerationState::parse_sql(&gen_state_str)?;

    if gen_rev != expected_generation_revision {
        return Err(Error::conflict(format!(
            "generation revision mismatch: expected {expected_generation_revision}, current {gen_rev}"
        )));
    }

    if !generation_allows_admit(gen_state, info_fn) {
        return Err(Error::invalid_transition(format!(
            "generation in state {gen_state_str} does not allow admitting {if_str} tasks"
        )));
    }

    // 3. Validate semantic input set provenance
    let gen_id_obj = GenerationId::from_string(gid.clone());
    let semantic_input_set = semantic_input_set_from_json(&json_load(&set_str)?)?;
    validate_semantic_input_set(tx, &gen_id_obj, &semantic_input_set)?;

    // 4. Resolve TaskSpec
    let norm_spec_opt: Option<TaskSpec> = match spec_str_opt {
        Some(s) => {
            let spec_val = json_load(&s)?;
            Some(task_spec_from_json(&spec_val)?)
        }
        None => None,
    };

    let task_spec = match (norm_spec_opt, override_task_spec) {
        (None, None) => {
            return Err(Error::invalid_authority(
                "proposal has no suggested task spec; override_task_spec must be provided at admission",
            ));
        }
        (None, Some(override_spec)) => override_spec,
        (Some(norm_spec), None) => norm_spec,
        (Some(norm_spec), Some(override_spec)) => {
            if !norm_spec.equivalent(&override_spec)? {
                return Err(Error::conflict(
                    "override_task_spec conflicts with compiled proposal normalized_task_spec",
                ));
            }
            norm_spec
        }
    };

    // TaskSpec dependencies must be empty in M6-A
    if !task_spec.dependencies.is_empty() {
        return Err(Error::invalid_authority(
            "TaskSpec dependencies must be empty in M6-A",
        ));
    }

    // Verify target partition exists and is active
    required_partition(tx, task_spec.partition.as_str(), true)?;

    // 5. Ensure independent mechanical execution batch for this Task exists (P1-2)
    let task_id = task_spec.task_id.clone().unwrap_or_else(TaskId::new);
    let batch_id = BatchId::new();
    tx.execute(
        "INSERT INTO batches(id, state, metadata_json, created_at, updated_at)
         VALUES(?1, 'ACTIVE', '{}', ?2, ?2)",
        params![batch_id.as_str(), now],
    )
    .map_err(map_sqlite)?;

    // 6. Create M5 Task
    let retry_json = Value::Array(
        task_spec
            .retry_policy
            .retry_classes
            .iter()
            .map(|c| Value::String(c.as_sql().to_string()))
            .collect(),
    );

    tx.execute(
        "INSERT INTO tasks(
            id, batch_id, name, payload_json, acceptance_json, partition_name,
            workstream_id, continuity, affinity_tags_json, workspace_mode, required, priority, state,
            max_attempts, retry_classes_json, base_backoff_seconds, max_backoff_seconds,
            supersedes_task_id, created_at, updated_at
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, 'QUEUED', ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
        params![
            task_id.as_str(),
            batch_id.as_str(),
            task_spec.name,
            json_dump(&task_spec.payload),
            json_dump(&task_spec.acceptance),
            task_spec.partition.as_str(),
            task_spec.workstream_id.as_ref().map(|w| w.as_str().to_string()),
            task_spec.continuity.as_sql(),
            json_dump(&Value::Array(
                task_spec.affinity_tags.iter().cloned().map(Value::String).collect()
            )),
            task_spec.workspace_mode.as_sql(),
            task_spec.priority,
            task_spec.retry_policy.max_attempts as i64,
            json_dump(&retry_json),
            task_spec.retry_policy.base_backoff_seconds,
            task_spec.retry_policy.max_backoff_seconds,
            task_spec.supersedes_task_id.as_ref().map(|s| s.as_str().to_string()),
            now
        ],
    )
    .map_err(map_sqlite)?;

    // 7. Create GenerationTaskBinding
    let new_seq = gen_seq + 1;
    let admitted_spec_json = json_dump(&task_spec_to_json(&task_spec)?);
    tx.execute(
        "INSERT INTO generation_task_bindings(
            generation_id, task_id, proposal_id, information_function,
            admission_seq, admitted_task_spec_json, semantic_input_set_json, created_at
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            gid,
            task_id.as_str(),
            proposal_id.as_str(),
            info_fn.as_sql(),
            new_seq,
            admitted_spec_json,
            set_str,
            now
        ],
    )
    .map_err(map_sqlite)?;

    // 8. Create the typed agent requirement, in the same transaction.
    if let Some(draft) = typed_draft {
        let generation_id = GenerationId::from_string(gid.clone());
        let requirement = crate::requirement::build_task_agent_requirement(
            tx,
            &generation_id,
            draft,
            info_fn,
            &task_spec,
        )?;
        crate::requirement::insert_task_agent_requirement(tx, now, &task_id, &requirement)?;
    }

    // 9. Transition proposal to ADMITTED
    let updated = tx
        .execute(
            "UPDATE compiled_work_proposals
         SET state='ADMITTED', admitted_task_id=?1, updated_at=?2
         WHERE proposal_id=?3 AND state='PENDING'",
            params![task_id.as_str(), now, proposal_id.as_str()],
        )
        .map_err(map_sqlite)?;

    if updated == 0 {
        // Concurrent race: check if proposal was already admitted by another transaction
        let winner_row: Option<Option<String>> = tx
            .query_row(
                "SELECT admitted_task_id FROM compiled_work_proposals WHERE proposal_id=?1 AND state='ADMITTED'",
                params![proposal_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;

        if let Some(Some(winner_tid)) = winner_row {
            return Ok(TaskId::from_string(winner_tid));
        }

        return Err(Error::conflict(format!(
            "proposal {:?} was admitted or state modified concurrently",
            proposal_id.as_str()
        )));
    }

    // 10. Increment generation admission_seq
    tx.execute(
        "UPDATE generations SET admission_seq=?1 WHERE generation_id=?2",
        params![new_seq, gid],
    )
    .map_err(map_sqlite)?;

    Ok(task_id)
}

/// Freeze a Generation: transitions OPEN -> FROZEN, expires all PENDING EXPAND proposals.
///
/// INV-A7 race eliminator.
pub fn freeze_generation(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &GenerationId,
    expected_revision: u64,
) -> Result<(), Error> {
    let gen_row = query_opt(
        tx,
        "SELECT state, revision FROM generations WHERE generation_id=?1",
        params![generation_id.as_str()],
        |r| {
            let s: String = r.get(0)?;
            let rev: u64 = r.get(1)?;
            Ok((s, rev))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("generation {:?}", generation_id.as_str())))?;

    let (st_str, rev) = gen_row;
    let state = GenerationState::parse_sql(&st_str)?;

    if state != GenerationState::Open {
        return Err(Error::invalid_transition(format!(
            "cannot freeze generation {:?} in state {st_str}; must be OPEN",
            generation_id.as_str()
        )));
    }

    if rev != expected_revision {
        return Err(Error::conflict(format!(
            "generation revision mismatch: expected {expected_revision}, current {rev}"
        )));
    }

    let new_rev = rev + 1;
    let rows = tx
        .execute(
            "UPDATE generations
         SET state='FROZEN', revision=?1, frozen_at=?2
         WHERE generation_id=?3 AND state='OPEN' AND revision=?4",
            params![new_rev, now, generation_id.as_str(), rev],
        )
        .map_err(map_sqlite)?;

    if rows == 0 {
        return Err(Error::conflict(format!(
            "concurrent modification freezing generation {:?}",
            generation_id.as_str()
        )));
    }

    // Atomically expire all PENDING EXPAND proposals in this generation
    tx.execute(
        "UPDATE compiled_work_proposals
         SET state='EXPIRED', expiration_reason=?1, updated_at=?2
         WHERE generation_id=?3 AND state='PENDING' AND information_function='EXPAND'",
        params![GENERATION_FROZEN, now, generation_id.as_str()],
    )
    .map_err(map_sqlite)?;

    // Atomic wakeup outbox record
    let event_id = OutboxEventId::new();
    let mut payload = serde_json::Map::new();
    payload.insert(
        "generation_id".into(),
        Value::String(generation_id.as_str().to_string()),
    );
    payload.insert("revision".into(), Value::Number(new_rev.into()));

    tx.execute(
        "INSERT INTO notification_outbox(
            id, event_type, aggregate_type, aggregate_id, payload_json,
            state, delivery_attempts, next_delivery_at, created_at
         )
         VALUES(?1, ?2, 'generation', ?3, ?4, 'PENDING', 0, ?5, ?5)",
        params![
            event_id.as_str(),
            GENERATION_FROZEN,
            generation_id.as_str(),
            json_dump(&Value::Object(payload)),
            now
        ],
    )
    .map_err(map_sqlite)?;

    Ok(())
}

/// Close a Generation: requires FROZEN and GenerationSettled(G).
///
/// INV-A9, INV-A10 barrier.
pub fn close_generation(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &GenerationId,
    expected_revision: u64,
) -> Result<(), Error> {
    let gen_row = query_opt(
        tx,
        "SELECT state, revision FROM generations WHERE generation_id=?1",
        params![generation_id.as_str()],
        |r| {
            let s: String = r.get(0)?;
            let rev: u64 = r.get(1)?;
            Ok((s, rev))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("generation {:?}", generation_id.as_str())))?;

    let (st_str, rev) = gen_row;
    let state = GenerationState::parse_sql(&st_str)?;

    if state != GenerationState::Frozen {
        return Err(Error::invalid_transition(format!(
            "cannot close generation {:?} in state {st_str}; must be FROZEN",
            generation_id.as_str()
        )));
    }

    if rev != expected_revision {
        return Err(Error::conflict(format!(
            "generation revision mismatch: expected {expected_revision}, current {rev}"
        )));
    }

    // Check GenerationSettled(G): all admitted tasks must be terminal (COMPLETED or CANCELLED)
    let mut stmt = tx
        .prepare(
            "SELECT t.id, t.state
         FROM generation_task_bindings b
         JOIN tasks t ON b.task_id = t.id
         WHERE b.generation_id = ?1",
        )
        .map_err(map_sqlite)?;

    let rows = stmt
        .query_map(params![generation_id.as_str()], |r| {
            let tid: String = r.get(0)?;
            let tstate: String = r.get(1)?;
            Ok((tid, tstate))
        })
        .map_err(map_sqlite)?;

    let mut snapshots = Vec::new();
    for res in rows {
        let (tid, tstate) = res.map_err(map_sqlite)?;
        let is_term = tstate == "COMPLETED" || tstate == "CANCELLED";
        snapshots.push(TaskSettledSnapshot {
            task_id: TaskId::from_string(tid),
            is_terminal: is_term,
        });
    }

    if !is_generation_settled(GenerationState::Frozen, &snapshots) {
        return Err(Error::invalid_transition(format!(
            "cannot close generation {:?}: generation is not settled (tasks still non-terminal)",
            generation_id.as_str()
        )));
    }

    let new_rev = rev + 1;
    let updated = tx
        .execute(
            "UPDATE generations
         SET state='CLOSED', revision=?1, closed_at=?2
         WHERE generation_id=?3 AND state='FROZEN' AND revision=?4",
            params![new_rev, now, generation_id.as_str(), rev],
        )
        .map_err(map_sqlite)?;

    if updated == 0 {
        return Err(Error::conflict(format!(
            "concurrent modification closing generation {:?}",
            generation_id.as_str()
        )));
    }

    // Expire all remaining PENDING proposals
    tx.execute(
        "UPDATE compiled_work_proposals
         SET state='EXPIRED', expiration_reason=?1, updated_at=?2
         WHERE generation_id=?3 AND state='PENDING'",
        params![GENERATION_CLOSED, now, generation_id.as_str()],
    )
    .map_err(map_sqlite)?;

    // Atomic wakeup outbox record
    let event_id = OutboxEventId::new();
    let mut payload = serde_json::Map::new();
    payload.insert(
        "generation_id".into(),
        Value::String(generation_id.as_str().to_string()),
    );
    payload.insert("revision".into(), Value::Number(new_rev.into()));

    tx.execute(
        "INSERT INTO notification_outbox(
            id, event_type, aggregate_type, aggregate_id, payload_json,
            state, delivery_attempts, next_delivery_at, created_at
         )
         VALUES(?1, ?2, 'generation', ?3, ?4, 'PENDING', 0, ?5, ?5)",
        params![
            event_id.as_str(),
            GENERATION_CLOSED,
            generation_id.as_str(),
            json_dump(&Value::Object(payload)),
            now
        ],
    )
    .map_err(map_sqlite)?;

    Ok(())
}

/// Reject a PENDING proposal with an explanatory reason.
pub fn reject_proposal(
    tx: &Transaction<'_>,
    now: UnixTime,
    proposal_id: &ProposalId,
    reason: &str,
) -> Result<(), Error> {
    let updated = tx
        .execute(
            "UPDATE compiled_work_proposals
         SET state='REJECTED', rejection_reason=?1, updated_at=?2
         WHERE proposal_id=?3 AND state='PENDING'",
            params![reason, now, proposal_id.as_str()],
        )
        .map_err(map_sqlite)?;

    if updated == 0 {
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM compiled_work_proposals WHERE proposal_id=?1",
                params![proposal_id.as_str()],
                |_| Ok(true),
            )
            .optional()
            .map_err(map_sqlite)?
            .unwrap_or(false);

        if !exists {
            return Err(Error::not_found(format!(
                "proposal {:?}",
                proposal_id.as_str()
            )));
        } else {
            return Err(Error::invalid_transition(format!(
                "cannot reject proposal {:?} that is not in PENDING state",
                proposal_id.as_str()
            )));
        }
    }

    Ok(())
}

/// Read a complete derived GenerationView.
pub fn get_generation_view(
    tx: &Transaction<'_>,
    generation_id: &GenerationId,
) -> Result<GenerationView, Error> {
    let gen_row = query_opt(
        tx,
        "SELECT state, revision, admission_seq, seed_payload_json, created_at, frozen_at, closed_at
         FROM generations WHERE generation_id=?1",
        params![generation_id.as_str()],
        |r| {
            let s: String = r.get(0)?;
            let rev: u64 = r.get(1)?;
            let seq: u64 = r.get(2)?;
            let seed: String = r.get(3)?;
            let cat: f64 = r.get(4)?;
            let fat: Option<f64> = r.get(5)?;
            let clat: Option<f64> = r.get(6)?;
            Ok((s, rev, seq, seed, cat, fat, clat))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("generation {:?}", generation_id.as_str())))?;

    let (st_str, rev, seq, seed_json, cat, fat, clat) = gen_row;
    let state = GenerationState::parse_sql(&st_str)?;
    let seed_payload = json_load(&seed_json)?;

    let record = GenerationRecord {
        generation_id: generation_id.clone(),
        state,
        revision: rev,
        admission_seq: seq,
        seed_payload,
        created_at: cat,
        frozen_at: fat,
        closed_at: clat,
    };

    // Task counts & snapshots
    let mut stmt = tx
        .prepare(
            "SELECT b.information_function, t.id, t.state
         FROM generation_task_bindings b
         JOIN tasks t ON b.task_id = t.id
         WHERE b.generation_id = ?1
         ORDER BY b.admission_seq ASC",
        )
        .map_err(map_sqlite)?;

    let rows = stmt
        .query_map(params![generation_id.as_str()], |r| {
            let if_str: String = r.get(0)?;
            let tid: String = r.get(1)?;
            let tstate: String = r.get(2)?;
            Ok((if_str, tid, tstate))
        })
        .map_err(map_sqlite)?;

    let mut task_count_by_info_fn: HashMap<InformationFunction, usize> = HashMap::new();
    let mut task_count_by_state: HashMap<TaskState, usize> = HashMap::new();
    let mut admitted_task_ids = Vec::new();
    let mut task_snapshots = Vec::new();

    for res in rows {
        let (if_str, tid, tstate) = res.map_err(map_sqlite)?;
        let if_fn = InformationFunction::parse_sql(&if_str)?;
        let ts = TaskState::parse_sql(&tstate)?;
        *task_count_by_info_fn.entry(if_fn).or_insert(0) += 1;
        *task_count_by_state.entry(ts).or_insert(0) += 1;
        admitted_task_ids.push(TaskId::from_string(tid.clone()));
        task_snapshots.push(TaskSettledSnapshot {
            task_id: TaskId::from_string(tid),
            is_terminal: ts == TaskState::Completed || ts == TaskState::Cancelled,
        });
    }

    let is_settled = is_generation_view_settled(state, &task_snapshots);

    // Proposals
    let mut prop_stmt = tx
        .prepare(
            "SELECT proposal_id, state
         FROM compiled_work_proposals
         WHERE generation_id = ?1
         ORDER BY created_at ASC, proposal_id ASC",
        )
        .map_err(map_sqlite)?;

    let prop_rows = prop_stmt
        .query_map(params![generation_id.as_str()], |r| {
            let pid: String = r.get(0)?;
            let pst: String = r.get(1)?;
            Ok((pid, pst))
        })
        .map_err(map_sqlite)?;

    let mut pending_proposal_ids = Vec::new();
    let mut expired_proposal_ids = Vec::new();
    let mut rejected_proposal_ids = Vec::new();

    for res in prop_rows {
        let (pid, pst) = res.map_err(map_sqlite)?;
        let pid_obj = ProposalId::from_string(pid);
        match pst.as_str() {
            "PENDING" => pending_proposal_ids.push(pid_obj),
            "EXPIRED" => expired_proposal_ids.push(pid_obj),
            "REJECTED" => rejected_proposal_ids.push(pid_obj),
            _ => {}
        }
    }

    Ok(GenerationView {
        generation: record,
        is_settled,
        task_count_by_info_fn,
        task_count_by_state,
        pending_proposal_ids,
        admitted_task_ids,
        expired_proposal_ids,
        rejected_proposal_ids,
    })
}
