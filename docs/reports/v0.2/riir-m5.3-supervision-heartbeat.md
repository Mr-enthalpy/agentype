# RIIR M5.3 — Supervision Admission, Heartbeat Authority, and Runtime Ownership

Status: Historical Report
Applies to: branch `rust/m5.3-supervision` (base: main @ M5.2 merge `1c0695f`)
Canonical path: `docs/reports/v0.2/riir-m5.3-supervision-heartbeat.md`
Not a specification.

Despite the historical `riir-` directory naming, this milestone is **native Rust
runtime implementation**, not rewrite-parity work. It builds on the frozen M4
kernel, the frozen M5.1 authoritative launch foundation, and the frozen M5.2
dispatch commitment boundary.

---

## 1. Frozen M5.3 mission

> The only mechanism by which a physically RUNNING Execution becomes eligible
> for continued Lease renewal, and the only runtime component allowed to
> perform that renewal.

```text
physical RUNNING observation
        ↓
fenced RUNNING confirmation + first Lease renewal   (M4 primitive, unchanged)
        ↓ atomic commit
SupervisionAdmission                                (minted only from the
        ↓                                            live transaction result)
runtime-local supervision ownership                 (SupervisionRegistry)
        ↓
periodic fenced heartbeat renewal                   (SupervisionService/Runner
        ↓                                            → Kernel::renew_supervised_execution)
```

M5.3 made it impossible for: an unconfirmed physical process to receive
renewal; a stale Attempt to keep receiving renewal; a restarted Runtime to
reconstruct supervision from `state='RUNNING'`; two supervisors to renew the
same Attempt; a heartbeat failure to be read as writer death/quiescence; an
ambiguous/indeterminate/terminal/contradictory start to enter supervision.

Not implemented (later milestones, unchanged): restart reconciliation
(M5.4), notifier/RootBridge (M5.5), absolute deadlines (M5.6), real adapter
(M5.7), daemon process/lifecycle (M5.8), M6 semantics.

## 2. Revised M5.2 outcome vocabulary

The two P2 items registered in the M5.2 report §8 are closed before any
supervision logic consumes the boundary:

| Old (M5.2) | New (M5.3) | Meaning |
|---|---|---|
| `StartedRunning { admission: SupervisionAdmissionSeed }` | `RunningAdmitted { admission: SupervisionAdmission }` | physical RUNNING observed, fenced confirmation + first renewal committed. **The only outcome that may enter supervision.** |
| `StartFailed` (indeterminate branches: invocation error, ambiguous/contradictory observations, unresolved collections, stale-authority-after-start) | `StartIndeterminate { execution_id, request_id, failure_class: Option<FailureClass> }` | the start MAY have happened; the Execution is durably unresolved; no admission; no blind restart. Never read as "the start definitely failed". |
| `StartFailed` (collected terminal failure branch) | `TerminalFailure { execution_id, request_id, failure_class }` | authoritative collected terminal failure; NACK consequences already applied. |
| `CompletedSynchronously { result_id: Some }` | `TaskCompleted { execution_id, request_id, result_id: ResultId }` | Task ACK succeeded; Result exists; `result_id` is non-optional. |
| `CompletedSynchronously { result_id: None }` | `WriterSafetySuspendedAfterSuccess { execution_id, request_id }` | physical success, but writer quiescence unproven → WRITER_SUCCESS_NOT_QUIESCENT suspension, no Result. Deliberately NOT "completed". |
| `StartAmbiguous` | folded into `StartIndeterminate` | one indeterminate class instead of two overlapping ones. |
| `NoWork` / `AuthorityRejected` / `ConfigurationUnavailable` | unchanged | pre-start outcomes. |

`SupervisionAdmission` (renamed from `SupervisionAdmissionSeed`) carries a
private process-local mint `generation` used purely as registry collection
hygiene — deliberately distinct from the durable `LeaseEpoch` fencing.

## 3. Admission authority model (design note, M5.3 plan §55)

**Authority.**

