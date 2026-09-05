# RIIR M5.7 — External Execution Adapter Conformance Layer

Status: Historical Report
Applies to: branch `rust/m5.7-local-process-adapter` (base: main @ M5.6 merge `bff85d8`)
Canonical path: `docs/reports/v0.2/riir-m5.7-adapter-conformance.md`
Not a specification.

Despite the historical `riir-` directory naming, this milestone is **native
Rust runtime work**. It consumes the frozen M5.6 invocation contract and
proves that a real external execution environment can attach to it without
turning the Scheduler into a model client, provider router, or agent
framework.

Architecture: `docs/architecture/v0.2/execution-adapter-boundary.md`.

---

## 1. Mission

M5.6 froze how the Scheduler safely calls an external execution environment.
M5.7 proves a real environment can be called.

> Scheduler owns execution semantics.
> Adapter owns execution mechanics.
> External environment owns agent implementation.

Forbidden drift:

```text
Execution Adapter → Model Adapter → Agent Framework → Prompt/Memory/Tool Orchestrator
```

M5.7 therefore implements `LocalProcessAgentAdapter`, not Codex, not
DeepSeek, not an ArchitectureAgent.

---

## 2. What landed

| Slice | What |
| --- | --- |
| A | crate `agentype-adapter-local-process`, workspace member, `fake-agent` bin, `target_options.{command,args,cwd,env}` |
| B | `start_execution`: spawn, bounded stdin write of `RenderedWorkerPrompt`, opaque handle, partial locator on stdin timeout |
| C | `observe_execution` / `interrupt_execution` / `terminate_execution` |
| D | `collect_outcome` (stdout JSON; never quiescence) |
| E | `reconcile_start` by `request_id` + persisted handle; no handle → ambiguous UNKNOWN |
| F/G | blocked I/O + sanitization tests (M5.6 §51) |
| H | this report + architecture document |

No Core type named `ExecutionSpec` was added. `ExecutionRequest` is already
the physical creation request. No schema bump. No `adapter_binding_key`
column. No runtime/composition-root wiring (M5.8). No observer, watchdog,
Codex, or RootBridge change.

---

## 3. Reference adapter

`adapter_kind = local_process` names this host's process table. The
executable is user configuration, not a Scheduler concern.

Handle:

```json
{"v": 1, "kind": "local_process", "pid": N, "birth": T, "request_id": "...", "stdout": "...", "stderr": "..."}
```

`birth` is the process-instance token (Linux starttime / Windows creation
FILETIME). PID match without birth match is UNKNOWN, never RUNNING.

Start returns RUNNING after spawn + same-thread stdin write + one
`try_wait`, only if the start deadline still remains. It does not wait
the remaining start budget for the agent to finish; that wait belongs to
`collect_outcome`. A fast-exit during start is UNKNOWN + ambiguous, not
SUCCEEDED.

`quiescent_confirmed` is always false. Process death is UNKNOWN on
observe, never SUCCEEDED. Kill-sent is not quiescence.
`WRITER_QUIESCENCE_UNKNOWN` or unknown `failure_class` in agent JSON is
`AdapterError::Protocol`, not laundered `START_FAILURE`.

Stdin write is nonblocking/PIPE_NOWAIT on the calling thread. There is
no helper-thread watchdog. Drop forgets live `Child` objects: it does
not kill committed executions.

`FAKE_AGENT_*` is passed per child through `target_options.env`. Tests
never call process-wide `set_var`.

---

## 4. Deadline proof (M5.6 §51)

| Obligation | Proof |
| --- | --- |
| blocked process/session initialization | start does not wait for agent-ready; missing executable → `Unavailable`; expired start deadline rejected before spawn |
| blocked request write | 2 MiB unread stdin + hang → `DeadlineExceeded` + handle hint; write is same-thread PIPE_NOWAIT / `O_NONBLOCK` (no helper thread) |
| blocked response read | collect of `FAKE_AGENT_HANG` returns by the collect deadline |
| deadline between start stages | spawn, stdin write, and one `try_wait` share the start endpoint; recheck before RUNNING |
| deadline after partial locator | stdin timeout returns `runtime_handle_hint` |
| cleanup with remaining budget | uncommitted start stdin timeout kill + `try_wait` before return |
| cleanup with depleted budget | collect timeout is `DeadlineExceeded` without SUCCEEDED; no fresh wait |
| interrupt timeout | expired interrupt → `DeadlineExceeded`; live interrupt attempts SIGINT/CTRL_BREAK |
| terminate timeout | expired terminate → `DeadlineExceeded` and does **not** claim TERMINATED (process still RUNNING) |
| reconcile timeout | expired reconcile → `DeadlineExceeded` |
| collect timeout | hang collect → `DeadlineExceeded` with hint, not terminal proof |
| bounded stdout | oversize file → Protocol, not SUCCEEDED |
| diagnostic length | `AdapterDiagnostic` still caps 512 chars. This crate does not implement a secret scanner; unread stderr is not sanitization evidence. |

