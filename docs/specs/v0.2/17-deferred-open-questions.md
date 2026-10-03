# 17 — Deferred Open Questions

Status: Normative registry of non-decisions
Canonical path: docs/specs/v0.2/17-deferred-open-questions.md

Unresolved items live **only** here. Normative sections MUST point here with
`DEFERRED` rather than inventing answers.

Classification:

- `BLOCKS_KERNEL` — RIIR M4 cannot start
- `BLOCKS_SEMANTIC_LAYER` — M6 cannot start
- `BLOCKS_M6B_B3` — M6-B.1 may proceed; resolve before M6-B B.3 typed admission (does not retroactively block M6-A/B.1)
- `BLOCKS_M6B_FREEZE` — M6-B.1 may proceed as a draft kernel, but M6-B MUST NOT be frozen/merged
- `DOES_NOT_BLOCK_RIIR_KERNEL` — M4 may proceed; resolve before the named gate

| ID | Question | Why unresolved | Blocks | Resolve by |
|---|---|---|---|---|
| D-GEN-POLICY | GenerationPolicy encoding (modes, budget shape, boolean vs numeric intents, drain/review flags) | **DEFERRED (Does not block M6-A)**: M6-A freezes explicit Root admission as the sole expansion authority; automated policy DSL and machine-governed admission deferred. Before M6-B B.3 typed admission, the interface by which a Generation policy folds into an immutable effective policy/requirement MUST be defined | BLOCKS_M6B_B3 | before M6-B B.3 |
| D-GEN-INTRA | May Root add Tasks after a Generation is already OPEN/ACTIVE? | **RESOLVED in M6-A**: Root may add Tasks dynamically; in OPEN all functions allowed, in FROZEN only compression allowed | None (Resolved) | M6-A |
| D-GEN-TOPOLOGY | Generation provenance chain vs DAG | **RESOLVED in M6-A**: DAG tracked via immutable `SemanticInputSet` on each task binding and generation seed | None (Resolved) | M6-A |
| D-INTENT-SCHEMA | RawWorkIntent strictness for domain workers | **RESOLVED in M6-A**: Ingress `RawWorkIntent` structure compiles deterministically to `CompiledWorkProposal` | None (Resolved) | M6-A |
| D-INTENT-FANOUT | Whether 1-to-many compile is ever allowed | **RESOLVED in M6-A**: 1 intent deterministically compiles to at most 1 proposal (0..1); fanout deferred | None (Resolved) | M6-A |
| D-TYPE-REL | Concrete `can_execute` / `can_provision` / `more_specific_for` / `is_valid_refinement` | **PARTIALLY RESOLVED in M6-B.1**: the four predicates are frozen for the capability/coarse dimensions (exact pin equality, polarity, security-class proof authority, coarse workspace/network/isolation) and MUST NOT collapse to subtype. The full `SandboxPolicyRef` ordering/intersection relation is NOT closed here and is tracked as D-SANDBOX-ORDER/D-SANDBOX-INTERSECTION | BLOCKS_M6B_FREEZE (until D-SANDBOX-*) | M6-B.1 (capability/coarse dimensions only) |
| D-SANDBOX-ORDER | Ordering/refinement of the opaque `SandboxPolicyRef` | **DEFERRED (Does not block M6-B.1 as a draft kernel)**: B.1 pins sandbox policy by exact reference (equality). Representing a genuinely smaller sandbox `Q ⊂ P` (read/write roots, visible files, tools, network all narrower) is required by spec 10 `Root-created refinements MUST only narrow` and MUST be defined before M6-B freeze | BLOCKS_M6B_FREEZE | B.3/B.5 (before M6-B freeze) |
| D-SANDBOX-INTERSECTION | Composition of AgentType/Task sandbox policy and consistency with the coarse `SecurityContract` | **DEFERRED (Does not block M6-B.1 as a draft kernel)**: B.1 composes coarse workspace/network/isolation and restriction capabilities. Spec 10 requires M6 effective permission to equal `AgentType ∩ Generation ∩ Task ∩ SpawnSource ceiling`; the full policy intersection, its consistency with the coarse contract, and the `Generation policy` fold (D-GEN-POLICY) MUST be defined before M6-B freeze | BLOCKS_M6B_FREEZE | B.5 (before M6-B freeze) |
| D-TYPE-REV-PIN | AgentType revision pinning for LogicalAgent and Task | **RESOLVED in M6-B.1**: published revisions immutable; refs pin an exact `AgentTypeRef`; selectors resolve to exact pre-commit | None (Resolved) | M6-B.1 |
| D-TYPE-REV-COMPAT | Cross-revision AgentType compatibility/ranges | **DEFERRED (Does not block M6-B v1)**: cross-revision compatibility is unsupported in M6-B v1; moving a LogicalAgent to a new revision is a manual admission / future Transform | DOES_NOT_BLOCK_RIIR_KERNEL | post-M6-B |
| D-INFO-FN | Information-function set/trait encoding | **RESOLVED in M6-A**: Fixed 3-value closed enum (`EXPAND`, `COMPRESS_POSITIVE`, `COMPRESS_NEGATIVE`) with strict state-gated admission rules | None (Resolved) | M6-A |
| D-MEM-SCHEMA | MemoryCapsule size, fields, merge, pos/neg specialization | design lists needs | BLOCKS_SEMANTIC_LAYER | M6 |
| D-MEM-PROMOTE | Who promotes Result delta to canonical MemoryCapsule | Root vs integration Task vs other | BLOCKS_SEMANTIC_LAYER | M6 |
| D-NEG-GC | Negative entry scope/assumptions/applicability/supersession/hot-cold GC | without it prohibitions rot | BLOCKS_SEMANTIC_LAYER | M6 |
| D-CONTINUITY-BIND | ContinuityBinding storage, security, expiry | opaque handle only | BLOCKS_SEMANTIC_LAYER | M6 |
| D-ROOT-API | Exact Generation review / admit / defer API | **RESOLVED in M6-A**: Narrow `RootSemanticControl` surface (`create_generation`, `compile_root_intent`, `admit_proposal`, `reject_proposal`, `freeze_generation`, `close_generation`, `read_generation_view`, `read_proposal`) plus a proposal-only `IntentIngress::compile_from_result` with no admission authority | None (Resolved) | M6-A |
| D-TRANSFORM-FAIL | Transform suspend/cancel/partial/rollback | saga happy path frozen | BLOCKS_SEMANTIC_LAYER | M6 |
| D-TOPOLOGY | Remaining type-refinement vs capacity vs MOVE vs MERGE vs TRANSFORM split | V0.1 MOVE/MERGE kernel is enough for M4 | BLOCKS_SEMANTIC_LAYER | M6 |
| D-ADAPTER2 | Minimal second-adapter conformance extras | M7 demonstration | DOES_NOT_BLOCK_RIIR_KERNEL | M7 |
| D-DB-MIGRATE | In-place V0.1 SQLite migrate vs import vs new DB | decide before upgrade claims. **M6-B.2 uses a fresh schema-v6 database and does NOT implement v5 -> v6 in-place upgrade**; the version gate rejects any older/newer schema at open | DOES_NOT_BLOCK_RIIR_KERNEL | before storage upgrade; M3 MAY use new DB |
| D-OBJECTIVE | Objective/problem-scope schema | **RESOLVED in M6-A**: `RawWorkIntent.objective` string + optional rationale + `SemanticInputSet` provenance references | None (Resolved) | M6-A |
| D-COMPILATION-CLOSURE | How model-backed compilation Tasks participate in Generation drain/REVIEWABLE (same-generation closure phase vs dedicated non-frontier system work vs other bounded form) | **DEFERRED (Does not block M6-A)**: M6-A ingress uses synchronous deterministic compiler yielding inert candidate proposals; asynchronous model-backed compilation tasks and their lifecycle closure deferred | BLOCKS_SEMANTIC_LAYER | post-M6-A |
| D-GEN-RESUME | Align Generation resume with Task/Batch/Escalation: mechanical vs semantic discriminator, recovery edges, atomicity. Scheduler-owned Generation SUSPENDED→ACTIVE is **not** frozen | **RESOLVED in M6-A**: Strictly monotonic state machine `OPEN -> FROZEN -> CLOSED`; no intermediate SUSPENDED state or resume machine | None (Resolved) | M6-A |

