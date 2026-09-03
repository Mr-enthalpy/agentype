# RIIR M5.5 — Notifier Isolation, Durable Outbox Delivery, RootBridge Wakeup Boundary

Status: Historical Report
Applies to: branch `rust/m5.5-notifier` (base: main @ M5.4 merge `a9d4d00`)
Canonical path: `docs/reports/v0.2/riir-m5.5-notifier-root-bridge.md`
Not a specification.

Despite the historical `riir-` directory naming, this milestone is **native Rust
runtime implementation**. It consumes the frozen M4 kernel and M5.1–M5.4
runtime boundaries and implements spec 03 / 12 / 14 notifier isolation.

---

## 1. Mission

> Deliver durable Scheduler wakeup events to Root through a bounded RootBridge,
> with at-least-once delivery, durable retry/backoff, and complete isolation
> from Task/Lease/Result execution authority.

Canonical flow:

```text
Scheduler authority transaction
        ↓
notification_outbox = PENDING
        ↓
NotifierService
        ↓
RootBridge
        ↓
external Root wakeup
        ↓
DELIVERED

later explicit Root acknowledgement
        ↓
ACKED
```

Result Queue ≠ RootBridge. The RootBridge is a wakeup/control side channel
only.

**Central invariant:** the Notifier owns delivery mechanics only. It owns no
Scheduler execution authority. The only durable state it may mutate is
`notification_outbox` delivery metadata/state. RootBridge mutates none.

M5.5 is complete when slow, failing, duplicated, restarted, or temporarily
unavailable Root notification transport cannot: block dispatcher progress;
block heartbeat renewal; mutate Task/Attempt/Lease/Batch authority; consume
or transport Result bodies; invent Root acknowledgement; lose a durable
wakeup because of a process crash; convert delivery ambiguity into false
delivery success.

---

## 2. Dependency diagram

```text
agentype-core
    OutboxEventId, OutboxState, BATCH_RESULTS_READY, DECISION_REQUIRED
        |
        v
agentype-storage-sqlite
    due_outbox / commit_outbox_delivery_success
    commit_outbox_delivery_failure / ack_outbox
        |
        v
agentype-runtime
    NotifierService / NotifierRunner / StartupGuard
        |
        v
agentype-root-bridge
    RootBridge, RootWakeup, DeliveryReceipt, RootBridgeError
    RecordingRootBridge (conformance fake)
```

Rules:

- Core MUST NOT depend on root-bridge/runtime.
- Storage MUST NOT depend on root-bridge.
- Root-bridge MAY depend on core ID/DTO types; MUST NOT depend on Kernel/storage.
- Runtime MAY depend on storage + root-bridge.

RootBridge is never given a Kernel, database connection, Scheduler,
Dispatcher, or SupervisionService. Spec 15 crate names are
implementation-defined; this crate split keeps ExecutionAdapter deadlines
(M5.6) a different authority boundary from RootBridge delivery.

No vendor Root transport (Codex / Grok / filesystem / terminal) is in M5.5.
There is no `NoopRootBridge` that marks events DELIVERED.

---

## 3. Outbox state machine (unchanged)

```text
             deliver success
PENDING --------------------------> DELIVERED
   |                                  |
   | delivery failure                 |
   | attempts++, backoff              | Root acknowledge
   v                                  v
PENDING                            ACKED
   |
   | Root acknowledge
   v
 ACKED
```

Legal transitions: `(none)→PENDING`, `PENDING→DELIVERED`, `PENDING→PENDING`,
`PENDING→ACKED`, `DELIVERED→ACKED`. `ACKED` is terminal.

No M5.5 states: `IN_FLIGHT`, `CLAIMED`, `FAILED`, `DEAD_LETTER`,
`DELIVERY_LEASED`.

| Durable Outbox State | Bridge call in progress? | Notifier may select? | Root may ACK? | Result affected? |
| -------------------- | -----------------------: | -------------------: | ------------: | ---------------: |
| PENDING, not due     |                       no |                   no |           yes |               no |
| PENDING, due         |                       no |                  yes |           yes |               no |
| PENDING              |                      yes |     already selected |           yes |               no |
| DELIVERED            |                       no |                   no |           yes |               no |
| ACKED                |                       no |                   no |    idempotent |               no |

| Bridge outcome                 | Durable consequence |
| ------------------------------ | ------------------- |
| positive delivered proof       | PENDING → DELIVERED |
| unavailable                    | PENDING + backoff   |
| deadline exceeded              | PENDING + backoff   |
| protocol failure               | PENDING + backoff   |
| ambiguous external side effect | PENDING + backoff   |
| internal storage corruption    | Notifier FAILED     |

