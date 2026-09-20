# 07 — SpawnSource and Adapter Contract

Status: Normative
Canonical path: docs/specs/v0.2/07-spawn-source-and-adapter-contract.md

## SpawnSource

SpawnSource is a physical provisioning source. It sits above V0.1
ExecutionTarget × ExecutionProfile (those remain adapter-bound).

It MUST NOT be AgentType identity or Core vendor semantics.

Suggested categories (representation IMPLEMENTATION-DEFINED): source_id,
adapter_ref, target_selector, profile_selector, provisionable capability
envelope, enforceable sandbox features, lifecycle modes, supported continuity
modes, source_tags, availability.

## Selection order (MUST)

1. correctness constraints
2. sandbox enforceability
3. AgentType compatibility (`can_provision`)
4. continuity value
5. availability
6. cost / resource policy

Cost MUST NOT override correctness or security eligibility.
A source that cannot enforce the required sandbox MUST be ineligible.

## ExecutionAdapter (correctness required)

Narrow interface, UNCHANGED from V0.1:

- `start_execution`
- `observe_execution`
- `interrupt_execution`
- `terminate_execution`
- `collect_outcome`
- `reconcile_start`

All adapter-controlled waits, retries, protocol stages, cleanup waits and
additional side-effect stages MUST obey the single absolute deadline.
Under the host-kernel progress assumption, the Scheduler-facing operation
MUST return within that deadline. An OS/kernel primitive that cannot be
interrupted by the process is outside the in-process liveness guarantee;
after such a primitive returns, an expired deadline MUST prohibit any new
blocking/side-effect stage except immediate allowed cleanup. A successful
Scheduler-facing return MUST qualify the same deadline after the last
evidence-producing operation; evidence obtained after the endpoint MUST
NOT become a successful observation. Cleanup
consumes remaining time; a depleted deadline MAY only kill or abandon
without a fresh wait budget. A helper thread, detached watchdog, or
fresh per-stage timeout MUST NOT be used to paper over an uninterruptible
kernel wait.

`StartObservation` MUST carry `terminal_confirmed` and `quiescent_confirmed`
(default false). Dispatcher MUST NOT derive those proofs from a
terminal-looking enum state.

`collect_outcome` reports **physical** environment end. It is not Task
Result authority and MUST NOT carry agent `{ok,payload,summary}` as a
Scheduler Result. ACK/NACK of Task Result is a separate Worker data
plane. A nonterminal collect MUST NOT inherit terminal/quiescence proof
from earlier `reconcile_start`.

After an adapter call returns, Runtime MUST re-qualify the same deadline
before admitting the evidence. Evidence obtained after the endpoint MUST
NOT become a successful observation; the error is `DeadlineExceeded`.

Runtime locators (thread id, session id, turn id) MUST be opaque handles on
Incarnation/Execution. Core MUST NOT interpret vendor enums.

Process death is not quiescence proof.

Adapter absolute deadlines are **M5** runtime conformance (the interface
itself is required for M4 observation vocabulary).

## Physical adapter identity and imported source (**M5.7**)

`AdapterKind` is the driver family (for example `local_process`). It is
not a vendor, model, or installation name.

`AdapterBindingKey` is an opaque concrete physical execution domain
(host/boot/namespace/root fingerprint). Core MUST NOT interpret it.
The key MUST distinguish every RuntimeHandle locator the adapter will
re-interpret after restart, including filesystem-root identity when
handles carry path locators.

An Execution MUST atomically freeze `adapter_kind` and
`adapter_binding_key` at creation. Recovery MUST `resolve_exact(kind, key)`
and MUST NOT fall back to another source of the same kind.

Until M6 SpawnSource exists, launch MUST `resolve_unique(kind)`. Ambiguous
installations of the same kind MUST fail closed.

An imported source owns its kind, binding key, and enforceable physical
capabilities. Effective safety is the intersection of the ExecutionTarget
requirement and the imported source's enforceability. A composition caller
MUST NOT mint durable isolation or a binding key without that intersection.

## ExecutionProfile registry (**M5**)

An Execution profile registry supplied by the composition root is
authoritative, including when it is empty. A persisted profile absent from
that registry MUST be `RESOURCE_UNAVAILABLE`. It MUST NEVER silently fall
back to adapter defaults. `None` is reserved for direct callers that
intentionally provide no registry.

## TerminalExperienceAdapter (optional)

MAY display child agents, conversations, status, workstreams.
MUST NOT be required for Core correctness.
MUST NOT grant claim, Result, or frontier authority.

## RootBridge

Independent of worker execution. See [03](03-task-attempt-lease-result.md)
outbox rules. Bridge MUST NOT `session/new` or otherwise own Root identity
when the transport has a load/resume primitive. Notifications MUST NOT
include Result payload.
