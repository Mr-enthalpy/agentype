# V0.2 specification freezing review

Status: Historical Report
Applies to: `docs/specs/v0.2/` after kernel-machine audit repair
Canonical path: docs/reports/v0.2/spec-freezing-review.md
Not a specification.

## What became hard invariants

Root retains frontier admission; Generation is a bounded admitted slice, not
a sub-Root. Claim, not Task creation, establishes execution authority.
Compilation is not admission and MUST NOT negatively admit by dropping
intents as generic redundant. Transform successor is created in one atomic
cutover (TARGET_READY → COMPLETED); `CUTTING_OVER` is not a durable
split-brain state. Revival preserves LogicalAgent and is not a new
Generation. Prompt is not a sandbox. Worker deltas do not auto-write
MemoryCapsule. Compiler has no privileged lifecycle. Information functions
are not exclusive AgentKind classes. V0.1 kernel machines (including
physical Execution UNKNOWN→RUNNING, LogicalAgent excess unassigned retire,
Outbox ACKED, Batch COMPLETED + BATCH_RESULTS_READY same tx), fencing,
Result atomicity, writer safety, recovery barrier, and SQLite WAL +
`synchronous=FULL` are unchanged.

## Implementation-defined vs deferred

IMPLEMENTATION-DEFINED: Rust crate/struct/SQL *names*, async runtime,
serialization, vendor wire formats (opaque handles).

DEFERRED / BLOCKS_SEMANTIC_LAYER (not a free implementation choice):
GenerationPolicy encoding, information-function set/trait encoding, and the
other rows in `docs/specs/v0.2/17-deferred-open-questions.md`.

M4 store itself is **not** implementation-defined: SQLite WAL +
`synchronous=FULL` MUST.

## Unresolved (see specs/v0.2/17)

Open *questions* in 17 are not BLOCKS_KERNEL after this repair.

The first spec landing *was* blocked for M4 until Execution/LogicalAgent/
Outbox/transaction tables matched the V0.1.2 oracle. Those are now
normative, not DEFERRED.

BLOCKS_SEMANTIC_LAYER: GenerationPolicy encoding, intra-generation Task
adds, Generation DAG vs chain, intent schema/fanout, **D-COMPILATION-CLOSURE**
(which Generation a model-backed compiler Task belongs to), type relation and
revision encodings, memory schema/promotion, negative GC, ContinuityBinding
storage, Root review API, Transform **failure/rollback** (not cutover
atomicity), remaining topology-vs-type split, Objective schema.

DOES_NOT_BLOCK_RIIR_KERNEL: second-adapter extras (M7), V0.1 DB migrate
(M3 MAY use a new DB).

## Did any V0.1 correctness behavior have to change?

No. Preservation table: kernel rows **unchanged**, including Outbox ACKED
and SQLite WAL. V0.2 **adds** semantic objects above the kernel.

Python still binds PoolPartition to ExecutionTarget/Profile. That is a
recorded V0.1 vs V0.2-intent conflict. Python was not modified.

## Can Rust kernel RIIR begin?

**M4 Core** MAY begin only with: Task MUST NOT require Generation;
RUNNING-confirm + first Lease renewal is an M4 transaction; **08 kernel**
LogicalAgent/Incarnation including RETIRED fencing live Incarnations to LOST
in the same transaction. M4 MUST NOT implement `04` Generation or Transform.

**M5 Runtime** is a separate gate: **one named** reference adapter, plus
supervision cleanup, profile registry, poll/heartbeat/lease timing, notifier
backoff-from-completion, daemon single-run, adapter deadlines. M5 is not
both V0.1.3 transports.

**M6** still waits on BLOCKS_SEMANTIC_LAYER including D-COMPILATION-CLOSURE.
Transform **failure** is not frozen. Cutover **option A** is an **explicit
freeze** after audit (not a literal translation of the design saga's
durable CUTTING_OVER). Split-brain (source and successor both schedulable)
remains forbidden.

This review MUST NOT be read as an unconditional “M4 MAY begin” that
includes Generation.

## Numbered design invariants → spec

| ID | Spec |
|---|---|
| 1–4 topology/teams | 02, 04, 01 provenance |
| 5–9 Root | 12, 02 |
| 10–15 information | 06, 09, 12 |
| 16–22 Generation/Batch | 04, 03 |
| 23–28 WorkIntent | 05, 02 |
| 29–34 type/provision | 06, 07 |
| 35–38 sandbox | 10 |
| 39–44 Transform/memory | 08, 09, 13 |
| 45–52 revival/terminal | 09, 07 |
| 53–62 V0.1 kernel + compiler lifecycle | 02, 03, 13, 14, 05 |
| 63 information-function orthogonality | 06 |
| 64 retry ≠ new Generation | 04, 14 |

## Architecture regression checklist

| Failure mode | Possible under this spec? |
|---|---|
| 1. Expensive Root / cheap worker in Core | No. Cost is last in SpawnSource selection. |
| 2. AgentType is a model alias | No. Explicit MUST NOT. |
| 3. SpawnSource is semantic identity | No. |
| 4. Manager/team-lead hierarchy | No. Forbidden Core relations. |
| 5. Generation as delegated sub-Root | No. Admission stays with Root. |
| 6. Worker recursively creates executable work | No. |
| 7. Compiler admits its proposal | No. Negative admit via generic redundant-drop also forbidden. |
| 8. retry/revival creates a Generation | No. |
| 9. Transform mutates type in place | No. |
| 10. Native session required for correctness | No. Level 3 is the floor. |
| 11. Transcript is MemoryCapsule | No. |
| 12. Prompt treated as sandbox | No. |
| 13. Semantic layer bypasses Task/Attempt/Lease | No. |
| 14. Root polls Scheduler | No. |
| 15. Scheduler silent LLM semantics | No. Hidden LLM forbidden; compiler/transform refinement are ordinary Tasks. |

## Design → spec gaps

All numbered design files and both operational contracts are mapped in
`docs/specs/v0.2/matrices.md`. Rationale-only prose (information-theoretic
motivation) remains in design and is classified J (non-normative).
