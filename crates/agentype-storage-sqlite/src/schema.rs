//! Rust-era schema (fresh database). D-DB-MIGRATE is unresolved; this is not
//! an in-place Python V0.1 upgrade.

/// Schema version 2 added `executions.adapter_kind`. Version 3 adds the
/// lossless pending-terminal envelope (`summary`, `incarnation_reusable`).
/// Version 4 adds `executions.adapter_binding_key` (opaque domain identity
/// frozen at Execution creation). Version 5 adds M6-A Semantic Frontier Kernel
/// tables (`generations`, `compiled_work_proposals`, `generation_task_bindings`).
/// Version 6 adds the M6-B.2 Agent Contract catalog (`capability_definitions`,
/// `agent_types`, `spawn_sources`, `source_configs`,
/// `adapter_binding_policies`) with immutable revision content kept separate
/// from mutable disposition overlays. Older files are rejected at open (fail
/// closed); D-DB-MIGRATE is still unresolved, so there is deliberately no
/// v5->v6 in-place upgrade.
pub const SCHEMA_VERSION: i64 = 6;

pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS scheduler_meta (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS batches (
    id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('OPEN','ACTIVE','SUSPENDED','COMPLETED','CANCELLED')),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS workstreams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    project_state_ref TEXT,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS pool_partitions (
    name TEXT PRIMARY KEY,
    desired_capacity INTEGER NOT NULL CHECK (desired_capacity >= 0),
    retention TEXT NOT NULL CHECK (retention IN ('resident','ephemeral')),
    execution_target TEXT NOT NULL,
    execution_profile TEXT NOT NULL,
    tags_json TEXT NOT NULL DEFAULT '[]',
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1)),
    merged_into TEXT REFERENCES pool_partitions(name) ON DELETE SET NULL,
    topology_revision INTEGER NOT NULL DEFAULT 0,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS pool_topology_revisions (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    operation TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES batches(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    acceptance_json TEXT NOT NULL DEFAULT '{}',
    partition_name TEXT NOT NULL REFERENCES pool_partitions(name) ON DELETE RESTRICT,
    workstream_id TEXT REFERENCES workstreams(id) ON DELETE SET NULL,
    continuity TEXT NOT NULL CHECK (continuity IN ('required','preferred','none')),
    affinity_tags_json TEXT NOT NULL DEFAULT '[]',
    workspace_mode TEXT NOT NULL CHECK (workspace_mode IN ('read_only','write')),
    required INTEGER NOT NULL CHECK (required = 1),
    priority INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL CHECK (state IN ('BLOCKED','QUEUED','LEASED','RUNNING','RETRY_WAIT','SUSPENDED','COMPLETED','CANCELLED')),
    max_attempts INTEGER NOT NULL CHECK (max_attempts >= 1),
    retry_classes_json TEXT NOT NULL,
    base_backoff_seconds REAL NOT NULL CHECK (base_backoff_seconds >= 0),
    max_backoff_seconds REAL NOT NULL CHECK (max_backoff_seconds >= base_backoff_seconds),
    next_eligible_at REAL,
    current_attempt_id TEXT,
    fencing_epoch INTEGER NOT NULL DEFAULT 0 CHECK (fencing_epoch >= 0),
    supersedes_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    PRIMARY KEY (task_id, depends_on_task_id),
    CHECK (task_id <> depends_on_task_id)
);

CREATE TABLE IF NOT EXISTS logical_agents (
    id TEXT PRIMARY KEY,
    partition_name TEXT NOT NULL REFERENCES pool_partitions(name) ON DELETE RESTRICT,
    retention TEXT NOT NULL CHECK (retention IN ('resident','ephemeral')),
    state TEXT NOT NULL CHECK (state IN ('INITIALIZING','READY','ASSIGNED','REVIVING','DRAINING','SUSPENDED','RETIRED')),
    workstream_id TEXT REFERENCES workstreams(id) ON DELETE SET NULL,
    tags_json TEXT NOT NULL DEFAULT '[]',
    current_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    pending_partition_name TEXT REFERENCES pool_partitions(name) ON DELETE SET NULL,
    retirement_requested INTEGER NOT NULL DEFAULT 0 CHECK (retirement_requested IN (0,1)),
    continuity_json TEXT NOT NULL DEFAULT '{}',
    continuity_version INTEGER NOT NULL DEFAULT 0,
    current_checkpoint_id TEXT,
    available_since REAL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS one_assigned_agent_per_task
ON logical_agents(current_task_id)
WHERE current_task_id IS NOT NULL AND state = 'ASSIGNED';

CREATE TABLE IF NOT EXISTS incarnations (
    id TEXT PRIMARY KEY,
    logical_agent_id TEXT NOT NULL REFERENCES logical_agents(id) ON DELETE RESTRICT,
    generation INTEGER NOT NULL CHECK (generation >= 1),
    execution_target TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('STARTING','WARM','COLD','LOST','TERMINATED')),
    runtime_handle_json TEXT NOT NULL DEFAULT '{}',
    started_at REAL NOT NULL,
    ended_at REAL,
    UNIQUE (logical_agent_id, generation)
);

CREATE UNIQUE INDEX IF NOT EXISTS one_active_incarnation_per_agent
ON incarnations(logical_agent_id)
WHERE state IN ('STARTING','WARM','COLD');

CREATE TABLE IF NOT EXISTS attempts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    logical_agent_id TEXT NOT NULL REFERENCES logical_agents(id) ON DELETE RESTRICT,
    incarnation_id TEXT REFERENCES incarnations(id) ON DELETE RESTRICT,
    attempt_number INTEGER NOT NULL CHECK (attempt_number >= 1),
    lease_epoch INTEGER NOT NULL CHECK (lease_epoch >= 1),
    state TEXT NOT NULL CHECK (state IN ('ACTIVE','SUCCEEDED','FAILED','EXPIRED','CANCELLED')),
    execution_target TEXT NOT NULL,
    execution_profile TEXT NOT NULL,
    partition_name TEXT NOT NULL,
    created_at REAL NOT NULL,
    ended_at REAL,
    UNIQUE (task_id, attempt_number)
);

