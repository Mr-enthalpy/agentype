# RIIR M5.1 — Authoritative Execution-Launch Foundation & Runtime Configuration Boundary

Status: Historical Report  
Applies to: branch `rust/m5-runtime`  
Canonical path: `docs/reports/v0.2/riir-m5.1-launch-foundation.md`  
Not a specification.

This milestone resolves the boundary ambiguity between Scheduler `Claim` authority receipts and physical execution requests, establishing an encapsulated `ExecutionLaunchSnapshot` foundation, clean domain-versus-runtime configuration separation via `agentype-execution-config`, compile-time-sealed isolated safety facts, authoritative physical `Incarnation` and `RuntimeHandle` binding, a canonical `prepare_execution_launch` runtime façade whose configuration resolution is keyed by durable Attempt authority (`Kernel::resolve_execution_binding` precedes resolution; fenced revalidation follows it) and assembles the worker request (its prompt is the deterministic V0.1 worker protocol derived inside `agentype-adapter-api`; the durable Task label is never sent as the prompt and no caller text can be injected), and bundled committed continuity for M5.

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
- `ExecutionRequest::from_launch(&launch, &resolved_environment)` in `agentype-adapter-api` ensures physical execution requests are constructed exclusively from `ExecutionLaunchSnapshot` plus the authoritative resolved environment, with the worker prompt deterministically derived as a `RenderedWorkerPrompt` (no caller-supplied text is accepted), binding both `incarnation_id` and `incarnation_runtime_handle` directly from durable storage without caller handle injection. (M5.2 audit refinement: the resolved environment also carries target options, profile options, and the configured timeout input into the request.)

