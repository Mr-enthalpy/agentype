# RIIR M5.2 — Runtime Composition & Dispatch Commit Boundary

Status: Historical Report
Applies to: branch `rust/m5.2-dispatch` (base: main @ M5.1 merge)
Canonical path: `docs/reports/v0.2/riir-m5.2-dispatch-foundation.md`
Not a specification.

Despite the historical `riir-` directory naming, this milestone is **native Rust M5
runtime implementation**, not another parity rewrite phase. It builds on the frozen
M4 kernel and the frozen M5.1 authoritative execution-launch foundation.

---

## 1. Mission and scope

> Establish the authoritative runtime composition boundary and the first safe
> dispatch path from a valid Scheduler Claim to one committed physical start
> attempt.

M5.2 answers two questions only:

1. Is this Attempt currently allowed and fully configured to start?
2. If yes, how does the Runtime commit exactly one physical start attempt without
   trusting mutable Claim semantics or introducing new Scheduler authority?

M5.2 is **not** the full daemon. No heartbeat, no supervision loop, no restart
reconciliation, no notifier, no real adapter, no daemon process (M5.3+).

---

## 2. Invariants (normative statements adopted by this milestone)

> Physical execution is permitted only after durable Scheduler authority,
> authoritative target/profile configuration, and installed adapter binding are
> all resolved.

> The AdapterRegistry represents physical implementation availability. It is not
> SpawnSource and carries no semantic scheduling authority.

> Once an Execution has been persisted and `start_execution` has been invoked, the
> start is treated as potentially side-effecting and MUST NOT be blindly repeated.

> Claim remains an authority receipt; `ExecutionRequest` is derived from the
> authoritative launch snapshot.

> Physical I/O occurs outside SQLite transactions; all post-I/O authority
> mutations are fenced.

---

## 3. Dispatch pipeline

```text
Claim (authority receipt; claimed by dispatch_one or supplied to dispatch_claim)
      ↓
resolve_physical_execution_environment
      ├─ Kernel::resolve_execution_binding      [authority tx: stale/tampered → AuthorityRejected]
      ├─ resolve_execution_environment(Authoritative(reg), &binding)
      │        [missing target/profile/compat → RESOURCE_UNAVAILABLE]
      └─ AdapterRegistry::resolve(target.adapter_kind)
               [kind not installed → RESOURCE_UNAVAILABLE; zero fallback]
      ↓
ResolvedPhysicalExecutionEnvironment (binding + config + installed adapter)
      ↓
Kernel::create_execution(&claim, env.safety())   [fenced tx; Execution STARTING;
      │                                           stable RequestId persisted]
      ↓
ExecutionRequest::from_launch(&snapshot, &resolved environment)         [deterministic worker protocol]
      ↓
adapter.start_execution(&request)                [EXACTLY ONCE; outside any tx]
      ↓
classify + persist observation (fenced primitives only)
      ├─ RUNNING                  → confirm_running_and_renew (§13 M4 invariant;
      │                             supervision admission is M5.3)
      ├─ ambiguous/UNKNOWN/STARTING → record_physical_outcome(Unknown, handle)
      │                             + nonterminal NACK (ExecutionLost, never
      │                             quiescent) → writer safety decides
      │                             RETRY_WAIT vs WRITER_QUIESCENCE_UNKNOWN
      ├─ terminal failure         → NACK (normalized FailureClass, terminal bit)
      ├─ synchronous success      → collect_outcome → ack_success
      ├─ invocation error         → normalized class + nonterminal NACK
      └─ stale authority after start → physical history ONLY (never restores
                                      Task authority; handle preserved for M5.4)
```

Composition failure (target/profile/adapter) happens **before** Execution
creation: no Execution is fabricated, no physical start occurs, and the attempt
is mechanically NACKed as `RESOURCE_UNAVAILABLE` via the existing
`report_configuration_unavailable` primitive — so no writer ambiguity can arise
from a start that never happened.

---

## 4. Key design decisions

