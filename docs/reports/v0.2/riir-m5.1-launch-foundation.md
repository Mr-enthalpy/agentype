# RIIR M5.1 — Authoritative Execution-Launch Foundation & Runtime Configuration Boundary

Status: Historical Report  
Applies to: branch `rust/m5-runtime`  
Canonical path: `docs/reports/v0.2/riir-m5.1-launch-foundation.md`  
Not a specification.

This milestone resolves the boundary ambiguity between Scheduler `Claim` authority receipts and physical execution requests, establishing an encapsulated `ExecutionLaunchSnapshot` foundation, clean domain-versus-runtime configuration separation via `agentype-execution-config`, unforgeable compile-time isolated safety proofs, authoritative physical `Incarnation` and `RuntimeHandle` binding, a canonical `prepare_execution_launch` runtime façade that assembles the worker request with the runtime-rendered V0.1 worker prompt protocol (the durable Task label is never sent as the prompt), and bundled committed continuity for M5.

---

## 1. M4 Boundary Debt Closed

In M4, `Claim` was returned to callers as a composite DTO carrying both authority tokens (`attempt_id`, `lease_epoch`) and task execution metadata (`payload`, `acceptance`, `workspace_mode`, `workstream_id`). While `Kernel::create_execution` validated identity and frozen target/profile fields against the Attempt, it did not construct an immutable launch snapshot directly from durable state, leaving an architectural risk that future runtime code might directly replay mutable `Claim` fields into physical worker dispatch.

**M5.1 formally closes this boundary debt:**
- `Claim` is strictly treated as an **authority receipt**, not the physical execution source of truth.
- `Kernel::create_execution` reconstructs `ExecutionLaunchSnapshot` inside the SQLite transaction directly from durable `TaskRow`, `AttemptRow`, `LeaseRow`, `AgentRow` (committed continuity), `IncarnationsRow` (`runtime_handle_json`), and the newly created `Execution` row.
- `ExecutionLaunchSnapshot` and `ExecutionRequest` have all **private fields and readonly getter methods**, preventing downstream mutation.
- `ExecutionLaunchSnapshot::from_persisted_kernel_authority` defines the explicit **storage trust boundary** between `agentype-storage-sqlite` and `agentype-core` / `agentype-execution-config`. Its contract requires: *"The only safe production construction path is the fenced Kernel execution-creation transaction."*
- Publicly-enableable test bypasses (`test-support` Cargo features and public `for_testing` constructors) have been eliminated across all workspace crates.
- `#![forbid(unsafe_code)]` is enforced on `agentype-runtime`, and `#![deny(unsafe_code)]` is enforced on `agentype-adapter-api`.
- `ExecutionRequest::from_launch(&launch, rendered_prompt)` in `agentype-adapter-api` ensures physical execution requests are constructed exclusively from `ExecutionLaunchSnapshot` plus the runtime-rendered worker prompt, binding both `incarnation_id` and `incarnation_runtime_handle` directly from durable storage without caller handle injection. Adapters MUST NOT compose scheduler semantics into the prompt themselves.

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
ExecutionLaunchSnapshot (private fields, readonly getters; task_name = durable Task label)
      ↓
runtime::render_worker_prompt(&launch)
      ↓ [V0.1 worker protocol: IDs, epoch, workstream, objective(payload),
         acceptance, continuity, + writer recovery rules when WRITE]
ExecutionRequest::from_launch(&launch, rendered_prompt)
      ↓ [complete worker contract: rendered prompt + payload, acceptance,
         continuity, IDs, incarnation handle]
ExecutionAdapter.start_execution(&request)
```

---

## 2. Layering & API Architecture

```text
[agentype-core] (100% Pure Domain, IDs, States, Clock, Authority Predicates)
      ▲
      │ (depends on core)
[agentype-execution-config] (Configuration Authority, Zero I/O)
      ├── FrozenExecutionSafety (pub(crate) new, pub unisolated)
      ├── ExecutionTargetConfig { name, adapter_kind, attempt_isolation, options }
      ├── ExecutionProfileConfig { name, timeout_seconds, allowed_targets, options }
      ├── ExecutionRegistry (fail-closed registration)
      ├── ExecutionResolutionMode (Authoritative / DirectUnconfigured)
      ├── ResolvedExecutionEnvironment (opaque, .safety() -> FrozenExecutionSafety)
      ├── resolve_execution_environment
      └── ExecutionLaunchSnapshot (storage trust boundary)
      ▲                     ▲
      │ (depends on config) │ (depends on config + storage)