---

## 4. Four operations remain distinct

| Operation | Meaning |
|---|---|
| Worker success ACK | Attempt SUCCEEDED, Lease RELEASED, Task COMPLETED, Result AVAILABLE, Batch recompute, possibly `BATCH_RESULTS_READY` PENDING |
| RootBridge delivery | Outbox PENDING → DELIVERED: this bridge positively proved its wakeup criterion |
| Outbox ACK | PENDING or DELIVERED → ACKED: explicit Root-facing acknowledgement |
| Result ACK | Result AVAILABLE → ACKED: Result Queue consumption |

```text
RootBridge delivery ≠ Outbox ACK
Outbox ACK          ≠ Result ACK
Result ACK          ≠ Batch completion
Outbox ACK          ≠ Batch completion
Worker ACK          ≠ Root delivery
```

`ack_outbox` remains the only Root-facing ACK. Notifier and RootBridge MUST
NOT call it.

---

## 5. RootWakeup envelope

```text
event_id, event_type, aggregate_type, aggregate_id
indexes: well-formed *_id (string) and *_ids (string list) only
```

Unknown non-index payload fields are dropped. Malformed `*_id` / `*_ids`
fail closed (`WakeupEnvelopeError` → notifier fatal), never forwarded as a
blob. `*_ids` is matched before `*_id`.

Not forwarded: raw `payload_json`, Result.payload, Result.summary, worker
output, Task payload, acceptance criteria, workspace content, provider
configuration, runtime handle.

Normal wakeup remains Batch-level `BATCH_RESULTS_READY` (exactly one, same
transaction as first `Batch → COMPLETED`). Control events such as
`DECISION_REQUIRED` pass through as opaque type + indexes. Notifier does
not enumerate the Result Queue.

---

## 6. RootBridge success and error

```rust
fn deliver(&self, wakeup: &RootWakeup) -> Result<DeliveryReceipt, RootBridgeError>
```

`Ok(DeliveryReceipt)` = positive bridge-specific proof that the wakeup
delivery criterion completed. `Err` = not proven delivered. There is no
`delivered: false` in the success channel.

`RootBridgeError` is mechanical (`Unavailable`, `DeadlineExceeded`,
`Protocol`, `Rejected`, `Other`). It is **not** a Scheduler `FailureClass`.
It does not NACK an Attempt, suspend a Task, or raise an Escalation.

Every `deliver()` MUST be bounded by the implementation. M5.5 does not add
a generic per-call watchdog thread. A non-bounded RootBridge is
non-conformant; notifier isolation protects Scheduler loops from slow
delivery, but shutdown is only finite if the bridge returns.

---

## 7. Retry policy and completion-time clock

`NotifierRetryPolicy { base_delay, max_delay }`: finite, positive,
`max >= base`, deterministic, overflow-safe exponential backoff. No jitter.
No max delivery attempts. No dead-letter. An undelivered event remains
retryable until Root ACKs.

`delivery_finished_at` is sampled **inside** the short post-call transaction
after `BEGIN IMMEDIATE`. The transaction cannot start until `deliver()`
returns, so backoff is completion-anchored **and** obeys the frozen
“timestamp after BEGIN IMMEDIATE” rule. Kernel methods do not accept a
pre-call timestamp.

Incorrect: `next_delivery_at = t_start + delay`.
Correct: `next_delivery_at = t_completion + delay`.

`delivery_attempts` increases only after the bridge call returns **and** the
matching state update commits. Crash-before-commit leaving attempts
unchanged is acceptable bookkeeping, not an exactly-once counter.

`last_error` is currently a **bounded** diagnostic (512 characters) from
`format!("{RootBridgeError}")`. That is not a fully sanitized vocabulary:
a vendor bridge could still put a token in the error string. First real
RootBridge MUST persist `safe category + bridge-defined sanitized short
detail`, not an arbitrary error string. Not an M5.5 merge blocker.

Hard invariant: **never hold a SQLite transaction across RootBridge I/O.**

```text
short DB read (due_outbox)
  → owned candidate snapshot
  → NO DB TRANSACTION
  → RootBridge.deliver(...)
  → short DB write (success or failure CAS)
```

---

## 8. At-least-once crash model

