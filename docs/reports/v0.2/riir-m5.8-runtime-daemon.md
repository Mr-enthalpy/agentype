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
| B | `RuntimeProcessLock` + `ReadyPermit` + file-store identity |
| C | `PhysicalObserverService` freshness gate + activation sweep |
| D | `ControlLoopService` / `ControlLoopRunner` behind `ReadyPermit` |
| E | `SchedulerDaemonBuilder` lifecycle, observer runner, shutdown |
| F | RootBridge bounded diagnostics; LocalProcess live acceptance; architecture |

SCHEMA_VERSION remains 4.

## Ownership graph

```text
SchedulerDaemon
  RuntimeProcessGuard          (stable lock dir, not temp_dir)
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

## Diagnostic contract

`RootBridgeDiagnostic` enforces max length and redacts `Authorization:`
lines. It does not claim full secret sanitization. Prefix matching uses
`str::get`, so ordinary UTF-8 messages must not panic.

---

## RootBridge diagnostics

`last_error` is `KIND: bounded-diagnostic`. Notifier does not persist
`format!("{RootBridgeError}")`. `Authorization:` lines are redacted.

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