1. The minting transaction is the frozen M4
   `Kernel::confirm_running_and_renew` (storage `kernel.rs`): one short
   transaction validates Attempt ACTIVE, Lease ACTIVE and unexpired
   (`expires_at <= now → stale`), lease-row epoch == fencing epoch,
   `task.current_attempt_id` match, `task.fencing_epoch` match, Execution
   belongs to the Attempt and is eligible (STARTING/RUNNING/UNKNOWN), then
   atomically persists Execution=RUNNING+handle, Task=RUNNING,
   Incarnation=WARM, Lease heartbeat/expires. LogicalAgent assignment
   consistency is owned by the frozen claim/retirement paths (documented
   mapping; no new check invented).
2. A persisted RUNNING row cannot mint: the constructor is crate-private and
   the only call site is the dispatcher's post-commit branch of that
   transaction. No read path returns a constructible admission.
3. Every heartbeat is fenced by `attempt_id + lease_epoch + execution_id`
   via the new `Kernel::renew_supervised_execution` — never TaskId or
   LogicalAgentId.
4. Permanently invalidating events: authority loss (stale/invalid/not-found),
   Execution no longer RUNNING, Attempt/Lease closed, explicit terminal
   handling, shutdown, invariant mismatch. Removal consumes the admission
   generation: no Dropped → Admitted without a fresh authoritative mint.

**Physical semantics — all three answers are NO.** Heartbeat failure does not
prove process death. Lease expiry does not prove writer quiescence. Removing
a registry entry does not change any durable state. Heartbeat code never
calls `record_physical_outcome` and never sets proof bits.

**Crash safety.**

8. Crash after first renewal but before registry insertion → no further
   renewal occurs → the Lease expires → M5.4 reconciliation handles physical
   reality. One-directional, fail-closed; no durable supervision table was
   created to eliminate the window (regression:
   `crash_window_without_admission_never_renews`).
9. Crash with ten admitted Executions → all ten lose ownership.
10. Does restart auto-renew them? **NO.** After restart the supervision set
    is empty; persisted RUNNING alone is insufficient (regressions:
    `new_service_is_empty_despite_persisted_running_row`,
    `renew_due_only_touches_admitted_entries`).

**Concurrency.** ACK vs heartbeat and cancellation vs heartbeat are decided
by Kernel fencing; both serializations are tested and neither can reopen a
completed Task. The registry lock is never held across the DB transaction:
the service snapshots the admission (identity + generation), releases the
lock, performs the short fenced renewal, re-acquires, and applies the result
only if the generation is still current — an old renewal result can never
mutate a newer registry entry.

**Scope.** No `reconcile_start`, no notifier, no real adapter, no daemon
lifecycle, no M6 semantics (all NO).

## 4. Mandatory state table

| Physical Execution | Current Authority | Admission exists? | May heartbeat? |
|---|---|---:|---:|
| STARTING | valid | no | no |
| RUNNING observed but confirm txn not committed | unknown | no | no |
| RUNNING + confirm/renew committed | valid | yes | yes |
| RUNNING | stale Attempt | no/removed | no |
| UNKNOWN | any | no | no |
| LOST | any | no | no |
| SUCCEEDED | closed | no | no |
| FAILED | closed | no | no |
| TERMINATED | closed | no | no |
| persisted RUNNING after Runtime restart | unknown until reconciliation | no | no |

Implementation check: UNKNOWN and LOST behind still-valid authority surface
as `RenewalOutcome::NoLongerRunning` (drop, never renew, never repair);
SUCCEEDED/FAILED/TERMINATED sit behind closed authority and fail stale
(`AuthorityLost`). Both are regression-tested.

## 5. Mandatory transition table

```text
NoAdmission
    | fenced RUNNING confirmation + first renewal succeeds
    v
Admitted
    | periodic renewal succeeds -> Admitted
    +-- stale/invalid authority ----------> Dropped  (token consumed)
    +-- Execution no longer RUNNING -----> Dropped  (token consumed)
    +-- shutdown -------------------------> Dropped  (token consumed)
    +-- persistence/invariant fault ------> FatalSupervisionError (loop stops)
```

There is NO `Dropped → Admitted` without a new authoritative mint (the
consumed generation set enforces this mechanically). There is NO
`PersistedRunning → Admitted` by reconstruction.

## 6. Crash-window reasoning

See §3 item 8. The window is safe because supervision adds no durability: it
is an optimization over lease expiry. Losing the registry entry costs at most
one lease interval of dead time; gaining a phantom entry is impossible
because entries only come from the live fenced transaction result.

