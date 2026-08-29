# RIIR M5.1 — Authoritative Execution-Launch Foundation & Runtime Configuration Boundary

Status: Historical Report  
Applies to: branch `rust/m5-runtime`  
Canonical path: `docs/reports/v0.2/riir-m5.1-launch-foundation.md`  
Not a specification.

This milestone resolves the boundary ambiguity between Scheduler `Claim` authority receipts and physical execution requests, establishing an encapsulated `ExecutionLaunchSnapshot` foundation, clean domain-versus-runtime configuration separation, authoritative physical `Incarnation` and `RuntimeHandle` binding, canonical `prepare_execution_launch` runtime façade, complete structured worker request contract, and bundled committed continuity for M5.

---

## 1. M4 Boundary Debt Closed

In M4, `Claim` was returned to callers as a composite DTO carrying both authority tokens (`attempt_id`, `lease_epoch`) and task execution metadata (`payload`, `acceptance`, `workspace_mode`, `workstream_id`). While `Kernel::create_execution` validated identity and frozen target/profile fields against the Attempt, it did not construct an immutable launch snapshot directly from durable state, leaving an architectural risk that future runtime code might directly replay mutable `Claim` fields into physical worker dispatch.

**M5.1 formally closes this boundary debt:**
- `Claim` is strictly treated as an **authority receipt**, not the physical execution source of truth.
- `Kernel::create_execution` reconstructs `ExecutionLaunchSnapshot` inside the SQLite transaction directly from durable `TaskRow`, `AttemptRow`, `LeaseRow`, `AgentRow` (committed continuity), `IncarnationsRow` (`runtime_handle_json`), and the newly created `Execution` row.
- `ExecutionLaunchSnapshot` and `ExecutionRequest` have all **private fields and readonly getter methods**, preventing downstream mutation.
- `ExecutionLaunchSnapshot::from_persisted_kernel_authority` defines the explicit **storage trust boundary** between `agentype-storage-sqlite` and `agentype-core`.
- Publicly-enableable test bypasses (`test-support` Cargo features and public `for_testing` constructors) have been eliminated.
- `#![forbid(unsafe_code)]` is enforced on `agentype-runtime`, and `#![deny(unsafe_code)]` is enforced on `agentype-adapter-api`.
- `ExecutionRequest::from_launch(&launch)` in `agentype-adapter-api` ensures physical execution requests are constructed exclusively from `ExecutionLaunchSnapshot`, binding both `incarnation_id` and `incarnation_runtime_handle` directly from durable storage without caller handle injection.

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
      ├─ Re-reads Incarnation ID and durable runtime_handle_json
      ├─ Freezes safety.attempt_isolation() on Execution row
      └─ Mints ExecutionId & RequestId
      ↓
ExecutionLaunchSnapshot (private fields, readonly getters)
      ↓
ExecutionRequest::from_launch(&launch)
      ↓ [complete worker contract: payload, acceptance, continuity, IDs, incarnation handle]
ExecutionAdapter.start_execution(&request)
```

---

## 2. Layering & API Architecture

### `agentype-core` (Pure Domain Layer)
- Strictly independent of adapters, timeouts, model options, and runtime resolution.
- `FrozenExecutionSafety`:
  - `pub fn unisolated(target, profile) -> Self` (no-isolation-assumption fail-safe).
  - `pub fn from_resolved_authority(target, profile, attempt_isolation: bool) -> Self` (called by authoritative configuration resolvers).
  - Readonly getters for `execution_target`, `execution_profile`, and `attempt_isolation`.
- `ExecutionLaunchSnapshot`:
  - Cross-crate storage constructor `pub unsafe fn from_persisted_kernel_authority(...)` with formal `# Safety` documentation.
  - Carries `incarnation_id: IncarnationId` and `incarnation_runtime_handle: Value`.
  - Readonly getters for all fields.

### `agentype-runtime` (Mechanical Runtime & Configuration Boundary)
- Enforces `#![forbid(unsafe_code)]`.
- Owns execution configuration models and registry:
  - `ExecutionTargetConfig`: `name`, `adapter_kind`, `attempt_isolation`, `options`.
  - `ExecutionProfileConfig`: `name`, `timeout_seconds`, `allowed_targets: Option<HashSet<String>>`, `options`.
  - `ExecutionRegistry`: fail-closed registration (`register_target`, `register_profile`), rejecting duplicates, empty names, and non-positive timeouts.
  - `ExecutionResolutionMode`:
    - `Authoritative(&'a ExecutionRegistry)`: Required for production daemon/dispatcher loops; fails closed on missing target/profile or incompatible pair (`ResolutionError::Incompatible`).
    - `DirectUnconfigured`: Standalone / single-shot test mode with unisolated defaults.
  - `ResolvedExecutionEnvironment`: Encapsulated private fields, readonly getters, and `.safety() -> FrozenExecutionSafety`.
  - `resolve_execution_environment(mode, target, profile) -> Result<ResolvedExecutionEnvironment, ResolutionError>`.
- Canonical launch preparation façade:
  ```rust
  pub fn prepare_execution_launch(
      kernel: &Kernel,
      claim: &Claim,
      environment: &ResolvedExecutionEnvironment,
  ) -> Result<ExecutionLaunchSnapshot, Error>
  ```
- Recovery orchestration: `recover_authority(&kernel) -> Result<ExpireReport, Error>`.

### `agentype-storage-sqlite` (Scheduler Storage Engine)
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
- Re-reads `incarnations.runtime_handle_json` from durable storage and bundles it into `ExecutionLaunchSnapshot`.

### `agentype-adapter-api` (Execution Adapter Contracts)
- Enforces `#![deny(unsafe_code)]`.
- `ExecutionRequest` retains complete structured worker contract:
  - `request_id`, `execution_id`, `task_id`, `batch_id`, `attempt_id`, `attempt_number`, `lease_id`, `lease_epoch`, `logical_agent_id`, `incarnation_id`, `execution_target`, `execution_profile`, `workspace_mode`, `prompt`, `payload`, `acceptance`, `workstream_id`, `continuity`, `incarnation_runtime_handle`.
- Closed derivation constructor:
  ```rust
  pub fn from_launch(launch: &ExecutionLaunchSnapshot) -> ExecutionRequest
  ```
- Readonly getters for all fields including `incarnation_id` and `incarnation_runtime_handle`.

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
| `incarnation_runtime_handle` | `incarnations.runtime_handle_json` | Loaded from durable `incarnations` row in tx |
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

## 4. Test Suite & Validation Results

### Rust Workspace (`cargo test --workspace`)
**121 passed, 0 failed:**
- `agentype-adapter-api`: 4 passed (including `execution_request_constructed_from_launch_snapshot`)
- `agentype-core`: 20 passed (domain authority, decision matrix, state machines)
- `agentype-runtime`: 9 passed (configuration, compatibility, fail-closed resolution, isolation persistence)
- `agentype-storage-sqlite`: 88 passed
  - `m4_kernel.rs`: 61 passed (including launch, continuity, mismatch & tamper regressions)
  - `recovery.rs`: 11 passed
  - `topology.rs`: 16 passed

### Quality & Linter Gates
- `cargo fmt --check`: Clean (0 diffs)
- `cargo clippy --workspace --all-targets -- -D warnings`: Clean (0 warnings)

### Python Oracle Suite (`py -3 -m unittest discover -s tests -t . -v`)
- **160 passed, 2 skipped, 0 failed** (100% parity preserved).
