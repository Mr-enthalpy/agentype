# RIIR M5.1 — Authoritative Execution-Launch Foundation & Runtime Configuration Boundary

Status: Historical Report  
Applies to: branch `rust/m5-runtime`  
Canonical path: `docs/reports/v0.2/riir-m5.1-launch-foundation.md`  
Not a specification.

This milestone resolves the boundary ambiguity between Scheduler `Claim` authority receipts and physical execution requests, establishing an encapsulated `ExecutionLaunchSnapshot` foundation, unforgeable `FrozenExecutionSafety` provenance, explicit `ExecutionResolutionMode`, target/profile compatibility checks, canonical `prepare_execution_launch` runtime façade, fail-closed runtime configuration layer, complete structured worker request contract, and bundled committed continuity for M5.

---

## 1. M4 Boundary Debt Closed

In M4, `Claim` was returned to callers as a composite DTO carrying both authority tokens (`attempt_id`, `lease_epoch`) and task execution metadata (`payload`, `acceptance`, `workspace_mode`, `workstream_id`). While `Kernel::create_execution` validated identity and frozen target/profile fields against the Attempt, it did not construct an immutable launch snapshot directly from durable state, leaving an architectural risk that future runtime code might directly replay mutable `Claim` fields into physical worker dispatch.

**M5.1 formally closes this boundary debt:**
- `Claim` is strictly treated as an **authority receipt**, not the physical execution source of truth.
- `Kernel::create_execution` reconstructs `ExecutionLaunchSnapshot` inside the SQLite transaction directly from durable `TaskRow`, `AttemptRow`, `LeaseRow`, `AgentRow` (committed continuity), and the newly created `Execution` row.
- `ExecutionLaunchSnapshot` and `ExecutionRequest` have all **private fields and readonly getter methods**, preventing downstream mutation or structural forgery.
- `ExecutionLaunchSnapshot::from_persisted_kernel_authority` is an `unsafe` storage-level constructor requiring callers to be within a fenced Kernel transaction.
- `FrozenExecutionSafety` cannot be forged in safe production code; it is produced exclusively through `ResolvedExecutionEnvironment::safety()` from authoritative registry resolution (or `FrozenExecutionSafety::unisolated` fail-safe default).
- `ResolvedExecutionEnvironment` has private fields to prevent post-resolution tampering with `attempt_isolation`.
- `ExecutionRequest::from_launch` in `agentype-adapter-api` ensures physical execution requests are constructed exclusively from `ExecutionLaunchSnapshot` + runtime handles, retaining the complete structured worker contract (`payload`, `acceptance`, `workstream_id`, `task_id`, `attempt_id`, `lease_epoch`, `continuity`, `workspace_mode`).
- `ExecutionRequest::for_testing` and `FrozenExecutionSafety::for_testing` are gated by `#[cfg(any(test, feature = "test-support"))]`, preventing test bypasses from appearing in production builds.

```text
Claim (receipt)
      ↓
resolve_execution_environment(ExecutionResolutionMode::Authoritative(reg), target, profile)
      ↓ [fails closed on missing target/profile or incompatible pair]
ResolvedExecutionEnvironment (opaque, private fields)
      ↓
prepare_execution_launch(&kernel, &claim, &env)
      ↓
Kernel::create_execution(&claim, env.safety())
      ↓ [inside SQLite transaction]
      ├─ Validates Attempt/Lease/Epoch fencing
      ├─ Cross-validates claim against Attempt
      ├─ Asserts safety.target == attempt.execution_target
      ├─ Asserts safety.profile == attempt.execution_profile
      ├─ Re-reads durable Task payload, acceptance, workspace_mode
      ├─ Re-reads Agent committed continuity capsule & version
      ├─ Re-reads Task continuity preference
      ├─ Freezes safety.attempt_isolation() on Execution row
      └─ Mints ExecutionId & RequestId
      ↓
ExecutionLaunchSnapshot (private fields, readonly getters)
      ↓
ExecutionRequest::from_launch(&launch, handle)
      ↓ [complete worker contract: payload, acceptance, continuity, IDs]
ExecutionAdapter.start_execution(&request)
```

---

## 2. API Changes

