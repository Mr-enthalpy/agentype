# RIIR M5.8 — Production Runtime Daemon, Physical Observation, Composition Closure

Status: Implementation Report
Applies to: branch `rust/m5.8-runtime-daemon` (base: main @ M5.7 merge `55a0b6d`)
Canonical path: `docs/reports/v0.2/riir-m5.8-runtime-daemon.md`
Not a specification. M5.6 remains frozen as amended. M5.7 remains frozen.

---

## Mission

One Scheduler store has exactly one production daemon. It recovers before
dispatch, distinguishes Lease supervision from physical observation, stops
renewal when physical freshness is stale, and fails/shuts down without
manufacturing Task, Result, or quiescence semantics.

---

## Slices

| Slice | What landed |
| --- | --- |
| A | `AuthorityConsequence`; preserving-nack; worker DTOs in `worker_protocol_v01` |
| B | `RuntimeProcessLock` + `ReadyPermit` + handle file identity |
| C | `PhysicalObserverService` freshness gate + activation sweep |
| D | `ControlLoopService` / `ControlLoopRunner` behind `ReadyPermit` |
| E | `SchedulerDaemonBuilder` lifecycle, observer runner, shutdown |
| F | RootBridge bounded diagnostics; LocalProcess live acceptance; architecture |
| G | Narrow host control surface (`SchedulerControl`); composition closure |

SCHEMA_VERSION remains 4.

File identity is read from the open handle. Unix flocks that inode.
Windows locks `.agentype-runtime-lock-<file id>` under the ProgramData
known folder, so a cross-session rename cannot split the singleton.
The dispatch permit is taken before `claim_next_available`; a gate that
is already stopping or failed does not create an Attempt.

The lock and SQLite must resolve the same file from the same filename, so the
production store is a literal file-backed filesystem path only. One shared
classifier (`classify_store_path`, applied by both `SqliteRuntimeConfig` and
`Store::open`) refuses three SQLite special filenames plus one Agentype
store-identity restriction:

```text
SQLite special filenames
    file:...   URI filename — the lock would hold ./file:scheduler.sqlite
               while SQLite opened ./scheduler.sqlite, so two daemons could
               each hold a lock and share one store; special only while URI
               processing is enabled
    :memory:   private in-memory database (bare spelling only); special
               independently of URI processing
    ""         private temporary database; special independently of URI
               processing

Agentype store-identity restriction
    non-UTF-8  not a SQLite special filename: the store identity must be
               UTF-8 representable, and the byte-level `file:` test exists so
               such a name cannot hide a URI prefix
```

`:memory:` and the empty filename are special even with URI processing
entirely off, which is why the contract is a positive check on the kind
rather than a list of URI rejections. The `file:` test runs on the filename
*bytes* SQLite receives — on Unix rusqlite passes `OsStrExt::as_bytes()`
through unchanged and SQLite's URI test is a raw `memcmp` against
`b"file:"`, so a prefix hidden inside a non-UTF-8 filename is still caught —
while everything else about the contract is UTF-8 representable. A non-UTF-8
filename that is *not* a URI is therefore refused as `UnsupportedEncoding`
rather than accepted: the process-lock identity stores its canonical pathname
as a lossy UTF-8 string and rebuilds the sidecar lock path from it, so
arbitrary Unix byte paths are not a losslessly representable Scheduler
identity. On Windows rusqlite requires UTF-8 and fails with `InvalidPath`, so
both platforms reject the same inputs. Bare `:memory:` is special only with
no additional text: `:memory:?cache=shared` is an ordinary filename, and the
shared-cache spelling is `file::memory:?cache=shared`. The load-bearing guard
is that Agentype rejects every `file:` filename *before* SQLite is asked to
open anything; the connection additionally does not request
`SQLITE_OPEN_URI`, which is not by itself sufficient because SQLite can also
enable URI handling through `SQLITE_CONFIG_URI` or compile-time
`SQLITE_USE_URI`. `Store::open_memory` / `Kernel::open_memory` remain the
explicit ephemeral path.

