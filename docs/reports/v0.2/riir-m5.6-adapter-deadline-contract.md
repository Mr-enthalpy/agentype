# RIIR M5.6 — Absolute Adapter Deadlines, Invocation Evidence, and Failure Contract

Status: Historical Report
Applies to: branch `rust/m5.6-adapter-deadlines` (base: main @ M5.5 merge `d1ad386`)
Canonical path: `docs/reports/v0.2/riir-m5.6-adapter-deadline-contract.md`
Not a specification.

Despite the historical `riir-` directory naming, this milestone is **native Rust
runtime work**. It consumes the frozen M4 kernel and the M5.1–M5.5 runtime
boundaries and freezes the provider-neutral Scheduler-facing
ExecutionAdapter invocation contract. M5.6 does NOT implement a real adapter.

---

## 1. Mission

> Every Scheduler-facing ExecutionAdapter invocation receives exactly one
> absolute monotonic deadline. The Adapter owns bounded process, transport,
> protocol, stream, and cleanup I/O under that deadline. Runtime does not
> manufacture physical proof from deadline expiry.

Canonical rule:

```text
Runtime policy
    ↓
one absolute monotonic deadline
    ↓
one Scheduler-facing Adapter operation
    ↓
all internal stages
    ↓
all exception cleanup
    ↓
return before deadline
```

There is NEVER `stage A timeout → stage B receives fresh timeout`, and NEVER
`operation fails → cleanup receives a new cleanup timeout`. Every internal
wait derives `remaining()` from the same endpoint.

M5.6 makes this mechanically true for all six operations:
`start_execution`, `reconcile_start`, `observe_execution`,
`collect_outcome`, `interrupt_execution`, `terminate_execution`.

Completion standard (mechanically enforced):

> Runtime cannot invoke an installed ExecutionAdapter through its canonical
> production path without first supplying a finite absolute monotonic
> deadline for that exact Scheduler-facing operation — and adapter
> invocation failure produces only standardized mechanical failure plus any
> physical locator evidence actually learned, never terminality, quiescence,
> writer safety, Task authority, or Result authority.

---

## 2. Deadline algebra

`crates/agentype-adapter-api/src/deadline.rs`:

```rust
pub struct AdapterDeadline { expires_at: std::time::Instant } // private

impl AdapterDeadline {
    pub fn after(timeout: Duration) -> Result<Self, DeadlineConfigError>;
    pub fn from_instant(expires_at: Instant) -> Self; // deterministic tests only
    pub fn remaining(&self) -> Duration;              // saturates at zero
    pub fn remaining_at(&self, now: Instant) -> Duration;
    pub fn is_expired(&self) -> bool;
    pub fn is_expired_at(&self, now: Instant) -> bool; // now == expires_at → expired
    pub fn expires_at(&self) -> Instant;
}
```

- Construction is fail-closed: zero → `DeadlineConfigError::NonPositive`;
  unrepresentable (`Instant::checked_add` overflow) → `Overflow`.
- `remaining()` saturates at zero; it never panics and is never negative.
- There is no `extend()` / `reset()` / `refresh()` API (structurally
  guaranteed: the endpoint is a private field with no mutating method).
- Not serialized, not persisted, not placed into `Execution`. The type has
  no `Serialize`/`Deserialize` derive and appears in no storage DTO.
- Reads never move the endpoint (test: `remaining_reads_do_not_move_the_endpoint`).

## 3. Monotonic-clock decision

Adapter deadlines use process-local `std::time::Instant` — never
`SystemTime`, Unix timestamps, the SQLite clock, the kernel `ManualClock`,
or `Lease.expires_at`. Wall-clock jumps cannot extend or shorten adapter
I/O budgets. Lease `UnixTime` and adapter `Instant` are different clocks and
different authorities (M5.6 §35): a Lease may expire while an adapter call
is in flight; kernel fencing decides on return, exactly the M5.2/M5.4
model. No `min(adapter timeout, remaining Lease)` coupling was introduced.

## 4. Operation matrix (§62)

| Operation           | May have physical side effect | Deadline/error proves state? | Partial handle useful? | Terminal Task proof? |
| ------------------- | ----------------------------: | ---------------------------: | ---------------------: | -------------------: |
| start_execution     |                           yes |                           no |                    yes |                   no |
| reconcile_start     |    observation/reconciliation |                           no |                    yes |                   no |
| observe_execution   |                   observation |                           no |               optional |                   no |
| collect_outcome     |                   observation |                  no on error |               optional | only successful validated collect |
| interrupt_execution |                           yes |                           no |               optional |                   no |
| terminate_execution |                           yes |                           no |               optional |                   no |

