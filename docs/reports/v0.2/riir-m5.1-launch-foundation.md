# RIIR M5.1 — Authoritative Execution-Launch Foundation & Runtime Configuration Boundary

Status: Historical Report  
Applies to: branch `rust/m5-runtime`  
Canonical path: `docs/reports/v0.2/riir-m5.1-launch-foundation.md`  
Not a specification.

This milestone resolves the boundary ambiguity between Scheduler `Claim` authority receipts and physical execution requests, establishing an immutable `ExecutionLaunchSnapshot` foundation and fail-closed runtime configuration layer for M5.

---

## 1. M4 Boundary Debt Closed

In M4, `Claim` was returned to callers as a composite DTO carrying both authority tokens (`attempt_id`, `lease_epoch`) and task execution metadata (`payload`, `acceptance`, `workspace_mode`, `workstream_id`). While `Kernel::create_execution` validated identity and frozen target/profile fields against the Attempt, it did not construct an immutable launch snapshot directly from durable state, leaving an architectural risk that future runtime code might directly replay mutable `Claim` fields into physical worker dispatch.

**M5.1 formally closes this boundary debt:**
- `Claim` is strictly treated as an **authority receipt**, not the physical execution source of truth.
- `Kernel::create_execution` reconstructs `ExecutionLaunchSnapshot` inside the SQLite transaction directly from durable `TaskRow`, `AttemptRow`, `LeaseRow`, and the newly created `Execution` row.
- `ExecutionRequest::from_launch` in `agentype-adapter-api` ensures physical execution requests are constructed exclusively from `ExecutionLaunchSnapshot` + runtime handles, with zero exposure to `Claim`, `Kernel`, or `SQLite`.

```text
Claim (receipt)
      ↓
Kernel::create_execution(claim, attempt_isolation)
      ↓ [inside SQLite transaction]
      ├─ Validates Attempt/Lease/Epoch fencing
      ├─ Cross-validates claim against Attempt
      ├─ Re-reads durable Task payload, acceptance, workspace_mode
      ├─ Freezes attempt_isolation on Execution row
      └─ Mints ExecutionId & RequestId
      ↓
ExecutionLaunchSnapshot (immutable authority)
      ↓
Future Dispatcher / Runtime
      ↓
ExecutionRequest::from_launch(&launch, handle)
      ↓
ExecutionAdapter
```

---

## 2. API Changes

### `agentype-core`
- Added `ExecutionLaunchSnapshot` in `records.rs` containing complete correctness-sensitive execution parameters.
- Re-exported `ExecutionLaunchSnapshot` in `agentype_core`.

### `agentype-storage-sqlite`
- Updated signature:
  ```rust
  pub fn create_execution(
      &self,
      claim: &Claim,
      attempt_isolation: bool,
  ) -> Result<ExecutionLaunchSnapshot, Error>
  ```
- Re-reads all task semantics (`payload`, `acceptance`, `workspace_mode`, `workstream_id`) directly from SQLite `tasks` table.

### `agentype-adapter-api`
- Added `ExecutionRequest::from_launch(launch: &ExecutionLaunchSnapshot, incarnation_runtime_handle: RuntimeHandle) -> ExecutionRequest`.

### `agentype-runtime`
- Added `ExecutionTargetConfig`, `ExecutionProfileConfig`, `ExecutionRegistry`, `ResolvedExecutionEnvironment`, and `ResolutionError`.
- Added `resolve_execution_environment(registry: Option<&ExecutionRegistry>, target_name: &str, profile_name: &str) -> Result<ResolvedExecutionEnvironment, ResolutionError>`.

---

## 3. Source-of-Truth Ownership Table

Every physical launch field has an unambiguous, enforced authoritative owner:

| Launch Field | Authoritative Source | Validation / Security Boundary |
|---|---|---|
| `task_id` | `AttemptRow.task_id` | Checked against `claim.task_id` (tamper-rejected) |
| `batch_id` | `TaskRow.batch_id` | Re-read from durable `tasks` row |
| `attempt_id` | `AttemptRow.id` | Loaded via `validate_authority_tx` |
| `attempt_number` | `AttemptRow.attempt_number` | Loaded from durable `attempts` row |
| `lease_id` | `LeaseRow.id` | Loaded from active `leases` row |
| `lease_epoch` | `LeaseRow.epoch` / `AttemptRow.lease_epoch` | Fencing token asserted in tx |
| `lease_expires_at` | `LeaseRow.expires_at` | Enforced `>= now` in tx |
| `logical_agent_id` | `AttemptRow.logical_agent_id` | Checked against `claim.logical_agent_id` |
| `incarnation_id` | `ensure_incarnation` transaction output | Fails closed if agent is RETIRED |
| `execution_id` | `ExecutionId::new()` | Minted in transaction; matches `executions.id` |
| `request_id` | `RequestId::new()` | Minted in transaction; matches `executions.request_id` |
| `execution_target` | `AttemptRow.execution_target` | Frozen on Attempt at claim time (immune to MERGE) |
| `execution_profile` | `AttemptRow.execution_profile` | Frozen on Attempt at claim time (immune to MERGE) |
| `workspace_mode` | `TaskRow.workspace_mode` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `prompt` | `TaskRow.name` | Re-read from durable `tasks` row |
| `payload` | `TaskRow.payload_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `acceptance` | `TaskRow.acceptance_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `workstream_id` | `TaskRow.workstream_id` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `attempt_isolation` | Runtime config registry, then frozen on `executions` row | Persisted before physical start; immutable for lifecycle |

