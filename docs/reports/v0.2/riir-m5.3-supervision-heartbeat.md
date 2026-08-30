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
cargo test --workspace                                   → 231 passed, 0 failed
python -m compileall -q src tests                        → OK   (py -3 = Python 3.13.5)
python -m unittest discover -s tests -t .                → 162 ran: 160 passed, 2 skipped, 0 failed
git diff --check                                         → clean
```

Rust breakdown (231):

- `agentype-adapter-api`: 8
- `agentype-core`: 20 (unchanged M4 domain suite; zero core changes in M5.3)
- `agentype-execution-config`: 8 (7 prior + blank-adapter-kind constructor test)
- `agentype-runtime`: 95
  - dispatch/launch suite: 76 (60 M5.2 + 4 outcome-vocabulary closure + 12 supervision integration)
  - `supervision` module: 14 (12 deterministic registry/service/crash-window + 2 real-thread runner smokes)
  - `timing` module: 5
- `agentype-storage-sqlite`: 100
  - `m4_kernel` 64, `recovery` 11, `topology` 16 (all untouched semantics)
  - `supervision` (new): 9 (fenced renewal primitive conformance)

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