### `agentype-core`
- Added `CommittedContinuitySnapshot` in `records.rs` carrying `preference: ContinuityPreference`, `version: i64`, and `capsule: Value`.
- Sealed `FrozenExecutionSafety` in `records.rs` carrying `(execution_target, execution_profile, attempt_isolation)`:
  - `pub fn unisolated(target, profile) -> Self` (safe fail-safe default).
  - `pub unsafe fn from_resolved_authority(...) -> Self` (internal boundary with formal `# Safety` contract).
  - `#[cfg(any(test, feature = "test-support"))] pub fn for_testing(...) -> Self`.
- Sealed `ExecutionLaunchSnapshot` with all private fields and readonly getter methods:
  - `pub unsafe fn from_persisted_kernel_authority(...) -> Self` (internal storage boundary with formal `# Safety` contract).
  - `#[cfg(any(test, feature = "test-support"))] pub fn for_testing(...) -> Self`.

### `agentype-storage-sqlite`
- Updated signature:
  ```rust
  pub fn create_execution(
      &self,
      claim: &Claim,
      safety: FrozenExecutionSafety,
  ) -> Result<ExecutionLaunchSnapshot, Error>
  ```
- Asserts that `safety.execution_target()` and `safety.execution_profile()` strictly match `attempt.execution_target` and `attempt.execution_profile`. Mismatched safety proofs are rejected with `Error::InvalidAuthority`.
- Re-reads all task semantics (`payload`, `acceptance`, `workspace_mode`, `workstream_id`) directly from SQLite `tasks` table.
- Re-reads agent continuity state (`continuity_json`, `continuity_version`) directly from SQLite `logical_agents` table and bundles it into `CommittedContinuitySnapshot`.
- Added `create_workstream` to `Kernel` to match `core.py` 1:1.

### `agentype-adapter-api`
- Encapsulated `ExecutionRequest` with all private fields and readonly getter methods, retaining the complete structured worker contract (`task_id`, `batch_id`, `attempt_id`, `attempt_number`, `lease_id`, `lease_epoch`, `logical_agent_id`, `execution_target`, `execution_profile`, `workspace_mode`, `prompt`, `payload`, `acceptance`, `workstream_id`, `continuity`, `incarnation_runtime_handle`).
- Primary constructor `ExecutionRequest::from_launch(launch: &ExecutionLaunchSnapshot, incarnation_runtime_handle: RuntimeHandle) -> ExecutionRequest`.
- Test constructor `ExecutionRequest::for_testing(...)` gated with `#[cfg(any(test, feature = "test-support"))]`.

### `agentype-runtime`
- Defined `attempt_isolation: bool` on `ExecutionTargetConfig` (target mechanical isolation property).
- Defined `ExecutionProfileConfig` for model settings, timeouts, options, and optional `allowed_targets: Option<HashSet<String>>`.
- Explicit `ExecutionResolutionMode`:
  ```rust
  pub enum ExecutionResolutionMode<'a> {
      Authoritative(&'a ExecutionRegistry),
      DirectUnconfigured,
  }
  ```
- Opaque `ResolvedExecutionEnvironment` with private fields and `.safety() -> FrozenExecutionSafety`.
- Canonical launch preparation façade:
  ```rust
  pub fn prepare_execution_launch(
      kernel: &Kernel,
      claim: &Claim,
      environment: &ResolvedExecutionEnvironment,
  ) -> Result<ExecutionLaunchSnapshot, Error>
  ```
- Fail-closed registry registration: `register_target` and `register_profile` return `Result<(), ConfigurationError>`, rejecting duplicates, empty names, and non-positive timeouts.
- `resolve_execution_environment` fails closed on missing target/profile or incompatible target/profile pairs (`ResolutionError::Incompatible`).

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
| `execution_target` | `AttemptRow.execution_target` | Frozen on Attempt at claim time (asserted vs `safety.target`) |
| `execution_profile` | `AttemptRow.execution_profile` | Frozen on Attempt at claim time (asserted vs `safety.profile`) |
| `workspace_mode` | `TaskRow.workspace_mode` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `prompt` | `TaskRow.name` | Re-read from durable `tasks` row |
| `payload` | `TaskRow.payload_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `acceptance` | `TaskRow.acceptance_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `workstream_id` | `TaskRow.workstream_id` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `continuity` | `AgentRow.continuity_json` + `continuity_version` + `TaskRow.continuity` | Atomically loaded and bundled into `CommittedContinuitySnapshot` in tx |
| `attempt_isolation` | Runtime config registry (`ExecutionTargetConfig.attempt_isolation`), verified against Attempt target, frozen on `executions` row | Persisted before physical start; immutable for lifecycle |

