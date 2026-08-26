# 02 — Authority and Correctness

Status: Normative
Canonical path: docs/specs/v0.2/02-authority-and-correctness.md

## Split that MUST NOT collapse

Root owns **semantic frontier admission** and **semantic integration**.
Scheduler owns **mechanical execution authority**.
Neither authority MAY be implicitly delegated to workers, compilers,
adapters, SpawnSources, RootBridge, or terminal UX.

Creating a Task MUST NOT grant execution authority.
Claim MUST atomically create Attempt + Lease + fencing epoch.
That claim is where execution authority is established.

## Authority matrix

| Concern | Root | Scheduler | Worker | Compiler | Adapter | SpawnSource | Terminal UX |
|---|---|---|---|---|---|---|---|
| Frontier admission | MUST | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Semantic integration | MUST | MUST NOT invent | evidence only | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Type/transform/topology *intent* | MAY | records/enforces | MUST NOT | MAY propose | MUST NOT | MUST NOT | MUST NOT |
| Materialize Task | MAY request | MUST persist | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Claim / Attempt / Lease | MUST NOT | MUST | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Task/Result state | MUST NOT mutate | MUST | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Retry / suspend / recover | MUST NOT | MUST | MUST NOT | MUST NOT | observe only | MUST NOT | MUST NOT |
| Revival mechanics | MUST NOT | MUST | MUST NOT | MUST NOT | continuity only | MAY host | MUST NOT |
| Physical start/observe/stop | MUST NOT | requests | runs | MUST NOT | MUST | MUST NOT | MUST NOT |
| Sandbox enforcement | intent | eligibility | MUST NOT bypass | MUST NOT | MUST | capability ceiling | MUST NOT |
| Compile intent | MAY trigger | records | MUST NOT | MUST | MUST NOT | MUST NOT | MUST NOT |
| Admit proposal | MUST | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Result ACK | MAY consume | records | MUST NOT | MUST NOT | MUST NOT | MUST NOT | MUST NOT |
| Notification delivery | receives wakeup | owns outbox | MUST NOT | MUST NOT | N/A | N/A | MUST NOT |

Root MUST NOT: claim Tasks, renew Leases, control retries, manage Incarnations,
manually revive LogicalAgents, treat a notification as Result payload, or treat
mechanical recovery as a new Generation.

Worker MUST NOT: create an executable Task, expand the frontier, promote its
own semantic delta into canonical MemoryCapsule, or bypass Attempt/Lease.

Compiler MUST NOT: admit work, recursively expand work by default, or possess a
privileged lifecycle.

Adapter MUST NOT: define Core scheduling semantics or grant Task authority from
physical observations alone.

SpawnSource MUST NOT: define AgentType identity.

TerminalExperienceAdapter MUST NOT: be required for correctness.

## Kernel correctness (UNCHANGED from V0.1)

- Execution is at-least-once. Exactly-once is NOT promised.
- Every authority-bearing completion, failure, or checkpoint MUST be fenced by
  current Attempt, ACTIVE unexpired Lease, matching fencing epoch, and
  `task.current_attempt_id`.
- Stale executions MAY refine their own physical history. They MUST NOT mutate
  current Task/Result authority.
- Task success and authoritative Result creation MUST be one atomic boundary.
  Exactly one authoritative Result per completed Task.
- Root Result ACK is consumption, not worker completion.
- Lease expiry MUST NOT prove a writer stopped.
- Unsafe duplicate writers MUST NOT be dispatched.
- Scheduler is sole claim/state authority. Generation, AgentType, compiler,
  RootBridge, and UX MUST NOT bypass Task/Attempt/Lease/Result.
- Scheduler MUST NOT invent semantic recovery (MUST NOT change model, role,
  affinity, or acceptance on its own).

## Failure taxonomy

Mechanical classes MUST remain available to adapters:

`TRANSIENT_EXTERNAL`, `TIMEOUT`, `EXECUTION_LOST`, `START_FAILURE`,
`RESOURCE_UNAVAILABLE`, `PERMISSION_FAILURE`, `INVALID_RESULT`,
`ADAPTER_PROTOCOL_FAILURE`, `UNKNOWN`.

Implementations MUST distinguish at least: mechanical failure, semantic
failure (Root/policy), invalid authority, stale authority, invariant
violation, configuration incompatibility, unrecoverable continuity failure.

Only Task-named retry classes MAY auto-retry. Exhausted or unclassified
mechanical failure MUST suspend and escalate.
