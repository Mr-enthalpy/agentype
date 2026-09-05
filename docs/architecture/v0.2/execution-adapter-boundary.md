# Execution Adapter Boundary

Status: Architecture
Applies to: V0.2 / M5.7
Canonical path: `docs/architecture/v0.2/execution-adapter-boundary.md`
Not a specification. Normative adapter interface remains
[spec 07](../../specs/v0.2/07-spawn-source-and-adapter-contract.md).
Deadline algebra remains
[M5.6](../../reports/v0.2/riir-m5.6-adapter-deadline-contract.md).

---

## 1. Adapter definition

An Execution Adapter is the Scheduler-facing owner of **one physical
execution environment**. It is not a model client, not a provider router,
and not an agent.

```text
Scheduler
    owns: Task, LogicalAgent, Execution lifecycle, Lease, Retry,
          Recovery, Authority
        |
        v
Execution Adapter
    owns: create, reconnect, observe, interrupt, terminate, collect
        |
        v
External Agent Environment
    owns: agent runtime, harness, tools, model/provider, credentials,
          network, prompt/system context, internal memory
```

The frozen contract is `ExecutionAdapter` in `agentype-adapter-api`:

- `start_execution`
- `reconcile_start`
- `observe_execution`
- `collect_outcome`
- `interrupt_execution`
- `terminate_execution`

Every method receives one `AdapterDeadline`. No method may block
indefinitely, spawn hidden background work, bypass the deadline, or mutate
Scheduler state.

If adding a new model or provider requires changing Scheduler Core, this
boundary has failed.

---

## 2. Non-goals

M5.7 MUST NOT implement, and an Execution Adapter MUST NOT own:

- AgentType hierarchy
- SpawnSource registry
- Memory / checkpoint compression
- Transform, Move, Merge
- Provider routing and credential management
- Prompt orchestration (the worker protocol is derived by
  `RenderedWorkerPrompt` from the launch snapshot; the adapter only
  transports it)
- Transcript database, dashboard, conversation UI
- Codex CLI / OpenCode runtime integration
- Steady-state observer loop, watchdog thread, process lock
- Schema bump, `adapter_binding_key` column
- RootBridge `last_error` sanitization

Optional ergonomics (transcript viewer, terminal attach, debug) MAY exist
beside an adapter. They MUST NOT be required for Scheduler correctness. A
minimal adapter without UI remains fully valid.

---

## 3. Runtime boundary

| Layer | Owns | Must not own |
| --- | --- | --- |
| Scheduler Core | execution semantics, authority, retry, recovery | process/session protocol, model, credentials |
| Execution Adapter | physical create/connect/observe/control/collect | Task/Lease/Result authority, logical lifecycle |
| External environment | agent implementation, harness, tools, model | Scheduler state |

`LogicalAgent` has many `Execution`s. Each Execution is created by some
adapter. The adapter does not know that two Executions are the same logical
agent. Crash and replacement mint a new physical instance; identity
preservation across restart is `reconcile_start(request_id, persisted_handle)`,
never a second `start_execution`.

Process death is not quiescence. Heartbeat failure is not process death.
Adapter timeout is not TERMINATED, LOST, or writer-safety proof.

---

## 4. ExecutionSpec

There is no new Core type named `ExecutionSpec`. The Scheduler semantic
request is already `ExecutionRequest`, assembled exclusively from:

1. `ExecutionLaunchSnapshot` — durable Scheduler identities, workspace,
   payload, acceptance, continuity;
2. `ResolvedExecutionEnvironment` — authoritative `target_options` /
   `profile_options` / `profile_timeout_seconds`.

The request MUST NOT carry:

```text
model / provider / api_key / caller-supplied prompt text
```

Those belong to user-owned external environment configuration, which the
adapter may see only as opaque `target_options` JSON (for example
`command`, `args`, `cwd`, `env` for a local process). Extra keys such as
`model` or `api_key` are ignored; they are not Scheduler fields.

`ExecutionProfile.timeout_seconds` remains execution/profile configuration.
It MUST NOT become any Scheduler-facing operation deadline. Operation
latency bounds come exclusively from the installed `AdapterDeadlinePolicy`.

---

## 5. RuntimeHandle semantics

`RuntimeHandle` is opaque JSON physical evidence. Scheduler may persist it,
compare it, and pass it back. Scheduler Core MUST NOT interpret vendor
fields.

Allowed contents (adapter-private): process id, session id, container id,
stdout/stderr paths, adapter kind, a copy of `request_id` used only as
reconcile identity check.

It is not Execution identity, LogicalAgent identity, or Task identity.
A handle hint on `AdapterError` is locator history only: not RUNNING, not
terminal, not quiescent, not Task authority.

Reference adapter (`local_process`) handle:

```json
{
  "v": 1,
  "kind": "local_process",
  "pid": 1234,
  "request_id": "<RequestId>",
  "stdout": "<path>",
  "stderr": "<path>"
}
```