CREATE UNIQUE INDEX IF NOT EXISTS one_active_attempt_per_task
ON attempts(task_id) WHERE state = 'ACTIVE';

CREATE UNIQUE INDEX IF NOT EXISTS one_active_attempt_per_agent
ON attempts(logical_agent_id) WHERE state = 'ACTIVE';

CREATE TABLE IF NOT EXISTS leases (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES attempts(id) ON DELETE RESTRICT,
    epoch INTEGER NOT NULL CHECK (epoch >= 1),
    state TEXT NOT NULL CHECK (state IN ('ACTIVE','RELEASED','EXPIRED','REVOKED')),
    expires_at REAL NOT NULL,
    heartbeat_at REAL NOT NULL,
    created_at REAL NOT NULL,
    ended_at REAL,
    UNIQUE (task_id, epoch)
);

CREATE UNIQUE INDEX IF NOT EXISTS one_active_lease_per_task
ON leases(task_id) WHERE state = 'ACTIVE';

CREATE TABLE IF NOT EXISTS executions (
    id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL UNIQUE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL REFERENCES attempts(id) ON DELETE RESTRICT,
    incarnation_id TEXT NOT NULL REFERENCES incarnations(id) ON DELETE RESTRICT,
    execution_target TEXT NOT NULL,
    execution_profile TEXT NOT NULL,
    adapter_kind TEXT NOT NULL,
    adapter_binding_key TEXT NOT NULL,
    attempt_isolation INTEGER NOT NULL DEFAULT 0 CHECK (attempt_isolation IN (0,1)),
    state TEXT NOT NULL CHECK (state IN ('STARTING','RUNNING','SUCCEEDED','FAILED','LOST','UNKNOWN','TERMINATED')),
    runtime_handle_json TEXT NOT NULL DEFAULT '{}',
    outcome_json TEXT,
    summary TEXT,
    incarnation_reusable INTEGER NOT NULL DEFAULT 0 CHECK (incarnation_reusable IN (0,1)),
    failure_class TEXT,
    failure_code TEXT,
    failure_signature TEXT,
    terminal_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (terminal_confirmed IN (0,1)),
    quiescent_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (quiescent_confirmed IN (0,1)),
    started_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    ended_at REAL,
    CHECK (quiescent_confirmed = 0 OR (terminal_confirmed = 1 AND state IN ('SUCCEEDED','FAILED','TERMINATED')))
);

CREATE UNIQUE INDEX IF NOT EXISTS one_execution_per_attempt
ON executions(attempt_id);

CREATE UNIQUE INDEX IF NOT EXISTS one_active_execution_per_incarnation
ON executions(incarnation_id)
WHERE state IN ('STARTING','RUNNING','UNKNOWN');

