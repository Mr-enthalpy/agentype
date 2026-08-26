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
| [03] Task/Attempt/Lease/Result/Batch/Escalation/Outbox machines | yes; Task has **no** Generation membership |
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
| [16] §A M4 tests | `crates/agentype-storage-sqlite/tests/m4_kernel.rs` |

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
| `test_v012_closure.test_unassigned_initializing_and_reviving_excess_retire_without_drain` | `excess_initializing_retires_directly`, `excess_reviving_retires_directly` |
| `test_v012_closure.test_semantic_retirement_fences_idle_reusable_incarnation` | `semantic_retirement_fences_live_incarnation_lost` |
| `test_v012_closure.test_running_confirmation_atomically_renews_near_deadline_lease` | `running_confirmation_and_first_lease_renewal_are_atomic` |
| `test_v012_closure.test_merge_adds_declared_capacities_and_preserves_population` | `merge_sums_desired_capacity` |
| `test_v012_closure.test_merge_migrates_future_task_classification_not_active_authority` | `merge_migrates_future_task_classification`, `active_attempt_keeps_frozen_authority_through_merge` |
| `test_recovery_runtime.test_claim_survives_process_exit…` / `test_process_restart` | `restart_recovery_prevents_blind_duplicate_execution` |
| `test_recovery_runtime.test_root_failure_does_not_change_result_or_batch` | Result/Batch complete before any delivery; Root ACK is consumption only |
| filesystem outbox ACK | `notifier_ack_allows_pending_to_acked`, `notifier_ack_from_delivered` |
| `test_v012_closure.test_unavailable_execution_target_is_normalized_not_raised` (mechanical class only) | `unavailable_runtime_configuration_is_standardized_failure` |

### PYTHON/ADAPTER-SPECIFIC — defer to M5

Daemon/heartbeat/notifier threads, adapter absolute deadlines, Codex/Grok
process handles, RootBridge transports, CLI, profile registry,
`dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`,
`collect_outcome` vs `reconcile_start` quiescence inheritance, empty
ExecutionProfile registry, notifier isolation. These are [16] §A2.

### OBSOLETE / not ported

Python SQLite v1–v7 in-place migrations (`test_storage_migration.py`).
D-DB-MIGRATE is unresolved; this landing uses a new database. Do not claim
in-place upgrade.

## Schema decisions

- New database. No import of Python V0.1 files.
- `schema_migrations.version = 1` is the Rust-era kernel.
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

## Known limitations (still M4-complete)

- Pool matching, MOVE/MERGE, and RETIRE cover the required M4 cases; not
  every Python topology composition test is ported (pending membership across
  consecutive moves, retention adoption, temporary replacement convergence).
  Those are V0.1 kernel rules and remain required if a gap is found; they are
  not deferred as M6.
- `Kernel::set_logical_agent_state` exists so tests can place INITIALIZING /
  REVIVING members. Production path births READY (V0.1 oracle).
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

- `agentype-core`: 4 tests
- `agentype-adapter-api`: 1 test
- `agentype-runtime`: 1 test
- `agentype-storage-sqlite` integration `m4_kernel`: 40 tests
- total: 46 passed, 0 failed

Python tests were not modified and are not required to pass as part of M4
Rust CI. The existing Python job remains for the V0.1 oracle package.
