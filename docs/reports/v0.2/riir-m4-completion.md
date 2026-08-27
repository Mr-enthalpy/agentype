# RIIR M3 + M4 completion

Status: Historical Report
Applies to: branch `rust/m3-m4-kernel`
Canonical path: `docs/reports/v0.2/riir-m4-completion.md`
Not a specification.

This records the first Rust landing of the V0.1 correctness kernel. It is
derived from `docs/specs/v0.2/` (M4 slices) and `docs/specs/v0.1.md`. Python
remains the behavior oracle, not a module template.

## Implemented contracts

### M3 — workspace

| Item | Status |
|---|---|
| Cargo workspace `agentype-core`, `agentype-storage-sqlite`, `agentype-adapter-api`, `agentype-runtime` | done |
| Core has no Tokio, vendor, SQLite row types, CLI, or env-var dependency | done |
| `/target/` gitignored | done |
| CI `rust` job (`cargo test --workspace`) on Ubuntu and Windows | done |
| Fresh Rust-era SQLite schema, `schema_version = 1` | done |
| WAL + `synchronous=FULL` + foreign keys on file databases | done |
| Claim-like writes use `BEGIN IMMEDIATE` | done |

Crate names deferred in the spec are: `agentype-core`, `agentype-storage-sqlite`,
`agentype-adapter-api`, `agentype-runtime`. Adapter implementations, RootBridge,
and CLI crates are not present (M5/M7).

### M4 — kernel

| Spec | Implemented |
|---|---|
| [03] Task/Attempt/Lease/Result/Batch/Escalation/Outbox machines | yes; Task has **no** Generation membership; Batch `OPEN/ACTIVE/SUSPENDED -> cancel -> CANCELLED` via `cancel_batch` (COMPLETED is terminal and rejects cancellation) |
| [03] Claim is the authority boundary | yes: epoch++, Attempt ACTIVE, Lease ACTIVE, Task LEASED, agent ASSIGNED, one tx |
| [03] Writer safety from persisted Execution | yes; omitted `execution_id` cannot hide a writer; isolation is frozen on the Execution |
| [03] Physical Execution graph including UNKNOWN→RUNNING and LOST→TERMINATED | yes; stale refine cannot mutate Task/Result |
| [03] Confirm RUNNING + first Lease renewal one fenced tx | `Kernel::confirm_running_and_renew` |
| [03] Outbox PENDING / DELIVERED / ACKED; PENDING→ACKED legal | yes |
| [03] First Batch COMPLETED + exactly one `BATCH_RESULTS_READY` same tx | yes |
| [08] LogicalAgent excess unassigned INITIALIZING/READY/REVIVING retire without DRAINING | yes |
| [08] RETIRED fences STARTING/WARM/COLD Incarnations LOST same tx | yes |
| [11] V0.1 upsert/resize/MOVE_CAPACITY/MERGE/RETIRE | yes; MERGE sums desired capacity and migrates future Task classification |
| [13] Unique indexes for active lease/attempt/agent, one Execution per Attempt, one Result per Task, one OPEN Escalation, one live Incarnation, one active Execution per Incarnation | yes |
| [14] Authority recovery barrier (expire overdue + unstarted claims, promote retry, reconcile pool, revive non-RETIRED) | `Kernel::recover_authority` / `agentype_runtime::recover_authority` |
| [15] Newtypes, closed enums, typed errors, no vendor types in Core | yes |
| [16] §A M4 tests | `crates/agentype-storage-sqlite/tests/{m4_kernel,recovery,topology}.rs` |

## Kernel rules frozen for M4 (writer-safety proof semantics)

Spec [16] §A requires `collect_outcome` MUST NOT inherit `reconcile_start`
quiescence. M4 has no dispatcher, so this landing freezes the kernel
representation of that rule (oracle parity verified against
`runtime.py` / `core.py`):

- Durable terminal/quiescence proof may only be persisted together with a
  terminal physical state. `record_physical_outcome` rejects proof bits on
  UNKNOWN / LOST targets (`invalid_transition`).
- Entering an unresolved physical state supersedes and **clears** earlier
  stored proof bits (no MAX inheritance across an authoritative nonterminal
  observation).
- `nack` with `terminal_confirmed=false` persists both proof bits as 0
  regardless of caller claims; `UNKNOWN` rows can never carry durable proof.