[agentype-storage-sqlite]   [agentype-runtime] (#![forbid(unsafe_code)])
      │                     ├── prepare_execution_launch(kernel, claim, env) -> PreparedExecutionLaunch
      │                     ├── render_worker_prompt(&launch) (V0.1 worker protocol)
      │                     └── recover_authority(kernel)
      ▲
      │ (depends on core + config)
[agentype-adapter-api] (#![deny(unsafe_code)])
      ├── ExecutionRequest::from_launch(&launch, rendered_prompt)
      └── ExecutionAdapter trait
```

### `agentype-core` (Pure Domain Layer)
- Strictly independent of adapters, timeouts, model options, and runtime resolution.
- Pure domain models, IDs, states, clock abstractions, and writer safety predicates.

### `agentype-execution-config` (Configuration Authority Layer)
- Owns execution configuration models and registry:
  - `ExecutionTargetConfig`: `name`, `adapter_kind`, `attempt_isolation`, `options`.
  - `ExecutionProfileConfig`: `name`, `timeout_seconds`, `allowed_targets: Option<HashSet<String>>`, `options`.
  - `ExecutionRegistry`: fail-closed registration (`register_target`, `register_profile`), rejecting duplicates, empty names, and non-positive timeouts.
  - `ExecutionResolutionMode`:
    - `Authoritative(&'a ExecutionRegistry)`: Required for production daemon/dispatcher loops; fails closed on missing target/profile or incompatible pair (`ResolutionError::Incompatible`).
    - `DirectUnconfigured`: Standalone / single-shot test mode with unisolated defaults.
  - `FrozenExecutionSafety`:
    - Constructor accepting `attempt_isolation: bool` is sealed as **`pub(crate)`**.
    - Safe public constructor: `pub fn unisolated(target, profile) -> Self` (`attempt_isolation = false`, no-isolation-assumption fail-safe).
    - Unforgeable: Ordinary safe code cannot forge `attempt_isolation = true` without authoritative registry resolution.
  - `ResolvedExecutionEnvironment`: Encapsulated private fields, readonly getters, and `.safety() -> FrozenExecutionSafety`.
  - `resolve_execution_environment(mode, target, profile) -> Result<ResolvedExecutionEnvironment, ResolutionError>`.
  - `ExecutionLaunchSnapshot`:
    - Private fields for all execution launch parameters.
    - `pub unsafe fn from_persisted_kernel_authority(...) -> Self` (storage trust boundary with formal `# Safety` invariant).

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

### `agentype-runtime` (Mechanical Runtime Façade)
- Enforces `#![forbid(unsafe_code)]`.
- Canonical launch preparation façade:
  ```rust
  pub fn prepare_execution_launch(
      kernel: &Kernel,
      claim: &Claim,
      environment: &ResolvedExecutionEnvironment,
  ) -> Result<PreparedExecutionLaunch, Error>
  ```
  Returns `PreparedExecutionLaunch { snapshot, request }`: the runtime façade is the single composition point between durable Scheduler facts and the physical worker contract.
- `render_worker_prompt(&ExecutionLaunchSnapshot) -> String` renders the provider-neutral worker protocol exactly as the V0.1 Python oracle (`Dispatcher._render_prompt`): `LOCAL AGENT SCHEDULER TASK` / `TASK_ID` / `ATTEMPT_ID` / `LEASE_EPOCH` / `WORKSTREAM` (or `none`) / `OBJECTIVE` (task payload) / `ACCEPTANCE` / `COMMITTED CONTINUITY`, plus `WRITER RECOVERY RULES` for WRITE tasks and a closing `RETURN` section, joined by blank lines; JSON sections use `json.dumps(sort_keys=True, ensure_ascii=False)` semantics. The durable Task label (`task_name`) is deliberately not part of the protocol.
- Recovery orchestration: `recover_authority(&kernel) -> Result<ExpireReport, Error>`.
- Verified regressions: `recovery_follows_persisted_isolation_fact_despite_registry_reconfiguration` and its control `unisolated_writer_expiry_without_quiescence_suspends` form a discriminating pair (identical retryable WRITE policy): recovery must follow the persisted Execution `attempt_isolation` fact (RETRY_WAIT and a replacement Attempt) even after the registry is reconfigured, while the unisolated control suspends with `WRITER_QUIESCENCE_UNKNOWN`. Worker-prompt regressions prove the request prompt is the derived V0.1 protocol and never the bare Task name.

### `agentype-adapter-api` (Execution Adapter Contracts)
- Enforces `#![deny(unsafe_code)]`.
- `ExecutionRequest` retains complete structured worker contract:
  - `request_id`, `execution_id`, `task_id`, `batch_id`, `attempt_id`, `attempt_number`, `lease_id`, `lease_epoch`, `logical_agent_id`, `incarnation_id`, `execution_target`, `execution_profile`, `workspace_mode`, `prompt` (the runtime-rendered worker protocol, NOT the Task name), `payload`, `acceptance`, `workstream_id`, `continuity`, `incarnation_runtime_handle`.
- Closed derivation constructor:
  ```rust
  pub fn from_launch(launch: &ExecutionLaunchSnapshot, prompt: String) -> ExecutionRequest
  ```
  `prompt` MUST be produced by `agentype_runtime::render_worker_prompt(&launch)`.
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
| `task_name` | `TaskRow.name` | Re-read from durable `tasks` row; durable task label fact only — never the worker prompt |
| worker prompt (`ExecutionRequest.prompt`) | Derived: `runtime::render_worker_prompt(&launch)` from the full launch protocol (IDs, epoch, workstream, payload, acceptance, continuity, workspace mode) | Rendered by the runtime façade (V0.1 worker protocol parity); adapters MUST NOT compose scheduler semantics |
| `payload` | `TaskRow.payload_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `acceptance` | `TaskRow.acceptance_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `workstream_id` | `TaskRow.workstream_id` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `continuity` | `AgentRow.continuity_json` + `continuity_version` + `TaskRow.continuity` | Atomically loaded and bundled into `CommittedContinuitySnapshot` in tx |
| `attempt_isolation` | Runtime config registry (`ExecutionTargetConfig.attempt_isolation`), verified against Attempt target, frozen on `executions` row | Persisted before physical start; immutable for lifecycle |

---

## 4. Test Suite & Validation Results

### Rust Workspace (`cargo test --workspace`)
**128 passed, 0 failed:**
- `agentype-adapter-api`: 4 passed (including `execution_request_constructed_from_launch_snapshot`)
- `agentype-core`: 20 passed (domain authority, decision matrix, state machines)
- `agentype-execution-config`: 7 passed (registry validation, configuration error checking, resolution modes, fail-closed handling)
- `agentype-runtime`: 7 passed (end-to-end isolation persistence, discriminating registry-reconfiguration pair, recovery isolation, V0.1 worker-prompt protocol regressions)
- `agentype-storage-sqlite`: 90 passed
  - `m4_kernel.rs`: 63 passed (including launch, continuity, mismatch & tamper regressions, and workstream project_state_ref birth seeding)
  - `recovery.rs`: 11 passed
  - `topology.rs`: 16 passed

### Quality & Linter Gates
- `cargo fmt --check`: Clean (0 diffs)
- `cargo clippy --workspace --all-targets -- -D warnings`: Clean (0 warnings)

### Python Oracle Suite (`py -3 -m unittest discover -s tests -t . -v`)
- **160 passed, 2 skipped, 0 failed** (100% parity preserved).

---

## 5. Review Round Corrections (post-completion)

PR review surfaced four findings against this milestone; all are incorporated on `rust/m5-runtime`:

1. **Worker prompt was the Task name (P1, corrected).** The original source-of-truth table mapped `prompt ← TaskRow.name`, which would have sent a bare task label to real workers instead of the V0.1 task protocol. The snapshot field is now `task_name` (durable label fact only), and the worker prompt is a derived representation rendered by `agentype_runtime::render_worker_prompt` from the full launch protocol, replicating the V0.1 oracle `Dispatcher._render_prompt` (including writer recovery rules for WRITE tasks). `ExecutionRequest::from_launch` takes the rendered prompt explicitly; adapters must not compose scheduler semantics.
2. **Newborn continuity ignored workstream project_state_ref (P1, corrected).** `birth_agent` hardcoded `continuity_json = '{}'`, so the authoritative project baseline never reached workstream-bound newborn agents and the launch snapshot preserved an empty capsule. Birth now seeds `{"CURRENT CHECKPOINT": {"project_state_ref": ...}}` per the V0.1 oracle (fail-closed on unknown workstreams). The affinity-birth runtime primitive (`ensure_task_consumers`) itself remains M5.2.
3. **Registry-reconfiguration evidence was non-discriminating (P2, corrected).** The original test used a task whose default retry policy forbids mechanical retry, so SUSPENDED occurred under both correct and broken recovery. It is replaced by a discriminating pair with an identical retryable WRITE policy: recovery must follow the persisted `attempt_isolation = true` fact to RETRY_WAIT and a replacement Attempt even after the registry flips to `false`, while the unisolated control suspends with `WRITER_QUIESCENCE_UNKNOWN`.
4. **Report test count was internally inconsistent (P2, corrected).** The earlier revision claimed 121 passed while its own breakdown summed to 122. Counts in this revision (128) are the measured workspace totals after the corrections above.
