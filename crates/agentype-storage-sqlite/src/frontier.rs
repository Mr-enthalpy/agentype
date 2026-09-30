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
use agentype_core::{
    generation_allows_admit, is_generation_settled, ContinuityPreference, Error, FailureClass,
    GenerationId, GenerationRecord, GenerationState, GenerationView, InformationFunction,
    OutboxEventId, PartitionId, ProposalExpirationReason, ProposalId, ProposalRecord,
    ProposalStateKind, RawWorkIntent, ResultId, RetryPolicy, SemanticInputSet, TaskId,
    TaskSettledSnapshot, TaskSpec, TaskState, UnixTime, WorkstreamId,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::HashMap;

pub fn semantic_input_set_to_json(set: &SemanticInputSet) -> Value {
    let results: Vec<Value> = set
        .result_ids
        .iter()
        .map(|r| Value::String(r.as_str().to_string()))
        .collect();
    let artifacts: Vec<Value> = set
        .artifact_refs
        .iter()
        .map(|a| Value::String(a.clone()))
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
    let mut set = SemanticInputSet::new();
    if let Some(arr) = val.get("result_ids").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = v.as_str() {
                set.result_ids.push(ResultId::from_string(s));
            }
        }
    }
    if let Some(arr) = val.get("artifact_refs").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = v.as_str() {
                set.artifact_refs.push(s.to_string());
            }
        }
    }
    if let Some(arr) = val.get("seed_refs").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = v.as_str() {
                set.seed_refs.push(s.to_string());
            }
        }
    }
    Ok(set)
}

pub fn task_spec_to_json(spec: &TaskSpec) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("name".into(), Value::String(spec.name.clone()));
    map.insert("payload".into(), spec.payload.clone());
    map.insert("acceptance".into(), spec.acceptance.clone());
    map.insert(
        "partition".into(),
        Value::String(spec.partition.as_str().to_string()),
    );
    map.insert(
        "workstream_id".into(),
        spec.workstream_id
            .as_ref()
            .map(|w| Value::String(w.as_str().to_string()))
            .unwrap_or(Value::Null),
    );
    map.insert(
        "continuity".into(),
        Value::String(spec.continuity.as_sql().to_string()),
    );
    map.insert(
        "affinity_tags".into(),
        Value::Array(
            spec.affinity_tags
                .iter()
                .map(|t| Value::String(t.clone()))
                .collect(),
        ),
    );
    map.insert(
        "workspace_mode".into(),
        Value::String(spec.workspace_mode.as_sql().to_string()),
    );
    map.insert(
        "dependencies".into(),
        Value::Array(
            spec.dependencies
                .iter()
                .map(|d| Value::String(d.clone()))
                .collect(),
        ),
    );
    map.insert("priority".into(), Value::Number(spec.priority.into()));
    map.insert(
        "max_attempts".into(),
        Value::Number(spec.retry_policy.max_attempts.into()),
    );
    map.insert(
        "retry_classes".into(),
        Value::Array(
            spec.retry_policy
                .retry_classes
                .iter()
                .map(|c| Value::String(c.as_sql().to_string()))
                .collect(),
        ),
    );
    map.insert(
        "base_backoff_seconds".into(),
        serde_json::Number::from_f64(spec.retry_policy.base_backoff_seconds)
            .map(Value::Number)
            .unwrap_or_else(|| Value::Number(1.into())),
    );
    map.insert(
        "max_backoff_seconds".into(),
        serde_json::Number::from_f64(spec.retry_policy.max_backoff_seconds)
            .map(Value::Number)
            .unwrap_or_else(|| Value::Number(60.into())),
    );
    map.insert(
        "supersedes_task_id".into(),
        spec.supersedes_task_id
            .as_ref()
            .map(|s| Value::String(s.as_str().to_string()))
            .unwrap_or(Value::Null),
    );
    map.insert(
        "task_id".into(),
        spec.task_id
            .as_ref()
            .map(|t| Value::String(t.as_str().to_string()))
            .unwrap_or(Value::Null),
    );
    Value::Object(map)
}