The startup path `RuntimeProcessLock(path)` → `Kernel::open(path)` →
`confirm_store_identity(path)` assumes the Scheduler database pathname is not
adversarially replaced during startup: the confirmation re-opens the path
rather than reading the identity of the file the live connection holds, so a
pathname ABA in that window is not detected. That is the same assumption the
process singleton already rests on.

Regressions: `uri_filenames_are_not_a_production_store`,
`literal_filesystem_paths_stay_accepted`, `memory_path_fails_closed`,
`temporary_database_path_fails_closed`,
`config_and_store_share_one_classification`,
`special_store_filenames_fail_closed_before_any_runtime_action`,
`classification_table_names_every_sqlite_special_filename`,
`literal_path_stays_file_backed_and_reopenable`,
`sqlite_special_filenames_are_refused_by_the_store_boundary`,
`explicit_memory_store_still_works`, and the Unix-only
`non_utf8_filenames_are_classified_by_bytes` plus
`non_utf8_store_path_fails_closed` and
`non_utf8_literal_is_not_a_literal_store_kind`, which pin the byte-level
`file:` test, the `UnsupportedEncoding` refusal, and that the config boundary
and the store classifier agree on it.

## Ownership graph

```text
SchedulerDaemon
  RuntimeProcessGuard          (handle identity; Windows ProgramData lock, not Local\)
  Arc<Kernel>
  DispatchGate                 (revoked on shutdown and first runner fatal)
  SupervisionRunner            (lease renewal only)
  PhysicalObserverRunner       (freshness + physical watch)
  ControlLoopRunner            (maintenance + gated dispatch)
  NotifierRunner?              (optional)
  daemon-health watchdog       (auto-stop; poll_health is diagnostic only)
```

READY is minted only after activation, a freshness re-check of remaining
current executions, and required runner health.

Shutdown revokes DispatchGate before claim/start, then joins workers and
summarizes fatal after those joins.

## Host control surface

`RunningSchedulerDaemon::control` lends a `SchedulerControl`, borrowed from
the daemon so it cannot outlive the process lock and carrying no public
constructor so it cannot be forged. It replaces the raw `Kernel` getter as
the production path into Scheduler authority; `daemon.kernel()` is
test-support only.

Reachable: submit Batch, cancel Task/Batch, pool topology, Result ACK, Outbox
ACK, diagnostics.

Unreachable by construction: Worker acknowledgement (`ack_success`, `nack`,
`nack_preserving_physical_history`), Runtime-owned mechanics (claim,
execution commit, physical outcome, lease expiry, retry promotion, recovery,
revival, renewal, outbox delivery pipeline), and the `cancel_task`
`quiescence_confirmed` override.

`resolve_escalation` is left out deliberately and is not claimed as closed:
it selects a recovery primitive, so it belongs to the Escalation milestone.

Four `compile_fail` doctests pin the absences, and one of them is backed by a
probe run confirming the referenced externs resolve, so each failure is
about the missing API rather than a missing crate.

## READY publication barrier

`ReadyPermit` gates *construction* of the control loop; it does not gate
*action*. Three things must hold before a physical start is dispatched —
`ReadyPermit` (the loop may exist), `ReadyRelease` (the worker may leave its
paused state), and `DispatchGate::Ready` (the gate admits a start) — so the
permit must not be read as "READY has been published" (M5.8 audit round 5
P2-1). M5.8 audit round 3 found that the observer runner started before
`try_commit_ready`, so a steady-state observation could stop renewal
eligibility between the final freshness check and the gate flip — READY would
be published with the invariant already false.

Two additions close it:

- `ReadyRelease` — a publication boundary. The observer and control runners
  start paused behind it and do no work until READY is published.
- `SupervisionService::with_readiness_barrier` — holds the registry lock
  across the readiness decision, so no observation-driven or heartbeat-driven
  change to eligibility or the renewal schedule can interleave with it.

The composition root now validates and flips the gate inside that barrier and
releases the workers there, strictly after the flip. Runner health is read
before the barrier because reading a runner's state lock under the registry
lock would invert the state → registry order that `refresh_positive`
establishes; it is also redundant inside, since every runner fatal publishes
to the gate synchronously.

