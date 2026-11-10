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

**M6-B.5 (`SECURITY_V1`):** a `start` or `reconcile_start` observation carries an
explicit `SecuredStartPhase` (`HELD`, `RELEASED`, `UNKNOWN`, `TERMINAL`) and an
**optional** `SecuredReceiptChain`: a `SandboxAppliedReceipt` plus, for a
credential-bound Execution, a `CredentialDeliveryReceipt` that binds the sandbox
receipt digest. `HELD` requires a valid, verified sandbox receipt; a
credential-bound `RELEASED` requires both receipts; `UNKNOWN` permits the chain to
be missing; `TERMINAL` may occur before any receipt exists.

The phase, not receipt presence, decides RUNNING:

| Phase | RUNNING |
|---|---|
| `HELD` | MUST NOT confirm |
| `RELEASED` | MAY confirm, only after positive physical observation |
| `UNKNOWN` | MUST NOT confirm |
| `TERMINAL` | MUST NOT confirm |

Physical start MUST hold worker code at a controlled pre-exec boundary until the
sandbox receipt and its directory metadata are durable. Receipt publication is
`durable contents → atomic no-replace publication → durable namespace update →
verify published identity`; syncing only the file does not prove the published
entry, and POSIX `fsync`/`rename` is not a cross-platform proof. The receipt's
`actual_root` MUST equal `ResolveRoot(committed launch descriptor, RequestId)`,
proven by materialized filesystem object identity through the trusted
path-resolution rule, never by path-string equality. The hold MUST stay closed
on daemon crash or control-channel loss; a wait mechanism that releases the worker
when the parent's handle or pipe closes is invalid. Credential delivery happens
while `HELD`, and its receipt is durable before authority revalidation and
`RELEASED`. The observation, the required receipt chain, the positive RUNNING
transition and the first renewal MUST commit in the same fenced transaction; no
separate security-confirmation state exists.

On restart, `reconcile_start` MUST recover the phase and, when the phase requires
them, the receipts for the same `RequestId` and the same physical instance. A
`HELD` instance MUST NOT be released until the Kernel revalidates current
Scheduler/Task authority and issues a proof for that exact `RequestId` and
instance witness, carrying a single-use `ReleaseOperationId` and the current
Lease/fencing identity; the Kernel checks that authority again immediately before
the release call. The Kernel creates and advances the `ReleaseOperationIntent`
only in fenced Core transactions, and a new operation id is authorized only when
no prior intent exists or the prior intent is terminal (`released` or `aborted`).
If a prior intent is `dispatched` or `unknown`, the instance remains `UNKNOWN`
until recovery proves
whether the operation was released, had no effect, was cancelled, or can no longer
take effect; observing `HELD` alone MUST NOT close the intent or authorize a second
operation. Release MUST be idempotent for the same operation: after the result is
durable and the phase is proven `RELEASED`, a repeated call returns that recorded
result without a second OS release or new authority; a terminal `aborted` intent
with the instance still `HELD` may obtain a fresh proof with a new operation id
under the current Lease/fencing identity.
A proof known to be invalid never initiates release; a Lease revocation racing an
already-started valid release is handled by M5 fencing/recovery and cannot create a
new valid semantic result. A reused PID
MUST NOT confer release authority; if authority is invalid the worker stays held
and follows physical cleanup plus M5 recovery; if the release result is uncertain
no second worker may be created. Receipt presence alone MUST NOT confirm RUNNING.
If a required receipt or the instance relation cannot be recovered, the Execution
MUST NOT be admitted as RUNNING, a receipt MUST NOT be synthesized, and the legacy
RUNNING path MUST NOT be used. A controlled process created before the sandbox
receipt was persisted is `UNKNOWN`: reconciliation must not release it, must not
synthesize a receipt, and must leave it to M5 physical recovery; the
crash-injection suite covers this window. The receipt artifacts are stored in a
daemon-owned state directory outside the worker-writable jail root/workspace,
separately from the SQLite rows; the rows remain authority-bearing and the
artifacts are verified physical evidence. The chain is append-only: the credential
receipt binds the sandbox receipt digest and never replaces it. Receipt read
confidentiality is not required; integrity, non-replaceability, identity binding
and recoverability are
([ADR-0014](../../decisions/0014-m6b5-enforcement-compilation-receipt.md)).

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

**M5.8:** one Scheduler store has exactly one production daemon, identified
by OS process lock on store file identity (not a Scheduler Lease). Startup
MUST acquire that lock before Kernel recovery mutation or Adapter I/O. The
production store is a literal, UTF-8 representable, file-backed filesystem
path. Three SQLite special filenames — URI filenames (`file:...`, which are
special only while SQLite URI processing is enabled), the bare in-memory
spelling (`:memory:`, and only that exact spelling: any additional text makes
it an ordinary filename), and the empty filename that selects a private
temporary database (both of which are special independently of URI
processing) — MUST be rejected at the config boundary and at the store-open
boundary, because the lock resolves a filesystem handle while SQLite applies
its own special-filename rules to the same filename. Separately, the store
identity MUST be UTF-8 representable: a non-UTF-8 path is not a SQLite
special filename but is likewise rejected, and the rejection MUST be decided
on the filename bytes SQLite actually receives, so that a `file:` prefix
hidden inside a filename that is not valid UTF-8 is refused as a URI
filename rather than mistaken for a literal path. The in-memory store MUST
remain reachable only through an explicit `open_memory`-style entry point.
Startup also assumes the Scheduler database pathname is not adversarially
replaced between lock acquisition and Kernel open; the post-open identity
confirmation
re-resolves the path, so a pathname ABA inside that window is not detected. `recover_runtime` grants
activation, not dispatch. READY requires the composition of a runtime-local
`ReadyPermit` (minted after the process lock, recovery, and the
physical-observation activation sweep, and only permitting construction of a
not-yet-published control loop), the worker's `ReadyRelease`, and the
DispatchGate being Ready; no one of them alone publishes READY. Physical
observation does not renew authority; stale freshness stops renewal
eligibility without inventing death, termination, or quiescence. Shutdown
stops Scheduler mechanics; it MUST NOT cancel semantic work or terminate
external agents. The only safe recovery from a failed Runtime component is a
new Runtime lifecycle through the full restart barrier.