M5.5 MUST NOT claim exactly-once external delivery. Stable `event_id` is
the idempotency/deduplication key. Receivers MUST tolerate repeated
delivery of the same event id. Idempotency is not derived from BatchId
alone (multiple control events may target the same aggregate).

| Case | Durable result |
|---|---|
| A. Crash before bridge call | PENDING remains → retry |
| B. Crash during bridge call | external unknown; PENDING remains → retry (duplicate allowed) |
| C. Bridge succeeded, crash before DELIVERED | external wakeup may exist; DB PENDING → retry |
| D. Bridge failed, crash before failure update | PENDING with previous eligibility (may retry earlier than intended backoff) |
| E. Success update committed | DELIVERED; no notifier retry |
| F. Failure update committed | PENDING; `next_delivery_at = completion + backoff` |
| G. ACK committed | ACKED; never reselected |

`IN_FLIGHT` cannot prove whether the external side effect occurred before
crash. It would need its own timeout/lease/reconciliation and still allow
duplicates. M5.5 chooses `event_id` + at-least-once retry + idempotent
RootBridge.

Two incorrectly concurrent notifier processes may both select the same
PENDING event. Tolerated. No delivery lease (M5.8 process singleton).
Within one Runtime instance there is at most one `NotifierRunner`.

---

## 9. ACK races

Legal: Notifier selects PENDING → Root ACKs → ACKED → bridge returns
success → success commit is a no-op. Final state ACKED. MUST NOT regress
to DELIVERED. A late duplicate physical wakeup is acceptable
at-least-once.

Likewise: candidate selected → Root ACK → bridge failure → failure
commit observes ACKED and does nothing (no attempts++, no retry, no
PENDING restore).

Duplicate success commits do not double-count. Failure cannot regress
DELIVERED to PENDING.

PENDING → ACKED without DELIVERED is legal: Root may obtain the event
through another authoritative interface first.

---

## 10. Notifier lifecycle

`NotifierService` is the deterministic engine (`deliver_one`,
`deliver_due`). Tests run it without threads or real sleeps.

`NotifierRunner` owns one worker thread and one service. Sequential
bounded `deliver()` is acceptable; Root throughput is not Scheduler
correctness. Notifier timing (`poll_interval`, `batch_limit`) has **no**
relation to heartbeat (`notifier_poll < heartbeat` is not required).

```text
NEW → start → RUNNING
            ├── stop request → STOPPING → STOPPED
            └── internal durable fault / panic → FAILED
```

Do not restart FAILED in place. Ordinary RootBridge errors: event retry,
runner stays RUNNING. Storage/invariant errors and unexpected panic:
FAILED. Notifier failure MUST NOT fabricate Task failure.

Stop: do not select new events. If one bounded call is already executing,
finish it, persist success/failure, then stop. Do not discard a positive
delivery result because shutdown was requested.

`Drop` requests stop, wakes, and joins — same discipline as
`SupervisionRunner`. A detached orphan thread is illegal.

---

## 11. Startup cleanup integration

Production composition:

```text
expire_leases(true)
        ↓
start empty SupervisionRunner
        ↓
start NotifierRunner          // same uncommitted StartupGuard scope
        ↓
terminal replay
        ↓
physical reconciliation (immediate admit)
        ↓
expire_leases(false) / promote / pool / revive
        ↓
READY invariant
        ↓
both runners healthy
        ↓
StartupGuard::commit
        ↓
RecoveredRuntime { supervision, notifier }
```

Canonical `recover_runtime(kernel, adapters, timing, NotifierBinding)`.
`NotifierBinding::Enabled` is production. `NotifierBinding::DisabledForTests`
/ `recover_runtime_without_notifier` is the **named** test-only path. There
is no silent “no RootBridge configured” success path. There is no
`RecoveredRuntime::into_runner()` that would keep heartbeat and drop
notifier.

`OutboxDeliveryCandidate` fields are private. Only `Kernel::due_outbox`
constructs it. `deliver_one` cannot mark a real event DELIVERED from a
caller-forged type/aggregate/payload.

Cleanup ordering (documented in `recovery.rs`):

```text
signal notifier stop
signal supervision stop
join supervision          // heartbeat cannot keep renewing
join notifier             // may wait on one in-flight bounded deliver
```

Stopping the notifier stops delivery work only. It does not ACK events,
revert DELIVERED, revoke a Lease, terminate a worker, or claim quiescence.