- `report_configuration_unavailable` passes `terminal=false, quiescent=false`:
  configuration unavailability is not physical proof. With no persisted
  Execution the RESOURCE_UNAVAILABLE retry policy applies unchanged; with a
  RUNNING writer the task suspends behind `WRITER_QUIESCENCE_UNKNOWN`.
- A semantically retired LogicalAgent cannot revive and cannot obtain a new
  Incarnation (`ensure_incarnation` rejects RETIRED inside the transaction;
  `create_execution` reaches it through the claim path).
- Claim matching preserves the frozen V0.1 order: continuity rank, oldest
  `available_since` (created_at fallback), then lowest LogicalAgent ID. The
  comparator re-applies the SQL ordering so the availability tiebreak cannot
  be lost in selection.
- Durable persisted JSON decodes fail-closed: corrupted documents surface as
  `InvariantViolation`, never as a silent alternative schedule
  (`store::json_load` returns `Result`).
- The continuity capsule byte bound is constructor-configured
  (`Kernel::open`/`open_memory`); composition supplies the bound at startup.
- `ExecutionAdapter::reconcile_start` is keyed by stable request identity
  (spec 07 narrow interface UNCHANGED from V0.1): `reconcile_start(request_id,
  Option<&RuntimeHandle>)` so an ambiguous start can be re-located even when
  the scheduler never obtained a complete runtime handle.

Regressions: `m4_kernel.rs` (proof bits), `recovery.rs` (RETIRED defences,
orphaned-claim restart, corrupted-state fail-closed), `topology.rs` (frozen
claim ordering).

M6 objects are absent: no Generation table, no WorkIntent, no AgentType matching,
no SpawnSource semantic integration, no Transform saga, no MemoryCapsule promotion,
no Root review API. Incarnation `generation` is the V0.1 physical presence
counter, not a V0.2 GenerationId.

## V0.1 test mapping

Classification of `tests/*.py` against this landing.

### PORT SEMANTICALLY (M4 Rust)

Oracle → Rust test (not a function-by-function port):

| Python oracle | Rust |
|---|---|
| `test_core.test_normal_result_flow_is_atomic_and_batch_does_not_wait_for_root` | `first_batch_completion_inserts_exactly_one_batch_results_ready`, `result_ack_does_not_change_task_completion` |
| `test_core.test_stale_attempt_cannot_ack_or_promote_checkpoint` | `stale_ack_cannot_complete_task`, `checkpoint_is_fenced_by_attempt_epoch` |
| `test_core.test_expired_lease_cannot_be_renewed_or_acknowledged_before_sweep` | core predicate `expired_lease_is_stale_before_sweeper` + claim/ack fencing tests |
| `test_core.test_writer_unknown_quiescence_never_blindly_retries` | `lease_expiration_alone_does_not_permit_duplicate_writer`, `writer_quiescence_unknown_suspends` |
| `test_core.test_dependency_is_not_claimable_until_parent_completes` | `dependency_is_not_claimable_until_parent_completes` |
| `test_core.test_late_terminal_confirmation_refines_lost_physical_history` | `stale_lost_can_refine_to_terminated` |
| `test_core.test_partition_retirement_rejects_nonterminal_tasks_atomically` | `retire_rejects_nonterminal_task` |
| `test_v012_closure.test_ack_success_without_execution_id_cannot_complete_running_writer` | `omitted_execution_id_cannot_bypass_writer_safety` |
| `test_v012_closure.test_writer_success_uses_frozen_execution_isolation` | `isolated_writer_may_safely_recover` |
| `test_v012_closure.test_cancelled_writer_keeps_safety_escalation_open` | `cancelled_writer_still_requires_quiescence` |
| `test_v012_closure.test_retire_rejects_open_cancelled_writer_safety_obligation` | `open_writer_safety_escalation_blocks_retire` |
| `test_v012_closure.test_unassigned_initializing_and_reviving_excess_retire_without_drain` | `excess_initializing_retires_directly_and_fences_presence`, `excess_reviving_retires_directly` |
| `test_v012_closure.test_semantic_retirement_fences_idle_reusable_incarnation` | `semantic_retirement_fences_live_incarnation_lost` |
| `test_v012_closure.test_running_confirmation_atomically_renews_near_deadline_lease` | `running_confirmation_and_first_lease_renewal_are_atomic` |
| `test_v012_closure.test_merge_adds_declared_capacities_and_preserves_population` | `merge_sums_desired_capacity` |
| `test_v012_closure.test_merge_migrates_future_task_classification_not_active_authority` | `merge_migrates_future_task_classification`, `active_attempt_keeps_frozen_authority_through_merge` |
| `test_recovery_runtime.test_claim_survives_process_exit…` / `test_process_restart` | `restart_recovery_prevents_blind_duplicate_execution` |
| `test_recovery_runtime.test_root_failure_does_not_change_result_or_batch` | Result/Batch complete before any delivery; Root ACK is consumption only |
| filesystem outbox ACK | `notifier_ack_allows_pending_to_acked`, `notifier_ack_from_delivered` |
| `test_v012_closure.test_unavailable_execution_target_is_normalized_not_raised` (mechanical class only) | `unavailable_runtime_configuration_is_standardized_failure`, `configuration_unavailable_with_running_writer_must_not_retry` |
| `test_core.test_batch_cancellation_revokes_authority_and_preserves_completed_result` | `cancel_queued_batch_cancels_tasks_and_batch`, `cancel_active_read_only_batch_closes_attempts_and_releases_agents`, `cancel_batch_is_idempotent`, `cancel_rejects_completed_batch` |
| `test_core.test_writer_cancellation_does_not_release_unknown_physical_writer` | `cancel_active_writer_with_unknown_quiescence_keeps_obligation_open`, `cancel_suspended_batch_preserves_open_writer_obligation`, `cancelled_writer_still_requires_quiescence` |

