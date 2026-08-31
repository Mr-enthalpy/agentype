# RIIR M5.4 — Restart Reconciliation and Recovery Barrier

Status: Historical Report
Applies to: branch `rust/m5.4-reconciliation` (base: main @ M5.3 merge `93c8083`)
Canonical path: `docs/reports/v0.2/riir-m5.4-restart-reconciliation.md`
Not a specification.

Despite the historical `riir-` directory naming, this milestone is **native Rust
runtime implementation**. It consumes the frozen M5.3 admission/heartbeat
boundary and implements spec 14's restart barrier.

---

## 1. Frozen M5.4 mission

> M5.3 defines who may continue renewing.
> M5.4 defines how a restarted Runtime earns that right again.

**READY invariant:** for every Task that still holds ACTIVE current
Attempt/Lease authority and has an Execution:

1. this Runtime MUST positively observe that Execution as exact RUNNING;
2. MUST pass fenced RUNNING confirmation + Lease renewal;
3. MUST already be in this Runtime's `SupervisionRunner`.

Otherwise the Attempt MUST lose execution authority via existing failure /
writer-safety policy **before READY**.

```text
READY + current ACTIVE authority + Execution exists + not supervised
```

is illegal.

Heartbeat still only renews Scheduler authority. It does not prove the
physical process is alive. M5.4 is restart reconciliation, not a long-running
process-observation loop.

Not implemented (later milestones): notifier/RootBridge (M5.5), adapter
deadlines (M5.6), real adapters (M5.7), daemon process lock/signals (M5.8),
steady-state physical monitor, M6 semantics.

---

## 2. Implementation slices

| Slice | What landed |
|---|---|
| **A** (`64b829b`) | `RunningAuthorityGrant`; `SupervisionAdmission::from_grant` is the only production mint |
| **B** | `ExecutionReconciliationSnapshot` + `Kernel::reconciliation_candidates` — facts, not a grant |
| **C** | `normalize_start_observation` / `normalize_collected_outcome` shared by Dispatch and Recovery |
| **D** | `replay_persisted_terminal_consequence` (Category A, before any `reconcile_start`) |
| **E** | `reconcile_one_execution` — identity-preserving `reconcile_start`, never `start_execution` |
| **F** | `StartupGuard` + `recover_runtime` (empty runner, RAII cleanup) |
| **G** | final `expire_leases` → `promote_retry_wait` → `reconcile_pool` → `revive_eligible_agents` → READY check |
| **H** | core crash windows and READY invariant tests; inherited M5.2/M5.3 race coverage |
| **I** | this report |

`Kernel::recover_authority` is unchanged (M4 convenience: expire + promote +
pool + revive). M5.4 **must not** call it as Phase 1 — promote/pool/revive
run after physical reconciliation.

---

## 3. Authority boundary (unchanged from A)

```text
adapter RUNNING (start or reconcile)
  → Kernel::confirm_running_and_renew
  → RunningAuthorityGrant
  → SupervisionAdmission::from_grant
  → SupervisionRunner::admit   (live runner; lifecycle gate + wake-up)
```

A persisted `state='RUNNING'` row cannot mint. Snapshot fields are getters.
`CurrentAuthorityHint` is a routing diagnostic, never a grant. Every ACK /
NACK / re-renew re-enters a Kernel transaction.

Live dispatch and restart re-admission share `confirm_running_and_renew`.

---

## 4. Candidate reader (B)

`ExecutionReconciliationSnapshot` carries persisted identity (`RequestId`,
frozen `adapter_kind`, handle, proof bits, isolation) plus a
`CurrentAuthorityHint` (attempt/lease active, expiry, task state, current
attempt). It does **not** carry Claim DTOs, current target/profile lookup,
model/provider, or SpawnSource.

Corrupt/blank durable `adapter_kind` or unparsable JSON fails the whole
read (internal durable uncertainty stops the Scheduler). Order is
availability-only: current-authority first by nearest expiry.

No schema version bump (stays at 2).

---

## 5. Shared classifiers (C)

`crates/agentype-runtime/src/observation.rs`:

- `StartObservationKind::{ExactRunning, TerminalCandidate, Unresolved}`
- `CollectedOutcomeKind::{TerminalSuccess, TerminalFailure, Unresolved}`
- `adapter_invocation_failure_class` (Unavailable → ResourceUnavailable,
  DeadlineExceeded → Timeout, Protocol → AdapterProtocolFailure, Other →
  StartFailure)

