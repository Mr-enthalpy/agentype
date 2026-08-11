from __future__ import annotations

from enum import StrEnum


class TaskState(StrEnum):
    BLOCKED = "BLOCKED"
    QUEUED = "QUEUED"
    LEASED = "LEASED"
    RUNNING = "RUNNING"
    RETRY_WAIT = "RETRY_WAIT"
    SUSPENDED = "SUSPENDED"
    COMPLETED = "COMPLETED"
    CANCELLED = "CANCELLED"


class AttemptState(StrEnum):
    ACTIVE = "ACTIVE"
    SUCCEEDED = "SUCCEEDED"
    FAILED = "FAILED"
    EXPIRED = "EXPIRED"
    CANCELLED = "CANCELLED"


class LeaseState(StrEnum):
    ACTIVE = "ACTIVE"
    RELEASED = "RELEASED"
    EXPIRED = "EXPIRED"
    REVOKED = "REVOKED"


class ExecutionState(StrEnum):
    STARTING = "STARTING"
    RUNNING = "RUNNING"
    SUCCEEDED = "SUCCEEDED"
    FAILED = "FAILED"
    LOST = "LOST"
    UNKNOWN = "UNKNOWN"
    TERMINATED = "TERMINATED"


class ResultState(StrEnum):
    AVAILABLE = "AVAILABLE"
    ACKED = "ACKED"


class BatchState(StrEnum):
    OPEN = "OPEN"
    ACTIVE = "ACTIVE"
    SUSPENDED = "SUSPENDED"
    COMPLETED = "COMPLETED"
    CANCELLED = "CANCELLED"


class AgentState(StrEnum):
    INITIALIZING = "INITIALIZING"
    READY = "READY"
    ASSIGNED = "ASSIGNED"
    REVIVING = "REVIVING"
    DRAINING = "DRAINING"
    SUSPENDED = "SUSPENDED"
    RETIRED = "RETIRED"


class IncarnationState(StrEnum):
    STARTING = "STARTING"
    WARM = "WARM"
    COLD = "COLD"
    LOST = "LOST"
    TERMINATED = "TERMINATED"


class EscalationState(StrEnum):
    OPEN = "OPEN"
    RESOLVED = "RESOLVED"
    CANCELLED = "CANCELLED"


class OutboxState(StrEnum):
    PENDING = "PENDING"
    DELIVERED = "DELIVERED"
    ACKED = "ACKED"


class FailureClass(StrEnum):
    TRANSIENT_EXTERNAL = "TRANSIENT_EXTERNAL"
    TIMEOUT = "TIMEOUT"
    EXECUTION_LOST = "EXECUTION_LOST"
    START_FAILURE = "START_FAILURE"
    RESOURCE_UNAVAILABLE = "RESOURCE_UNAVAILABLE"
    PERMISSION_FAILURE = "PERMISSION_FAILURE"
    INVALID_RESULT = "INVALID_RESULT"
    ADAPTER_PROTOCOL_FAILURE = "ADAPTER_PROTOCOL_FAILURE"
    UNKNOWN = "UNKNOWN"
    WRITER_QUIESCENCE_UNKNOWN = "WRITER_QUIESCENCE_UNKNOWN"


class WorkspaceMode(StrEnum):
    READ_ONLY = "read_only"
    WRITE = "write"


class Retention(StrEnum):
    RESIDENT = "resident"
    EPHEMERAL = "ephemeral"


class ContinuityPreference(StrEnum):
    REQUIRED = "required"
    PREFERRED = "preferred"
    NONE = "none"