CREATE TABLE IF NOT EXISTS results (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id) ON DELETE RESTRICT,
    batch_id TEXT NOT NULL REFERENCES batches(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL REFERENCES attempts(id) ON DELETE RESTRICT,
    logical_agent_id TEXT NOT NULL REFERENCES logical_agents(id) ON DELETE RESTRICT,
    execution_id TEXT REFERENCES executions(id) ON DELETE SET NULL,
    payload_json TEXT NOT NULL,
    summary TEXT,
    checkpoint_id TEXT,
    workspace_state_ref TEXT,
    state TEXT NOT NULL CHECK (state IN ('AVAILABLE','ACKED')),
    created_at REAL NOT NULL,
    consumed_at REAL,
    consumer_ref TEXT,
    disposition TEXT
);

CREATE TABLE IF NOT EXISTS failures (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    attempt_id TEXT REFERENCES attempts(id) ON DELETE SET NULL,
    execution_id TEXT REFERENCES executions(id) ON DELETE SET NULL,
    failure_class TEXT NOT NULL,
    failure_code TEXT,
    normalized_signature TEXT,
    detail TEXT,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS escalations (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    batch_id TEXT NOT NULL REFERENCES batches(id) ON DELETE RESTRICT,
    logical_agent_id TEXT REFERENCES logical_agents(id) ON DELETE SET NULL,
    workstream_id TEXT REFERENCES workstreams(id) ON DELETE SET NULL,
    failure_class TEXT NOT NULL,
    normalized_signature TEXT,
    snapshot_json TEXT NOT NULL,
    decision_required TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('OPEN','RESOLVED','CANCELLED')),
    created_at REAL NOT NULL,
    resolved_at REAL
);

CREATE UNIQUE INDEX IF NOT EXISTS one_open_escalation_per_task
ON escalations(task_id) WHERE state = 'OPEN';