All six operations reject an already-expired `AdapterDeadline` before I/O.

---

## 5. Plan §16 test map

Creation: `start_creates_environment_and_returns_persisted_handle`
Observation: `observe_running_and_exited_environments`, `observe_unknown_handle_is_protocol`
Deadline: `expired_deadline_is_rejected_on_all_six_operations`, collect/start blocked-I/O tests
Control: `interrupt_attempts_physical_signal_or_reports_unsupported`, `terminate_kill_is_not_quiescence_or_task_cancel`, `terminate_timeout_does_not_imply_termination`
Collection: success / structured failure / malformed protocol / oversize / `WRITER_QUIESCENCE_UNKNOWN` Protocol
Restart: reconnect live handle; failed reconnect UNKNOWN; no handle is not a new start; mismatched `request_id` is Protocol; birth mismatch is UNKNOWN
Lifecycle: `adapter_drop_does_not_kill_committed_execution`

Boundary extras: opaque `target_options` keys are not Core fields; worker prompt is the V0.1 protocol, never caller text.

---

## 6. Explicit non-goals (still not done)

AgentType, SpawnSource, Memory, Transform, provider routing, credentials,
prompt management, transcript DB, dashboard, Codex/OpenCode adapter,
observer loop, watchdog, process lock, schema bump, `adapter_binding_key`
column, RootBridge `last_error` sanitization.

---

## 7. Hung P1s (must not be lost)

- **adapter_binding_key** — BLOCKS_REAL_ADAPTER_PARITY. M5.7 documents that
  `local_process` is unique for this host's process table, so the column is
  not added. Multi-host / multi-install Codex still needs an opaque
  provider-neutral key frozen at Execution creation. Core must not interpret
  it.
- **last_error sanitization** — BLOCKS_REAL_ROOT_BRIDGE (M5.5). Not M5.7.
- **M5.8** — composition root must mechanically run process lock →
  RECOVERING → `RecoveredRuntime` → enable Dispatcher → READY; one
  production `SupervisionRunner` owner per process; steady-state observer
  consuming these bounded adapter calls, separate from heartbeat.

---

## 8. Completion criteria

1. Scheduler can create an external execution environment — yes, via
   `start_execution`.
2. Observe — yes.
3. Control (interrupt/terminate) — yes.
4. Collect outcome — yes.
5. All operations obey M5.6 deadlines — yes.
6. Scheduler does not know model/provider — `ExecutionRequest` has no such
   fields; extra option keys are opaque.
7. Scheduler does not manage harness — the executable is user-owned.
8. Adapter does not own logical lifecycle — no Task/Lease/Result mutation.
9. Optional ergonomics do not affect correctness — none implemented.
10. Reference adapter can be replaced without changing Scheduler Core —
    Core and runtime do not depend on this crate.

---

## 8a. Audit closure (PR #13 REQUEST CHANGES)

Closed in-milestone, not deferred to M5.8:

- P1-1 detached stdin helper thread removed
- P1-2 `pid+birth` instance identity; empty `request_id` is Protocol
- P1-3 deadline recheck before positive evidence; bounded stdout
- P1-4 illegal/nonmechanical `failure_class` → Protocol
- P1-5 Drop forgets, does not kill; terminate by pid after Drop
- interrupt attempts a physical signal or returns Unavailable

## 9. Future Codex strategy

Implement Codex as another Execution Environment Adapter on this contract.
Do not teach Core session/thread/provider enums. If the CLI cannot honor
`AdapterDeadline` including cleanup, it is not a legal Agentype adapter.
