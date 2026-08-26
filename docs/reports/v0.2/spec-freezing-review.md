# V0.2 specification freezing review

Status: Historical Report
Applies to: `docs/specs/v0.2/` as landed on this branch
Canonical path: docs/reports/v0.2/spec-freezing-review.md
Not a specification.

## What became hard invariants

Root retains frontier admission; Generation is a bounded admitted slice, not
a sub-Root. Claim, not Task creation, establishes execution authority.
Compilation is not admission. Transform creates a successor. Revival
preserves LogicalAgent and is not a new Generation. Prompt is not a sandbox.
Worker deltas do not auto-write MemoryCapsule. Compiler has no privileged
lifecycle. Information functions are not exclusive AgentKind classes. V0.1
kernel machines, fencing, Result atomicity, writer safety, and recovery
barrier are unchanged.

## Implementation-defined

Rust crate/struct/SQL names, async runtime, serialization, vendor wire
formats (as opaque handles), exact GenerationPolicy bytes, exact
information-function encoding.

## Unresolved (see specs/v0.2/17)

None classified BLOCKS_KERNEL.

BLOCKS_SEMANTIC_LAYER: GenerationPolicy encoding, intra-generation Task
adds, Generation DAG vs chain, intent schema/fanout, type relation and
revision encodings, memory schema/promotion, negative GC, ContinuityBinding
storage, Root review API, Transform rollback, remaining topology-vs-type
split, Objective schema.

DOES_NOT_BLOCK_RIIR_KERNEL: second-adapter extras (M7), V0.1 DB migrate
(M3 MAY use a new DB).

## Did any V0.1 correctness behavior have to change?

No. Preservation table: all kernel rows **unchanged**. V0.2 **adds**
semantic objects above the kernel.

Python still binds PoolPartition to ExecutionTarget/Profile. That is a
recorded V0.1 vs V0.2-intent conflict. Python was not modified.

## Can Rust kernel RIIR begin?

Yes for M4: authority, Task/Attempt/Lease/Result/Batch, fencing, recovery,
topology MOVE/MERGE, adapter contract, and V0.1 tests are specified.
Implementers MAY choose representation. They MUST NOT decide the questions
listed in `docs/specs/v0.2/README.md` “What an implementer may choose”.

M6 MUST wait on the BLOCKS_SEMANTIC_LAYER registry.

## Architecture regression checklist

| Failure mode | Possible under this spec? |
|---|---|
| 1. Expensive Root / cheap worker in Core | No. Cost is last in SpawnSource selection. |
| 2. AgentType is a model alias | No. Explicit MUST NOT. |
| 3. SpawnSource is semantic identity | No. |
| 4. Manager/team-lead hierarchy | No. Forbidden Core relations. |
| 5. Generation as delegated sub-Root | No. Admission stays with Root. |
| 6. Worker recursively creates executable work | No. |
| 7. Compiler admits its proposal | No. |
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