CREATE TABLE IF NOT EXISTS checkpoints (
    id TEXT PRIMARY KEY,
    logical_agent_id TEXT NOT NULL REFERENCES logical_agents(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL REFERENCES attempts(id) ON DELETE RESTRICT,
    lease_epoch INTEGER NOT NULL,
    continuity_version INTEGER NOT NULL,
    capsule_json TEXT NOT NULL,
    project_state_ref TEXT,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS notification_outbox (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('PENDING','DELIVERED','ACKED')),
    delivery_attempts INTEGER NOT NULL DEFAULT 0,
    next_delivery_at REAL NOT NULL,
    created_at REAL NOT NULL,
    delivered_at REAL,
    acknowledged_at REAL,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS tasks_queue_idx
ON tasks(state, next_eligible_at, priority DESC, created_at);
CREATE INDEX IF NOT EXISTS leases_expiry_idx ON leases(state, expires_at);
CREATE INDEX IF NOT EXISTS outbox_delivery_idx
ON notification_outbox(state, next_delivery_at, created_at);
CREATE INDEX IF NOT EXISTS executions_attempt_idx ON executions(attempt_id, updated_at);

CREATE TRIGGER IF NOT EXISTS tasks_required_insert_only
BEFORE INSERT ON tasks WHEN NEW.required<>1 BEGIN
    SELECT RAISE(ABORT,'optional Tasks are not supported');
END;

CREATE TRIGGER IF NOT EXISTS tasks_required_update_only
BEFORE UPDATE OF required ON tasks WHEN NEW.required<>1 BEGIN
    SELECT RAISE(ABORT,'optional Tasks are not supported');
END;

CREATE TABLE IF NOT EXISTS generations (
    generation_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('OPEN','FROZEN','CLOSED')),
    revision INTEGER NOT NULL DEFAULT 0,
    admission_seq INTEGER NOT NULL DEFAULT 0,
    seed_payload_json TEXT NOT NULL DEFAULT '{}',
    created_at REAL NOT NULL,
    frozen_at REAL,
    closed_at REAL
);

CREATE TABLE IF NOT EXISTS compiled_work_proposals (
    proposal_id TEXT PRIMARY KEY,
    generation_id TEXT NOT NULL REFERENCES generations(generation_id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    raw_intent_key TEXT NOT NULL,
    intent_fingerprint TEXT NOT NULL,
    objective TEXT NOT NULL,
    rationale TEXT,
    information_function TEXT NOT NULL CHECK (information_function IN ('EXPAND','COMPRESS_POSITIVE','COMPRESS_NEGATIVE')),
    normalized_task_spec_json TEXT,
    semantic_input_set_json TEXT NOT NULL DEFAULT '{}',
    compiler_version INTEGER NOT NULL DEFAULT 1,
    state TEXT NOT NULL CHECK (state IN ('PENDING','ADMITTED','REJECTED','EXPIRED')),
    admitted_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    expiration_reason TEXT CHECK (expiration_reason IS NULL OR expiration_reason IN ('GENERATION_FROZEN','GENERATION_CLOSED','SUPERSEDED','POLICY_CHANGED')),
    rejection_reason TEXT,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    UNIQUE(generation_id, source_kind, source_ref, raw_intent_key, compiler_version)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_proposals_admitted_task_id
ON compiled_work_proposals(admitted_task_id)
WHERE admitted_task_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS generation_task_bindings (
    generation_id TEXT NOT NULL REFERENCES generations(generation_id) ON DELETE CASCADE,
    task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    proposal_id TEXT NOT NULL UNIQUE REFERENCES compiled_work_proposals(proposal_id) ON DELETE RESTRICT,
    information_function TEXT NOT NULL CHECK (information_function IN ('EXPAND','COMPRESS_POSITIVE','COMPRESS_NEGATIVE')),
    admission_seq INTEGER NOT NULL,
    admitted_task_spec_json TEXT NOT NULL,
    semantic_input_set_json TEXT NOT NULL DEFAULT '{}',
    created_at REAL NOT NULL
);

-- =========================================================================
-- M6-B.2 Agent Contract catalog (schema v6)
--
-- Immutable revision content lives in the primary tables; the mutable
-- ACTIVE/DRAINING/DISABLED/PUBLISHED/DEPRECATED dispositions live in separate
-- overlay tables so status never enters a revision content digest.
-- =========================================================================

CREATE TABLE IF NOT EXISTS capability_definitions (
    capability_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    matcher_kind TEXT NOT NULL CHECK (matcher_kind IN ('BOOL','SET','ORDERED','QUANTITY','EXACT')),
    security_class TEXT NOT NULL CHECK (security_class IN ('FUNCTIONAL','AUTHORITY','SANDBOX','CONTINUITY')),
    polarity TEXT NOT NULL CHECK (polarity IN ('ABILITY','RESTRICTION')),
    content_json TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (capability_id, revision)
);

CREATE TABLE IF NOT EXISTS adapter_binding_policies (
    policy_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    adapter_kind TEXT NOT NULL CHECK (length(trim(adapter_kind)) > 0),
    binding_ref TEXT NOT NULL CHECK (length(trim(binding_ref)) > 0),
    content_json TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (policy_id, revision)
);

CREATE TABLE IF NOT EXISTS adapter_binding_policy_dispositions (
    policy_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ACTIVE','DRAINING','DISABLED')),
    updated_at REAL NOT NULL,
    PRIMARY KEY (policy_id, revision),
    FOREIGN KEY (policy_id, revision) REFERENCES adapter_binding_policies(policy_id, revision)
);

CREATE TABLE IF NOT EXISTS agent_types (
    type_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    based_on_type_id TEXT,
    based_on_revision INTEGER,
    content_json TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (type_id, revision),
    CHECK ((based_on_type_id IS NULL) = (based_on_revision IS NULL)),
    FOREIGN KEY (based_on_type_id, based_on_revision) REFERENCES agent_types(type_id, revision)
);

CREATE TABLE IF NOT EXISTS agent_type_dispositions (
    type_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('PUBLISHED','DEPRECATED')),
    deprecated_at REAL,
    updated_at REAL NOT NULL,
    PRIMARY KEY (type_id, revision),
    FOREIGN KEY (type_id, revision) REFERENCES agent_types(type_id, revision)
);

CREATE TABLE IF NOT EXISTS spawn_sources (
    source_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    adapter_policy_id TEXT NOT NULL,
    adapter_policy_revision INTEGER NOT NULL,
    content_json TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (source_id, revision),
    FOREIGN KEY (adapter_policy_id, adapter_policy_revision)
        REFERENCES adapter_binding_policies(policy_id, revision)
);

CREATE TABLE IF NOT EXISTS spawn_source_dispositions (
    source_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ACTIVE','DRAINING','DISABLED')),
    updated_at REAL NOT NULL,
    PRIMARY KEY (source_id, revision),
    FOREIGN KEY (source_id, revision) REFERENCES spawn_sources(source_id, revision)
);

CREATE TABLE IF NOT EXISTS source_configs (
    source_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    config_id TEXT NOT NULL,
    config_revision INTEGER NOT NULL CHECK (config_revision >= 1),
    config_mode TEXT NOT NULL CHECK (config_mode IN ('OPAQUE_JSON','EXTERNAL_REF')),
    config_payload_json TEXT,
    config_locator TEXT,
    config_digest TEXT NOT NULL,
    content_json TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (source_id, source_revision, config_id, config_revision),
    CHECK ((config_mode = 'OPAQUE_JSON') = (config_payload_json IS NOT NULL)),
    CHECK ((config_mode = 'EXTERNAL_REF') = (config_locator IS NOT NULL)),
    FOREIGN KEY (source_id, source_revision) REFERENCES spawn_sources(source_id, revision)
);

CREATE TABLE IF NOT EXISTS source_config_dispositions (
    source_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    config_id TEXT NOT NULL,
    config_revision INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ACTIVE','DRAINING','DISABLED')),
    updated_at REAL NOT NULL,
    PRIMARY KEY (source_id, source_revision, config_id, config_revision),
    FOREIGN KEY (source_id, source_revision, config_id, config_revision)
        REFERENCES source_configs(source_id, source_revision, config_id, config_revision)
);

CREATE INDEX IF NOT EXISTS agent_types_based_on_idx
ON agent_types(based_on_type_id, based_on_revision);
CREATE INDEX IF NOT EXISTS source_configs_source_idx
ON source_configs(source_id, source_revision);
CREATE INDEX IF NOT EXISTS spawn_sources_policy_idx
ON spawn_sources(adapter_policy_id, adapter_policy_revision);

-- =========================================================================
-- Mechanical catalog guards
--
-- Immutable revision content is frozen by SQLite itself, not only by the
-- Kernel API: the parent revision row cannot be updated or deleted. Mutable
-- disposition overlays stay writable but can only advance. Direct mutation
-- that bypasses the Kernel transaction boundary is therefore rejected here.
-- =========================================================================

CREATE TRIGGER IF NOT EXISTS capability_definitions_immutable_update
BEFORE UPDATE ON capability_definitions
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be updated');
END;

CREATE TRIGGER IF NOT EXISTS capability_definitions_immutable_delete
BEFORE DELETE ON capability_definitions
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS adapter_binding_policies_immutable_update
BEFORE UPDATE ON adapter_binding_policies
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be updated');
END;

CREATE TRIGGER IF NOT EXISTS adapter_binding_policies_immutable_delete
BEFORE DELETE ON adapter_binding_policies
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS agent_types_immutable_update
BEFORE UPDATE ON agent_types
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be updated');
END;

CREATE TRIGGER IF NOT EXISTS agent_types_immutable_delete
BEFORE DELETE ON agent_types
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS spawn_sources_immutable_update
BEFORE UPDATE ON spawn_sources
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be updated');
END;

CREATE TRIGGER IF NOT EXISTS spawn_sources_immutable_delete
BEFORE DELETE ON spawn_sources
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS source_configs_immutable_update
BEFORE UPDATE ON source_configs
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be updated');
END;

CREATE TRIGGER IF NOT EXISTS source_configs_immutable_delete
BEFORE DELETE ON source_configs
BEGIN
    SELECT RAISE(ABORT, 'immutable catalog revision content cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS agent_type_dispositions_monotonic
BEFORE UPDATE OF status ON agent_type_dispositions
WHEN OLD.status = 'DEPRECATED' AND NEW.status = 'PUBLISHED'
BEGIN
    SELECT RAISE(ABORT, 'a deprecated AgentType revision cannot be republished');
END;

CREATE TRIGGER IF NOT EXISTS spawn_source_dispositions_monotonic
BEFORE UPDATE OF status ON spawn_source_dispositions
WHEN (CASE NEW.status WHEN 'ACTIVE' THEN 0 WHEN 'DRAINING' THEN 1 WHEN 'DISABLED' THEN 2 END)
   < (CASE OLD.status WHEN 'ACTIVE' THEN 0 WHEN 'DRAINING' THEN 1 WHEN 'DISABLED' THEN 2 END)
BEGIN
    SELECT RAISE(ABORT, 'spawn source disposition must not reverse');
END;

CREATE TRIGGER IF NOT EXISTS source_config_dispositions_monotonic
BEFORE UPDATE OF status ON source_config_dispositions
WHEN (CASE NEW.status WHEN 'ACTIVE' THEN 0 WHEN 'DRAINING' THEN 1 WHEN 'DISABLED' THEN 2 END)
   < (CASE OLD.status WHEN 'ACTIVE' THEN 0 WHEN 'DRAINING' THEN 1 WHEN 'DISABLED' THEN 2 END)
BEGIN
    SELECT RAISE(ABORT, 'source config disposition must not reverse');
END;

CREATE TRIGGER IF NOT EXISTS adapter_binding_policy_dispositions_monotonic
BEFORE UPDATE OF status ON adapter_binding_policy_dispositions
WHEN (CASE NEW.status WHEN 'ACTIVE' THEN 0 WHEN 'DRAINING' THEN 1 WHEN 'DISABLED' THEN 2 END)
   < (CASE OLD.status WHEN 'ACTIVE' THEN 0 WHEN 'DRAINING' THEN 1 WHEN 'DISABLED' THEN 2 END)
BEGIN
    SELECT RAISE(ABORT, 'adapter binding policy disposition must not reverse');
END;

-- An overlay row has an immutable identity and must live as long as its
-- revision: it cannot be deleted, re-inserted (including INSERT OR REPLACE,
-- which resolves the conflict before this BEFORE INSERT trigger would otherwise
-- see a duplicate), or have its primary key rewritten. Together with the
-- monotonic-status triggers this makes "dispositions only advance" a SQLite
-- invariant, not merely a Kernel convention.

CREATE TRIGGER IF NOT EXISTS agent_type_dispositions_no_delete
BEFORE DELETE ON agent_type_dispositions
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS agent_type_dispositions_no_reinsert
BEFORE INSERT ON agent_type_dispositions
WHEN EXISTS(SELECT 1 FROM agent_type_dispositions
            WHERE type_id=NEW.type_id AND revision=NEW.revision)
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay already exists');
END;

CREATE TRIGGER IF NOT EXISTS agent_type_dispositions_identity
BEFORE UPDATE ON agent_type_dispositions
WHEN OLD.type_id IS NOT NEW.type_id OR OLD.revision IS NOT NEW.revision
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS spawn_source_dispositions_no_delete
BEFORE DELETE ON spawn_source_dispositions
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS spawn_source_dispositions_no_reinsert
BEFORE INSERT ON spawn_source_dispositions
WHEN EXISTS(SELECT 1 FROM spawn_source_dispositions
            WHERE source_id=NEW.source_id AND revision=NEW.revision)
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay already exists');
END;

CREATE TRIGGER IF NOT EXISTS spawn_source_dispositions_identity
BEFORE UPDATE ON spawn_source_dispositions
WHEN OLD.source_id IS NOT NEW.source_id OR OLD.revision IS NOT NEW.revision
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS source_config_dispositions_no_delete
BEFORE DELETE ON source_config_dispositions
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS source_config_dispositions_no_reinsert
BEFORE INSERT ON source_config_dispositions
WHEN EXISTS(SELECT 1 FROM source_config_dispositions
            WHERE source_id=NEW.source_id AND source_revision=NEW.source_revision
              AND config_id=NEW.config_id AND config_revision=NEW.config_revision)
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay already exists');
END;

CREATE TRIGGER IF NOT EXISTS source_config_dispositions_identity
BEFORE UPDATE ON source_config_dispositions
WHEN OLD.source_id IS NOT NEW.source_id
  OR OLD.source_revision IS NOT NEW.source_revision
  OR OLD.config_id IS NOT NEW.config_id
  OR OLD.config_revision IS NOT NEW.config_revision
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS adapter_binding_policy_dispositions_no_delete
BEFORE DELETE ON adapter_binding_policy_dispositions
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS adapter_binding_policy_dispositions_no_reinsert
BEFORE INSERT ON adapter_binding_policy_dispositions
WHEN EXISTS(SELECT 1 FROM adapter_binding_policy_dispositions
            WHERE policy_id=NEW.policy_id AND revision=NEW.revision)
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay already exists');
END;

CREATE TRIGGER IF NOT EXISTS adapter_binding_policy_dispositions_identity
BEFORE UPDATE ON adapter_binding_policy_dispositions
WHEN OLD.policy_id IS NOT NEW.policy_id OR OLD.revision IS NOT NEW.revision
BEGIN
    SELECT RAISE(ABORT, 'disposition overlay identity is immutable');
END;
"#;