## 5. Profile-timeout distinction (§5)

`ExecutionProfile.timeout_seconds` is execution/profile configuration
input. It MUST NOT automatically become any operation deadline. Operation
latency bounds come exclusively from the installed `AdapterDeadlinePolicy`.
A future real adapter (M5.7) may consider both the profile timeout and
`AdapterDeadline::remaining()`, but the Scheduler-facing call always
returns by its `AdapterDeadline`. This is now stated in the
`ExecutionRequest::profile_timeout_seconds` doc comment and asserted by
`start_execution_receives_the_registered_start_budget` (a 30 s profile with
a 2 s start budget yields a 2 s start deadline).

`AdapterDeadlinePolicy` is also NOT part of `RuntimeTimingConfig`
(dispatcher poll / heartbeat / lease relationships) — it is a separate
runtime composition object.

## 6. AdapterRegistry / binding architecture (§10-12, §36-37)

```text
AdapterRegistry::register(kind, adapter, AdapterDeadlinePolicy)   // policy mandatory
AdapterRegistry::resolve(kind) -> ResolvedAdapterBinding          // fail-closed

ResolvedAdapterBinding
    start_execution(&ExecutionRequest)
    reconcile_start(&RequestId, Option<&RuntimeHandle>)
    observe_execution(&RuntimeHandle)
    collect_outcome(&RuntimeHandle)
    interrupt_execution(&RuntimeHandle)
    terminate_execution(&RuntimeHandle)
```

- `ResolvedAdapterBinding` (in `runtime/src/deadlines.rs`) owns the adapter
  implementation plus its policy. Each façade method mints exactly one
  `AdapterDeadline` from the registered per-operation budget and passes it
  to the underlying adapter. The raw `Arc<dyn ExecutionAdapter>` is a
  private field with **no public accessor** — there is no production path
  that invokes an adapter without deadline construction (hard gate §12).
- Registration without a bounded policy is impossible: the policy argument
  is mandatory and `AdapterDeadlinePolicy::new` validates every slot
  (> 0, representable). No registry-internal default exists; the
  composition root must supply a valid policy.
- `ResolvedPhysicalExecutionEnvironment::adapter_binding()` returns the
  binding; `environment.adapter().start_execution(...)` does not exist.
- Recovery resolves the same binding type by persisted
  `Execution.adapter_kind` only (§37); it does not re-resolve
  target/profile to obtain deadline policy.

The `ExecutionAdapter` trait itself (adapter-api) takes `&AdapterDeadline`
as the last parameter of all six methods.

## 7. Structured error model (§19-23)

```rust
pub enum AdapterErrorKind { DeadlineExceeded, Unavailable, Protocol, Other }

pub struct AdapterError {
    kind: AdapterErrorKind,                       // private
    diagnostic: Option<AdapterDiagnostic>,        // bounded 512 chars
    runtime_handle_hint: Option<RuntimeHandle>,   // partial locator evidence
}
```

Canonical failure mapping (single shared function
`observation::adapter_invocation_failure_class`; no call site maintains a
second mapping — Dispatch, Recovery, and future observers all use it):

| Adapter error    | FailureClass             | Physical proof |
| ---------------- | ------------------------ | -------------- |
| DeadlineExceeded | TIMEOUT                  | none           |
| Unavailable      | RESOURCE_UNAVAILABLE     | none           |
| Protocol         | ADAPTER_PROTOCOL_FAILURE | none           |
| Other            | UNKNOWN                  | none           |

`Other → START_FAILURE` was **removed** (§24): a generic invocation error
does not prove the start was rejected. `START_FAILURE` remains reserved for
positively collected terminal failure (e.g. a validated terminal
`ExecutionOutcome` without an explicit class).

Adapters cannot author Scheduler-derived failure classes (§25):
`normalize_start_observation` / `normalize_collected_outcome` reject any
non-mechanical class (only `FailureClass::WriterQuiescenceUnknown` is
non-mechanical) as `AdapterProtocolFailure`. Writer-safety escalation
remains exclusively Scheduler policy.

`AdapterDiagnostic` enforces bounded length (512 chars). Sanitization
(credentials, Authorization headers, env secrets, full payloads/bodies)
remains an adapter implementation obligation to be proven by the M5.7 real
adapter (§52).

## 8. Locator-evidence rules (§20-22, §27-28)

`AdapterError::with_handle_hint(RuntimeHandle)` carries physical locator
evidence earned before failure (e.g. thread_id captured, then turn/start
timed out). The hint means "the adapter learned this locator" — nothing
more: no RUNNING, no terminality, no quiescence, no Task authority.