The first landing of this spec omitted V0.1.2 physical Execution transitions,
LogicalAgent excess-retire, Outbox ACKED, and the Batch-COMPLETED/outbox
atomicity rule. Those omissions **were** kernel blockers. They are specified
in [03](03-task-attempt-lease-result.md), [08](08-logical-agent-lineage-transform.md),
and [13](13-storage-and-transactions.md); they are **not** DEFERRED items.

No **open question** in this table is `BLOCKS_KERNEL`.

M4 Core MAY begin only from the **M4** slices of [03](03-task-attempt-lease-result.md)
(no Generation membership), [08](08-logical-agent-lineage-transform.md)
**kernel** LogicalAgent/Incarnation including retirement fencing (not
Transform), [11](11-pool-topology.md),
[13](13-storage-and-transactions.md) including RUNNING-confirm + first
renewal and RETIRED+Incarnation-LOST, [14](14-recovery-and-reconciliation.md)
authority reconciliation, and [16](16-conformance-tests.md) section A.

M4 MUST NOT implement [04](04-generation-and-frontier.md). GenerationPolicy
and related items remain BLOCKS_SEMANTIC_LAYER.

M5 is a separate gate ([16](16-conformance-tests.md) section A2).

M6 MUST NOT treat Transform failure rollback or compiler exact-duplicate
auto-drop as frozen. Cutover atomicity (option A) **is** frozen.