```text
Claim (receipt)
      ↓
Kernel::resolve_execution_binding(&claim)
      ↓ [short authority tx: attempt/lease/epoch/expiry; Claim copies
         cross-validated vs the frozen Attempt — a tampered or stale
         Claim is rejected here, BEFORE any configuration resolution]
AuthoritativeExecutionBinding (attempt_id, lease_epoch,
                               target/profile ← Attempt row)
      ↓
resolve_execution_environment(mode, binding.target, binding.profile)
      ↓ [fails closed on missing target/profile or incompatible pair]
ResolvedExecutionEnvironment (opaque, private fields; keyed by durable authority;
      carries the binding — safety() mints Attempt-bound proofs)
      ↓
Kernel::create_execution(&claim, env.safety())
      ↓ [fenced revalidation inside SQLite transaction — no TOCTOU hole]
      ├─ Validates Attempt/Lease/Epoch fencing
      ├─ Cross-validates claim against Attempt
      ├─ Asserts safety.target == attempt.execution_target
      ├─ Asserts safety.profile == attempt.execution_profile
      ├─ Asserts safety.attempt_id / safety.lease_epoch == authoritative Attempt
      │   (Attempt-bound proof: proof(A) + claim(B) rejected even when
      │    target and profile coincide)
      ├─ Re-reads durable Task payload, acceptance, workspace_mode
      ├─ Re-reads Agent committed continuity capsule & version
      ├─ Re-reads Task continuity preference
      ├─ Re-reads Incarnation ID and durable runtime_handle_json
      ├─ Freezes safety.attempt_isolation() on Execution row
      └─ Mints ExecutionId & RequestId
      ↓
PreparedExecutionLaunch { snapshot, request, resolved_environment }
      ├─ snapshot: ExecutionLaunchSnapshot (private fields, readonly getters;
      │            task_name = durable Task label)
      ├─ request: ExecutionRequest::from_launch(&launch, &resolved_environment)
      │           └─ prompt = RenderedWorkerPrompt::from_launch(&launch):
      │              deterministic V0.1 protocol (IDs, epoch, workstream,
      │              objective = payload, acceptance, continuity, + writer
      │              recovery rules when WRITE); no caller text injectable
      └─ resolved_environment: the same resolved environment that minted the
         persisted attempt_isolation proof — the single binding for physical
         start (resolution followed by fenced revalidation)
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
      │                     ├── prepare_execution_launch(kernel, claim, mode) -> PreparedExecutionLaunch
      │                     │     (binding-first: authority tx → config resolution → fenced creation)
      │                     └── recover_authority(kernel)
      ▲
      │ (depends on core + config)
[agentype-adapter-api] (#![deny(unsafe_code)])
      ├── RenderedWorkerPrompt::from_launch(&launch) (deterministic V0.1 protocol)
      ├── ExecutionRequest::from_launch(&launch, &resolved_environment)
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
  - `FrozenExecutionSafety` (Attempt-bound proof):
    - Fields: `attempt_id`, `lease_epoch`, `execution_target`, `execution_profile`, `attempt_isolation`.
    - Constructor accepting `attempt_isolation: bool` is sealed as **`pub(crate)`**.
    - Safe public constructor: `pub fn unisolated(binding) -> Self` (`attempt_isolation = false`, no-isolation-assumption fail-safe).
    - Unforgeable: Ordinary safe code cannot forge `attempt_isolation = true` without authoritative registry resolution.
    - Attempt-bound: `Kernel::create_execution` rejects a proof whose attempt/lease epoch do not match the authoritative attempt (`InvalidAuthority`), even when target and profile coincide — a proof can never be replayed across attempts.
  - `ResolvedExecutionEnvironment`: Encapsulated private fields, readonly getters, and `.safety() -> FrozenExecutionSafety`.
  - `resolve_execution_environment(mode, &AuthoritativeExecutionBinding) -> Result<ResolvedExecutionEnvironment, ResolutionError>`: resolution is keyed by the durable binding, and the resolved environment carries it.
  - `ExecutionLaunchSnapshot`:
    - Private fields for all execution launch parameters.
    - `pub unsafe fn from_persisted_kernel_authority(...) -> Self` — a **trusted unchecked constructor**: the `unsafe` marker is a procedural contract whose canonical caller is the Kernel execution-creation transaction; it is NOT an access-control mechanism, and Rust memory safety does not enforce the kernel-only construction invariant.

### `agentype-storage-sqlite` (Scheduler Storage Engine)
- `Kernel::resolve_execution_binding(&claim) -> AuthoritativeExecutionBinding`: short authority-validation transaction (attempt/lease/epoch/expiry, then cross-validation of the Claim's task/agent/target/profile copies against the durable Attempt) producing the configuration-resolution key from the frozen Attempt row.
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
      mode: ExecutionResolutionMode<'_>,
  ) -> Result<PreparedExecutionLaunch, ExecutionPreparationError>
  ```
- Returns `PreparedExecutionLaunch { snapshot, request, resolved_environment }`. Configuration resolution is keyed by the durable `AuthoritativeExecutionBinding` and runs inside the façade between the authority-validation transaction and the fenced execution-creation transaction, so the persisted `attempt_isolation` fact and the environment exposed for physical start are bound to the same resolved environment (resolution followed by fenced revalidation — not a cross registry+SQLite atomic transaction). The dispatcher MUST select the adapter binding, options, and timeouts from `resolved_environment` and MUST NOT re-resolve; a pre-resolved environment is not an accepted parameter.
- `Kernel::resolve_execution_binding(&claim)` (storage crate): a short authority-validation transaction producing `AuthoritativeExecutionBinding` from the frozen Attempt row. A stale/expired Claim fails with `StaleAuthority` and a Claim whose task/agent/target/profile copies disagree with the Attempt fails with `InvalidAuthority` — both BEFORE any configuration resolution, so a tampered Claim cannot masquerade as a configuration failure.
- `ExecutionPreparationError` freezes configuration-resolution failures to the standardized Task failure class `RESOURCE_UNAVAILABLE` (`standard_failure_class()`), anchored on `core::authority::unavailable_configuration_failure` and spec 16 §A2 (the supplied registry is authoritative, no adapter default). Kernel authority errors are deliberately NOT mapped to a Task failure class.
- The worker prompt is not a façade concern: `ExecutionRequest::from_launch(&launch, &resolved_environment)` derives it deterministically via `RenderedWorkerPrompt` (see adapter-api).
- Recovery orchestration: `recover_authority(&kernel) -> Result<ExpireReport, Error>`.
- Verified regressions: `launch_binds_current_registry_state_not_a_stale_resolved_environment` (a registry generation swap between attempts binds the current generation), `preparation_errors_standardize_configuration_failures_as_resource_unavailable`, the discriminating pair `recovery_follows_persisted_isolation_fact_despite_registry_reconfiguration` / `unisolated_writer_expiry_without_quiescence_suspends` (identical retryable WRITE policy: recovery follows the persisted `attempt_isolation` fact to RETRY_WAIT and a replacement Attempt, while the unisolated control suspends with `WRITER_QUIESCENCE_UNKNOWN`), the worker-prompt protocol regressions, and `stale_safety_proof_cannot_authorize_later_attempt_after_registry_reconfiguration` (a proof minted for attempt A cannot authorize attempt B even with identical target/profile).

