# 13 — Storage and Transactions

Status: Normative
Canonical path: docs/specs/v0.2/13-storage-and-transactions.md

SQL table names, column names, and indexes beyond the semantic unique
constraints are IMPLEMENTATION-DEFINED.

## Source of truth

Scheduler durable state MUST have a single-machine authoritative store for
the first Rust V0.2 line. SQLite WAL SHOULD be that store (same as V0.1).
Core domain types MUST NOT leak SQL types into semantic APIs where avoidable.

V0.1 database migration/import vs new database is DEFERRED (D-DB-MIGRATE).
M3 MAY start a new DB. Import MUST be specified before any claim of
in-place upgrade.

Times MUST be UTC epoch seconds or equivalent unambiguous UTC.
IDs MUST be unique durable strings (UUID recommended, IMPLEMENTATION-DEFINED).

## Kernel unique constraints (MUST)

- one ACTIVE lease per Task
- one ACTIVE Attempt per Task
- one ACTIVE Attempt per LogicalAgent
- one Execution history row per Attempt
- one authoritative Result per Task
- one open Escalation per suspended Task
- one assigned LogicalAgent per Task
- one STARTING/WARM/COLD Incarnation per LogicalAgent
- at most one STARTING/RUNNING/UNKNOWN Execution per Incarnation

## Transaction boundaries (MUST be atomic)

| Operation | Includes |
|---|---|
| Batch submit | Batch + Task graph + dependencies + initial BLOCKED/QUEUED |
| Claim | fencing epoch increment + Attempt + Lease + LogicalAgent ASSIGNED |
| Execution create | Execution associated with Attempt and Incarnation |
| Success ACK | Attempt SUCCEEDED, Lease RELEASED, Task COMPLETED, exactly one Result AVAILABLE, dependency release, Batch recompute; wakeup only at Batch/control boundary |
| Retryable NACK | Failure, Attempt FAILED, Lease RELEASED, Task RETRY_WAIT, agent release |
| Suspend | Task SUSPENDED, Lease REVOKED, Escalation, Batch SUSPENDED, decision outbox |
| Result ACK | Result state only |
| Checkpoint promote | matching Attempt + epoch |
| Topology mutation | revision + desired membership (V0.1 rules) |
| Generation REVIEWABLE | drain predicates + durable intents/proposals flags |
| Proposal persist | intent → proposal outcome; MUST NOT admit |
| Transform cutover | successor identity + lineage + source RETIRED + topology; writer safety held |
| MemoryCapsule version | MUST NOT be hidden LLM; promotion protocol DEFERRED so this tx MUST NOT auto-apply worker deltas |

Stale writes MUST fail closed (no canonical mutation). Physical-only history
MAY still record on the old Execution/Incarnation.