Evidence-first persistence on the dispatch start-error path
(`lib.rs` `dispatch_claim`):

```text
adapter returns Err with handle hint
        ↓
persist runtime_handle_hint (UNKNOWN, zero proof bits)
        ↓
STARTING → UNKNOWN physical history
        ↓
nonterminal NACK using the mapped FailureClass
        ↓
writer safety
```

The same rule applies in recovery: `reconcile_start` errors prefer
`err.runtime_handle_hint()` over the persisted handle
(`recovery.rs` `reconcile_active_physical`). A partial external start
remains recoverable: the locator is durably preserved and the same
Execution is later reconciled by stable `RequestId` — never blindly
started again (tests:
`start_timeout_after_partial_locator_persists_the_locator`,
`stale_start_timeout_keeps_handle_as_physical_history_only`,
`reconcile_timeout_is_unresolved_not_fatal_and_keeps_handle_hint`).

## 9. Cleanup budget rules (§15-17)

One deadline flows through all internal stages and exception cleanup.
`remaining(stage)` / `remaining(next stage)` / `remaining(cleanup)` all
derive from the same endpoint. If `remaining == 0`, cleanup MUST NOT open a
fresh wait — only immediate/best-effort action (kill, close nonblocking,
abandon opaque state). Error precedence: if the primary operation failed
`Protocol` and cleanup completes within the remaining budget, `Protocol` is
returned; if cleanup consumes the deadline, the whole operation normalizes
to `DeadlineExceeded` (the original cause survives only as sanitized
diagnostic context).

The deterministic `DeadlineProbe` conformance harness
(`adapter-api` tests) proves the contract shape without OS timing:
stage A / stage B / cleanup observe one endpoint; remaining decreases
rather than resets; no fresh cleanup budget exists after exhaustion;
cleanup exhaustion normalizes to DeadlineExceeded; timely cleanup preserves
the original kind. Actual blocked-I/O cleanup conformance is M5.7's
obligation.

## 10. No-watchdog rationale (§14, §40)

M5.6 deliberately does NOT implement spawn-thread-and-detach or
Tokio-timeout-around-blocking-adapter watchdogs. A watchdog would produce
"Scheduler stopped waiting but adapter still mutating physical state in
background", which does not satisfy the contract. The conformance boundary
is:

> An ExecutionAdapter that fails to return by the supplied deadline is not
> a valid Agentype ExecutionAdapter.

M5.7 must prove the first real adapter complies. Non-conformance of a
malicious adapter is out of scope (§40).

## 11. Dispatch integration (§27-28, §47)

`dispatch_claim` invokes `physical.adapter_binding().start_execution(&request)`
(bounded). On `Err`:

1. `failure_class = adapter_invocation_failure_class(&err)`
2. `hint = err.runtime_handle_hint()`
3. `persist_unresolved_physical_then_nack` — locator first, then
   `STARTING → UNKNOWN`, then nonterminal NACK, then writer safety
4. return `StartIndeterminate { failure_class }` — no supervision
   admission, no blind restart

`commit_start_observation` / `commit_collected_outcome` take the
`ResolvedAdapterBinding` and call `collect_outcome` through it (bounded).
Positive evidence must be obtained before the deadline (§18) — an
`AdapterError` is never converted into an observation.

## 12. Recovery integration (§29-31, §48, §39)

`reconcile_active_physical` and `collect_and_apply` hold a
`ResolvedAdapterBinding`; both calls are bounded. Semantics frozen:

- reconcile timeout = "current physical state was not established within
  the operation deadline". It grants no supervision, proves no death, and
  is a per-Execution mechanical failure — it can NEVER make the whole
  `RecoveryCoordinator` fatal. This closes the M5.4 P1
  (`Execution A readmitted → heartbeat running → reconcile B hangs →
  startup RECOVERING forever`): every reconcile/collect now has a finite
  absolute deadline, so one conformant adapter invocation cannot block
  recovery forever. StartupGuard still owns error/unwind cleanup.
- collect timeout = no collected terminal proof, no inherited terminal or
  quiescence proof from the preceding terminal candidate, no ACK, no
  Result. The existing nonterminal/writer-safety path applies.
- Deadline policy changes after restart are legal (runtime availability
  behavior, not Scheduler authority); recovery still reconciles the exact
  persisted Execution and applies fencing after return.

## 13. Observe / interrupt / terminate contracts (§30, §32-34, §49)