`adapter_kind = local_process` uniquely identifies this host's process table
as spawned by this runtime. Multi-host or multi-installation domains (future
Codex) still need an opaque `adapter_binding_key` frozen at Execution
creation (BLOCKS_REAL_ADAPTER_PARITY; not added in M5.7, no schema bump).

---

## 6. Capability model

Required:

```text
ExecutionControlCapability  = the six ExecutionAdapter methods
```

Optional, non-correctness:

```text
SessionInspectionCapability
TerminalAttachmentCapability
TranscriptCapability
DebugCapability
```

The reference adapter implements only the required lifecycle contract.

---

## 7. Failure model

Adapter failure is not Scheduler failure.

| `AdapterErrorKind` | Scheduler mechanical class |
| --- | --- |
| `DeadlineExceeded` | `TIMEOUT` |
| `Unavailable` | `RESOURCE_UNAVAILABLE` |
| `Protocol` | `ADAPTER_PROTOCOL_FAILURE` |
| `Other` | `UNKNOWN` (not `START_FAILURE`) |

Diagnostics are bounded (512 chars) and MUST be sanitized by the adapter
(secrets, tokens, Authorization, env, worker payload, full provider bodies).
The type enforces length only.

`WRITER_QUIESCENCE_UNKNOWN` is Scheduler-owned. An adapter that surfaces it
from external JSON is treated as protocol / mechanical `START_FAILURE`; it
cannot mint writer-safety.

Timeout, kill-sent, and process-not-running prove nothing about Task
cancellation, writer safety, or quiescence. Successful validated
`collect_outcome` may produce execution completion evidence
(`terminal_confirmed`); this reference adapter never sets
`quiescent_confirmed`.

---

## 8. Deadline inheritance from M5.6

Unchanged:

- one absolute monotonic `AdapterDeadline` per Scheduler-facing call;
- `now == expires_at` is expired; `remaining` saturates at zero; no
  extend/reset;
- every internal stage and exception cleanup derives from the same endpoint;
- depleted deadline may only kill or abandon, never open a fresh wait;
- production invocation is `ResolvedAdapterBinding` (M5.6 façade);
- this crate does not depend on runtime; composition is M5.8.

The reference adapter:

- rejects already-expired calls on all six methods before I/O;
- writes stdin on a helper thread with `recv_timeout(remaining)`;
- waits for child exit in `WAIT_SLICE` bounded by `remaining`;
- on start stdin timeout, persists the partial handle as
  `runtime_handle_hint` then returns `DeadlineExceeded`;
- on collect timeout, best-effort kill then `DeadlineExceeded` (not
  TERMINATED);
- on terminate wait exhaustion, `DeadlineExceeded` with hint
  ("kill sent is not quiescence").

---

## 9. Reference adapter design

Crate: `agentype-adapter-local-process`.
Type: `LocalProcessAgentAdapter`.
Kind: `local_process`.

User-owned executable from `target_options.command` / `args` / `cwd` /
`env`. The crate ships `fake-agent` only as a scriptable external
environment for conformance (behavior selected by `FAKE_AGENT_*` env passed
**per child**, never process-wide `set_var`).

| Operation | Mechanics |
| --- | --- |
| start | spawn + bounded stdin write of the Scheduler worker protocol + one `try_wait`. RUNNING if still alive; UNKNOWN+ambiguous if it already exited. Does not wait the remaining budget for agent completion. |
| observe | live `Child` or OS pid liveness. Alive → RUNNING; not alive → UNKNOWN. Never SUCCEEDED. |
| interrupt | best-effort observe. Not Task cancellation. |
| terminate | `kill` + wait remaining. Confirmed exit → TERMINATED with `terminal_confirmed=false`, `quiescent_confirmed=false`. No live `Child` → observe only; do not invent TERMINATED. |
| collect | wait remaining for exit, parse stdout JSON `{ok, payload, summary, failure_class}`. Process death is not quiescence. |
| reconcile | reconnect persisted handle + `request_id`. No handle → ambiguous UNKNOWN, not a new start. |

Windows pid liveness: `OpenProcess` + `GetExitCodeProcess(STILL_ACTIVE)`.
Spawn uses `CREATE_NO_WINDOW`. Drop kills leftover live children.

---

## 10. Future Codex / OpenCode integration

Codex CLI Adapter and OpenCode Runtime Adapter are the same **kind** of
object as `LocalProcessAgentAdapter`: they create and control an externally
owned environment. They are not model adapters.

A future Codex adapter SHOULD:

- speak the same six methods and `AdapterDeadline`;
- store an opaque session/thread locator as `RuntimeHandle`;
- treat CLI/auth/sandbox/transcript as environment concerns;
- not teach Scheduler about providers, models, or conversation UI.

It SHOULD NOT land until this local-process adapter remains replaceable
without Core changes. If Codex integration forces Scheduler to learn
session semantics, the boundary is wrong — fix the adapter, not Core.

`adapter_binding_key` remains hung for the case where `adapter_kind`
(`codex`) can name different hosts or installations.
