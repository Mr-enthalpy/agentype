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
      ├─ terminal observation (success OR failure) → collect_outcome first,
      │        then authoritative classification: ACK only on SUCCEEDED +
      │        terminal proof; terminal failure → NACK under the collected
      │        class; nonterminal/contradictory (active or LOST state with
      │        proof, success without proof, quiescence without terminality)
      │        → UNKNOWN with zero inherited proof + nonterminal NACK
      │        (ADAPTER_PROTOCOL_FAILURE / INVALID_RESULT)
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
| 41a | audit r3: unresolved collect keeps observed handle | `dispatch_start_failure_claim_never_bypasses_collect` (handle assertion) |
| 41b | audit r3: ambiguous observation keeps observed handle | `dispatch_ambiguous_start_is_persisted_and_never_restarted` (handle assertion) |
| 41c | audit r3: unusual nonterminal shape keeps observed handle | `dispatch_unusual_nonterminal_observation_keeps_observed_handle` |
| 42a | audit r4: RUNNING + terminal proof → protocol failure, WRITE suspends | `dispatch_running_state_with_terminal_proof_is_protocol_failure` |
| 42b | audit r4: STARTING + terminal proof → protocol failure | `dispatch_starting_state_with_terminal_proof_is_protocol_failure` |
| 42c | audit r4: UNKNOWN + terminal proof → protocol failure | `dispatch_unknown_state_with_terminal_proof_is_protocol_failure` |
| 43 | audit r5: terminal failure without quiescence retains handle | `dispatch_terminal_failure_without_quiescence_retains_handle` |
| 44 | audit r6: collected LOST never unlocks writer replacement | `dispatch_lost_outcome_never_unlocks_writer_replacement` |
| 45 | audit r7: kernel faults classify as Persistence, never Authority | `kernel_faults_are_never_classified_as_authority` |
| 46 | audit r7: durable-state fault during binding resolution is fatal | `dispatch_surfaces_kernel_faults_as_persistence_not_authority_rejection` |
| 47 | audit r7: success without quiescence retains handle | `dispatch_success_without_quiescence_retains_handle` |
| 48 | audit r8: collected-success locator durable before a hard ack failure | `dispatch_collected_success_evidence_durable_before_ack_consequence` |
| 49 | audit r8: collected-failure locator durable before a hard nack failure | `dispatch_collected_failure_evidence_durable_before_nack_consequence` |
| 50 | audit r9: execution commitment freezes the adapter binding identity | `execution_commitment_freezes_adapter_kind` |
| 51 | audit r10: schema v1 database rejected at open after the adapter_kind change | `schema_v1_database_is_rejected_after_adapter_kind_column` |
| 52 | audit r10: contradictory RUNNING observation never reaches admission | `dispatch_contradictory_running_observation_never_reaches_admission` |
| 53 | audit r11: reusable sync success keeps the WARM incarnation handle | `dispatch_reusable_sync_success_keeps_warm_incarnation_handle` |
| 54 | audit r11: continuity locator flows into the next launch snapshot | `dispatch_next_launch_carries_continuity_locator` |
| — | audit r6: pairing rejects attempt_isolation drift | asserted in `from_launch_rejects_mixed_launch_environment_pairs` |

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
cargo test --workspace                                   → 185 passed, 0 failed
python -m compileall -q src tests                        → OK
python -m unittest discover -s tests -t .                → 160 passed, 2 skipped, 0 failed
git diff --check                                         → clean
```

Rust breakdown (185):

- `agentype-adapter-api`: 8 (FakeAdapter invocation controls; fixture identity
  coherence §21; deterministic prompt; launch/environment pairing validation
  including attempt_isolation drift)
- `agentype-core`: 20 (unchanged M4 domain suite)
- `agentype-execution-config`: 7 (registry fail-closed, Attempt-bound proofs)
- `agentype-runtime`: 60 (M5.1 façade 13 + composition 6 + dispatcher 41)
- `agentype-storage-sqlite`: 91 (m4_kernel 64, recovery 11, topology 16)

---

## 8. Known boundaries handed to M5.3+

- `executions.request_id` and `runtime_handle_json` are durably persisted, and
  every unresolved dispatch path now preserves the observed handle; a narrow
  verification reader (`Kernel::execution_runtime_handle`) exists, but the full
  M5.4 reconciliation identity reader (request_id + handle by attempt) is
  deferred to M5.4.
- `expire_leases` leaves orphaned STARTING/RUNNING/UNKNOWN execution rows
  untouched; reconciliation of stale physical rows is M5.4.
- **Adapter routing identity — frozen in M5.2 (was an M5.4 hard prerequisite).**
  `executions.adapter_kind TEXT NOT NULL` is persisted inside the
  execution-commitment transaction via `FrozenPhysicalExecutionBinding`, so a
  registry configuration drift between a crash and recovery (target "local"
  served by `codex-a` at T0, by `codex-b` at T1) can never hand the old
  physical execution to an adapter of a different binding family. Scope note
  (audit round 10): `adapter_kind` is the adapter ROUTING key / binding family
  identity, not the specific implementation/configuration identity — the same
  kind may bind to a different implementation instance across a restart. M5.4
  must therefore not read "adapter_kind present" as "recovery identity fully
  solved"; whether an adapter instance/config fingerprint beyond the routing
  key is required is decided with the first real adapter's reconciliation
  identity (M5.4/M5.7) — no generic plugin identity framework either way.
- **Schema version 2.** The adapter_kind structural change is gated by
  `SCHEMA_VERSION = 2`: a structurally valid rust-v0.2 database at version 1
  (no adapter_kind column) is rejected at open (fail closed, "does not match
  expected 2") instead of failing later at the first execution-commitment
  INSERT; a fresh v2 database is required. Backfilling adapter_kind from
  current configuration would violate the binding-frozen-at-commitment
  invariant, so no repair is attempted (D-DB-MIGRATE unresolved).
- **P2 — outcome vocabulary.** `DispatchOneOutcome::StartFailed` currently also
  covers physically-unresolved paths (adapter invocation errors, collection
  errors — durable state UNKNOWN), which could mislead a daemon into reading
  "start definitely failed". Before M5.3 consumes the boundary, split into
  `StartFailed` (authoritative collected terminal failure) vs
  `StartIndeterminate` (start/collect invocation uncertainty — the Execution
  may be UNKNOWN with a preserved handle), or fold the indeterminate case into
  `StartAmbiguous` carrying the failure class. The type name must not imply
  physical execution is definitely absent.
- **P2 — CompletedSynchronously mixes physical and Task completion.**
  `result_id: Option<ResultId>` is None when the worker physically SUCCEEDED
  but WRITE quiescence was not proven (WRITER_SUCCESS_NOT_QUIESCENT suspension)
  — easily read as "Task completed". Before M5.3/M5.8 consume the outcome,
  split into `CompletedSynchronously { result_id: ResultId }` plus
  `WriterSafetySuspendedAfterSuccess` (or an explicit authority-consequence
  enum). Not a current correctness issue: the durable states are correct.
- **P2 — no-start STARTING row.** `create_execution` commits STARTING before
  `from_launch` pairing validation runs; unreachable on the canonical path
  (both sides share one attempt-bound safety proof, runtime forbids unsafe)
  and further reduced by the isolation pairing check. If the physical-binding
  freeze refactor naturally absorbs it, resolve it then; no typestate rework
  for its own sake.
- **P2 — no-start STARTING row robustness.** `create_execution` persists
  STARTING before `from_launch` pairing validation runs; on the canonical path
  that validation cannot fail (same attempt-bound safety on both sides,
  runtime forbids unsafe), but a stronger typed composition could express the
  invariant so future changes cannot reintroduce a no-start STARTING row.
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

### Round 3 (third audit)

9. **Every unresolved dispatch path preserves the observed runtime handle (P1, merge blocker,
   corrected).** `nack_start` threaded the handle only into its stale-authority fallback record;
   on the normal path `Kernel::nack` does not write `runtime_handle_json`, so collected-nonterminal,
   contradictory, and generic-unresolved branches landed the Execution in UNKNOWN while dropping
   the adapter-returned handle. A runtime-private `persist_unresolved_physical_then_nack` helper
   now records UNKNOWN + handle (zero proof bits) before the NACK on every unresolved branch,
   and a narrow `Kernel::execution_runtime_handle` reader verifies persistence (the full M5.4
   identity reader remains M5.4). Regressions: three handle-preservation assertions/tests across
   the collect, ambiguous, and generic-fallback paths.
10. **Registered for M5.4/M5.3 (not implemented in M5.2):** durable adapter binding identity
   (P1, M5.4 hard prerequisite — see §8), outcome vocabulary refinement (P2), no-start
   STARTING-row typed composition (P2). See §8 for the full statements.

### Round 4 (fourth audit)

11. **An ACTIVE physical state with terminal proof is rejected as contradictory (P1, merge
    blocker, corrected).** `commit_collected_outcome` checked success-without-terminal-proof and
    quiescence-without-terminality but accepted state=RUNNING/STARTING/UNKNOWN together with
    `terminal_confirmed=true`, routing it into the terminal-failure NACK where
    `durable_quiescent = terminal && quiescent` injected a quiescence proof through the failure
    path — a WRITE task would RETRY_WAIT and permit a replacement writer while the adapter
    reports the physical execution still active. `commit_collected_outcome` now fails closed at
    its head on `terminal_confirmed && state.is_active_physical()`: unresolved physical state
    (UNKNOWN, zero inherited proof), observed handle preserved, nonterminal NACK under
    ADAPTER_PROTOCOL_FAILURE; for a WRITE task without attempt isolation this lands SUSPENDED
    with WRITER_QUIESCENCE_UNKNOWN — never RETRY_WAIT. Regressions: RUNNING/STARTING/UNKNOWN
    each with terminal+quiescent proof and a retryable failure class (mapping rows 42a-42c);
    the RUNNING case asserts the full writer-safety discriminator.

### Round 5 (fifth audit — P2s, no merge blocker)

12. **Terminal failure without quiescence retains the observed handle (P2 / M5.4 hardening,
    corrected).** `Kernel::nack` writes state/failure_class/proof bits/ended_at but not
    `runtime_handle_json`, so a terminal start/collect with a non-null handle dropped the known
    handle on the normal-authority path. The dispatcher now retains the handle after a
    terminal-failure NACK whenever quiescence is NOT proven (the suspended-WRITE case whose
    cleanup M5.4 owns); a quiescence-proven terminal execution deliberately does not retain its
    handle (no physical cleanup needed) — the distinction is an explicit design choice, not an
    accident of the Kernel parameter list. Regression: WRITE + terminal-without-quiescence →
    Execution FAILED, handle durable, task suspended.
13. **Registered:** the StartFailed vocabulary split (see §8) and the
    `ExecutionLaunchSnapshot` layering-wording fix — the worker prompt is rendered by the
    provider-neutral execution contract (agentype-adapter-api), not "the runtime".

### Round 6 (sixth audit)

14. **Collected LOST never unlocks writer replacement (P1, merge blocker, corrected).**
    `is_active_physical()` excludes LOST, so a collected LOST+terminal+quiescent outcome slipped
    past the round-4 gate into the terminal-failure NACK, where `durable_quiescent = true` let an
    unisolated WRITE task RETRY_WAIT and unlock a replacement writer — despite core semantics
    that LOST is never a confirmed end (incarnation presence stays LOST under terminal/quiescence
    claims; `record_physical_outcome` refuses proof bits for unresolved states) and that
    laundering LOST into FAILED forecloses the later LOST refinement. Any collected LOST carrying
    terminal or quiescence claims is now ADAPTER_PROTOCOL_FAILURE: unresolved (UNKNOWN, zero
    inherited proof), handle preserved, nonterminal NACK. Regression: WRITE with EXECUTION_LOST
    in the retry policy → SUSPENDED with WRITER_QUIESCENCE_UNKNOWN, never RETRY_WAIT.
15. **Pairing validation covers attempt_isolation (P1, corrected).** The same attempt identity
    and target/profile names can be re-resolved under a different registry whose target carries
    different isolation; `from_launch` now rejects the mixed pair (`attempt_isolation` mismatch)
    so a durable isolated safety proof can never be combined with a non-isolated physical request
    configuration. Registered P2s from this round (outcome vocabulary split, pipeline wording)
    are tracked in §8 / the diagram above.

### Round 7 (seventh audit)

16. **Kernel faults during binding resolution are persistence faults, never AuthorityRejected
    (P1, corrected).** `resolve_physical_execution_environment` mapped every
    `resolve_execution_binding` error to `DispatchError::Authority`, and `dispatch_claim` folds
    Authority into `Ok(AuthorityRejected)` — so a SQLite storage fault or corrupted durable
    state was reported as "this claim is no longer authorized", letting a daemon keep running on
    an unverified durable state. `classify_kernel_authority_error` now draws the line the error
    model promises: StaleAuthority/InvalidAuthority/NotFound-as-stale-receipt → Authority;
    StorageFailure/InvariantViolation/RecoveryRequired/anything else a normal claim validation
    should not produce → Persistence. Wired through binding resolution and `create_execution`.
    Regressions: exhaustive classifier unit test, plus an end-to-end durable-corruption test
    (lease epoch corrupted below the API boundary → `dispatch_claim` returns
    `Err(DispatchError::Persistence)`, never AuthorityRejected).
17. **Collected success without quiescence retains the observed handle (P2 / M5.4 hardening,
    corrected).** `ack_success` writes state/outcome/proof bits but not `runtime_handle_json`,
    and the success branch had no retention — a collected SUCCEEDED with
    `quiescent_confirmed=false` (unisolated WRITE: WRITER_SUCCESS_NOT_QUIESCENT suspension)
    dropped the adapter-returned handle. `retain_terminal_handle` generalized to
    `retain_collected_handle`, applied symmetrically to collected failures and successes; a
    quiescence-proven end deliberately retains nothing (no physical cleanup needed). Regression:
    WRITE + collected success without quiescence → Execution SUCCEEDED (terminal bit set, no
    Result), task SUSPENDED, handle durable.
18. **Registered (unchanged):** the StartFailed/StartIndeterminate vocabulary split before M5.3
    consumes the boundary (§8), and the no-start STARTING-row typed composition note — the
    latter's risk is further reduced now that pairing validation covers attempt_isolation.

### Round 8 (eighth audit)

19. **Terminal physical evidence commits before the authority consequence (P1, merge blocker,
    corrected).** The collected terminal paths ran the authority consequence first
    (`ack_success`/`nack` commit their own transactions) and retained the observed handle after
    (`retain_collected_handle`) — a crash between the two transactions left the authority
    consequence durable (WRITE task suspended) with no physical locator in durable history. The
    order is now evidence-first: a collected terminal outcome whose quiescence is NOT proven is
    pre-persisted as UNKNOWN with the observed handle and zero proof bits BEFORE
    `ack_success`/`nack` run; the terminal state itself is applied by the authority transaction
    (UNKNOWN → SUCCEEDED/FAILED), preserving the frozen M4 Kernel API. A quiescence-proven end
    needs no locator and skips the pre-persist. Regressions: a `CollectCorruptingAdapter`
    corrupts one durable column during `collect_outcome`, forcing the ack (corrupted lease read)
    / nack (corrupted retry-policy decode) to fail hard — the Execution is left UNKNOWN with
    the observed handle durable; the pre-evidence order would have left STARTING with no handle.

### Round 9 (ninth audit)

20. **The adapter binding identity is frozen at the execution-commitment transaction (P1, merge
    blocker, corrected).** Once `start_execution` may have run, the durable Execution did not
    record WHICH adapter binding owns the physical start — `adapter_kind` lived only in the
    in-memory composition, and M5.4 cannot recover information M5.2 did not record. The M5.2
    commitment invariant is now enforced: `executions.adapter_kind TEXT NOT NULL` is persisted
    inside the same fenced transaction as `request_id` and STARTING, carried by
    `FrozenPhysicalExecutionBinding { safety, adapter_kind }` (execution-config) through
    `Kernel::create_execution`; the canonical dispatcher freezes
    `ResolvedPhysicalExecutionEnvironment::physical_binding()` (the same kind string used for the
    AdapterRegistry lookup), and the standalone facade freezes the target configuration's
    declared binding. Regression: the commitment row carries the resolved adapter_kind after a
    normal dispatch. Provider-neutral metadata only; a stronger
    adapter_binding_id/config fingerprint is deferred to M5.4/M5.7 (§8 updated). Registered
    P2s from this round: the CompletedSynchronously physical/Task completion split, and the
    no-start STARTING-row typed composition (§8).

### Round 10 (tenth audit)

21. **Schema version gates the adapter_kind structural change (P1, merge blocker, corrected).**
    `executions.adapter_kind` was added to the CREATE TABLE while `SCHEMA_VERSION` stayed 1, so an
    M5.1-era Rust database (version 1, no adapter_kind column) passed the lineage and version
    gates — CREATE TABLE IF NOT EXISTS does not alter existing tables — and the incompatibility
    only surfaced at the first execution-commitment INSERT as a raw storage failure. The store
    now claims version 2 and the existing strict gate rejects a v1 database at open ("does not
    match expected 2"), explicitly requiring a fresh v2 database; backfilling adapter_kind from
    current configuration was rejected as a violation of the binding-frozen-at-commitment
    invariant. Regression: a downgraded v1 database is refused at open; a fresh v2 database
    carries the column.
22. **Contradictory RUNNING observations never reach supervision admission (P1, M5.3
    prerequisite, closed early in M5.2).** The RUNNING branch consumed state/ambiguous alone; a
    RUNNING observation carrying terminal/quiescence claims or a failure class is internally
    contradictory (an active state cannot carry end-of-execution semantics). Fail closed before
    the confirmation: unresolved physical state, zero inherited proof, handle preserved,
    ADAPTER_PROTOCOL_FAILURE — no SupervisionAdmissionSeed. Regression included.
23. **Wording:** `adapter_kind` described as the adapter ROUTING key / binding family identity,
    not the full implementation/configuration identity (see §8).

### Round 11 (eleventh audit)

26. **A reusable synchronous success preserves the WARM incarnation's continuity locator (P1,
    corrected).** A collected SUCCEEDED with terminal+quiescent+incarnation_reusable skipped
    handle persistence (the round-5 rule considered only the cleanup locator) while ack_success
    promoted the Incarnation to WARM without writing runtime_handle_json — and the synchronous
    path never ran confirm_running_and_renew (the only primitive that writes it). The next task
    on the same resident incarnation therefore read an empty continuity locator: the system
    claimed reusability while discarding the locator needed to reuse it. The pre-ACK evidence
    record now also fires when the incarnation is reusable (persist_terminal_evidence writes the
    handle into executions AND incarnations via COALESCE; the WARM promotion preserves it).
    Regressions: reusable sync success leaves the incarnation WARM with runtime_handle_json ==
    the adapter handle; the next launch snapshot carries that locator forward.
