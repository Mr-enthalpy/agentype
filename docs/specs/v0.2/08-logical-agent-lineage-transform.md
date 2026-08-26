# 08 — LogicalAgent, Lineage, and Transform

Status: Normative
Canonical path: docs/specs/v0.2/08-logical-agent-lineage-transform.md

## LogicalAgent (kernel machine UNCHANGED)

| From | Operation | To |
|---|---|---|
| INITIALIZING | birth ready | READY |
| INITIALIZING | excess unassigned / policy retire | RETIRED |
| READY | claim assignment | ASSIGNED |
| READY | excess unassigned / policy retire | RETIRED |
| ASSIGNED | task boundary | READY or RETIRED |
| READY/ASSIGNED | physical loss requiring continuity | REVIVING |
| REVIVING | replacement incarnation ready | READY or ASSIGNED |
| REVIVING | excess unassigned / policy retire | RETIRED |
| ASSIGNED | topology drain | DRAINING |
| DRAINING | assignment boundary | RETIRED |
| non-RETIRED | policy/manual / writer-safety suspension | SUSPENDED |
| SUSPENDED | safety resolved, resident destination | REVIVING |
| SUSPENDED | safety resolved, ephemeral destination | RETIRED |

RETIRED is terminal and MUST NOT revive.
READY MUST NOT imply physical presence.

Excess unassigned INITIALIZING / READY / REVIVING members MUST retire
directly. Only ASSIGNED members enter DRAINING and apply a pending
transition at their assignment boundary.

Any committed transition of a LogicalAgent to RETIRED MUST fence every
STARTING/WARM/COLD Incarnation of that LogicalAgent to LOST in the **same**
semantic-retirement transaction, before RETIRED is observable. This applies
to excess unassigned retirement (which is **not** a topology cutover),
assignment-boundary retirement, and Transform source retirement. A reusable
physical presence MUST NOT remain scheduler-authoritative after semantic
death.

V0.2 Transform retirement is the same terminal RETIRED after successful
cutover of a successor, and MUST apply the same Incarnation fencing.

## Incarnation (kernel UNCHANGED)

One physical embodiment, not an Execution wrapper.
A resident Incarnation MAY host sequential Executions, never more than one
active Execution.

| From | Operation | To |
|---|---|---|
| (none) | physical start requested | STARTING |
| STARTING | adapter confirms live presence | WARM |
| STARTING/WARM | safely detached presence (reserved) | COLD |
| STARTING/WARM/COLD | confirmed closed | TERMINATED |
| STARTING/WARM/COLD | lost / unconfirmed / topology cutover fence | LOST |
| STARTING/WARM/COLD | owning LogicalAgent committed RETIRED | LOST |
| LOST | late confirmed closed | TERMINATED |
| LOST | still lost | LOST |
| TERMINATED | (terminal; no reopen as live) | TERMINATED |

`STARTING`/`WARM`/`COLD` denote potentially live physical presence.
Late stale outcomes MAY refine `LOST` → `TERMINATED` on **that** Incarnation
and MUST NOT alter Task, Result, checkpoint, or replacement Incarnation
authority. Physical refine MUST NOT return a fenced Incarnation to
STARTING/WARM/COLD.

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
| TARGET_READY | atomic cutover (below) | COMPLETED |
| REQUESTED/QUIESCING/REFINING_CONTEXT/TARGET_READY | policy/safety stop | SUSPENDED |
| non-COMPLETED | cancel | CANCELLED |
| SUSPENDED | resume | prior non-terminal |

`COMPLETED` and `CANCELLED` are terminal.

`CUTTING_OVER` is an **operation / internal phase**, not a durable semantic
state in which topology has already switched while the source LogicalAgent
remains schedulable.

Crash-visible states around cutover MUST be only `TARGET_READY` or
`COMPLETED`. `COMPLETED` means successor created, lineage linked, topology
cut over, and source RETIRED — one atomic transaction
([13](13-storage-and-transactions.md)).

A durable `CUTTING_OVER` in which both source and successor can be claimed
MUST NOT exist. RIIR MUST NOT invent split-brain scheduling to fill that gap.

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