### `agentype-adapter-api` (Execution Adapter Contracts)
- Enforces `#![deny(unsafe_code)]`.
- `RenderedWorkerPrompt`: the deterministic, provider-neutral worker protocol (V0.1 task protocol sections) with private text and the sole constructor `from_launch(&ExecutionLaunchSnapshot)`. Given the same snapshot, the worker instruction is uniquely determined; no constructor accepts arbitrary text.
- `ExecutionRequest` retains complete structured worker contract:
  - `request_id`, `execution_id`, `task_id`, `batch_id`, `attempt_id`, `attempt_number`, `lease_id`, `lease_epoch`, `logical_agent_id`, `incarnation_id`, `execution_target`, `execution_profile`, `workspace_mode`, `prompt` (deterministically derived V0.1 worker protocol via `RenderedWorkerPrompt`; not a parameter), `payload`, `acceptance`, `workstream_id`, `continuity`, `incarnation_runtime_handle`, plus runtime-configuration inputs from the resolved environment (`target_options`, `profile_options`, `profile_timeout_seconds`; M5.2 audit refinement — scheduler semantics come exclusively from the snapshot, physical runtime configuration exclusively from the resolved environment).
- Closed derivation constructor:
  ```rust
  pub fn from_launch(
      launch: &ExecutionLaunchSnapshot,
      environment: &ResolvedExecutionEnvironment,
  ) -> ExecutionRequest
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
| configuration-resolution key | `AttemptRow.execution_target` / `execution_profile` (via `Kernel::resolve_execution_binding`) | Claim DTO copies cross-validated and rejected (`InvalidAuthority`) before resolution; stale authority (`StaleAuthority`) precedes configuration failure |
| `task_name` | `TaskRow.name` | Re-read from durable `tasks` row; durable task label fact only — never the worker prompt |
| worker prompt (`ExecutionRequest.prompt`) | Derived deterministically inside `agentype-adapter-api`: `RenderedWorkerPrompt::from_launch(&launch)` from the full launch protocol (IDs, epoch, workstream, payload, acceptance, continuity, workspace mode) | Not a constructor parameter — no caller text can be injected; the same snapshot always yields the same instruction |
| `payload` | `TaskRow.payload_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `acceptance` | `TaskRow.acceptance_json` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `workstream_id` | `TaskRow.workstream_id` | Re-read from durable `tasks` row (Claim tamper ignored) |
| `continuity` | `AgentRow.continuity_json` + `continuity_version` + `TaskRow.continuity` | Atomically loaded and bundled into `CommittedContinuitySnapshot` in tx |
| `attempt_isolation` | Runtime config registry (`ExecutionTargetConfig.attempt_isolation`), verified against Attempt identity (`attempt_id`/`lease_epoch`) and Attempt target/profile, frozen on `executions` row | Persisted before physical start; immutable for lifecycle; cross-attempt proof replay rejected (`InvalidAuthority`) |