Delivery during RECOVERY is legal: a wakeup asserts a durable event
exists, not that the daemon is READY. A successful DELIVERED commit is
not rolled back if a later recovery step fails. Ordinary RootBridge
unavailability does **not** prevent READY. Durable notifier corruption
**does**.

---

## 12. Authority matrix

| Component           |    May read Outbox | May deliver wakeup | May mutate Outbox delivery state |       May ACK Outbox | May mutate Task/Lease/Result |
| ------------------- | -----------------: | -----------------: | -------------------------------: | -------------------: | ---------------------------: |
| Kernel authority tx |                yes |                 no |                     enqueue only | Root-facing API only |       yes, according to Core |
| NotifierService     |                yes |     via RootBridge |                              yes |                   no |                           no |
| NotifierRunner      | orchestration only |        via service |                      via service |                   no |                           no |
| RootBridge          |      envelope only |                yes |                               no |                   no |                           no |
| Root                |       via Root API |                 no |                               no |                  yes |        Result ACK separately |
| ExecutionAdapter    |                 no |                 no |                               no |                   no |                           no |

Threads:

```text
Dispatcher thread       ──────┐
Heartbeat thread        ──────┼── independent
Notifier thread         ──────┘
```

---

## 13. Schema decision

**SCHEMA_VERSION remains 3.** The existing `notification_outbox` already
has `state`, `delivery_attempts`, `next_delivery_at`, `created_at`,
`delivered_at`, `acknowledged_at`, `last_error`. No `IN_FLIGHT` column, no
delivery owner, no delivery lease, no persisted RootBridge identity, no
durable runner state.

`Kernel::mark_outbox_delivered` remains the M4 test helper. Production
notifier uses `commit_outbox_delivery_success` /
`commit_outbox_delivery_failure` (clear `last_error` on success; no-op on
DELIVERED/ACKED; missing identity is invariant failure).

---

## 14. Test mapping

| Plan § | Coverage |
|---|---|
| §50 storage 1–22 | `crates/agentype-storage-sqlite/tests/outbox_delivery.rs` |
| §51 wakeup 23–33 | `agentype-root-bridge` unit tests + `NotifierService` envelope tests |
| §52 service 34–45 | `crates/agentype-runtime/src/notifier.rs` tests |
| §53 races 46–53 | ACK-before-scan, ACK-during-success/failure, duplicate success, failure-cannot-regress |
| §54 runner 54–65 | start/deliver/stop/in-flight persist/Drop/panic/ordinary-vs-fatal |
| §55 isolation 66–72 | slow bridge vs heartbeat, vs `dispatch_one`, vs SQLite write; shutdown does not revoke Lease / change Execution |
| §56 recovery 73–84 | `StartupGuard` owns both; READY despite Root unavailability; no redelivery of DELIVERED; deferred backoff survives restart; failed recovery stops both |

Existing M4 / M5.1–M5.4 suites remain green. Python V0.1 oracle remains
green, including `test_notifier_backoff_is_measured_from_delivery_completion`
and `test_slow_notifier_does_not_block_dispatcher_or_lease_supervision`.
Python `OutboxDispatcher` was not transliterated.

---

## 15. Exact final-head counts

Recorded on `rust/m5.5-notifier` after `cargo fmt --all --check` (via
`cargo fmt --all`), `cargo clippy --workspace --all-targets -- -D warnings`,
and `cargo test --workspace`.

**Rust: 328 passed, 0 failed** (audit-closure head; removed the synthetic-candidate test)

| Crate / suite | Count |
|---|---|
| adapter-api | 8 |
| core | 20 |
| execution-config | 8 |
| root-bridge | 8 |
| runtime | 159 |
| storage m4_kernel | 65 |
| storage outbox_delivery | 17 |
| storage reconciliation | 5 |
| storage recovery | 11 |
| storage supervision | 11 |
| storage topology | 16 |

storage 125 = 65 + 17 + 5 + 11 + 11 + 16.
Total 8 + 20 + 8 + 8 + 159 + 125 = 328.

Do not reuse M5.4's 280 or the pre-audit 329.

**Python oracle** (`py -3`, 3.13 local; CI Ubuntu 3.11 is the oracle
environment): 160 passed, 2 skipped (Ran 162 tests).

Default Windows `python` 3.8 cannot import `StrEnum`; that is a local
interpreter mismatch, not a suite regression. Exact-head CI remains the
Python evidence.

`git diff --check`: no whitespace errors (CRLF-on-checkout warnings only).

---

## 16. Remaining M5.6 / M5.7 / M5.8 prerequisites