1. **Adapter resolution precedes Execution creation** (§4 preferred design). The
   `resolve_physical_execution_environment` composition performs the M5.1
   authority transaction, then pure configuration resolution, then adapter-kind
   lookup — all before `create_execution`. A missing adapter can therefore never
   be confused with an attempted physical start (proved by tests).
2. **`DirectUnconfigured` is structurally unreachable from the Dispatcher** (§8):
   `Dispatcher` accepts only `&ExecutionRegistry` + `&AdapterRegistry`. Direct
   mode remains available for standalone/single-shot callers of the M5.1 façade.
3. **Exactly-once start (§16-17)**: `start_execution` is invoked at most once per
   persisted Execution/RequestId on the initial dispatch path — never after an
   error, timeout, or ambiguous result. `RequestId` comes from the persisted
   snapshot and is never regenerated; later reconciliation uses
   `reconcile_start(request_id, persisted_handle)` (M5.4).
4. **Physical I/O outside transactions (§26)**: `create_execution`, observation
   persistence, NACK/ACK each run in their own short transaction;
   `start_execution` runs between commits.
5. **Post-start stale authority (§27)**: if authority expires between Execution
   creation and the start result, the Runtime persists physical history only
   (UNKNOWN/FAILED with the observed handle preserved for M5.4), never restores
   Task authority, and never admits supervision. Proven with a
   `ClockAdvancingAdapter` that advances the shared manual clock inside
   `start_execution`.
6. **Ambiguous WRITE starts (§28)**: nonterminal NACK (never quiescent) lands the
   Execution in UNKNOWN and lets the M4 writer-safety rules suspend with
   `WRITER_QUIESCENCE_UNKNOWN`; read-only attempts follow the retry policy. No
   blind restart (proved: a follow-up `dispatch_one` returns NoWork).
7. **Error model (§31)**: `DispatchError::{Authority, Configuration,
   AdapterAvailability, AdapterInvocation, Persistence}`. Only configuration and
   adapter-availability failures map to `FailureClass::ResourceUnavailable`;
   adapter invocation errors normalize
   unavailable/deadline/protocol/other → ResourceUnavailable / Timeout /
   AdapterProtocolFailure / StartFailure; authority and persistence errors are
   never Task failure classes.
8. **Fixture hygiene (§21)**: the adapter-api synthetic launch fixture now shares
   one `AuthoritativeExecutionBinding` between the snapshot and its Attempt-bound
   safety proof; a regression asserts `snapshot.safety().attempt_id() ==
   snapshot.attempt_id()`.
9. **Layering wording (§33)**: spec 15 now describes adapter-api as the
   "provider-neutral execution contract: traits, adapter-facing DTOs, and
   deterministic worker execution protocol" (hard prohibitions unchanged: no
   vendor names, no scheduling transitions, no SQLite, no runtime loops, no Root
   authority, no M6 semantics).

---

## 5. Test mapping (task §29 checklist)