---

## 4. Test Suite & Validation Results

### Rust Workspace (`cargo test --workspace`)
**135 passed, 0 failed:**
- `agentype-adapter-api`: 5 passed (including deterministic, non-injectable worker-prompt regressions)
- `agentype-core`: 20 passed (domain authority, decision matrix, state machines)
- `agentype-execution-config`: 7 passed (registry validation, configuration error checking, resolution modes, fail-closed handling)
- `agentype-runtime`: 13 passed (authority-precedence discriminating set: tampered target/profile → authority rejection, stale claim precedes configuration failure; registry-generation-swap binding; standardized preparation failures; end-to-end isolation persistence; discriminating registry-reconfiguration pair; Attempt-bound proof replay regression; V0.1 worker-prompt protocol regressions)
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

1. **Worker prompt was the Task name (P1, corrected).** The original source-of-truth table mapped `prompt ← TaskRow.name`, which would have sent a bare task label to real workers instead of the V0.1 task protocol. The snapshot field is now `task_name` (durable label fact only), and the worker prompt is a derived representation rendered from the full launch protocol, replicating the V0.1 oracle `Dispatcher._render_prompt` (including writer recovery rules for WRITE tasks). (The rendering location and injection hardening were further tightened in Round 2 item 6; the intermediate `render_worker_prompt`/`from_launch(launch, prompt)` design is superseded.)
2. **Newborn continuity ignored workstream project_state_ref (P1, corrected).** `birth_agent` hardcoded `continuity_json = '{}'`, so the authoritative project baseline never reached workstream-bound newborn agents and the launch snapshot preserved an empty capsule. Birth now seeds `{"CURRENT CHECKPOINT": {"project_state_ref": ...}}` per the V0.1 oracle (fail-closed on unknown workstreams). The affinity-birth runtime primitive (`ensure_task_consumers`) itself remains M5.2.
3. **Registry-reconfiguration evidence was non-discriminating (P2, corrected).** The original test used a task whose default retry policy forbids mechanical retry, so SUSPENDED occurred under both correct and broken recovery. It is replaced by a discriminating pair with an identical retryable WRITE policy: recovery must follow the persisted `attempt_isolation = true` fact to RETRY_WAIT and a replacement Attempt even after the registry flips to `false`, while the unisolated control suspends with `WRITER_QUIESCENCE_UNKNOWN`.
4. **Report test count was internally inconsistent (P2, corrected).** The initial revision claimed 121 passed while its own breakdown summed to 122. Historical per-revision figures above are retained as history; **current audited head: 135 passed, 0 failed** (measured at the round-4 head).

### Round 2

5. **attempt_isolation proof was not atomically bound to the resolved environment (P1, corrected).** `prepare_execution_launch` accepted a pre-resolved environment, and neither it nor the `FrozenExecutionSafety` proof carried registry or Attempt identity (only target/profile names plus the isolation bit), so an environment resolved under an older registry generation could be replayed to freeze `attempt_isolation = true` on a later attempt. The façade now takes an `ExecutionResolutionMode` and resolves internally immediately before the launch transaction; `PreparedExecutionLaunch.resolved_environment` is the environment that minted the persisted proof (the single binding for physical start), and a pre-resolved environment is no longer an accepted parameter. Discriminating regression: a registry generation swap between attempts binds the current generation.
6. **Worker prompt was still injectable (P1, corrected).** `from_launch(launch, prompt: String)` allowed arbitrary caller text to replace the scheduler instruction. The prompt is now derived inside `agentype-adapter-api` as `RenderedWorkerPrompt` (private text, sole snapshot-based constructor); the same snapshot always yields the same instruction and no API path accepts caller text. Residual boundary: `Kernel::create_execution` remains a public storage-level API that accepts a cloned proof; the façade is the canonical M5.2 path, and binding the proof to the Attempt identity inside the Kernel is recorded as optional follow-up hardening.
7. **Configuration-resolution failure semantics were comment-only (P2, corrected).** `ExecutionPreparationError` freezes `ResolutionError` failures to the standardized Task failure class `RESOURCE_UNAVAILABLE` at the façade boundary (`standard_failure_class()`), anchored on `core::authority::unavailable_configuration_failure` and spec 16 §A2; kernel authority errors are deliberately not Task failure classes.
8. **CI evidence gap (P2, corrected).** The Rust CI job now runs `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` before `cargo test --workspace`, so the quality gates cited here are reproducible in CI.