Dispatcher `commit_start_observation` / `commit_collected_outcome` call the
same functions as recovery. One vocabulary.

---

## 6. Terminal replay (D)

Physical history and ACK/NACK are different machines (PR #10 audit P1-1).
After `collect_outcome`, the dispatcher/recovery collect path persists an
honest pending terminal fact via `Kernel::record_pending_physical_terminal`
(`SUCCEEDED`/`FAILED` + `terminal_confirmed`, handle copied, **no**
incarnation presence) **before** ACK/NACK. Incarnation WARM/TERMINATED is
decided by the subsequent authority transaction (`incarnation_reusable`).

Legal crash window:

```text
Execution = SUCCEEDED, terminal_confirmed = true
Attempt ACTIVE, Lease ACTIVE, Task RUNNING, Result = none
```

Replay matrix:

- `SUCCEEDED` + `terminal_confirmed` + current → `ack_success` (one Result,
  or writer-safety suspension). `ack_success` accepts an already-SUCCEEDED
  row without requiring a fake UNKNOWN rewrite.
- `FAILED`/`TERMINATED` + `terminal_confirmed` + current → `nack` (skips
  rewriting already-terminal/LOST physical history).
- terminal physical + stale → physical history only, never a Result.
- Result already exists → `AlreadyApplied`.
- STARTING/RUNNING/UNKNOWN/LOST → not Category A (`outcome_json` on UNKNOWN
  is **not** success proof).
- `SUCCEEDED` without `terminal_confirmed`, or `SUCCEEDED` plus a
  `failure_class` → inconsistent durable evidence, startup-fatal.

---

## 7. Single-execution reconcile (E)

Routes by persisted `adapter_kind` only. Missing kind →
`RESOURCE_UNAVAILABLE` + existing retry/writer-safety; never death,
quiescence, TERMINATED, or adapter fallback.

`reconcile_start(request_id, handle_hint)` is identity-preserving.
`start_execution` is never called.

| Observation | Current authority | Stale |
|---|---|---|
| exact RUNNING | grant → from_grant → admit | persist RUNNING history, no grant |
| terminal-looking | `collect_outcome` then ACK/NACK/writer-safety | physical history only |
| ambiguous / STARTING / UNKNOWN / protocol-invalid | nonterminal NACK + writer safety | physical history only |
| LOST | close authority, never admit; Kernel `nack` does not rewrite LOST | history only |

Per-Execution adapter/protocol uncertainty is an outcome, not startup-fatal.

Kernel change (gap proven): `nack` of a LOST Execution closes Task/Attempt/
Lease authority **without** rewriting the LOST row. Physical history stays
LOST.

---

## 8. Coordinator and READY (F/G)

`recover_runtime(kernel, adapters, timing)`:

1. `expire_leases(true)`
2. start **empty** `SupervisionRunner` inside `StartupGuard`
3. replay Category A
4. reconcile STARTING / UNKNOWN / RUNNING / LOST (admit through the runner)
5. `expire_leases(false)` (leases that expired during adapter I/O)
6. `promote_retry_wait` → `reconcile_pool` → `revive_eligible_agents`
7. runner health
8. READY invariant
9. `StartupGuard::commit`

Uncommitted Drop: `shutdown` + join + clear admissions. Cleanup ≠ revoke
Lease ≠ terminate worker ≠ quiescence.

READY check: every still-current Execution is supervised RUNNING, or the
function refuses to return.

---

## 9. Cross-process and schema

`SupervisionAdmission` is process-local. Two OS processes opening the same
database can both theoretically grant+admit. M5.4 does **not** add a durable
supervision-owner table. Single active Scheduler daemon is a composition
precondition; M5.8 enforces it.

`adapter_binding_key` remains `BLOCKS_REAL_ADAPTER_PARITY`,
`DOES_NOT_BLOCK_M5.4_FAKE_RECONCILIATION_KERNEL`.

Steady-state "worker dies 30s after admission" is not solved here.

---

## 10. Test mapping (plan §26)

Workspace: adapter-api 8 + core 20 + execution-config 8 + runtime 132 +
storage 107 = **275** rust tests. Clippy `-D warnings` clean.

Exact-head CI also ran the Python V0.1 oracle: Ubuntu 3.11 `Ran 162 tests`,
**160 passed + 2 skipped**.

| Group | Coverage |
|---|---|
| A Candidate identity (1–7) | `tests/reconciliation.rs` — STARTING/UNKNOWN/RUNNING RequestId, frozen adapter_kind, blank/corrupt fail-closed, read is not a grant |
| B Re-admission (8–20) | UNKNOWN→RUNNING grant+admit; persisted RUNNING / adapter presence never admit alone; stale cannot admit; `start_execution` count stays 0; `now == expires_at` on reconcile; ACK/cancel vs re-admission both orders |
| C Unresolved (21–29) | default reconcile UNKNOWN; LOST never admitted; missing adapter is availability; unisolated WRITE → SUSPENDED |
| D Terminal collection (30–37) | terminal-looking reconcile requires collect; collected success ACKs |
| E Durable replay (38–43) | physical SUCCEEDED+current → Result; FAILED+current → NACK; stale success no Result; UNKNOWN+outcome_json is **not** proof; exactly-one Result |
| F Startup lifecycle (44–52) | empty recover; readmit during barrier; **readmit then later fatal clears admissions** (StartupGuard Drop) |
| G Final barrier (53–60) | READY invariant uses `looks_current_at(now)`; lease expiry during adapter I/O swept before READY; no dispatch inside recovery |
| H Regression (61–66) | M4/M5.1–M5.3 suites green; no M6 types |

---

## 11. Review questions (plan §27)

1. Can persisted RUNNING automatically receive heartbeat? — **NO**
2. Can adapter presence alone create admission? — **NO**
3. Can Recovery construct SupervisionAdmission from raw IDs? — **NO** (`from_grant` only)
4. Can stale Attempt be re-admitted? — **NO**
5. Can expired Lease be renewed during recovery? — **NO**
6. Can `start_execution` be called during recovery? — **NO**
7. Can `reconcile_start` directly authorize ACK? — **NO** (collect required)
8. Is `collect_outcome` still authoritative for new terminal proof? — **YES**
9. Can stale terminal success create Result? — **NO**
10. Can process death establish quiescence? — **NO**
11. Can missing adapter establish process death? — **NO**
12. Can missing adapter fall back to another adapter? — **NO**
13. Does current target/profile configuration re-route an existing Execution? — **NO**
14. Can failed startup leave heartbeat ownership behind? — **NO** (StartupGuard Drop)
15. Does cleanup revoke Lease by itself? — **NO**
16. Can recovery return READY while supervision runner is Failed? — **NO**
17. Can a current unresolved writer be silently retried without writer safety? — **NO**
18. Can durable terminal evidence survive crash-before-ACK and be replayed? — **YES**
19. Does recovery create a new Attempt/Execution to replace an ambiguous old one? — **NO**
20. Does M5.4 provide cross-process daemon exclusivity? — **NO, M5.8**
21. Does M5.4 implement continuous physical process monitoring? — **NO**
22. Does M5.4 introduce Generation/AgentType/SpawnSource semantics? — **NO**

---

## 12. Public façade

```text
recover_runtime(Arc<Kernel>, &AdapterRegistry, RuntimeTimingConfig)
  → RecoveredRuntime { runner }
```

Dispatch is a separate object. Callers MUST NOT `Dispatcher::dispatch_one`
until `recover_runtime` returns. There is no daemon `run()` (M5.8).

---

## 13. PR #10 audit closure

REQUEST CHANGES on `515c195` closed in this order, no extra scope:

**P1-1.** Physical terminal + current authority is a legal pending
consequence. `UNKNOWN + outcome_json` is not success proof.
`record_pending_physical_terminal` writes the Execution row without
applying incarnation presence (so reusable WARM continuity still belongs
to ACK). `ack_success` accepts already-SUCCEEDED; `nack` skips rewrite of
FAILED/TERMINATED/LOST.

**P1-2.** `RunningAuthorityGrant` is move-only (`#[derive(Debug)]` only).
One confirm+renew commit → one grant → `from_grant` consumes it → one
admission. A later confirm still mints a **new** grant.

**P1-3.** Recovery-level proofs: post-admit startup fatal cleanup, ACK vs
re-admission both orders, cancel vs re-admission both orders, exact expiry
boundary, lease expiry during adapter I/O before READY.

**P2.** `structurally_current()` vs `looks_current_at(now)` (READY uses the
latter). Python oracle evidence is the exact-head CI result (160 passed,
2 skipped), not “not re-run this session”.