pub fn task_spec_from_json(val: &Value) -> Result<TaskSpec, Error> {
    let name = val
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("task")
        .to_string();
    let payload = val
        .get("payload")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let acceptance = val
        .get("acceptance")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let partition = PartitionId::new(
        val.get("partition")
            .and_then(Value::as_str)
            .unwrap_or("default"),
    );
    let workstream_id = val
        .get("workstream_id")
        .and_then(Value::as_str)
        .map(WorkstreamId::from_string);
    let continuity = val
        .get("continuity")
        .and_then(Value::as_str)
        .and_then(|s| ContinuityPreference::parse_sql(s).ok())
        .unwrap_or(ContinuityPreference::None);
    let affinity_tags = val
        .get("affinity_tags")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let workspace_mode = val
        .get("workspace_mode")
        .and_then(Value::as_str)
        .and_then(|s| agentype_core::WorkspaceMode::parse_sql(s).ok())
        .unwrap_or(agentype_core::WorkspaceMode::Write);
    let dependencies = val
        .get("dependencies")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let priority = val.get("priority").and_then(Value::as_i64).unwrap_or(0);

    let max_attempts = val
        .get("max_attempts")
        .and_then(Value::as_u64)
        .map(|u| u as u32)
        .unwrap_or(1);
    let retry_classes = val
        .get("retry_classes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter_map(|s| FailureClass::parse_sql(s).ok())
                .collect()
        })
        .unwrap_or_default();
    let base_backoff_seconds = val
        .get("base_backoff_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let max_backoff_seconds = val
        .get("max_backoff_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(60.0);

    let supersedes_task_id = val
        .get("supersedes_task_id")
        .and_then(Value::as_str)
        .map(TaskId::from_string);
    let task_id = val
        .get("task_id")
        .and_then(Value::as_str)
        .map(TaskId::from_string);

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
    let generation_id = GenerationId::new();
    let seed_json = json_dump(&seed_payload);

    tx.execute(
        "INSERT INTO generations(generation_id, state, revision, admission_seq, seed_payload_json, created_at)
         VALUES(?1, 'OPEN', 0, 0, ?2, ?3)",
        params![generation_id.as_str(), seed_json, now],
    )
    .map_err(map_sqlite)?;

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

/// Compile a raw intent into a durable CompiledWorkProposal.
///
/// Idempotent on `(source_ref, raw_intent_key, compiler_version)`.
pub fn compile_intent(
    tx: &Transaction<'_>,
    now: UnixTime,
    generation_id: &GenerationId,
    intent: RawWorkIntent,
    source_kind: &str,
    source_ref: &str,
    compiler_version: u32,
) -> Result<ProposalRecord, Error> {
    // 1. Verify generation exists and is not CLOSED.
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

    // 2. Check for existing proposal with same (source_ref, raw_intent_key, compiler_version)
    if let Some(existing) = query_opt(
        tx,
        "SELECT proposal_id, generation_id, source_kind, source_ref, raw_intent_key,
                information_function, normalized_task_spec_json, semantic_input_set_json,
                compiler_version, state, admitted_task_id, expiration_reason, created_at, updated_at
         FROM compiled_work_proposals
         WHERE source_ref=?1 AND raw_intent_key=?2 AND compiler_version=?3",
        params![source_ref, intent.raw_intent_key, compiler_version],
        |r| {
            let pid: String = r.get(0)?;
            let gid: String = r.get(1)?;
            let sk: String = r.get(2)?;
            let sr: String = r.get(3)?;
            let rik: String = r.get(4)?;
            let if_str: String = r.get(5)?;
            let spec_str: String = r.get(6)?;
            let set_str: String = r.get(7)?;
            let cv: u32 = r.get(8)?;
            let st_str: String = r.get(9)?;
            let at_str: Option<String> = r.get(10)?;
            let er_str: Option<String> = r.get(11)?;
            let cat: f64 = r.get(12)?;
            let uat: f64 = r.get(13)?;

            Ok((
                pid, gid, sk, sr, rik, if_str, spec_str, set_str, cv, st_str, at_str, er_str, cat,
                uat,
            ))
        },
    )? {
        let (
            pid,
            gid,
            sk,
            sr,
            rik,
            if_str,
            spec_str,
            set_str,
            cv,
            st_str,
            at_str,
            er_str,
            cat,
            uat,
        ) = existing;
        let info_fn = InformationFunction::parse_sql(&if_str)?;
        let spec_val = json_load(&spec_str)?;
        let norm_spec = task_spec_from_json(&spec_val)?;
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
            information_function: info_fn,
            normalized_task_spec: norm_spec,
            semantic_input_set: input_set,
            compiler_version: cv,
            state,
            admitted_task_id: at_str.map(TaskId::from_string),
            expiration_reason: exp_reason,
            created_at: cat,
            updated_at: uat,
        });
    }

    // 3. Normalize TaskSpec
    let norm_spec = match intent.suggested_task_spec {
        Some(s) => s,
        None => {
            let mut payload = serde_json::Map::new();
            payload.insert("objective".into(), Value::String(intent.objective.clone()));
            if let Some(r) = &intent.rationale {
                payload.insert("rationale".into(), Value::String(r.clone()));
            }
            TaskSpec {
                name: intent.raw_intent_key.clone(),
                payload: Value::Object(payload),
                acceptance: Value::Object(Default::default()),
                partition: PartitionId::new("default"),
                workstream_id: None,
                continuity: ContinuityPreference::None,
                affinity_tags: Vec::new(),
                workspace_mode: agentype_core::WorkspaceMode::Write,
                dependencies: Vec::new(),
                priority: 0,
                retry_policy: RetryPolicy::default(),
                supersedes_task_id: None,
                task_id: None,
            }
        }
    };

    let proposal_id = ProposalId::new();
    let spec_json = json_dump(&task_spec_to_json(&norm_spec));
    let set_json = json_dump(&semantic_input_set_to_json(&intent.semantic_input_set));

    tx.execute(
        "INSERT INTO compiled_work_proposals(
            proposal_id, generation_id, source_kind, source_ref, raw_intent_key,
            information_function, normalized_task_spec_json, semantic_input_set_json,
            compiler_version, state, created_at, updated_at
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'PENDING', ?10, ?10)",
        params![
            proposal_id.as_str(),
            generation_id.as_str(),
            source_kind,
            source_ref,
            intent.raw_intent_key,
            intent.information_function.as_sql(),
            spec_json,
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
        information_function: intent.information_function,
        normalized_task_spec: norm_spec,
        semantic_input_set: intent.semantic_input_set,
        compiler_version,
        state: ProposalStateKind::Pending,
        admitted_task_id: None,
        expiration_reason: None,
        created_at: now,
        updated_at: now,
    })
}

