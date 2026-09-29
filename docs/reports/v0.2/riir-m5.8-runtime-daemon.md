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

## Composition closure follow-ups

`Kernel::attempt_count_for_task` is a test-support read that makes "no
Attempt was created" directly assertable; the gate regression now asserts
`attempt_count == 0` instead of inferring it from the next claim's number.

`m4_kernel` and `supervision` declare `required-features = ["test-support"]`,
because they exercise the legacy `heartbeat` / `renew_supervised_execution`
primitives that only exist under that feature. A default-feature
`cargo test --workspace` now compiles; the CI `--all-features` run still
executes both targets.

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
