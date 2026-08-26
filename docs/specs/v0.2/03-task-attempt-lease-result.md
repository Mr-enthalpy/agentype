# 03 — Task, Attempt, Lease, Result, Batch, Execution

Status: Normative
Canonical path: docs/specs/v0.2/03-task-attempt-lease-result.md
V0.1 preservation: UNCHANGED kernel state machines and fencing
Source: [docs/specs/v0.1.md](../v0.1.md) §§1–4, 7–8

Materializing a Task MUST NOT establish execution authority.
Claim MUST.

## Task

| From | Operation | To | Authority | Tx |
|---|---|---|---|---|
| BLOCKED | all dependencies completed | QUEUED | Scheduler | Y |
| QUEUED | transactional claim | LEASED | Scheduler | Y |
| LEASED | execution start confirmed | RUNNING | Scheduler | Y |
| LEASED/RUNNING | retryable NACK | RETRY_WAIT | Scheduler | Y |
| RETRY_WAIT | retry eligibility reached | QUEUED | Scheduler | Y |
| LEASED/RUNNING/RETRY_WAIT | policy exhausted or ambiguity | SUSPENDED | Scheduler | Y |
| LEASED/RUNNING | valid success ACK | COMPLETED | Scheduler | Y |
| BLOCKED/QUEUED/LEASED/RUNNING/RETRY_WAIT/SUSPENDED | cancel | CANCELLED | Scheduler | Y |

`COMPLETED` and `CANCELLED` are terminal. A completed Task MUST NOT be reopened.
Completed work is superseded by a new Task.

V0.1 has no optional Task path. Every submitted Task participates in the Batch
barrier. V0.2 MUST NOT add an optional-task bypass of Result creation.

A V0.2 Task MUST belong to a Generation ([04](04-generation-and-frontier.md)).
Retry of that Task MUST remain in the same Generation.

## Attempt

| From | Operation | To |
|---|---|---|
| ACTIVE | valid success ACK | SUCCEEDED |
| ACTIVE | NACK/start failure | FAILED |
| ACTIVE | lease expiry | EXPIRED |
| ACTIVE | cancellation | CANCELLED |

Every claim MUST create a new Attempt, even if no Execution is ever created.

## Lease

| From | Operation | To |
|---|---|---|
| ACTIVE | success/failure release | RELEASED |
| ACTIVE | deadline passes | EXPIRED |
| ACTIVE | cancellation/suspension | REVOKED |

Fencing epoch MUST increase monotonically per Task.
Authority validation MUST reject a lease at or beyond `expires_at` even if the
expiry sweep has not yet updated the row.

## Execution

| From | Operation | To |
|---|---|---|
| STARTING | adapter confirms start | RUNNING |
| STARTING | adapter rejects start | FAILED |
| STARTING | start outcome ambiguous | UNKNOWN |
| RUNNING | standardized success | SUCCEEDED |
| RUNNING | standardized failure | FAILED |
| RUNNING | lost during observation | LOST |
| STARTING/RUNNING/UNKNOWN | confirmed termination | TERMINATED |

Execution MUST NOT mutate Task authority without a current Attempt/Lease check.
At most one Execution history row per Attempt (kernel).
At most one STARTING/RUNNING/UNKNOWN Execution per Incarnation.

## Result

| From | Operation | To |
|---|---|---|
| (none) | success ACK transaction | AVAILABLE |
| AVAILABLE | Root ACK | ACKED |

Exactly one authoritative Result per completed Task.
Root ACK MUST NOT complete the worker or change Task/Batch completion.

## Batch

| From | Operation | To |
|---|---|---|
| OPEN | submission committed | ACTIVE |
| ACTIVE | all Tasks completed | COMPLETED |
| ACTIVE | any Task suspended | SUSPENDED |
| OPEN/ACTIVE/SUSPENDED | cancel | CANCELLED |
| SUSPENDED | Root recovery operation | ACTIVE |

Batch completion depends on Task completion and durable Result creation, not
Root Result acknowledgement.

Batch MUST remain distinct from Generation.

## Writer safety (UNCHANGED)

Tasks declare `workspace_mode` `read_only` or `write`.
Lease expiry MUST NOT prove a writer stopped.
Replacement dispatch for a write Task is permitted only when the previous
Execution is confirmed terminal/quiescent **or** that Execution's frozen
creation-time snapshot records attempt isolation.
Otherwise recovery MUST atomically suspend Task/Batch, open Escalation
`WRITER_QUIESCENCE_UNKNOWN`, and MUST NOT dispatch a duplicate writer.
Writer safety is derived from the Execution persisted for the current Attempt.
Omitting `execution_id` on ACK/NACK MUST NOT treat an existing physical writer
as executionless.
Cancellation is not quiescence proof. Open `WRITER_QUIESCENCE_UNKNOWN` on a
current partition member MUST block RETIRE.

## Escalation

Open while a decision or writer-safety obligation exists. Terminal when
resolved. One open Escalation per suspended Task (kernel).

## Notification outbox

| From | Operation | To |
|---|---|---|
| pending | bounded deliver success | DELIVERED |
| pending | bounded deliver failure | backoff then retry |
| DELIVERED | (terminal for delivery) | — |

RootBridge failure MUST NOT change Task, Result, or Batch state.
Notifier thread MUST NOT supervise Tasks, renew Leases, recompute Batches, or
consume Results.
Payload MUST be event id, type, and indexes — not Result body.
Normal wakeup is `BATCH_RESULTS_READY`. `RESULT_AVAILABLE` is durable Result
state, not a per-Task wakeup.

Vendor RootBridge transports (Codex thread, Grok session) are
IMPLEMENTATION-DEFINED. Core MUST treat locators as opaque.