Hung from `status.txt`; not lost; not implemented here.

**M5.6 P1 — recovery adapter calls must obtain an absolute deadline.**
`reconcile_start` / `collect_outcome` (and later observe/interrupt/terminate
if they enter recovery) are still unbounded. A hang on Execution B can
leave startup RECOVERING forever while Execution A's heartbeat continues.
This does not break fencing correctness and is not final daemon behavior.
StartupGuard covers error/unwind, not hang. Do not invent a recovery-only
timeout in a follow-up to M5.5; wait for the M5.6 absolute-deadline
framework. RootBridge boundedness in M5.5 is a **different** authority
boundary and must not be unified with ExecutionAdapter I/O.

**M5.7-before P1 — adapter binding identity.** FakeAdapter routing by
persisted `adapter_kind` is enough. Real adapters may need an opaque
`adapter_binding_key`. BLOCKS_REAL_ADAPTER_PARITY.

**M5.8 P1 — barrier must be mechanically executed by the composition root.**
`recover_runtime` is the correct barrier, but Dispatcher can still be
constructed without holding `RecoveredRuntime`. M5.8 must mechanize:

```text
process lock → RECOVERING → RecoveredRuntime → construct/enable Dispatcher → READY
```

Cannot rely on “caller MUST remember to recover first.” One production
`SupervisionRunner` owner per process; move-only grant prevents capability
replay, not OS/process composition singleton. Also: process lock/signals;
two OS processes on one DB can still each grant+admit; LEGACY
`Kernel::heartbeat` visibility; runner fatal/phase as daemon seam.

**M5.8-before P1 — independent steady-state physical observer.** Admitted
RUNNING + worker dies + DB still RUNNING + heartbeat continues. Heartbeat
must not observe. Not M5.5/M5.6.

Not implemented: vendor Root transports, real worker adapters, SchedulerDaemon,
M6 Generation/WorkIntent/AgentType/Transform/MemoryCapsule.

---

## Completion questions

1. Can RootBridge failure mutate a Task? — NO
2. Can RootBridge success ACK a Result? — NO
3. Can Notifier read Result payload for wakeup transport? — NO
4. Can PENDING be ACKED without DELIVERED? — YES
5. Can DELIVERED be automatically retried? — NO
6. Can ACKED return to PENDING? — NO
7. Can external delivery happen twice after a crash? — YES
8. Is that allowed? — YES, event-id deduplicated at-least-once
9. Is delivery success persisted only after positive bridge proof? — YES
10. Is retry backoff anchored after the bridge call completes? — YES
11. Is any SQLite transaction held during bridge I/O? — NO
12. Can slow RootBridge block heartbeat? — NO
13. Can slow RootBridge block dispatcher? — NO
14. Does ordinary RootBridge timeout fail the NotifierRunner? — NO
15. Does durable storage corruption fail the NotifierRunner? — YES
16. Does failed recovery clean up notifier and supervision? — YES
17. Does NotifierRunner shutdown revoke Lease? — NO
18. Does notifier have durable IN_FLIGHT authority? — NO
19. Does outbox persist RootBridge/vendor identity? — NO
20. Does M5.5 require a schema bump? — NO
21. Is normal Root wakeup still BATCH_RESULTS_READY rather than per-Result? — YES
22. Does M5.5 introduce any M6 semantic object? — NO
23. Can a caller-built delivery candidate forge wakeup content for a real event_id? — NO (storage-produced, private fields)
24. Can RecoveredRuntime discard notifier while retaining supervision? — NO (`into_runner` removed)

---

## 17. PR #11 audit closure

First review: REQUEST CHANGES on two public-API escape hatches. Internal
state machine stayed frozen.

**P1-1 closed.** `OutboxDeliveryCandidate` is opaque. Regression:
`malformed_index_is_fatal_not_bridge_failure` writes a real durable
malformed `payload_json` then `due_outbox` → `deliver_one`; envelope
fatal, row stays PENDING. Positive path:
`successful_bridge_marks_delivered_and_does_not_ack` asserts candidate
getters match the durable row before delivery.

**P1-2 closed.** `RecoveredRuntime::into_runner()` deleted. Production
recovery with `NotifierBinding::Enabled` has no consume-supervision-only
API.

**P2-1 closed.** `NotifierService::kernel()` removed; runner uses
`NotifierService::now()`.

**P2-2 recorded, not implemented.** Bounded `last_error` only; sanitization
vocabulary waits for the first real RootBridge.