| # | Required case | Test |
|---|---|---|
| 1 | authoritative target + profile + adapter resolves | `physical_composition_resolves_binding_config_and_adapter` |
| 2 | missing target → RESOURCE_UNAVAILABLE | `dispatch_missing_target_fails_closed_without_physical_start` |
| 3 | missing profile → RESOURCE_UNAVAILABLE | `dispatch_missing_profile_fails_closed_without_physical_start` |
| 4 | incompatible pair → RESOURCE_UNAVAILABLE | `dispatch_incompatible_pair_fails_closed_without_physical_start` |
| 5 | adapter_kind missing → RESOURCE_UNAVAILABLE | `dispatch_missing_adapter_creates_no_writer_ambiguity` |
| 6 | empty AdapterRegistry is authoritative | `physical_composition_fails_closed_on_missing_adapter_kind` |
| 7 | no adapter fallback | `physical_composition_never_falls_back_to_another_installed_adapter` |
| 8 | Dispatcher cannot use DirectUnconfigured | structural: `Dispatcher` has no mode parameter (§8) |
| 9 | tampered target → authority before adapter | `dispatch_tampered_claim_target_is_authority_rejected` |
| 10 | tampered profile → authority before adapter | `dispatch_tampered_claim_profile_is_authority_rejected` |
| 11 | stale claim → authority before adapter | `dispatch_stale_claim_is_authority_rejected` |
| 12 | mutated workspace cannot alter request | `dispatch_read_only_task_stays_read_only_even_if_claim_says_write` |
| 13 | mutated payload cannot alter request | `dispatch_claim_semantic_copies_cannot_alter_request` |
| 14 | mutated acceptance cannot alter request | `dispatch_claim_semantic_copies_cannot_alter_request` |
| 15 | mutated workstream cannot alter request | `dispatch_claim_semantic_copies_cannot_alter_request` |
| 16 | one Execution → one stable RequestId | `dispatch_one_starts_running_exactly_once` |
| 17 | adapter receives persisted ExecutionId | `dispatch_one_starts_running_exactly_once` |
| 18 | adapter receives persisted RequestId | `dispatch_one_starts_running_exactly_once` |
| 19 | adapter receives authoritative IncarnationId | `dispatch_one_starts_running_exactly_once` |
| 20 | adapter receives authoritative incarnation handle | M5.1 `execution_request_constructed_from_launch_snapshot` (request handle == snapshot's durable handle; dispatcher never injects) |
| 21 | start called exactly once | `dispatch_one_starts_running_exactly_once` |
| 22 | ambiguous start never re-started | `dispatch_ambiguous_start_is_persisted_and_never_restarted` |
| 23 | RUNNING persisted through fenced mechanism | `dispatch_one_starts_running_exactly_once` |
| 24 | ambiguous persisted unresolved | `dispatch_ambiguous_start_is_persisted_and_never_restarted` |
| 25 | terminal failure follows NACK rules | `dispatch_terminal_start_failure_follows_nack_rules` |
| 26 | stale authority after RUNNING: no Task restore | `dispatch_stale_authority_after_running_never_restores_task` |
| 27 | stale authority after failure: physical history only | `dispatch_stale_authority_after_failure_records_physical_history_only` |
| 28 | ambiguous writer never gains quiescence | `dispatch_ambiguous_write_start_never_gains_quiescence` |
| 29 | READ_ONLY stays READ_ONLY vs Claim WRITE | `dispatch_read_only_task_stays_read_only_even_if_claim_says_write` |
| 30 | WRITE only from durable WRITE Task | `dispatch_write_task_requests_write_from_durable_authority` |
| 31 | missing adapter → no writer ambiguity | `dispatch_missing_adapter_creates_no_writer_ambiguity` |
| 32 | no physical start on configuration failure | `start_call_count == 0` asserted in every configuration-failure test |
| 33 | M4 tests green | full workspace run |
| 34 | M5.1 tests green | full workspace run |
| 35 | MERGE preserves current Attempt target/profile | M4 suite `claim_on_source_then_merge_before_execution_preserves_frozen_target` (green) |
| 36 | retry after MERGE adopts new target/profile | M4 suite `retry_after_merged_attempt_uses_new_partition_target` (green) |
| 37 | persisted attempt_isolation creation-time immutable | M4/M5.1 suites (green, incl. `stale_safety_proof_cannot_authorize_later_attempt_after_registry_reconfiguration`) |
| 38 | Python oracle green | 160 passed, 2 skipped |
| 39a | audit: collect overrides start success with failure | `dispatch_collect_overrides_start_success_with_failure` |
| 39b | audit: nonterminal collection never ACKs / inherits proof | `dispatch_collect_nonterminal_never_acks_or_inherits_proof` |
| 39c | audit: contradictory success collection → INVALID_RESULT | `dispatch_contradictory_success_collection_is_never_acked` |
| 39d | audit: quiescence without terminality → ADAPTER_PROTOCOL_FAILURE | `dispatch_quiescence_without_terminal_is_protocol_failure` |
| 40 | audit: resolved options/timeout reach the adapter request | `dispatch_request_carries_resolved_runtime_configuration` |
| — | audit: admission seed matches durable attempt + live lease epoch | asserted in `dispatch_one_starts_running_exactly_once` |

---

## 6. Mandatory review questions (task §35)

1. Can a mutated Claim widen filesystem authority? — **NO** (workspace_mode comes
   from durable Task; tested).
2. Can a mutated Claim replace payload, acceptance, or workstream? — **NO**
   (snapshot-derived; tested).
3. Can a Claim choose a different adapter than authoritative target
   configuration? — **NO** (adapter_kind resolved from the Attempt-frozen
   target's config).
4. Can a missing configured adapter silently fall back to another adapter? —
   **NO** (fail-closed registry; tested).
5. Can Dispatcher use `DirectUnconfigured`? — **NO** (no mode parameter exists on
   the dispatch path).
6. Can `start_execution` happen before adapter availability is resolved? — **NO**
   (composition precedes Execution creation; tested).
7. Can one Execution invoke initial `start_execution` twice after ambiguity? —
   **NO** (exactly-once; tested).
8. Can adapter I/O happen inside the SQLite authority transaction? — **NO**
   (start runs between commits).
9. Can a stale post-start observation restore Task authority? — **NO** (physical
   history only; tested).
10. Can ambiguous WRITE start be treated as quiescent? — **NO** (tested).
11. Does current Attempt retain frozen target/profile across MERGE? — **YES**
    (M4 suite green).
12. Does a later retry adopt the new topology target/profile? — **YES** (M4
    suite green).
13. Does persisted Execution retain creation-time `attempt_isolation`? — **YES**
    (M4/M5.1 suites green).
14. Does AdapterRegistry represent SpawnSource? — **NO** (availability only).
15. Did M5.2 implement heartbeat/restart/notifier/daemon? — **NO**.
16. Did M5.2 introduce Generation/AgentType/Transform/WorkIntent semantics? —
    **NO**.
17. Can M5.3 now consume this dispatch boundary without making a new
    launch-authority decision? — **YES** (StartedRunning outcomes carry the
    fenced identity; admission is the only remaining step).

---

## 7. Validation (exact final-head evidence)

Commands run at head `rust/m5.2-dispatch`:

```text
cargo fmt --all --check                                  → clean
cargo clippy --workspace --all-targets -- -D warnings    → 0 warnings
cargo test --workspace                                   → 169 passed, 0 failed
python -m compileall -q src tests                        → OK
python -m unittest discover -s tests -t .                → 160 passed, 2 skipped, 0 failed
git diff --check                                         → clean
```

Rust breakdown (169):

- `agentype-adapter-api`: 8 (FakeAdapter invocation controls; fixture identity
  coherence §21; deterministic prompt; launch/environment pairing validation)
- `agentype-core`: 20 (unchanged M4 domain suite)
- `agentype-execution-config`: 7 (registry fail-closed, Attempt-bound proofs)
- `agentype-runtime`: 44 (M5.1 façade 13 + composition 6 + dispatcher 25)
- `agentype-storage-sqlite`: 90 (m4_kernel 63, recovery 11, topology 16)

---

## 8. Known boundaries handed to M5.3+

- `executions.request_id` and `runtime_handle_json` are durably persisted (the
  ambiguous-start path preserves the observed handle), but there is no public
  kernel reader exposing them yet — the M5.4 reconciler will need one.
- `expire_leases` leaves orphaned STARTING/RUNNING/UNKNOWN execution rows
  untouched; reconciliation of stale physical rows is M5.4.
- Supervision admission, heartbeat, notifier, restart barrier: M5.3/M5.4/M5.5.
- First real (reference) adapter: M5.7.

---

## 9. Audit round corrections (post-PR review)

1. **collect_outcome is authoritative for ACK/NACK proof (P1, merge blocker, corrected).** The
   synchronous-success path consumed only the collected payload/summary/quiescence/incarnation
   fields and never re-classified `outcome.state`/`outcome.terminal_confirmed`, so a start that
   claimed SUCCEEDED could collect a failure, an unresolved state, or a contradictory result and
   still ACK the Task into a durable Result. `commit_collected_outcome` now classifies
   authoritatively: SUCCEEDED+terminal is the only ACK path; terminal failures NACK under the
   collected class; nonterminal/unresolved collections are NO-ACK with zero inherited
   terminal/quiescence proof (execution stays UNKNOWN); contradictory collections (success without
   terminal proof → INVALID_RESULT; quiescence without terminality → ADAPTER_PROTOCOL_FAILURE)
   are never ACKed as success. Four negative regressions added (mapping rows 39a-39d).
2. **Authoritative runtime configuration now crosses the composition boundary (P1, merge blocker,
   corrected).** Only adapter_kind and attempt_isolation previously reached the physical start;
   target options, profile options, and the configured profile timeout were resolved and then
   dropped. `ExecutionRequest::from_launch(launch, environment)` enforces the two-source rule
   structurally: scheduler semantics come exclusively from the authoritative launch snapshot;
   physical runtime configuration (target options, profile options, configured timeout input)
   comes exclusively from the authoritative resolved environment — whose fields are private and
   only obtainable through authoritative resolution, so options cannot be fabricated. Deadline
   *enforcement* remains M5.6; the configured input is carried from M5.2.
3. **SupervisionAdmissionSeed (P1, M5.3 prerequisite, corrected).** `StartedRunning` now carries
   `SupervisionAdmissionSeed { execution_id, request_id, attempt_id, lease_epoch }`, generated
   exclusively after `confirm_running_and_renew` succeeds. It grants no new authority — it
   carries exactly the identity the fenced transaction just confirmed, so M5.3 supervision does
   not re-derive launch authority from the database.
4. **Observed handle preservation on stale-authority NACK paths (P2, corrected).** `nack_start`
   threads the observed runtime handle through every path; the stale-authority physical-history
   fallback no longer drops it, and the stale-failure regression exercises a non-null handle.

### Round 2 (second audit)

5. **Every terminal start observation goes through authoritative collect (P1, merge blocker,
   corrected).** Only the SUCCEEDED branch previously called `collect_outcome`; a start claiming
   FAILED (or Terminated/Lost) with terminal+quiescent bits NACKed directly from the
   StartObservation, bypassing the authoritative ACK/NACK proof source — and for WRITE tasks
   potentially unlocking a replacement writer while the physical execution may still be RUNNING
   (duplicate-writer violation). All terminal observations now collect first and
   `commit_collected_outcome` remains the sole classifier: collected terminal failures NACK under
   the collected class; collected nonterminal outcomes land UNKNOWN with zero inherited proof
   (the start's quiescence claim is never used, so writer safety suspends instead of retrying);
   collected success overrides a failure claim. Regressions: start FAILED+quiescent / collect
   RUNNING nonterminal → WRITE suspends with WRITER_QUIESCENCE_UNKNOWN (no replacement writer);
   start FAILED / collect SUCCEEDED → collected success semantics ACK.
6. **collect error after a terminal claim is handled as unresolved (adjacent, corrected).** When
   `collect_outcome` itself errors, the start's terminal/quiescence claims are not inherited:
   unresolved physical history is persisted with the observed handle (the Execution no longer
   lingers in STARTING) and a nonterminal NACK runs. The collected-ACK path also handles stale
   authority by recording physical history only (§27 alignment).
7. **from_launch verifies launch/environment pairing (P2, corrected).** The constructor now
   verifies attempt_id, lease_epoch, execution_target, and execution_profile across the two
   sources and returns `LaunchEnvironmentMismatch` (fail closed) instead of assembling a mixed
   request.
8. **Report arithmetic and pipeline drift (P2, corrected).** Breakdown heading (161→169) and the
   pipeline's two-source `from_launch` signature reconciled.