M5.6 freezes the call contract but does NOT implement the steady-state
observer loop (that is pre-M5.8). Direct binding-level contract tests
establish: observe timeout is an invocation error, never an observation —
no terminality, no quiescence, and no RUNNING→UNKNOWN transition (which the
frozen physical graph does not allow); interrupt timeout proves no
interruption success (interrupt may or may not have taken effect; Task
cancellation stays an independent Scheduler authority transition);
terminate timeout — even with a "kill signal sent" diagnostic — proves
neither TERMINATED nor process exit nor writer quiescence ("kill sent" ≠
"quiescence confirmed"). Process death remains weaker than quiescence on
every timeout/error path (§34): no M5.6 code writes
`quiescent_confirmed = true` because of timeout, broken pipe, EOF, exit,
transport disconnect, kill issued, or terminate requested.

## 14. Schema decision (§53)

No schema bump — `SCHEMA_VERSION` remains 3. Deadline is ephemeral;
partial runtime-handle evidence fits existing physical-history persistence
(`runtime_handle_json`, failure metadata, proof bits, request_id,
adapter_kind). Test
`deadline_policy_is_not_persisted_to_execution_state` asserts the
executions schema carries `adapter_kind` (routing remains) but no column
matching deadline/policy/remaining. Nothing like an absolute deadline,
deadline owner, remaining budget, or operation timeout lease was persisted.

## 15. Scope boundaries

- No real adapter (M5.7 implements exactly one against this contract).
- No steady-state physical observer loop / cadence decisions (pre-M5.8).
- No RootBridge changes; RootBridge boundedness (M5.5) remains an
  independent transport contract — no generic
  `ExternalOperationDeadline` merged the two authorities (§54).
- No M6 concepts (`AgentType`, `SpawnSource`, `Generation`, `RawWorkIntent`,
  Transform, MemoryCapsule semantics).
- Dependency direction preserved: Core has no runtime/adapter dependency
  and no `std::time::Instant`; adapter-api owns `AdapterDeadline` and the
  error contract; runtime owns the registry, policy, and bounded binding;
  storage has no `AdapterDeadline`.

## 16. Tests

New/extended suites (test names in parentheses):

- **Deadline algebra** (`adapter-api/src/deadline.rs`, 7 tests): positive
  accepted, zero rejected, overflow rejected, positive remaining before
  expiry, exact expiry = zero, after expiry = zero never negative, reads
  don't move the endpoint. Items 9/10 (no extend/reset; not serialized) are
  structural guarantees — private endpoint field, no mutating method, no
  serde derive, no storage DTO usage — plus the schema-absence test below.
- **Registry/binding** (`runtime/src/deadlines.rs`): per-operation budget
  selection with a distinct 1–6 s policy including independent endpoints
  (`binding_selects_each_operation_deadline_budget`), policy stored per
  installed adapter (`registry_resolves_the_policy_installed_with_each_adapter`),
  zero rejected, positive live deadline; dispatch-side start-budget
  selection (`start_execution_receives_the_registered_start_budget`);
  policy not persisted (`deadline_policy_is_not_persisted_to_execution_state`).
- **Error taxonomy** (`runtime/src/observation.rs`):
  `invocation_other_is_unknown_and_start_failure_needs_positive_proof`,
  `scheduler_derived_quiescence_class_is_rejected_as_protocol_failure`,
  existing `adapter_error_taxonomy`; handle-hint unit
  (`adapter_error_carries_handle_hint_without_terminal_proof`).
- **Start timeout evidence** (`runtime/src/lib.rs`):
  timeout before locator → physical UNKNOWN/RetryWait/no admission/no
  re-start; timeout after partial locator → locator persisted; class matrix
  (DeadlineExceeded→TIMEOUT, Protocol→ADAPTER_PROTOCOL_FAILURE,
  Other→UNKNOWN≠START_FAILURE); stale-authority timeout → history only;
  unisolated WRITE timeout → suspension; isolated WRITE timeout → frozen
  isolation retry policy.
- **Recovery** (`runtime/src/recovery.rs`): reconcile receives finite
  deadline; reconcile timeout unresolved-not-fatal + hint preserved +
  barrier completes; stale reconcile timeout → physical history only;
  unisolated writer reconcile timeout → suspension; terminal candidate →
  collect gets its own new deadline; collect timeout → no ACK/no Result/no
  inherited proof; recovery proceeds to later candidates after an ordinary
  adapter timeout.
- **Observe/control contracts** (`runtime/src/deadlines.rs`): observe /
  interrupt / terminate timeouts are invocation errors carrying no proof;
  kill-issued diagnostic alone is not proof.
