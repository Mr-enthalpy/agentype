# 14 — Recovery and Reconciliation

Status: Normative
Canonical path: docs/specs/v0.2/14-recovery-and-reconciliation.md
V0.1 preservation: UNCHANGED startup barrier
Source: [docs/specs/v0.1.md](../v0.1.md) §11, writer recovery §7

## Restart barrier (MUST)

Before dispatch after restart:

1. Enter RECOVERY. Identify/expire overdue authority and ACTIVE claims that
   never created an Execution, before either can heartbeat or ACK.
2. Apply deterministic read-only retry policy. Suspend writers whose
   quiescence is unknown.
3. Ask the owning adapter to reconcile still-authoritative or stale-unresolved
   STARTING/UNKNOWN/RUNNING handles by persisted execution identity.
   Physical-history MUST allow UNKNOWN → RUNNING
   ([03](03-task-attempt-lease-result.md)).
   Unavailable adapter: lose Task authority per failure policy; physical
   state remains UNKNOWN while termination/quiescence unconfirmed.
   Adapter presence alone MUST NOT admit Lease renewal. The current daemon
   MUST positively observe the exact Execution as RUNNING.
4. Record confirmed terminal outcomes; fence stale completion to Execution
   history only.
5. Promote eligible retry waits.
6. Reconcile pool; revive eligible LogicalAgents; MUST NOT revive RETIRED.
7. Drain pending topology at safe boundaries.
8. Mark READY and begin dispatch.

V0.2 semantic layers MUST NOT weaken this barrier.
Restart during Generation, Transform, compilation, or revival MUST resume
those objects; it MUST NOT invent a new Generation or a new LogicalAgent.

## Ambiguous start

`reconcile_start` MUST be identity-preserving. Process death is not
quiescence. Nonterminal `collect_outcome` MUST NOT inherit
`reconcile_start` quiescence.

## Heartbeat (**M4** admission + **M5** timing)

Heartbeat renewal is restricted to Executions this daemon positively
admitted as RUNNING. UNKNOWN database state or mere adapter presence is
insufficient. Core heartbeat and bulk renewal MUST require the persisted
Execution itself to be RUNNING.

The positive RUNNING transition and first Lease renewal MUST commit in one
fenced Core transaction before daemon admission ([13](13-storage-and-transactions.md)).

**M5:** configuration MUST satisfy
`dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`.

## Recovery startup cleanup (**M5**)

Heartbeat/notifier startup and the remaining recovery steps MUST share one
cleanup scope. If any later reconciliation fails after supervision started,
the daemon MUST stop those threads and clear all in-memory supervision
admissions before propagating the error. A failed startup MUST NOT leave
renewable authority behind.

## Notifier isolation (**M5** includes backoff clock)

Outbox delivery MUST run independently of dispatcher and heartbeat.
Slow delivery MUST NOT block lease supervision.
Backoff clock: [03](03-task-attempt-lease-result.md) (completion, not start).

## Daemon lifecycle (**M5**)

A SchedulerDaemon object MUST be single-run. A second `run` while notifier
shutdown is in progress MUST be rejected.
