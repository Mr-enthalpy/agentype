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

**M4 — Core correctness parity.** MUST satisfy Task/Attempt/Lease/Result/Batch
machines in [03](03-task-attempt-lease-result.md) **except Generation
membership**, the **kernel** LogicalAgent and Incarnation machines in
[08](08-logical-agent-lineage-transform.md) (semantic retirement fencing
included; **not** the Transform saga), writer safety, fencing, RUNNING-confirm
+ first Lease renewal atomicity, SQLite transactions in
[13](13-storage-and-transactions.md), topology kernel in
[11](11-pool-topology.md), restart **authority** reconciliation in
[14](14-recovery-and-reconciliation.md), and M4 rows of
[16](16-conformance-tests.md).

M4 MUST NOT require Generation, WorkIntent, AgentType, SpawnSource semantic
integration, Transform, or MemoryCapsule promotion. M4 MUST NOT invent a
default Generation to satisfy later sections.

**M5 — Runtime / one reference-adapter parity.** MUST satisfy supervision
lifecycle cleanup, adapter absolute deadlines, authoritative ExecutionProfile
registry, `dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`,
notifier completion-based backoff, daemon single-run, **one named** reference
adapter acceptance (which adapter is IMPLEMENTATION-DEFINED; M5 MUST NOT be
read as requiring both V0.1.3 transports), and M5 rows of
[16](16-conformance-tests.md).

**M6 — V0.2 semantic conformance.** MUST also satisfy Generation (every
semantic Task belongs to exactly one Generation; retry stays in that
Generation), WorkIntent, AgentType, SpawnSource semantic integration,
Transform, memory, Root contract, and M6 rows of [16](16-conformance-tests.md).

**M7 — Frontend neutrality.** MUST add a **second independent** adapter
without changing Core state machines. M5's single reference adapter does not
satisfy M7.

## Staged implementation

RIIR MUST follow: freeze spec (this directory) → workspace/bootstrap (M3) →
kernel parity (M4) → first-adapter runtime parity (M5) → semantic layer (M6) →
second adapter (M7).

An implementer MUST NOT simultaneously change language, weaken the kernel, and
add semantic architecture. An implementer MUST NOT implement M6 objects in
order to pass M4.

## Conflict policy

If this spec conflicts with [docs/design/v0.2/12-normative-invariants.md](../../design/v0.2/12-normative-invariants.md),
the invariant wins and the conflict MUST be reviewed explicitly.

If this spec conflicts with [docs/specs/v0.1.md](../v0.1.md) on a kernel rule,
the V0.1 rule remains authoritative for kernel conformance until an explicit
preservation-table row marks it evolved or removed. Silent weakening is
forbidden.

If Python code conflicts with this spec, record the conflict. Do not change
Python to implement V0.2.
