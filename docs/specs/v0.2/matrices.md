# V0.2 consistency matrices

Status: Normative index
Canonical path: docs/specs/v0.2/matrices.md

## A. Architecture direction → spec

| Design document | Spec landing |
|---|---|
| 00-design-charter.md | 00, 15 |
| 01-system-thesis-and-information-functions.md | 06 information functions, 09 memory |
| 02-root-operating-model.md | 12 |
| ROOT_OPERATING_DOCTRINE.md | 12 |
| 03-flat-organization-and-generations.md | 02, 04 |
| 04-work-intent-compilation.md | 05 |
| 05-agent-type-and-spawn-source.md | 06, 07, 11 |
| 06-transform-lineage-and-memory.md | 08, 09 |
| 07-revival-continuity-and-terminal-boundary.md | 09, 07 experience adapter |
| 08-sandbox-and-capability-enforcement.md | 10 |
| 09-v01-correctness-kernel-boundary.md | 02, 03, 14 |
| 10-rust-rewrite-boundary.md | 00, 15, 16 |
| 11-open-questions.md | 17 |
| 12-normative-invariants.md | numbered 1–64 mapped in [spec-freezing-review.md](../../reports/v0.2/spec-freezing-review.md) |
| AGENT_INGESTION_GUIDE.md | process only; not a runtime contract |

## B. V0.1 kernel → V0.2

| V0.1 rule | V0.2 | Status |
|---|---|---|
| SQLite WAL + `synchronous=FULL` authoritative; at-least-once | 02, 13 | unchanged (M4 MUST SQLite; not an abstract store) |
| Claim = Attempt+Lease+epoch | 02, 03, 13 | unchanged |
| Task/Attempt/Lease/Execution/Batch/LogicalAgent machines | 03, 08 | unchanged (Execution physical graph includes UNKNOWN→RUNNING; LogicalAgent excess unassigned retire). Generation membership is **M6 added**, not M4. |
| Fencing + stale physical-only history | 02, 03 | unchanged |
| RUNNING confirm + first Lease renewal one fenced tx before admission | 13, 14 | unchanged (M4) |
| One Result per completed Task; ACK is consumption | 03 | unchanged |
| Writer quiescence / isolation snapshot / RETIRE block | 03, 11 | unchanged |
| Retry classes; no semantic recovery by Scheduler | 02 | unchanged |
| Adapter deadline + nonterminal collect MUST NOT inherit reconcile quiescence | 07, 14 | unchanged |
| Outbox PENDING/DELIVERED/ACKED; ACK MAY skip DELIVERED; first Batch COMPLETED + BATCH_RESULTS_READY same tx | 03, 13 | unchanged (M4) |
| Notifier isolation; backoff from call **completion**; poll ≤ heartbeat < lease; recovery startup cleanup; empty profile registry authoritative | 03, 07, 14 | unchanged (M5) |
| Startup RECOVERY barrier | 14 | unchanged |
| MOVE/MERGE preserve identity; RETIRE guards | 11 | unchanged |
| Revival preserves LogicalAgent; READY ≠ physical | 08, 09 | unchanged |
| Codex/Grok wire mapping | 07 | implementation-defined transport; Core opaque |
| Optional Tasks | — | still forbidden |
| Generation / WorkIntent / AgentType / SpawnSource / Transform | 04–09 | **added** (M6); MUST NOT bypass kernel |
| PoolPartition absorbs target+profile+capacity | 11 | V0.1 kernel unchanged; V0.2 split DEFERRED |

No V0.1 **kernel or runtime** correctness property listed above is removed.
Generation membership is an M6 addition, not a V0.1 removal.

## C. Spec → Rust boundary

| Subsystem | Boundary |
|---|---|
| Domain objects, authority, state machines | core/domain |
| Persistence, uniqueness, transactions | storage |
| Dispatcher, heartbeat, notifier, daemon single-run | runtime |
| ExecutionAdapter / RootBridge traits | adapter-api |
| Codex/Grok/filesystem adapters | adapter crates |
| Root wakeup transports | root bridge |
| Config + wiring | CLI/composition |
| Generation, WorkIntent, AgentType, Transform (M6) | core + storage; still no vendor types |
