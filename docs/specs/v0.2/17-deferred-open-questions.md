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
- `BLOCKS_M6B_B5_FREEZE` — M6-B.4 remains frozen; the item blocks only the named M6-B.5 slice freeze (policy-only, credential-bound, or both as stated)
- `DOES_NOT_BLOCK_RIIR_KERNEL` — M4 may proceed; resolve before the named gate

| ID | Question | Why unresolved | Blocks | Resolve by |
|---|---|---|---|---|
| D-GEN-POLICY-FOLD | The interface by which a Generation policy folds into an immutable effective requirement | **RESOLVED in M6-B.3**: a Generation MAY carry an immutable authority ceiling fixed at `create_generation`, applied as the spec 10 intersection on each typed admission (a Task exceeding the coarse workspace/network ceiling fails closed; a stricter Task under a wider ceiling keeps its own value). A policy-bearing Generation forbids legacy `admit_proposal`. Full capability/sandbox ordering/intersection remains D-SANDBOX-* | None (Resolved) | M6-B.3 |
| D-GEN-POLICY-DSL | Automated GenerationPolicy encoding (modes, budget shape, drain/review flags) and machine-governed admission | **DEFERRED (Does not block M6-A/B.3)**: M6-A freezes explicit Root admission as the sole expansion authority; the automated policy DSL and machine-governed admission remain deferred | DOES_NOT_BLOCK_RIIR_KERNEL | post-M6-B |
| D-GEN-INTRA | May Root add Tasks after a Generation is already OPEN/ACTIVE? | **RESOLVED in M6-A**: Root may add Tasks dynamically; in OPEN all functions allowed, in FROZEN only compression allowed | None (Resolved) | M6-A |
| D-GEN-TOPOLOGY | Generation provenance chain vs DAG | **RESOLVED in M6-A**: DAG tracked via immutable `SemanticInputSet` on each task binding and generation seed | None (Resolved) | M6-A |
| D-INTENT-SCHEMA | RawWorkIntent strictness for domain workers | **RESOLVED in M6-A**: Ingress `RawWorkIntent` structure compiles deterministically to `CompiledWorkProposal` | None (Resolved) | M6-A |
| D-INTENT-FANOUT | Whether 1-to-many compile is ever allowed | **RESOLVED in M6-A**: 1 intent deterministically compiles to at most 1 proposal (0..1); fanout deferred | None (Resolved) | M6-A |
| D-TYPE-REL | Concrete `can_execute` / `can_provision` / `more_specific_for` / `is_valid_refinement` | **PARTIALLY RESOLVED in M6-B.1**: the four predicates are frozen for the capability/coarse dimensions (exact pin equality, polarity, security-class proof authority, coarse workspace/network/isolation) and MUST NOT collapse to subtype. M6-B.5 defines the `SandboxPolicyRef` ordering/intersection relation for `SECURITY_V1` in [ADR-0013](../../decisions/0013-m6b5-policy-authority-algebra-identity.md); backend feature coverage remains tracked as D-SANDBOX-* | BLOCKS_M6B_B5_FREEZE (until D-SANDBOX-*) | M6-B.1 (capability/coarse dimensions); M6-B.5 (sandbox relation) |
| D-SANDBOX-ORDER | Ordering/refinement of the opaque `SandboxPolicyRef` | **DESIGN DECIDED in M6-B.5 / B5.0**: the `SECURITY_V1` ordering relation and explicit durable admission mode are defined in [ADR-0013](../../decisions/0013-m6b5-policy-authority-algebra-identity.md), and the relation defined in B.5 is not a claim that every backend can enforce every policy; backend feature coverage is tracked in the B.5 enforcement matrix and `SECURITY_V1_POLICY_ONLY_REFERENCE_V1`. `COARSE_V1` keeps exact-reference equality and its frozen replay | BLOCKS_M6B_B5_FREEZE (policy-only) | B.5 (B5.2/B5.5) |
| D-SANDBOX-INTERSECTION | Composition of AgentType/Task sandbox policy and consistency with the coarse `SecurityContract` | **DESIGN DECIDED in M6-B.5 / B5.0**: [ADR-0013](../../decisions/0013-m6b5-policy-authority-algebra-identity.md) fixes the four-object model, per-object coarse projection/consistency including Task-local coarse/fine validation before normalization and any cross-object meet, typed `R_min` with admission-level versus candidate-level satisfaction, and the inline derived-policy identity. The relation defined in B.5 is not a claim that every backend can enforce every composite policy; backend feature coverage is tracked in the B.5 enforcement matrix. `COARSE_V1` replay is unchanged | BLOCKS_M6B_B5_FREEZE (policy-only) | B.5 (B5.2/B5.5) |
| D-CREDENTIAL-DELIVERY | Trusted source-bound credential qualification, delivery and recovery | **DESIGN DECIDED in M6-B.5 / B5.0** ([ADR-0015](../../decisions/0015-m6b5-credentials-feature-gates-scope-freeze.md)): per-acquisition observation epoch with explicit failure classes, unknown candidates judged by continuity dominance rather than enumeration position, three independent imported feature gates, requested scope contained in `P_eff` and the Source ceiling, non-secret proof identity carried into the Execution snapshot, and v1 production `BrokeredAccess` only; `TransientRawDelivery` is defined but unsupported and never selected. Implementation and cross-platform delivery proof remain pending | BLOCKS_M6B_B5_FREEZE (credential-bound) | B.5 (B5.3/B5.5) |
| D-TYPE-REV-PIN | AgentType revision pinning for LogicalAgent and Task | **RESOLVED in M6-B.1**: published revisions immutable; refs pin an exact `AgentTypeRef`; selectors resolve to exact pre-commit | None (Resolved) | M6-B.1 |
| D-TYPE-REV-COMPAT | Cross-revision AgentType compatibility/ranges | **DEFERRED (Does not block M6-B v1)**: cross-revision compatibility is unsupported in M6-B v1; moving a LogicalAgent to a new revision is a manual admission / future Transform | DOES_NOT_BLOCK_RIIR_KERNEL | post-M6-B |
| D-INFO-FN | Information-function set/trait encoding | **RESOLVED in M6-A**: Fixed 3-value closed enum (`EXPAND`, `COMPRESS_POSITIVE`, `COMPRESS_NEGATIVE`) with strict state-gated admission rules | None (Resolved) | M6-A |
| D-MEM-SCHEMA | MemoryCapsule size, fields, merge, pos/neg specialization | design lists needs | BLOCKS_SEMANTIC_LAYER | M6 |
| D-MEM-PROMOTE | Who promotes Result delta to canonical MemoryCapsule | Root vs integration Task vs other | BLOCKS_SEMANTIC_LAYER | M6 |
| D-NEG-GC | Negative entry scope/assumptions/applicability/supersession/hot-cold GC | without it prohibitions rot | BLOCKS_SEMANTIC_LAYER | M6 |
| D-CONTINUITY-BIND | ContinuityBinding storage, security, expiry | opaque handle only | BLOCKS_SEMANTIC_LAYER | M6 |
| D-ROOT-API | Exact Generation review / admit / defer API | **RESOLVED in M6-A**: Narrow `RootSemanticControl` surface (`create_generation`, `compile_root_intent`, `admit_proposal`, `reject_proposal`, `freeze_generation`, `close_generation`, `read_generation_view`, `read_proposal`) plus a proposal-only `IntentIngress::compile_from_result` with no admission authority | None (Resolved) | M6-A |
| D-TRANSFORM-FAIL | Transform suspend/cancel/partial/rollback | saga happy path frozen | BLOCKS_SEMANTIC_LAYER | M6 |
| D-TOPOLOGY | Remaining type-refinement vs capacity vs MOVE vs MERGE vs TRANSFORM split | **PARTIALLY RESOLVED in M6-B.4**: V0.1 `reconcile_pool` capacity excludes typed (`BOUND`) population ([ADR-0009](../../decisions/0009-m6b-topology-capacity-seam.md)); full typed-population targets and the type-refinement vs MOVE/MERGE/TRANSFORM remainder remain deferred | BLOCKS_SEMANTIC_LAYER (remainder) | M6 (remainder post-M6-B) |
| D-WORKER-RESULT-DP | Production worker Result transport, authentication, durable ingress and delivery retry | **DEFERRED (does not block M6-B.4 freeze)**: [ADR-0012](../../decisions/0012-m6b4-scope-typed-physical-execution-ack.md) excludes the worker-originated data plane; M6-B.4 proves only the durable Kernel ACK interface, invoked by the harness | DOES_NOT_BLOCK_RIIR_KERNEL | post-M6-B (new milestone/ADR) |
| D-ARTIFACT-GC | Durable artifact owner, retention policy and safe-GC authority for jail roots, workspaces, stdout/stderr, locators and applied-security receipts | **DEFERRED (does not block M6-B.4 freeze)**: there is no durable deletion authority or positive safety witness; deleting possibly-live artifacts would be unsafe, so long-running deployments retain an explicit operational disk-growth responsibility. B.5 adds the applied-security receipt to this retained-artifact class ([ADR-0014](../../decisions/0014-m6b5-enforcement-compilation-receipt.md)) | DOES_NOT_BLOCK_RIIR_KERNEL | post-M6-B (artifact-owner design) |
| D-SELECTION-POLICY | Availability/cost selection policy for equal-continuity candidates | **DEFERRED (does not block M6-B.4 freeze)**: [ADR-0010](../../decisions/0010-m6b-selection-order-scope.md) keeps the fail-closed pre-acquisition `SelectionAmbiguous` deferral; no ID tie-break, random choice or implicit fallback is added | DOES_NOT_BLOCK_RIIR_KERNEL | post-M6-B (selection policy) |
| D-ADAPTER2 | Minimal second-adapter conformance extras | M7 demonstration | DOES_NOT_BLOCK_RIIR_KERNEL | M7 |
| D-DB-MIGRATE | In-place V0.1 SQLite migrate vs import vs new DB | decide before upgrade claims. **M6-B.2 uses a fresh schema-v6 database and does NOT implement v5 -> v6 in-place upgrade**; **M6-B.3 uses fresh v7, M6-B.4 uses fresh v8, and M6-B.5 uses fresh v9, with no v6 -> v7, v7 -> v8 or v8 -> v9 in-place upgrade**; the version gate rejects any older/newer schema at open and no missing security row is synthesized | DOES_NOT_BLOCK_RIIR_KERNEL | before storage upgrade; M3 MAY use new DB |
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