## 7. Race analysis

- **ACK vs heartbeat**: `ack_success` closes the Lease; a later renewal fails
  stale → `AuthorityLost`, Task stays COMPLETED (`race_ack_wins_before_heartbeat`).
  Heartbeat just before ACK extends the lease briefly; the ACK then completes
  normally (`race_heartbeat_wins_before_ack`). No serialization reopens a Task.
- **Cancellation vs heartbeat**: closed authority → renewal loses fencing
  (`race_cancellation_closes_renewal_authority`).
- **Expiry boundary**: `now == expires_at` fails stale — frozen M4 authority
  validation treats `expires_at <= now` as expired; renewal cannot revive
  (`expired_lease_cannot_be_revived_even_at_exact_boundary`).
- **MERGE vs heartbeat**: the current Attempt's frozen binding is untouched;
  the same Attempt/epoch keeps renewing (`race_merge_preserves_heartbeat_identity`).
- **Registry races**: snapshot → renew → apply-if-generation-current
  (see §3 Concurrency).

## 8. Registry ownership

`SupervisionRegistry` is in-memory (`HashMap<ExecutionId, SupervisedExecution>`
plus a consumed-generation set), starts empty, is never restored from the
database, and is authoritative only for "which executions this runtime may
attempt to renew". Duplicate insertion is idempotent only for an exactly
identical identity; a conflicting identity (same ExecutionId, different
attempt/epoch/request) is `AdmissionIdentityConflict`, never a silent
replacement. Removal marks the mint generation consumed. Registry presence
means ONLY "this runtime currently intends to renew this admitted authority".

## 9. Heartbeat failure taxonomy

| Class | Examples | Consequence |
|---|---|---|
| Authority loss | StaleAuthority, InvalidAuthority, NotFound, expired lease, closed Attempt/Lease | drop entry, consume token, never retry the old admission, never mutate Task authority |
| Persistence/invariant fault | SQLite failure, durable corruption, RecoveryRequired, invariant violation | `SupervisionError::Fatal` — fail closed, stop the supervision loop, surface on the runner handle; never classified as authority loss |
| Non-RUNNING behind valid authority | UNKNOWN/LOST/STARTING refinement | `NoLongerRunning` — drop ownership, no durable repair (M5.4 owns reconciliation) |

Regression: `persistence_fault_is_fatal_not_authority_loss` (corrupted lease
row via direct file access) and `runner_surfaces_fatal_persistence_fault_and_stops`.

## 10. Shutdown behavior

`SupervisionRunner::shutdown` stops the heartbeat thread and clears local
supervision ownership. It does NOT mark Executions terminal, claim quiescence,
revoke Leases, or terminate adapter processes — the Leases simply stop being
renewed and naturally expire (fail closed). Regression:
`runner_renews_admitted_execution_until_shutdown` (real-thread smoke: renews
in real time, then no renewal after shutdown, no proof bits anywhere). A
panicked loop surfaces through `shutdown`'s join; the loop is never restarted
and admissions are never reconstructed (M5.3 §34). The heartbeat thread is
private and shared-nothing with the dispatcher; the M5.5 notifier will get
its own thread, so the structure does not pre-break notifier isolation.

## 11. Test mapping

M5.3 plan required-test numbers → tests:

