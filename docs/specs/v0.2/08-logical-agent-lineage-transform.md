# 08 — LogicalAgent, Lineage, and Transform

Status: Normative
Canonical path: docs/specs/v0.2/08-logical-agent-lineage-transform.md

## LogicalAgent (kernel machine UNCHANGED)

| From | Operation | To |
|---|---|---|
| INITIALIZING | birth ready | READY |
| READY | claim assignment | ASSIGNED |
| ASSIGNED | task boundary | READY or RETIRED |
| READY/ASSIGNED | physical loss requiring continuity | REVIVING |
| REVIVING | replacement incarnation ready | READY or ASSIGNED |
| ASSIGNED | topology drain | DRAINING |
| DRAINING | assignment boundary | RETIRED |
| non-RETIRED | policy/manual suspension | SUSPENDED |

RETIRED is terminal and MUST NOT revive.
READY MUST NOT imply physical presence.

V0.2 Transform retirement is the same terminal RETIRED after successful
cutover of a successor.

## Incarnation (kernel UNCHANGED)

One physical embodiment, not an Execution wrapper.
A resident Incarnation MAY host sequential Executions, never more than one
active Execution.
`STARTING`/`WARM`/`COLD` = potential live presence; `TERMINATED` = confirmed
closed; `LOST` = unconfirmed/fenced.
Late stale outcomes MAY refine their own Incarnation `LOST` → `TERMINATED`
and MUST NOT alter Task/Result/checkpoint/replacement authority.

## AgentLineage

Continuity across semantic successors. Transform successor MUST share the
source lineage. MOVE/MERGE MUST NOT create a new lineage.

## Transform

Intentional semantic transition. MUST NOT be implemented as MOVE.

MUST NOT mutate LogicalAgent AgentType identity in place.

MUST create successor LogicalAgent B, type Y, memory M2, `B supersedes A`,
same AgentLineage. A becomes RETIRED after successful cutover.

### States

| From | Operation | To |
|---|---|---|
| (none) | request | REQUESTED |
| REQUESTED | begin quiesce | QUIESCING |
| QUIESCING | source safe + inputs frozen | REFINING_CONTEXT |
| REFINING_CONTEXT | refinement Task succeeded + type valid | TARGET_READY |
| TARGET_READY | successor created + topology cutover | CUTTING_OVER |
| CUTTING_OVER | source retired | COMPLETED |
| non-terminal | policy/safety stop | SUSPENDED |
| non-COMPLETED | cancel | CANCELLED |
| SUSPENDED | resume | prior non-terminal | 

`COMPLETED` and `CANCELLED` are terminal.

Quiesce MUST block new claims, finish active assignment at a safe boundary,
and preserve writer safety.

Context refinement MUST be an ordinary scheduled Task (Attempt/Lease/Result/
retry/suspend/escalation). Scheduler MUST NOT secretly invoke an untracked
memory-management LLM.

Type garbage collection is separate from Transform and MUST NOT delete a
type still referenced by agents, partitions, tasks, transforms, or history.

Rollback, cancellation-before-cutover, partial target preparation, and
lineage undo are DEFERRED (D-TRANSFORM-FAIL). Until specified,
implementations MUST NOT guess a silent rollback that mutates identity.