### PYTHON/ADAPTER-SPECIFIC — defer to M5

Daemon/heartbeat/notifier threads, adapter absolute deadlines, Codex/Grok
process handles, RootBridge transports, CLI, profile registry,
`dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`,
empty ExecutionProfile registry, notifier isolation. These are [16] §A2.
`collect_outcome` vs `reconcile_start` quiescence inheritance is **not**
deferred: its kernel representation and regressions are frozen in M4 (see
"Kernel rules frozen for M4" above); the M5 dispatcher only wires the oracle
callsite that already exists in `runtime.py`.

### OBSOLETE / not ported

Python SQLite v1–v7 in-place migrations (`test_storage_migration.py`).
D-DB-MIGRATE is unresolved; this landing uses a new database. Do not claim
in-place upgrade.

## Schema decisions

- New database. No import of Python V0.1 files.
- `schema_migrations.version = 1` is the Rust-era kernel.
- Databases carry a permanent lineage marker
  `scheduler_meta.implementation_line = "rust-v0.2"`; any pre-existing
  database without it is rejected at open (Python V0.1 databases also
  start at schema version 1, so the version number alone cannot identify a
  lineage — D-DB-MIGRATE remains unresolved and import is never guessed).
- Table/column names follow the semantic unique constraints, not a copy of
  Python `SCHEMA` as an architecture document. Uniqueness that the oracle
  enforced (partial unique indexes, `required = 1` trigger) is preserved.
- `tasks` has no `generation_id`. M6 may add a nullable column later; M4
  MUST NOT require it.
- Incarnation `generation INTEGER` is physical presence numbering.

In-memory connections (`Kernel::open_memory`) are a test convenience: SQLite
reports `journal_mode=MEMORY`. File databases used by `Kernel::open` are WAL
+ `synchronous=FULL`. The WAL conformance test opens a file.

## Deviations from Python structure (intentional)

- No `core.py` transliteration. Kernel API is transactional operations on
  typed IDs (`claim_next_available`, `ack_success`, `record_physical_outcome`,
  …), not a Scheduler god-class with `get(table, id)`.
- Domain errors live in `agentype-core::Error`. Mechanical `FailureClass` is
  separate. Storage I/O is mapped into `Error::InvariantViolation` /
  `Error::Conflict`; a later split is allowed if it does not leak rusqlite
  types into Core.
- Adapter trait + `FakeAdapter` live in `agentype-adapter-api`. M4 tests
  drive the kernel directly, as the Python unit oracle did.
- Runtime crate only sequences authority recovery. No dispatcher loop.
  **Recovery scope (authority half)**: the M4 restart barrier covers spec 14
  steps 1/2/5/6 and the kernel/decision side of steps 3/4 — identify and
  expire overdue authority and never-created claims, apply the deterministic
  read-only retry policy with writer-quiescence suspension, promote eligible
  retry waits, reconcile the pool and revive non-RETIRED agents. Steps 3/4
  require the owning adapter (reconcile/collect_outcome over persisted
  handles, confirming terminal outcomes back into physical history) and
  belong to M5 §A2 runtime/adapter parity; their kernel representation is
  frozen here (proof-bit rules, UNKNOWN→RUNNING, LOST→TERMINATED). This
  landing therefore claims the authority half of recovery only.

