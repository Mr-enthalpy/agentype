# 00 — Scope and Conformance

Status: Normative
Canonical path: docs/specs/v0.2/00-scope-and-conformance.md

## Scope

V0.2 specifies a single-machine, local-first Scheduler that:

1. Preserves the V0.1 correctness kernel (Task / Attempt / Lease / fencing /
   Result / Batch / writer safety / recovery / LogicalAgent / Incarnation /
   Execution / topology / outbox).
2. Adds a typed semantic work-generation layer (Generation, WorkIntent,
   AgentType, SpawnSource, Transform, MemoryCapsule, Root contract).

V0.2 is the first Rust implementation line. Python V0.1.2/0.1.3 is a behavior
oracle, not a module template.

## Non-goals

V0.2 MUST NOT expand into distributed consensus, a generic workflow DSL, a
vector-memory platform, a transcript database, an autonomous Scheduler LLM, a
provider credential pool, dashboard-first architecture, or hierarchical
manager/worker organizations.

## Conformance classes

A implementation claiming **V0.2 kernel conformance** (RIIR M4) MUST satisfy
[03](03-task-attempt-lease-result.md), [11](11-pool-topology.md) MOVE/MERGE/RETIRE
rules that already exist in V0.1, [13](13-storage-and-transactions.md) kernel
transactions, [14](14-recovery-and-reconciliation.md), and the V0.1 rows of
[16](16-conformance-tests.md).

A implementation claiming **V0.2 semantic conformance** (RIIR M6) MUST also
satisfy Generation, WorkIntent, AgentType, SpawnSource, Transform, memory, Root
contract, and the V0.2 rows of [16](16-conformance-tests.md).

A implementation claiming **frontend neutrality** (RIIR M7) MUST add a second
adapter without changing Core state machines.

## Staged implementation

RIIR MUST follow design milestone order: freeze spec (this directory) →
workspace/bootstrap → kernel parity → first-adapter runtime parity → semantic
layer → second adapter.

An implementer MUST NOT simultaneously change language, weaken the kernel, and
add semantic architecture.

## Conflict policy

If this spec conflicts with [docs/design/v0.2/12-normative-invariants.md](../../design/v0.2/12-normative-invariants.md),
the invariant wins and the conflict MUST be reviewed explicitly.

If this spec conflicts with [docs/specs/v0.1.md](../v0.1.md) on a kernel rule,
the V0.1 rule remains authoritative for kernel conformance until an explicit
preservation-table row marks it evolved or removed. Silent weakening is
forbidden.

If Python code conflicts with this spec, record the conflict. Do not change
Python to implement V0.2.