- **#1-14 (admission)**: `running_admitted_handoff_enters_supervision_and_renew` (#1-5),
  `unresolved_start_observations_never_admit` (#6-9),
  `terminal_outcomes_never_admit` (#10-11),
  `stale_confirmation_never_admits` (#12-13),
  `authority_rejection_never_admits` (#14).
- **#15-24 (registry)**: `registry_starts_empty_and_accepts_one_admission` (#15, #20),
  `identical_duplicate_admission_is_idempotent` (#16),
  `identity_conflicts_are_invariant_violations` (#17-19),
  `new_service_is_empty_despite_persisted_running_row` (#21),
  `registry_removal_touches_no_durable_state` (#22-24).
- **#25-44 (heartbeat)**: `admitted_running_execution_renews` (#25-27, #41),
  `supervised_renewal_requires_exact_attempt_epoch_and_execution` (#28-30),
  `expired_lease_cannot_be_revived_even_at_exact_boundary` (#31-32),
  `expired_attempt_cannot_renew` (#33), `completed_task_cannot_renew` (#34),
  `cancelled_task_cannot_renew` (#35),
  `closed_executions_cannot_renew` (#36/37/40),
  `non_running_executions_behind_valid_authority_report_not_running` (#38-39),
  `authority_loss_removes_admission_and_consumes_the_token` (#42, #50),
  `non_running_execution_drops_admission` (#43),
  `persistence_fault_is_fatal_not_authority_loss` (#44).
- **#45-52 (races)**: `race_ack_wins_before_heartbeat` (#45),
  `race_heartbeat_wins_before_ack` (#46),
  `race_cancellation_closes_renewal_authority` (#47),
  `expired_lease_cannot_be_revived_even_at_exact_boundary` (#48),
  `race_merge_preserves_heartbeat_identity` (#49),
  `authority_loss_removes_admission_and_consumes_the_token` (#50),
  `new_service_is_empty_despite_persisted_running_row` (#51),
  `crash_window_without_admission_never_renews` (#52).
- **#53-58 (writer safety)**: `race_cancellation_closes_renewal_authority` (#53-54),
  `supervision_loss_then_expiry_suspends_unisolated_writer` (#55),
  `isolated_writer_recovery_follows_persisted_isolation_not_registry` (#56),
  `removing_ownership_does_not_permit_write_replacement` (#57),
  `supervision_loss_preserves_reconcilable_physical_state` (#58).
- **#59-64 (vocabulary closure)**: `invocation_error_is_start_indeterminate_not_terminal_failure`
  (#59), `ambiguous_observation_maps_to_start_indeterminate` (#60),
  `dispatch_terminal_start_failure_follows_nack_rules` (#61),
  `task_completed_carries_concrete_result_and_only_running_admitted_carries_admission` (#62/#64),
  `writer_safety_suspension_is_distinct_from_task_completed` (#63).
- **Timing gate (§A2)**: `valid_timing_chain_is_accepted`,
  `poll_exceeding_heartbeat_is_rejected`,
  `heartbeat_at_or_above_lease_is_rejected`, `non_positive_durations_are_rejected`,
  `lease_authority_drift_is_rejected`.
- **§36 hardening**: `physical_binding_rejects_blank_adapter_kind`
  (execution-config) + kernel invariant check.

## 12. Validation (exact final-head evidence)

Commands run at head `rust/m5.3-supervision`:

```text
cargo fmt --all --check                                  → clean
cargo clippy --workspace --all-targets -- -D warnings    → 0 warnings
cargo test --workspace                                   → 239 passed, 0 failed
python -m compileall -q src tests                        → OK   (py -3 = Python 3.13.5)
python -m unittest discover -s tests -t .                → 162 ran: 160 passed, 2 skipped, 0 failed
git diff --check                                         → clean
```

Rust breakdown (239):

- `agentype-adapter-api`: 8
- `agentype-core`: 20 (unchanged M4 domain suite; zero core changes in M5.3)
- `agentype-execution-config`: 8 (7 prior + blank-adapter-kind constructor test)
- `agentype-runtime`: 102
  - dispatch/launch suite: 76 (60 M5.2 + 4 outcome-vocabulary closure + 12 supervision integration)
  - `supervision` module: 19 (12 deterministic registry/service/crash-window + 4 audit-closure regressions + 3 real-thread runner smokes)
  - `timing` module: 7
- `agentype-storage-sqlite`: 101
  - `m4_kernel` 64, `recovery` 11, `topology` 16 (all untouched semantics)
  - `supervision` (new): 10 (fenced renewal primitive conformance + finite lease authority)

All prior suites remain green: M4 correctness kernel, M5.1 launch authority,
M5.2 dispatch commitment, Python V0.1 oracle (160 passed / 2 skipped, unchanged).

## 13. Remaining M5.4 prerequisites

- The full reconciliation identity reader (request_id + runtime_handle by
  attempt) is still M5.4; `execution_runtime_handle` remains the narrow reader.
- Reconciliation of orphaned STARTING/RUNNING/UNKNOWN execution rows left by
  `expire_leases` is M5.4 (untouched, evidence-first handles preserved).
- The M5.4 re-admission flow uses the SAME admission API: after adapter
  reconciliation proves a persisted execution RUNNING, a fenced
  current-authority renewal/admission transaction mints a FRESH
  SupervisionAdmission (new generation) and `SupervisionService::admit` is
  the only insertion point. No bypass or manual registry population exists
  or is needed.
- Whether an adapter instance/config fingerprint beyond the frozen
  `adapter_kind` routing key is required is decided with the first real
  adapter (M5.4/M5.7), unchanged from the M5.2 report.
- `recover_authority` remains the M4 authority half of the startup barrier;
  adapter physical reconcile and the full daemon startup order are M5.4.
## 14. Post-PR audit round (request-changes review, closed as one commit set)

The PR #9 review returned 4 P1 findings and 1 P2. All four P1s were closed in
a single closure commit (`fix(runtime): M5.3 audit closure ...`); each fix
lands with its own regression. No fix tightens the timing gate to
`heartbeat <= lease/2` — the §A2 normative chain is unchanged; the
implementation was corrected instead.

### P1-1 — Healthy supervisor could self-expire a lease under legal timing (deadline phase)

Root cause: the registry anchored `last_renewal_at` at admission INSERTION
time and the runner was a fixed-phase ticker sleeping full heartbeat
intervals from its own phase. With the fully legal `heartbeat=6 < lease=10`
(no `2*heartbeat <= lease` headroom), a tick at t=6.0 found the entry not due
(anchor 0.1 → elapsed 5.9), slept 6 more seconds, and the lease expired at
10.1 — a healthy supervisor lost authority. Second layer: insertion time is
not the fenced first-renewal commit time, so handoff delay pushed the
schedule while the durable expiry stayed put.

Fix:

- `SupervisionAdmission` carries the fenced first-renewal COMMIT timing:
  `first_renewed_at` is recovered exactly from the admission transaction's
  own output (`expires_at - kernel.lease_seconds()`), and `lease_expires_at`
  is carried verbatim. The mint site is unchanged (dispatcher post-commit).
- The registry stores `next_due_at = first_renewed_at + heartbeat_interval`;
  insertion time no longer participates in scheduling.
- Each successful renewal re-anchors at ITS OWN commit time (the anchor is
  taken BEFORE the renewal, so `anchor + interval` is strictly earlier than
  the renewal's new durable expiry — mathematically safe under the §A2 gate).
- The runner is a deadline scheduler: renew due → sleep until
  `earliest_next_due` → repeat. The earliest deadline is read under the same
  state lock that `admit`/`remove` serialize on, so a mutation's wake-up can
  never be lost and the loop can never oversleep past an earlier deadline.
  Idle (no entries) falls back to the heartbeat interval.

Regressions: `wide_legal_timing_never_self_expires` (deterministic: anchor
1000.0, handoff delayed to 1005.9, asserts `earliest_next_due == 1006.0` —
the old implementation computed 1011.9, already past expiry — then drives
three full renewal cycles; the old code fails this test),
`runner_deadline_schedule_survives_wide_legal_timing` (real-thread smoke,
heartbeat 0.6 / lease 1.0, mid-phase admission).

### P1-2 — Dropping the runner detached the heartbeat thread

`JoinHandle` dropped = detached thread still owning a service clone and the
registry, still renewing, with no handle to stop or observe it — ownership
outliving its owner.

Fix: internal `stop_and_join(&mut self)` (set shutting_down → notify → join →
clear ownership → return recorded fatal) is now the single stop path used by
both `shutdown(mut self)` and `Drop::drop` (Drop cannot report the fatal;
the semantic requirement is set/notify/join, never detach).

Regression: `drop_runner_stops_renewal` (drop without shutdown; the lease
expiry freezes for ≥2 heartbeat intervals; durable state untouched).

### P1-3 — A cloneable admission made "one supervisor per Execution" only per-registry

`SupervisionAdmission: Clone` + registry-local consumed sets allowed
`service_1.admit(A.clone())` + `service_2.admit(A.clone())` — two renewals
both pass kernel fencing — and a `Dropped → Admitted` replay in a fresh
service without any fresh mint.

Fix: the admission is a **move-only capability** (`Clone`/`PartialEq`
removed from the admission AND from `DispatchOneOutcome`, which carries it).
`admit` consumes the token; the registry stores only a plain
`SupervisionIdentity` snapshot (identity fields + generation). The same
token can therefore never exist in two registries: single ownership is
structural, cross-service replay is impossible, and the only re-admission
path is a fresh authoritative mint through the same `admit` API (the M5.4
shape, unbroken). `SupervisionService::new` islands are harmless by the same
argument — a service without the token can only observe `NoSuchEntry`.

Regressions: `move_only_capability_prevents_cross_service_replay`
(`service_2` sees an empty registry and `NoSuchEntry`; removal destroys the
only token), `after_authority_loss_only_a_fresh_mint_reenters_supervision`
(a fresh mint re-enters — M5.4 shape — but durable fencing still refuses the
renewal; no stale resurrection).

### P1-4 — Non-finite timing authorities were not fail-closed

`+inf` passed every ordering check in the timing gate, `Kernel::from_store`
only checked `lease_seconds <= 0.0` (NaN passes; `+inf` lease never expires),
and the lease-authority matcher compared with `inf - inf = NaN`, whose
comparison is false — two infinite authorities "matched".

Fix: `RuntimeTimingConfig` rejects non-finite durations (new
`TimingConfigError::NonFiniteDuration`); the Kernel requires
`lease_seconds.is_finite() && lease_seconds > 0.0`; the matcher fails closed
on any non-finite input on either side before comparing.

Regressions: `non_finite_durations_are_rejected`,
`lease_authority_matcher_fails_closed_on_non_finite_input`,
`kernel_rejects_non_finite_lease_seconds`,
`admit_fails_closed_on_malformed_admission_timing` (the admission capability
itself is validated at admit: finite anchor/expiry, expiry strictly after
the anchor).

### P2 — Fatal semantics documented per surface

`SupervisionService` (deterministic primitive/testing surface) returns
`Err(SupervisionError::Fatal)` and leaves the entry in place — it does not
fail-stop by itself. `SupervisionRunner` is the production fail-stop owner:
Fatal stops the loop and clears ownership. Documented on the type; a service
that has produced a Fatal must not be reused for renewal (the Runner
enforces this mechanically).

### Audit-round test mapping additions

| Finding | Regression(s) |
|---|---|
| P1-1 deadline phase / delayed handoff | `wide_legal_timing_never_self_expires`, `runner_deadline_schedule_survives_wide_legal_timing` |
| P1-2 runner Drop | `drop_runner_stops_renewal` |
| P1-3 move-only capability / replay | `move_only_capability_prevents_cross_service_replay`, `after_authority_loss_only_a_fresh_mint_reenters_supervision` |
| P1-4 finite timing | `non_finite_durations_are_rejected`, `lease_authority_matcher_fails_closed_on_non_finite_input`, `kernel_rejects_non_finite_lease_seconds`, `admit_fails_closed_on_malformed_admission_timing` |


### Round 2 (second request-changes review, closed as two commits)

#### P1-1 — Transaction timestamps were sampled before the write serialization

Root cause: `Kernel::tx` sampled `clock.now()` at caller entry, then handed
the reading to `Store::with_immediate_at`, which acquired the connection
mutex and BEGIN IMMEDIATE afterwards. Under legitimate writer contention a
renewal could capture `now` before expiry, block on the SQLite write lock
past the durable expiry, then validate and renew with the stale reading —
resurrecting a lease that had already lost authority between the two points.
This is a latent M4 flaw that only became reachable with M5.3's concurrent
heartbeat.

Fix: `Store::begin_immediate` carries the lock/commit/rollback mechanics;
the new `Store::with_immediate_clock(&dyn Clock, f)` samples the
authoritative time ONLY after the connection lock is held and BEGIN
IMMEDIATE has succeeded — i.e. after the transaction has actually won the
SQLite write serialization. `Kernel::tx` routes every transaction through
it, so all authority validation (heartbeat, ACK, NACK, expiry, claims) runs
against a post-serialization reading. The schema-bootstrap helper keeps its
0.0 timestamps (no clock exists at init). The exact-boundary semantics are
unchanged; only the sampling point moved.

Regression: `renewal_timestamp_is_sampled_after_transaction_serialization`
(storage suite) — a second connection holds `BEGIN IMMEDIATE` while the
clock moves past the durable expiry; the blocked renewal must fail stale.
The pre-fix kernel samples at caller entry and renews, so the test fails on
the old implementation.

#### P1-2 — The runner accepted admissions after fail-stop (and a clear/admit race)

`SupervisionRunner::admit` did not consult the loop's health: after a fatal
the thread exited but `admit` kept returning `Ok`, leaving registry entries
with no supervisor behind them; the fatal-path registry clear also raced
in-flight admits.

Fix: an explicit runner lifecycle — `Running → ShuttingDown → Stopped` and
`Running → Failed` — replaces the shutdown bool. `admit` is accepted ONLY in
`Running` (otherwise `SupervisionError::RunnerStopped`); the fatal path
flips the phase to `Failed` under the same state lock admissions serialize
on and clears ownership afterwards, so an in-flight admit either completes
before `Failed` (its entry is cleared) or is rejected after — an unowned
entry can never survive. The thread body is wrapped in `catch_unwind`: any
unexpected exit (panic) marks the runner `Failed` with a fatal fault — a
dead supervisor must never look alive. `remove`/`contains`/`active_count`
stay ungated (releasing or observing ownership is legal in any phase).

Regressions: the fatal smoke now asserts post-fatal admit rejection with an
empty registry; `heartbeat_thread_panic_marks_runner_failed_and_rejects_admit`
drives a deterministic thread panic through a tripped test clock
(`TripwireClock` delegates to a `ManualClock` until armed).

#### P1-3 — NotFound during renewal was classified as ordinary authority loss

`renew_identity` mapped `StaleAuthority | InvalidAuthority | NotFound` to
`AuthorityLost` (quiet drop). But an admitted execution's durable identity
(Execution/Attempt/Lease) existed at mint time and Agentype never deletes
execution history: a `NotFound` after admission is durable corruption or an
impossible identity — for a WRITE worker precisely the case that must not
masquerade as a normal expiry.

Fix: only `StaleAuthority | InvalidAuthority` are `AuthorityLost`; every
other kernel fault (NotFound, InvariantViolation, StorageFailure,
RecoveryRequired) is `Fatal` (fail-stop). The Kernel primitive still reports
the durable fact (`NotFound`); classification is the caller's responsibility
— the storage-suite helper was narrowed accordingly and the
unknown-execution case now asserts `NotFound` explicitly.

Regression: `missing_execution_is_fatal_not_authority_loss` (runtime) — the
durable execution row is deleted below the API boundary after admission;
renewal returns `Err(Fatal(NotFound))` and the service leaves the entry in
place (the production runner fail-stops on the same classification).

#### P2-1 — Legacy `Kernel::heartbeat` documented

The attempt/epoch-only M4 primitive remains public solely for the frozen M4
test surface (its only callers are m4_kernel tests). It now carries an
explicit LEGACY doc: no execution fence, not wired to supervision admission,
production renewal must use `renew_supervised_execution`, visibility
reduction scheduled for the M5.8 composition freeze.

#### P2-2 — Duration representability + liveness wording

`RuntimeTimingConfig` now rejects finite-but-unrepresentable durations
(`Duration::try_from_secs_f64` gate; new `UnrepresentableDuration` variant —
`from_secs_f64` panics beyond the Duration range). The deadline-scheduler
guarantee wording was weakened to the accurate liveness claim: the scheduler
introduces no deterministic phase drift that places a renewal deadline
beyond the durable expiry; external OS/storage stalls can still delay a tick
past its deadline, in which case the frozen expiry fencing fails closed.

#### Round-2 test mapping

| Finding | Regression(s) |
|---|---|
| P1-1 clock sampled before serialization | `renewal_timestamp_is_sampled_after_transaction_serialization` (fails on the pre-fix kernel) |
| P1-2 lifecycle / fatal admit / panic exit | fatal-smoke extension, `heartbeat_thread_panic_marks_runner_failed_and_rejects_admit` |
| P1-3 NotFound fatality | `missing_execution_is_fatal_not_authority_loss`, storage helper narrowed |
| P2-1 legacy heartbeat | doc-only (no behavior change) |
| P2-2 representability + wording | `unrepresentable_durations_are_rejected`, doc-only |
