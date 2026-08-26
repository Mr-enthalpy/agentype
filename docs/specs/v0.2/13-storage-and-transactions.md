# 13 — Storage and Transactions

Status: Normative
Canonical path: docs/specs/v0.2/13-storage-and-transactions.md

SQL table names, column names, and indexes beyond the semantic unique
constraints are IMPLEMENTATION-DEFINED.

## Source of truth

Scheduler durable state MUST have a single-machine authoritative store for
the first Rust V0.2 line. For M4 that store MUST be SQLite with WAL and
`synchronous=FULL` (UNCHANGED from V0.1). Core domain types MUST NOT leak
SQL types into semantic APIs where avoidable. A later storage backend is
out of M4 scope.

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
| Success ACK | Attempt SUCCEEDED, Lease RELEASED, Task COMPLETED, exactly one Result AVAILABLE, dependency release, Batch recompute. If this transaction is the **first** `Batch → COMPLETED`, it MUST also insert **exactly one** `BATCH_RESULTS_READY` outbox row. MUST NOT complete Batch in tx1 and enqueue wakeup in tx2. |
| Retryable NACK | Failure, Attempt FAILED, Lease RELEASED, Task RETRY_WAIT, agent release |
| Suspend | Task SUSPENDED, Lease REVOKED, Escalation, Batch SUSPENDED, and the decision/control outbox event in the **same** transaction |
| Result ACK | Result state only |
| Checkpoint promote | matching Attempt + epoch |
| Topology mutation | revision + desired membership (V0.1 rules) |
| Generation REVIEWABLE | drain predicates + durable intents/proposals flags |
| Proposal persist | intent → proposal outcome; MUST NOT admit |
| Transform cutover | single transaction: successor create + lineage + topology cutover + source RETIRED + writer safety held. Durable state jumps TARGET_READY → COMPLETED. No persisted split-brain CUTTING_OVER |
| MemoryCapsule version | MUST NOT be hidden LLM; promotion protocol DEFERRED so this tx MUST NOT auto-apply worker deltas |

Stale writes MUST fail closed (no canonical mutation). Physical-only history
MAY still record on the old Execution/Incarnation.