## Test fixture discipline

Test-only state construction lives below the persistence boundary:
`tests/common/mod.rs` opens the schema via `Kernel::open(file)` and mutates
rows directly (short-lived connections, `PRAGMA foreign_keys=ON`, semantic
helpers such as `fixture_agent_state` / `fixture_incarnation` /
`fixture_execution`). The operation under test always runs through the Kernel
public API afterwards. The production API surface contains no unrestricted
state setter: `set_logical_agent_state`, the public `ensure_incarnation`
wrapper, and `warm_incarnation` were removed in this landing. States that the
schema itself forbids are asserted to be rejected by the database.

## Known limitations (still M4-complete)

- Pool matching and MOVE/MERGE/RETIRE cover the required M4 cases and the
  V0.1 topology composition regressions ported in this landing (consecutive
  MOVE / MERGE pending rebasing, target retention adoption, SUSPENDED
  identity through temporary replacement, MOVE/MERGE x lease-expiry ordering,
  assignment-boundary retirement fencing, multi-task partial Batch). Any
  further V0.1 kernel rule found by the M5 oracle port remains required and
  is not deferred as M6.
- `Kernel::set_logical_agent_state` does not exist; tests place
  INITIALIZING / REVIVING members through storage fixtures, and production
  births READY (V0.1 oracle).
- Heartbeat bulk renewal of a daemon-admitted set is M5. Single-attempt
  `heartbeat` requires a persisted RUNNING Execution.
- Outbox delivery/backoff clock is M5. M4 persists states and ACK.
- `thiserror` is listed on some crates but Core errors are handwritten.

## Unresolved M6 items

Do not resolve from this code. See `docs/specs/v0.2/17-deferred-open-questions.md`:

D-GEN-POLICY, D-GEN-INTRA, D-GEN-TOPOLOGY, D-GEN-RESUME, D-INTENT-SCHEMA,
D-INTENT-FANOUT, D-TYPE-REL, D-TYPE-REV, D-INFO-FN, D-MEM-SCHEMA,
D-MEM-PROMOTE, D-NEG-GC, D-CONTINUITY-BIND, D-ROOT-API, D-TRANSFORM-FAIL,
D-TOPOLOGY, D-OBJECTIVE, D-COMPILATION-CLOSURE.

D-DB-MIGRATE remains DOES_NOT_BLOCK_RIIR_KERNEL. D-ADAPTER2 is M7.

## Validation

```text
cargo test --workspace
```

Result on this landing (Windows, rustc via cargo 1.x):

- `agentype-core`: 19 tests (authority + `decisions` unit coverage)
- `agentype-adapter-api`: 3 tests
- `agentype-runtime`: 1 test
- `agentype-storage-sqlite` integration: 73 tests
  - `m4_kernel`: 48
  - `recovery`: 11
  - `topology`: 14
- total: 96 passed, 0 failed

Python production implementation was not modified. Two Python oracle
tests received timing-only stabilization so that the existing V0.1 CI
remains deterministic while Rust CI was added. No Python scheduler
semantics were changed, and the Python suite is not required to pass as
part of M4 Rust CI; the existing Python job remains for the V0.1 oracle
package.

## Scheduling-semantics placement (spec 15 direction)

Spec 15: storage persists core state; it MUST NOT define scheduling
semantics. This landing enforces the substance of the rule, not just the
import graph:

- `agentype-core::decisions` owns every scheduling decision as a pure,
  SQLite-free function with unit tests: claim matching rank and tiebreak,
  claim task eligibility, claim agent selection, cross-target cutover safety,
  partition cutover planning, agent release & post-safety revival dispositions,
  dependency release planning, durable quiescence, suspension failure
  classification, batch aggregate state, excess-member disposition,
  incarnation presence outcome, retry gating and backoff (over the frozen
  `RetryPolicy`).
- `agentype-storage-sqlite` loads authoritative rows, invokes core
  decisions inside `BEGIN IMMEDIATE`, persists results atomically, and
  enforces DB constraints; presence SQL translates the returned
  `PresenceAction` rather than deciding it.
- `agentype-runtime` may keep depending on the storage-backed `Kernel`
  in M4: the normative constraint forbids semantics defined in storage,
  not the runtime-to-persistence composition direction. Whether M5's
  dispatcher wants a storage-free command trait boundary is an M5 design
  decision to be driven by its actual needs, not anticipated here.