/// Atomically admit a CompiledWorkProposal into a Generation.
///
/// Creates M5 Task + GenerationTaskBinding and transitions proposal to ADMITTED.
pub fn admit_proposal(
    tx: &Transaction<'_>,
    now: UnixTime,
    proposal_id: &ProposalId,
    expected_generation_revision: u64,
    override_task_spec: Option<TaskSpec>,
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
            let spec_str: String = r.get(2)?;
            let set_str: String = r.get(3)?;
            let st_str: String = r.get(4)?;
            let at_str: Option<String> = r.get(5)?;
            Ok((gid, if_str, spec_str, set_str, st_str, at_str))
        },
    )?
    .ok_or_else(|| Error::not_found(format!("proposal {:?}", proposal_id.as_str())))?;

    let (gid, if_str, spec_str, set_str, st_str, admitted_tid) = proposal_row;
    let prop_state = ProposalStateKind::parse_sql(&st_str)?;
    let info_fn = InformationFunction::parse_sql(&if_str)?;

    if prop_state == ProposalStateKind::Admitted {
        if let Some(existing_tid) = admitted_tid {
            return Err(Error::conflict(format!(
                "proposal {:?} already admitted as task {existing_tid}",
                proposal_id.as_str()
            )));
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

    // 3. Resolve TaskSpec
    let task_spec = match override_task_spec {
        Some(s) => s,
        None => {
            let spec_val = json_load(&spec_str)?;
            task_spec_from_json(&spec_val)?
        }
    };

    // Verify target partition exists and is active
    required_partition(tx, task_spec.partition.as_str(), true)?;

    // 4. Ensure internal batch for this Generation exists
    let batch_id = format!("batch_gen_{gid}");
    tx.execute(
        "INSERT INTO batches(id, state, metadata_json, created_at, updated_at)
         VALUES(?1, 'ACTIVE', '{}', ?2, ?2)
         ON CONFLICT(id) DO NOTHING",
        params![batch_id, now],
    )
    .map_err(map_sqlite)?;

    // 5. Create M5 Task
    let task_id = task_spec.task_id.unwrap_or_else(TaskId::new);
    let state = if task_spec.dependencies.is_empty() {
        "QUEUED"
    } else {
        "BLOCKED"
    };

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
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)",
        params![
            task_id.as_str(),
            batch_id,
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
            state,
            task_spec.retry_policy.max_attempts as i64,
            json_dump(&retry_json),
            task_spec.retry_policy.base_backoff_seconds,
            task_spec.retry_policy.max_backoff_seconds,
            task_spec.supersedes_task_id.as_ref().map(|s| s.as_str().to_string()),
            now
        ],
    )
    .map_err(map_sqlite)?;

    for dep in &task_spec.dependencies {
        tx.execute(
            "INSERT INTO task_dependencies(task_id, depends_on_task_id) VALUES(?1, ?2)",
            params![task_id.as_str(), dep],
        )
        .map_err(map_sqlite)?;
    }

    // 6. Create GenerationTaskBinding
    let new_seq = gen_seq + 1;
    tx.execute(
        "INSERT INTO generation_task_bindings(
            generation_id, task_id, proposal_id, information_function,
            admission_seq, semantic_input_set_json, created_at
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            gid,
            task_id.as_str(),
            proposal_id.as_str(),
            info_fn.as_sql(),
            new_seq,
            set_str,
            now
        ],
    )
    .map_err(map_sqlite)?;

    // 7. Transition proposal to ADMITTED
    let updated = tx
        .execute(
            "UPDATE compiled_work_proposals
         SET state='ADMITTED', admitted_task_id=?1, updated_at=?2
         WHERE proposal_id=?3 AND state='PENDING'",
            params![task_id.as_str(), now, proposal_id.as_str()],
        )
        .map_err(map_sqlite)?;

    if updated == 0 {
        return Err(Error::conflict(format!(
            "proposal {:?} was admitted concurrently",
            proposal_id.as_str()
        )));
    }

    // 8. Increment generation admission_seq
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
         SET state='EXPIRED', expiration_reason='GENERATION_FROZEN', updated_at=?1
         WHERE generation_id=?2 AND state='PENDING' AND information_function='EXPAND'",
        params![now, generation_id.as_str()],
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
         VALUES(?1, 'GENERATION_FROZEN', 'generation', ?2, ?3, 'PENDING', 0, ?4, ?4)",
        params![
            event_id.as_str(),
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
         SET state='EXPIRED', expiration_reason='GENERATION_CLOSED', updated_at=?1
         WHERE generation_id=?2 AND state='PENDING'",
        params![now, generation_id.as_str()],
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
         VALUES(?1, 'GENERATION_CLOSED', 'generation', ?2, ?3, 'PENDING', 0, ?4, ?4)",
        params![
            event_id.as_str(),
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
         SET state='REJECTED', expiration_reason=?1, updated_at=?2
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
         WHERE b.generation_id = ?1",
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

    let is_settled = is_generation_settled(state, &task_snapshots);

    // Proposals
    let mut prop_stmt = tx
        .prepare("SELECT proposal_id, state FROM compiled_work_proposals WHERE generation_id = ?1")
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
