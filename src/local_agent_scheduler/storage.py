from __future__ import annotations

import contextlib
import json
import sqlite3
import time
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1


def new_id(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex}"


def utc_now() -> float:
    return time.time()


def json_dumps(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def json_loads(value: str | None, default: Any = None) -> Any:
    if value is None:
        return default
    return json.loads(value)


SCHEMA = r"""
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
    required INTEGER NOT NULL CHECK (required IN (0,1)),
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

CREATE UNIQUE INDEX IF NOT EXISTS one_active_assignment_per_agent
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

CREATE TABLE IF NOT EXISTS attempts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE RESTRICT,
    logical_agent_id TEXT NOT NULL REFERENCES logical_agents(id) ON DELETE RESTRICT,
    incarnation_id TEXT NOT NULL REFERENCES incarnations(id) ON DELETE RESTRICT,
    attempt_number INTEGER NOT NULL CHECK (attempt_number >= 1),
    lease_epoch INTEGER NOT NULL CHECK (lease_epoch >= 1),
    state TEXT NOT NULL CHECK (state IN ('ACTIVE','SUCCEEDED','FAILED','EXPIRED','CANCELLED')),
    created_at REAL NOT NULL,
    ended_at REAL,
    UNIQUE (task_id, attempt_number)
);

CREATE UNIQUE INDEX IF NOT EXISTS one_active_attempt_per_task
ON attempts(task_id) WHERE state = 'ACTIVE';

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
    state TEXT NOT NULL CHECK (state IN ('STARTING','RUNNING','SUCCEEDED','FAILED','LOST','UNKNOWN','TERMINATED')),
    runtime_handle_json TEXT NOT NULL DEFAULT '{}',
    outcome_json TEXT,
    failure_class TEXT,
    failure_code TEXT,
    failure_signature TEXT,
    terminal_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (terminal_confirmed IN (0,1)),
    quiescent_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (quiescent_confirmed IN (0,1)),
    started_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    ended_at REAL
);

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
"""


class Database:
    def __init__(self, path: str | Path):
        self.path = str(path)

    def connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.path, timeout=30.0, isolation_level=None)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA foreign_keys = ON")
        conn.execute("PRAGMA busy_timeout = 30000")
        conn.execute("PRAGMA journal_mode = WAL")
        conn.execute("PRAGMA synchronous = FULL")
        return conn

    def initialize(self) -> None:
        conn = self.connect()
        try:
            conn.executescript(SCHEMA)
            row = conn.execute("SELECT MAX(version) AS version FROM schema_migrations").fetchone()
            current = row["version"] if row and row["version"] is not None else 0
            if current > SCHEMA_VERSION:
                raise RuntimeError(
                    f"database schema {current} is newer than supported {SCHEMA_VERSION}"
                )
            if current < SCHEMA_VERSION:
                conn.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (?, ?)",
                    (SCHEMA_VERSION, utc_now()),
                )
        finally:
            conn.close()

    @contextlib.contextmanager
    def transaction(self, *, immediate: bool = True) -> Iterator[sqlite3.Connection]:
        conn = self.connect()
        try:
            conn.execute("BEGIN IMMEDIATE" if immediate else "BEGIN")
            yield conn
            conn.execute("COMMIT")
        except BaseException:
            if conn.in_transaction:
                conn.execute("ROLLBACK")
            raise
        finally:
            conn.close()

    def fetch_one(self, sql: str, params: tuple[Any, ...] = ()) -> sqlite3.Row | None:
        conn = self.connect()
        try:
            return conn.execute(sql, params).fetchone()
        finally:
            conn.close()

    def fetch_all(self, sql: str, params: tuple[Any, ...] = ()) -> list[sqlite3.Row]:
        conn = self.connect()
        try:
            return list(conn.execute(sql, params).fetchall())
        finally:
            conn.close()

    def integrity_check(self) -> str:
        conn = self.connect()
        try:
            return str(conn.execute("PRAGMA integrity_check").fetchone()[0])
        finally:
            conn.close()
