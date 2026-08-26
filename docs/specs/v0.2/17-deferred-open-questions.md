# 17 — Deferred Open Questions

Status: Normative registry of non-decisions
Canonical path: docs/specs/v0.2/17-deferred-open-questions.md

Unresolved items live **only** here. Normative sections MUST point here with
`DEFERRED` rather than inventing answers.

Classification:

- `BLOCKS_KERNEL` — RIIR M4 cannot start
- `BLOCKS_SEMANTIC_LAYER` — M6 cannot start
- `DOES_NOT_BLOCK_RIIR_KERNEL` — M4 may proceed; resolve before the named gate

| ID | Question | Why unresolved | Blocks | Resolve by |
|---|---|---|---|---|
| D-GEN-POLICY | GenerationPolicy encoding (modes, budget shape, boolean vs numeric intents, drain/review flags) | design forbids guessing a workflow DSL | BLOCKS_SEMANTIC_LAYER | M6 |
| D-GEN-INTRA | May Root add Tasks after a Generation is already OPEN/ACTIVE? | not frozen in design | BLOCKS_SEMANTIC_LAYER | M6 |
| D-GEN-TOPOLOGY | Generation provenance chain vs DAG | only `parent_generation_id` sketched | BLOCKS_SEMANTIC_LAYER | M6 |
| D-INTENT-SCHEMA | RawWorkIntent strictness for domain workers | trade-off compiler vs architecture leak | BLOCKS_SEMANTIC_LAYER | M6 |
| D-INTENT-FANOUT | Whether 1-to-many compile is ever allowed | design prefers 0..1 | BLOCKS_SEMANTIC_LAYER | M6 |
| D-TYPE-REL | Concrete `can_execute` / `can_provision` / `more_specific_for` / `is_valid_refinement` | must not collapse to subtype | BLOCKS_SEMANTIC_LAYER | M6 |
| D-TYPE-REV | AgentType revision compatibility and Task pins | revisions mentioned, rules not | BLOCKS_SEMANTIC_LAYER | M6 |
| D-INFO-FN | Information-function set/trait encoding | exclusivity frozen; encoding not | BLOCKS_SEMANTIC_LAYER | M6 |
| D-MEM-SCHEMA | MemoryCapsule size, fields, merge, pos/neg specialization | design lists needs | BLOCKS_SEMANTIC_LAYER | M6 |
| D-MEM-PROMOTE | Who promotes Result delta to canonical MemoryCapsule | Root vs integration Task vs other | BLOCKS_SEMANTIC_LAYER | M6 |
| D-NEG-GC | Negative entry scope/assumptions/applicability/supersession/hot-cold GC | without it prohibitions rot | BLOCKS_SEMANTIC_LAYER | M6 |
| D-CONTINUITY-BIND | ContinuityBinding storage, security, expiry | opaque handle only | BLOCKS_SEMANTIC_LAYER | M6 |
| D-ROOT-API | Exact Generation review / admit / defer API | doctrine is behavioral, not wire | BLOCKS_SEMANTIC_LAYER | M6 |
| D-TRANSFORM-FAIL | Transform suspend/cancel/partial/rollback | saga happy path frozen | BLOCKS_SEMANTIC_LAYER | M6 |
| D-TOPOLOGY | Remaining type-refinement vs capacity vs MOVE vs MERGE vs TRANSFORM split | V0.1 MOVE/MERGE kernel is enough for M4 | BLOCKS_SEMANTIC_LAYER | M6 |
| D-ADAPTER2 | Minimal second-adapter conformance extras | M7 demonstration | DOES_NOT_BLOCK_RIIR_KERNEL | M7 |
| D-DB-MIGRATE | In-place V0.1 SQLite migrate vs import vs new DB | decide before upgrade claims | DOES_NOT_BLOCK_RIIR_KERNEL | before storage upgrade; M3 MAY use new DB |
| D-OBJECTIVE | Objective/problem-scope schema | optional Root model | BLOCKS_SEMANTIC_LAYER | M6 |

The first landing of this spec omitted V0.1.2 physical Execution transitions,
LogicalAgent excess-retire, Outbox ACKED, and the Batch-COMPLETED/outbox
atomicity rule. Those omissions **were** kernel blockers. They are specified
in [03](03-task-attempt-lease-result.md), [08](08-logical-agent-lineage-transform.md),
and [13](13-storage-and-transactions.md); they are **not** DEFERRED items.

No **open question** in this table is `BLOCKS_KERNEL`.

M4 Core MAY begin only from the **M4** slices of [03](03-task-attempt-lease-result.md)
(no Generation membership), [11](11-pool-topology.md),
[13](13-storage-and-transactions.md) including RUNNING-confirm + first
renewal, [14](14-recovery-and-reconciliation.md) authority reconciliation,
and [16](16-conformance-tests.md) section A.

M4 MUST NOT implement [04](04-generation-and-frontier.md). GenerationPolicy
and related items remain BLOCKS_SEMANTIC_LAYER.

M5 is a separate gate ([16](16-conformance-tests.md) section A2).

M6 MUST NOT treat Transform failure rollback or compiler exact-duplicate
auto-drop as frozen. Cutover atomicity (option A) **is** frozen.