---

## 4. Runtime Configuration & Attempt-Isolation Ownership

1. **Configuration Registry**:
   - Belongs strictly to runtime/composition (`agentype-runtime`), not Core semantics.
   - Core contains zero vendor, provider, endpoint, or model brand identifiers.
2. **Fail-Closed Resolution**:
   - When an explicit `ExecutionRegistry` is provided (even if empty), missing targets or profiles return `ResolutionError::TargetNotFound` / `ResolutionError::ProfileNotFound` (RESOURCE_UNAVAILABLE). No silent fallback to adapter defaults occurs.
   - When `registry` is `None` (permissive direct-caller mode), explicit defaults are populated without ambiguity.
3. **Attempt Isolation**:
   - `attempt_isolation` is determined by runtime profile configuration, passed to `create_execution`, and durably written to `executions.attempt_isolation`.
   - Adapters cannot observe, declare, or alter `attempt_isolation` after start.
   - Writer safety and crash recovery strictly inspect the persisted Execution fact.

---

## 5. Review Checklist (10 Verification Questions)

| # | Question | Answer | Evidence |
|---|---|---|---|
| 1 | Can a mutated Claim widen filesystem authority? | **NO** | `mutated_claim_workspace_mode_cannot_widen_launch_authority` regression proves Task `ReadOnly` persists even if Claim is set to `Write`. |
| 2 | Can a mutated Claim replace Task payload or acceptance criteria? | **NO** | `mutated_claim_payload_does_not_alter_launch_snapshot` & `mutated_claim_acceptance_does_not_alter_launch_snapshot` regressions verify durable DB values are used. |
| 3 | Does topology MERGE after claim change the current Attempt's physical target/profile? | **NO** | `claim_on_source_then_merge_before_execution_preserves_frozen_target` regression proves frozen Attempt fields govern launch. |
| 4 | Can future runtime construct a real `ExecutionRequest` without re-reading authoritative Scheduler state? | **NO** | `ExecutionRequest::from_launch` strictly consumes `ExecutionLaunchSnapshot`. |
| 5 | Can a missing profile silently fall back to an adapter default? | **NO** | `missing_profile_fails_closed` unit test confirms `ResolutionError::ProfileNotFound`. |
| 6 | Can an adapter decide or alter `attempt_isolation` after physical start? | **NO** | Persisted on `executions` table before physical start; adapter returns only execution observations. |
| 7 | Can later configuration changes rewrite the safety meaning of an already-created Execution? | **NO** | `executions.attempt_isolation` column is immutable. |
| 8 | Did this task add Generation, AgentType, SpawnSource, Transform, or other M6 semantics? | **NO** | M6 semantics remain completely prohibited and absent. |
| 9 | Does Core now contain vendor/model/frontend names? | **NO** | Core remains 100% vendor-neutral. |
| 10 | Can M5.2 Dispatcher consume the new launch/config interfaces without making new authority decisions? | **YES** | Flow is mechanical: claim -> resolve config -> create execution (get snapshot) -> dispatch. |

---

## 6. Test Suite & Validation Results

### Rust Workspace (`cargo test --workspace`)
**114 passed, 0 failed:**
- `agentype-adapter-api`: 4 passed (including `execution_request_constructed_from_launch_snapshot`)
- `agentype-core`: 20 passed
- `agentype-runtime`: 5 passed (including `explicitly_empty_registry_fails_closed`, `missing_profile_fails_closed`, `valid_target_and_profile_resolve_isolation`, `unsupplied_registry_returns_direct_caller_mode`)
- `agentype-storage-sqlite`:
  - `m4_kernel.rs`: 59 passed (including 6 new launch & tamper regressions)
  - `recovery.rs`: 11 passed
  - `topology.rs`: 16 passed

### Linters & Quality Checks
- `cargo fmt --check`: Clean (0 diffs)
- `cargo clippy --workspace --all-targets -- -D warnings`: Clean (0 warnings)

### Python Oracle Suite (`py -3 -m unittest discover -s tests -t . -v`)
- **160 passed, 2 skipped, 0 failed** (100% parity preserved).

---

## 7. Next Steps: M5.2 Dispatcher

With M5.1 complete, the foundation is set for **M5.2**:
1. Implement the asynchronous Dispatcher worker loop in `agentype-runtime` using `resolve_execution_environment` and `create_execution`.
2. Connect `ExecutionAdapter` lifecycle (start -> observe -> interrupt -> terminate -> collect).
3. Implement heartbeat supervision thread and status notification loop.