---

## 4. Runtime Configuration & Attempt-Isolation Ownership

1. **Configuration Registry**:
   - Belongs strictly to runtime/composition (`agentype-runtime`), not Core semantics.
   - Core contains zero vendor, provider, endpoint, or model brand identifiers.
   - Registration is fail-closed against duplicate names, invalid names, and negative timeouts.
2. **Fail-Closed Resolution & Compatibility**:
   - In `ExecutionResolutionMode::Authoritative(reg)` (even if empty), missing targets or profiles return `ResolutionError::TargetNotFound` / `ResolutionError::ProfileNotFound` (RESOURCE_UNAVAILABLE). No silent fallback to adapter defaults occurs.
   - If a profile defines `allowed_targets` and the target is not in the set, resolution returns `ResolutionError::Incompatible`.
   - In `ExecutionResolutionMode::DirectUnconfigured` (permissive standalone mode), explicit unisolated defaults are populated.
3. **Attempt Isolation & Unforgeable Provenance**:
   - `attempt_isolation` represents whether the target execution environment mechanically guarantees attempt-scoped writer isolation.
   - Passed to `create_execution` bound to target and profile inside `FrozenExecutionSafety`.
   - Kernel verifies that the safety proof's target and profile match the Attempt's frozen target and profile.
   - Normal production code cannot instantiate a `FrozenExecutionSafety` with `attempt_isolation = true` outside of `resolve_execution_environment`.
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
| 5 | Can a mismatched or forged safety proof be used for a different Attempt target/profile? | **NO** | `mismatched_target_or_profile_safety_proof_rejected` regression verifies Kernel checks `safety.target == attempt.target` and `safety.profile == attempt.profile`. |
| 6 | Can an adapter decide or alter `attempt_isolation` after physical start? | **NO** | Persisted on `executions` table before physical start; adapter returns only execution observations. |
| 7 | Can later configuration changes rewrite the safety meaning of an already-created Execution? | **NO** | `executions.attempt_isolation` column is immutable. |
| 8 | Did this task add Generation, AgentType, SpawnSource, Transform, or other M6 semantics? | **NO** | M6 semantics remain completely prohibited and absent. |
| 9 | Does Core now contain vendor/model/frontend names? | **NO** | Core remains 100% vendor-neutral. |
| 10 | Does `ExecutionRequest` forward acceptance criteria, continuity, and task context to the adapter? | **YES** | All structured fields are present on `ExecutionRequest` with readonly getters. |

---

## 6. Test Suite & Validation Results

### Rust Workspace (`cargo test --workspace`)
**121 passed, 0 failed:**
- `agentype-adapter-api`: 4 passed (including `execution_request_constructed_from_launch_snapshot`)
- `agentype-core`: 20 passed
- `agentype-runtime`: 9 passed (including `incompatible_target_and_profile_fails_closed`, `end_to_end_launch_preserves_registry_isolation_fact`, `duplicate_target_or_profile_registration_fails_closed`, `invalid_configuration_parameters_fail_closed`, `explicitly_empty_registry_fails_closed`, `missing_profile_fails_closed`, `valid_target_and_profile_resolve_isolation`, `direct_unconfigured_mode_returns_unisolated_defaults`)
- `agentype-storage-sqlite`: 88 passed
  - `m4_kernel.rs`: 61 passed (including launch, continuity, mismatch & tamper regressions)
  - `recovery.rs`: 11 passed
  - `topology.rs`: 16 passed

### Quality & Linter Gates
- Local Verification:
  - `cargo fmt --check`: Clean (0 diffs)
  - `cargo clippy --workspace --all-targets -- -D warnings`: Clean (0 warnings)
- CI Automation:
  - GitHub Actions runs `cargo test --workspace` on all PRs.

### Python Oracle Suite (`py -3 -m unittest discover -s tests -t . -v`)
- **160 passed, 2 skipped, 0 failed** (100% parity preserved).

---

## 7. Next Steps: M5.2 Dispatcher

With M5.1 complete, the foundation is set for **M5.2**:
1. Implement the asynchronous Dispatcher worker loop in `agentype-runtime` using `resolve_execution_environment` (`ExecutionResolutionMode::Authoritative`) and `prepare_execution_launch`.
2. Connect `ExecutionAdapter` lifecycle (start -> observe -> interrupt -> terminate -> collect).
3. Implement heartbeat supervision thread and status notification loop.