### Round 3

9. **Configuration resolution was keyed by the Claim DTO before authority validation (P1, merge blocker, corrected).** `prepare_execution_launch` resolved configuration from the Claim's `execution_target`/`execution_profile` copies before any kernel authority check, so a tampered Claim (target renamed to a nonexistent entry) surfaced as `Configuration(TargetNotFound)` → `RESOURCE_UNAVAILABLE` instead of an authority rejection. Harmless as a bare error, but at M5.2 a dispatcher that mechanically NACKs `RESOURCE_UNAVAILABLE` would let a forged Claim DTO push a fully configured Task into retry/suspension — violating "Claim = authority receipt, not launch semantics". The façade is now three-step: `Kernel::resolve_execution_binding` (short authority tx; stale/expired Claim → `StaleAuthority`, mismatched Claim copies → `InvalidAuthority`) produces `AuthoritativeExecutionBinding` (target/profile ← the frozen Attempt row), configuration resolution is keyed by that binding, and `Kernel::create_execution` still revalidates lease/epoch transactionally (no TOCTOU hole). Discriminating regressions: tampered target / profile yield authority rejection while the authoritative target is fully available in the registry; a stale Claim with an empty registry yields `StaleAuthority` ahead of configuration failure; the untampered missing-registry case remains `RESOURCE_UNAVAILABLE`.
10. **"Atomically bound" terminology overclaimed (P2, corrected).** Configuration resolution runs outside the SQLite transaction; the real guarantee is "bound to the same resolved environment (resolution followed by fenced revalidation)", not a cross registry+SQLite atomic transaction. Report and code wording normalized accordingly.
11. **`pub unsafe fn` documented as an exclusive/unforgeable boundary (P2, corrected).** `unsafe` is not access control; `ExecutionLaunchSnapshot::from_persisted_kernel_authority` is now documented as a trusted internal unchecked constructor whose kernel-only invariant is procedural (canonical caller is the fenced execution-creation transaction). The genuinely type-system-sealed `FrozenExecutionSafety` claims (`pub(crate)` constructor) are unchanged.

### Round 4

12. **Safety proofs could be replayed across attempts (P1, corrected).** `FrozenExecutionSafety` carried only target/profile/isolation, and the public `Kernel::create_execution` checked only those attributes, so a proof minted for attempt A under an isolated registry generation could authorize attempt B with the identical target/profile — persisting a stale `attempt_isolation = true`. Proofs are now Attempt-bound (`attempt_id` + `lease_epoch` fields; resolution is keyed by `AuthoritativeExecutionBinding`, and the resolved environment mints Attempt-bound proofs), and the Kernel rejects identity mismatch with `InvalidAuthority` before the attribute checks. Discriminating regression: proof(A) replayed onto B after a registry reconfiguration is rejected, while the canonical façade for B succeeds with the current isolation fact. This closes the last M5.1 launch-authority escape hatch (supersedes the residual boundary noted in Round 2 item 6).
13. **Report history counts drifted (P2, corrected).** Stale "counts in this revision" figures in the review-history section were replaced by an explicit earlier-revision-vs-current-audited-head statement.
14. **Empty adapter_kind registration (P2, corrected).** `register_target` now fails closed on an empty/whitespace `adapter_kind` (`ConfigurationError::InvalidAdapterKind`); whether the named adapter is actually installed remains a composition/runtime-resolution concern.