Regressions: `observer_does_not_observe_before_ready_is_published` and
`readiness_barrier_excludes_freshness_mutation`.

## Global fatal closure

The composition-closure gap is closed rather than deferred. Component fatal
tests are not composition closure, so `observer_fatal_stops_dispatch_renewal_and_releases_the_lock`
drives a real observer fatal through a running daemon — a panicking test
adapter, no fault-injection framework — and checks the whole chain:

```text
observer fatal → daemon phase Failed
               → the supervised Lease stops being renewed
               → no new Attempt and no physical start for later work
               → join() == DaemonExit::Failed
               → the process lock is released
               → a fresh daemon takes the store and reaches READY
```

That last step is the restart barrier the M5.8 frozen phrases require: the
only safe recovery from a failed Runtime component is a new Runtime lifecycle.

### Verification boundary

The daemon and process-lock tests need a machine-writable ProgramData lock
and writable temporary directories. In the environment where this change was
written, confined child processes may write only inside the workspace, so
those targets could not be executed there; every test that does not need
those paths was run and passes. CI executes the full set on Ubuntu and
Windows.

## Freshness recovery closure

A stale freshness tick stops renewal and re-schedules the entry one heartbeat
interval later. If the observer then re-established freshness by only setting
`renewal_eligible = true`, the deadline stayed where the stale tick put it —
past the durable expiry under legal `heartbeat < lease` timing — so a healthy
Execution lost authority to the Scheduler's own schedule.

Recovery now has both halves, and both are required:

```text
positive RUNNING
    ↓
next_due_at = min(next_due_at, observed_at)     re-arm
    ↓
wake the heartbeat loop                          wake
    ↓
SupervisionRunner performs the fenced renewal
```

The observer reaches this through `SupervisionFreshnessSink`, which exposes
`refresh_positive` plus the observation read/stop/drop paths and no renewal
at all: `PhysicalObserver` remains not a Lease renewer. `refresh_positive`
takes the runner's scheduling mutex across the registry mutation and the
notification, which is what makes the wake unloseable — the loop computes its
wait from `earliest_next_due()` under the same mutex. A refresh may bring a
renewal forward, never push one later.

Three regressions cover it, and the two clock-exact ones were confirmed red
before the re-arm landed:

| Test | Asserts |
| --- | --- |
| `freshness_recovery_rearms_the_renewal_deadline` | stale tick does not renew and moves the deadline to 12; the refresh must re-arm it to ≤ 7; the heartbeat step then renews to 17 |
| `freshness_recovery_wakes_the_heartbeat_loop` | with the loop asleep toward the old deadline, a refresh wakes it and the renewal lands before the old expiry |
| `stale_freshness_during_recovery_still_reaches_activation` | after recovery + activation refresh, reaching READY leaves the entry renewable, not skipped as not-yet-due |

The wake test was confirmed to fail with the notification removed, so it
detects the second "deadline changed but the condvar never fired" bug rather
than passing for another reason.

The activation regression lives in the recovery suite and asserts only that a
positive observation restores **fresh renewable** supervision; it does not
drive a renewal step, because `RecoveredRuntime` already has a live heartbeat
loop that owns renewal, and a test that competed with it would be reading a
racing registry.

## Fatal coordinator closure

The daemon health watchdog is the global fatal coordinator, so it is now
inside the fatal contract it enforces. Its body runs under `catch_unwind`
with an exit guard: any exit that is not an orderly shutdown publishes the
runtime fatal — gate failed, daemon phase Failed, every worker asked to stop.
`join` reports a dead coordinator as `DaemonExit::Failed` independently of
that publication winning the race, so the exit status cannot be lost.

Without this, a coordinator that died while the notifier failed would leave
supervision and observation running and renewing, which contradicts
"structural runner failure stops the whole production Runtime".

Regression: `watchdog_death_is_a_runtime_fatal`, driven by a test-support
fault seam (`arm_watchdog_panic`).

The cross-process lock helper moved out of the production surface: the logic
now lives entirely in `src/bin/hold-process-lock.rs`, and
`hold_process_lock_until_stdin_closes` is no longer exported. It takes OS
ownership of a store, so nothing a production consumer should be able to
call.