- **Cleanup-budget conformance** (`adapter-api` `DeadlineProbe`): one
  endpoint across stages and cleanup; independent endpoints across
  operations; remaining decreases; no fresh cleanup budget after
  exhaustion; cleanup exhaustion → DeadlineExceeded; timely cleanup
  preserves the original kind.

No flaky sleep-based deadline suite: determinism comes from
`AdapterDeadline::from_instant` + `remaining_at` and scripted fakes.

### Final-head CI evidence

```text
cargo fmt --all --check                      # clean
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo test --workspace                       # 366 passed, 0 failed
  adapter-api 21 + core 20 + execution-config 8 + root-bridge 8
  + runtime 184 + storage 125
  (storage 125 = m4_kernel 65 / outbox_delivery 17 / reconciliation 5 /
   recovery 11 / supervision 11 / topology 16)
py -3 -m compileall -q src tests             # clean (Python 3.13 local)
py -3 -m unittest discover -s tests -t .     # 160 passed, 2 skipped (162)
git diff --check                             # clean
```

(Do not reuse M5.5's 328 / 160 counts as this head's evidence.)

## 17. Crash/ambiguity matrix (§63)

### Start side effect + timeout + crash

```text
Execution STARTING durable
→ physical side effect
→ partial locator
→ timeout
→ locator persisted / UNKNOWN
→ crash
→ M5.4 reconcile by RequestId + persisted locator
```

### Timeout before locator persistence

If the process crashes before the physical-evidence commit, the Execution
remains STARTING; M5.4 already reconciles STARTING using the stable
RequestId. No wakeup is lost.

### Locator persisted, authority consequence not yet committed

Restart sees unresolved physical history plus possibly-ACTIVE authority;
M5.4 applies normal recovery fencing/writer safety. No new protocol was
needed.

## 18. Completion questions (§64)

1. Every ExecutionAdapter method receives an absolute deadline? — YES
2. Monotonic? — YES (`std::time::Instant`)
3. Cleanup shares that same deadline? — YES (contract + probe harness)
4. Cleanup can open a fresh wait after exhaustion? — NO
5. Runtime exposes a deadline-bypassing raw adapter path? — NO
6. Adapter registration can omit deadline policy? — NO
7. Profile timeout automatically the operation deadline? — NO
8. Start timeout can preserve a partial locator? — YES
9. Timeout implies Execution LOST? — NO
10. Timeout implies TERMINATED? — NO
11. Timeout implies quiescence? — NO
12. Generic `AdapterError::Other` mapped to UNKNOWN? — YES
13. START_FAILURE reserved for positively classified physical failure? — YES
14. Adapter can emit authoritative WRITER_QUIESCENCE_UNKNOWN? — NO
15. Reconcile/collect can block Recovery forever for a conformant adapter? — NO
16. Schema bump required? — NO (SCHEMA_VERSION still 3)
17. Real adapter implemented? — NO
18. Physical observer loop implemented? — NO
19. RootBridge semantics modified? — NO
20. M6 concepts introduced? — NO

## 19. M5.7 acceptance obligations (§51)

M5.6 does not prove any production adapter is bounded. M5.7 MUST prove the
first real adapter against at least: blocked process/session
initialization; blocked request write; blocked flush; blocked response
read; deadline between start stages; deadline after partial locator;
cleanup with remaining budget; cleanup with depleted budget; interrupt
timeout; terminate timeout; reconcile timeout; collect timeout. All
internal waits must receive the same operation endpoint — no fresh
per-stage timeout. Adapter diagnostic sanitization must also be tested
there (the bounded `AdapterDiagnostic` type enforces length only).

Additionally still open before real adapters (carried from M5.5 status):
adapter binding identity — whether one `adapter_kind` uniquely identifies
the runtime domain that created the physical Execution, or whether an
opaque provider-neutral `adapter_binding_key` frozen at Execution creation
is required (BLOCKS_REAL_ADAPTER_PARITY; explicitly not M5.6).

## 20. Remaining M5.8 prerequisites

- Steady-state physical observer: an independent
  execution-observation / collect_outcome / ACK-NACK owner separate from
  heartbeat authority, consuming the M5.6-frozen bounded calls (lost
  observations handled at Scheduler-policy level without inventing physical
  history — no RUNNING→UNKNOWN).
- Process lock / single-run daemon mechanically enforcing
  recover-before-dispatch (composition-root barrier), single production
  SupervisionRunner owner per process.
- Cross-process exclusivity remains out of scope (two OS processes on one
  DB can still each grant+admit; outbox likewise at-least-once).
- Legacy `Kernel::heartbeat(attempt, epoch)` visibility convergence and
  runner fatal/phase daemon seams.
