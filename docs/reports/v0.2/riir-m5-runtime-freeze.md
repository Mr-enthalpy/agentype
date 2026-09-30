# RIIR M5 — Runtime Substrate Freeze and M6 Handoff

Status: Historical Report
Applies to: V0.2 / M5 closure (merge `9cae0bc`)
Canonical path: `docs/reports/v0.2/riir-m5-runtime-freeze.md`
Not a specification.

This is a milestone boundary marker. It records that M5 is closed and frozen,
and that M6 semantic organization is the next active phase. It adds no feature,
supersedes no specification, and designs no M6 behavior.

## Status

```text
M5.1  authoritative launch/config basis      FROZEN
M5.2  dispatch commitment                    FROZEN
M5.3  supervision authority                  FROZEN
M5.4  restart reconciliation                 FROZEN
M5.5  notifier / RootBridge isolation        FROZEN
M5.6  Adapter invocation contract            FROZEN AS AMENDED
M5.7  external execution Adapter boundary    FROZEN
M5.8  production runtime daemon closure      FROZEN
```

M5 is closed.

The next implementation/design phase is M6.

The substrate M5 hands to M6:

```text
durable Task / Attempt / Lease / Result authority
LogicalAgent / Incarnation / Execution separation
authoritative launch configuration
dispatch commitment
supervision admission
heartbeat renewal
restart reconciliation
Root notification isolation
absolute Adapter invocation deadlines
external execution-environment Adapter boundary
production daemon singleton
recover-before-dispatch barrier
steady-state physical observation
physical-freshness-gated renewal
continuous mechanical maintenance
runtime fatal propagation
safe shutdown
```

## Frozen M5 boundary

M5 establishes a provider-neutral execution/runtime substrate.

The frozen ownership model is:

```text
Scheduler durable state
    owns Task / Attempt / Lease / Result authority

SchedulerDaemon
    owns one production Runtime lifecycle for one Scheduler store

SupervisionRunner
    owns Lease renewal

PhysicalObserver
    owns physical-observation freshness and physical facts

ExecutionAdapter
    owns external execution-environment mechanics

External environment
    owns agent implementation, harness, model/provider,
    credentials, prompts, tools, and internal memory

Notifier / RootBridge
    owns Root wakeup delivery
```

No layer may impersonate another.

## Frozen Adapter boundary

```text
ExecutionAdapter is NOT:
- a model adapter
- a provider adapter
- an agent-behavior adapter
- a prompt orchestrator
- Worker Result transport

ExecutionAdapter IS:
- create
- reconnect
- observe
- interrupt
- terminate
- collect physical execution-environment facts/artifacts
```

Physical environment end is not Task success.

Adapter timeout is not death.

Termination request is not quiescence proof.

## Frozen daemon boundary

```text
one production daemon per Scheduler store

process lock before recovery

recovery before production dispatch

positive physical observation gates continued renewal eligibility

PhysicalObserver never renews a Lease

SupervisionRunner is the only periodic Lease renewer

stale physical freshness stops renewal but does not manufacture
LOST / TERMINATED / quiescence

mechanical maintenance continues in steady state

structural Runtime failure fails the Runtime

Runtime shutdown does not cancel Tasks or kill external agents
```

## M5 correctness authority

M5 correctness remains defined by the existing V0.2 normative specs and
architecture documents. This report does not supersede them.

In particular:

```text
M5.6 deadline amendment remains authoritative for Adapter deadlines.

M5.7 execution-adapter boundary remains authoritative for physical-only
Adapter semantics.

M5.8 runtime-daemon architecture and specs remain authoritative for the
production composition root.
```

## M6 entry boundary

M6 may now assume the M5 runtime substrate exists.

M6 is allowed to work on semantic organization such as:

```text
AgentType
SpawnSource
Generation
RawWorkIntent
CompiledWorkProposal
Transform
MemoryCapsule
semantic continuity/refinement
```

But M6 MUST NOT casually modify M5 mechanical authority merely because a
semantic feature is easier to implement that way.

If M6 appears to require changing:

```text
Task/Attempt/Lease/Result authority

Lease fencing

writer safety

Adapter deadline algebra

ExecutionAdapter physical-only boundary

process singleton

recover-before-dispatch

physical freshness / renewal relationship

Root notification authority
```

treat that as an explicit cross-milestone architecture review.

Do not silently weaken M5.

## M6 semantic direction

The already-established conceptual separation, recorded here without designing
M6:

```text
M5 answers:

"How can externally implemented agent executions be scheduled,
supervised, recovered, and controlled safely?"


M6 answers:

"How should semantic agent roles, generations, creation sources,
transformations, proposals, and continuity be organized on top of
that safe runtime?"
```

M6 state machines are not specified here.

M6 schema is not added here.

M6 APIs are not invented here.

## Baseline

```text
M5.8 merge commit:
9cae0bc554441d9047459140a7847462cc0fcc28

SCHEMA_VERSION = 4 at M5 freeze.
```

The freeze report itself is the first commit after that baseline.