### Production API boundary: closed

`agentype-storage-sqlite` is declared an internal implementation crate
(`publish = false`), and the mechanical half is now mechanical rather than
declarative. Every runtime-mechanical Kernel method — claim, execution
commitment, `confirm_running_and_renew`, physical-history recording, worker
ACK/NACK, `expire_leases`, `promote_retry_wait`, `recover_authority`,
revival, consumer birth, checkpoint promotion, and the outbox delivery
commits — is compiled only under
`#[cfg(any(test, feature = "runtime-internal"))]`. `agentype-runtime` is the
only crate that requests `runtime-internal`; the storage crate's own
integration targets request it explicitly through a self dev-dependency, and
the adapter acceptance tests that deliberately drive the Kernel request it as
well.

`agentype-runtime`'s default production surface was narrowed in the same
pass. `ControlLoopService`, `ControlLoopRunner`, `PhysicalObserverService`,
`PhysicalObserverRunner`, `DispatchGate`, `ReadyRelease`, `ReadyPermit`,
`SupervisionRegistry`, `SupervisionAdmitSink`, `RecoveredRuntime`, and
recovery's reconciliation entry points are `pub(crate)`;
`SupervisionRunner`, `SupervisionRegistry`, and the unlocked /
notifier-less recovery entry points are re-exported only under the
`test-support` gate. `ControlError` stays public because `DaemonError::Control`
carries it, and the storage crate is not re-exported at all, so a consumer
cannot name the mechanical Kernel handle through `agentype-runtime`.

`tests/public-api` is the compile-time witness: an external-consumer fixture
depending on `agentype-runtime` with default features, holding twelve
`compile_fail` rustdoc probes (one per boundary item) plus a `no_run` positive
probe for the supported
`SchedulerDaemonBuilder → RunningSchedulerDaemon → SchedulerControl`
composition. CI runs it as its own `cargo test -p
agentype-public-api-boundary` invocation and excludes it from the workspace
`--all-features` run, because `test-support` would defeat the probes.

The frozen statement is deliberately narrower than "unforgeable capability":
`SchedulerDaemon` is the sole supported production composition root, and
runtime-mechanical Kernel and supervision APIs belong to non-published internal
implementation surfaces that the supported production API does not expose. A
repository consumer that deliberately depends on the internal crate and
requests its feature can still bypass this — it is a package trust boundary,
not a capability sandbox. The capability-token alternative was considered and
rejected for M5.8.

## Composition closure follow-ups

`Kernel::attempt_count_for_task` is a test-support read that makes "no
Attempt was created" directly assertable; the gate regression now asserts
`attempt_count == 0` instead of inferring it from the next claim's number.

All six storage integration targets (`m4_kernel`, `supervision`,
`outbox_delivery`, `reconciliation`, `recovery`, `topology`) declare
`required-features = ["test-support"]`, because they drive the mechanical
Kernel surface directly. A default-feature `cargo test --workspace` now
compiles; the CI `--all-features` run still executes every target.

## Diagnostic contract

`RootBridgeDiagnostic` enforces max length and redacts `Authorization:`
lines. It does not claim full secret sanitization. Prefix matching uses
`str::get`, so ordinary UTF-8 messages must not panic.

---

## RootBridge diagnostics

Durable `last_error` stores only the safe RootBridge kind:
`UNAVAILABLE`, `DEADLINE_EXCEEDED`, `PROTOCOL`, `REJECTED`, or `OTHER`.
M5.8 does not persist a bounded diagnostic. `Authorization:` redaction
still applies to non-durable `RootBridgeDiagnostic` text.

---

## Live acceptance

LocalProcess + file-backed SQLite, `fake-agent` as the user executable.
Adapter collect is still `PhysicalExecutionOutcome`. Worker ACK in
acceptance B is a test-only `Kernel::ack_success` fixture.

---

## Intentionally not in this milestone

- M6 AgentType / SpawnSource / Generation
- Worker Result transport protocol
- Codex / vendor adapters
- Tightening the 300ms+2s real-I/O deadline test slack
