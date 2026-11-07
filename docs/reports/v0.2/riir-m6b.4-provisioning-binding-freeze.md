# RIIR M6-B.4 — Provisioning Binding and Typed Physical Execution Freeze Record

Status: Historical Report
Applies to: V0.2 / M6-B.4 closure (PR #24, merge `5808783`)
Date: 2026-11-06
Updated: 2026-11-08
Freeze state: **FREEZE READY — awaiting maintainer decision**
Canonical path: `docs/reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md`
Not a specification. The frozen scope, if approved, is only ADR-0012's.

Verdict: **FREEZE READY — implementation, evidence and scope are closed within
ADR-0012; the FROZEN label is the repository maintainer's decision**. The fifth
independent audit returned `CLOSED WITH STATED
SCOPE` for the ADR-0012 boundary with no new P0/P1 blocker. The accepted scope
is **typed physical execution → durable ACK
interface**: typed Task → B.3 matching → B.4 physical eligibility →
Attempt/Lease/ProvisioningBinding → pure prepare → Execution/BindingSnapshot →
exact adapter start → observation/recovery → durable Result ACK boundary, with a
real source integration and a real OS-enforced workspace jail. A production
worker-originated Result transport, worker authentication, delivery retry, and
network ingress are **not** part of this scope (see *Harness-injected ACK
boundary*); the suite does not claim worker-originated end-to-end ACK. That
boundary is now a normative decision in
[ADR-0012](../../decisions/0012-m6b4-scope-typed-physical-execution-ack.md),
not only a PR note. B.5
credentials, cold/revivable matching, full typed topology, executed-WARM
replacement and v7→v8 migration remain out of scope and fail closed (see
*Remaining limits*).

## Closure states

This record separates four states that earlier text used interchangeably:

```text
IMPLEMENTED                 production typed dispatch, source integration and
                            real OS-enforced execution exist (PR #24)
CLOSED WITH STATED SCOPE    fifth independent audit accepted the ADR-0012
                            boundary with no new P0/P1 blocker
FREEZE READY                implementation, evidence, normative text, deferred
                            ownership and CI are reconciled at this record
FROZEN                      the maintainer accepts the freeze; not asserted here
```

`FREEZE READY` is a statement about evidence completeness, not a substitute for
the maintainer's decision. This record does **not** freeze the worker-originated
semantic Result data plane: that data plane is outside ADR-0012 and remains
deferred.

## Scope and acceptance baseline

PR #24 implements the authority-bearing typed acquisition above the frozen
baselines M4, M5, M6-A, M6-B.1/B.2/B.3
(`cbd5a7518395247dc7abfef996eb307e7a526f25`, schema v7). The first revision
started from PR head `c8ed98f78ec599976903eae1e141849cfb361c52` and closed the
three gaps recorded there:

1. production `ControlLoopService` typed dispatch (not a test-only helper),
2. a concrete `SourceConfigIntegration` (`agentype-source-workspace-jail`),
3. a provisioning-capable real adapter with real OS enforcement
   (`JailedProcessAgentAdapter`).

This revision starts from the independently re-audited head
`bf4963370f65cf0c8e20b3869b445d080d6af35d` and closed the follow-up P1/P2
findings of that round: the Windows window-station ACE was reduced to a verified
minimal mask with real `OpenWindowStationW` negatives for the write-shaped
rights, the Linux enforcement claim was contracted to a content-write boundary
with real `chmod`/`utimensat` counterexamples, and locator persistence was made
atomic and fsynced with a canonical locator root bound into the physical domain
identity.

The current revision starts from head `8864ed2` and closes the second re-audit's
P1-1, P1-2 and P2-1:

- Windows `GetTokenInformation` output is read from a word-aligned allocation
  with validated returned length, array count and allocation bounds, instead of
  casting a byte vector to `#[repr(C)]` structures (P1-1);
- the adapter no longer grants any ACE on the shared window station. Only the
  per-Execution private desktop DACL is extended, so repeated Execution
  lifecycles cannot accumulate station grants (P1-2);
- spec 07 now states that the typed exact-binding path is production-wired
  through `ControlLoopService::cycle` and `SchedulerDaemonBuilder::typed_dispatch`
  (P2-1);
- [ADR-0012](../../decisions/0012-m6b4-scope-typed-physical-execution-ack.md)
  records the accepted `typed physical execution → durable ACK interface` scope,
  and the deferred worker-originated Result data plane inside it, as a normative
  decision rather than a PR comment.

The head after this revision started from the third independently audited commit
`d2499c71c6edffa18918a44d04630b89fec58452` and closes its P1 and both new P2
findings:

- Windows birthless-locator identity is proved by **one** flow
  (`pin::prove_windows_instance`): one open `PROCESS` handle yields the birth
  time and the per-Execution restricting-SID witness, and the same handle is
  retained in `ProvenInstance` for the subsequent control act. Every
  locator-to-control entry point (`observe_execution`, `interrupt_execution`,
  `terminate_execution`, `collect_outcome`, `reconcile_start`) reuses that flow;
  a failed SID witness is `unproven`, which stays `Unknown`/`Absent` and never
  interrupts, terminates, or collects. The reused M5
  `LocalProcessAgentAdapter` has no OS witness on Windows, so it treats any
  birthless Windows locator as unproven rather than deriving identity from a
  PID.
- The typed dispatch enumeration is bounded in SQL: readiness, cooldown
  exclusion, the frozen `priority DESC, created_at ASC, id ASC` order, and
  `LIMIT` are all applied by the database, and Core's `order_ready_tasks`
  remains the authoritative re-check. A blocked head of 30 Tasks no longer
  forces an O(N) load/sort to find the ready Tasks behind it.
- Spec 07 (and the matching conformance note in spec 16) now separates a
  pre-acquisition `SelectionAmbiguous` deferral — no Attempt, no
  `RESOURCE_UNAVAILABLE` NACK, re-probe after cooldown/frontier change — from a
  post-acquisition Task failure.

The new negative tests point a birthless locator at the test process's own PID
without that Execution's restricting SID and assert that `observe_execution`,
`interrupt_execution`, `terminate_execution`, and `collect_outcome` all refuse to
act (`Unknown`, no artifacts). `sid_and_birth_proof_come_from_the_same_open_handle`
is the deterministic race injection: the test-only probe records the handle used
for each phase and asserts they are identical, so reopening the PID between the
two proofs fails the test without needing a real PID recycle.

The head after this revision started from the fourth independently audited commit
`3e51c4b8b9daa342d814c73c0e2c415c0ef2c938` and closes its P1 and both new P2
findings:

- The variable-length `TOKEN_GROUPS`/`TOKEN_PRIVILEGES` result is no longer read
  through a Rust reference formed before the length check. `TokenInfoBuffer`
  first bounds-checks and reads only the four-byte count, then validates the
  array extent, and only then forms a typed slice. A short header (including a
  zero-entry return), a count beyond the returned bytes, and a returned length
  beyond the allocation all fail before any typed access. The `fake-agent`
  privilege probe follows the same order. P1.
- The cooldown filter is a single JSON-array parameter consumed by `json_each`,
  so the bind-parameter count is fixed at three however many Tasks are cooling.
  A 40,000-id cooldown set no longer risks `SQLITE_MAX_VARIABLE_NUMBER` and a
  fatal persistence error, while exclusion still happens before `LIMIT`. P2.
- Process-tree lifecycle is now an explicit boundary: termination covers the
  proven direct process, no process group/cgroup/Job Object is created, and
  process death remains non-quiescence. Managing descendants and its conformance
  are deferred. P2 boundary, not a correctness fix.

The prior round's closure of P1-1, P1-2, P1-3, P2-2 and P2-3 remains in place:
locator evidence survives the `spawn → identity` window, Windows children keep
only `SeChangeNotifyPrivilege` and run on a per-Execution desktop, the typed
daemon restart test uses a real process crash over the same store, a mixed
legacy/typed queue test proves no starvation, and the PR description is
re-synchronized with the actual head.

### Fifth independent audit (`ec093753f3`, 2026-11-08)

The fifth independent audit found no new P0/P1 implementation blocker and
returned `CLOSED WITH STATED SCOPE` for the ADR-0012 boundary. It restated three
limits rather than requesting code changes:

- **F1 — worker-originated Result E2E is a scope gap, not a code error.** The
  suite proves the durable ACK interface (the harness reads the committed
  Attempt/Lease/Execution and calls `Kernel::ack_success`), not a worker that
  transports an authenticated semantic Result through retry and ingress. The
  audit explicitly refuses to call this full worker-to-Result closure. No code
  change is implied, because ADR-0012 already excludes that data plane.
- **F2 — physical Execution artifacts have no explicit retention/safe-GC
  contract.** Per-RequestId jail/workspace directories, stdout/stderr and the
  locator are created on the start path and are retained across termination,
  collect and restart. This is a deployment/long-running limitation, recorded
  below; deleting potentially live artifacts without a retention owner would be
  unsafe, so this revision intentionally adds no aggressive deletion.
- **F3 — equal-continuity `SelectionAmbiguous` liveness limit.** Two equally
  ranked SourceConfigs still defer rather than picking a winner. ADR-0010 already
  accepts this fail-closed deferral; this record does not invent an ID tie-break.

These three findings are closed for the freeze decision as follows:

- **F1** — accepted scope exclusion. ADR-0012 already excludes the
  worker-originated data plane; the durable Kernel ACK interface remains proven
  by the harness-injected E2E. Not a blocker. Tracking:
  `D-WORKER-RESULT-DP` in [spec 17](../../specs/v0.2/17-deferred-open-questions.md).
- **F2** — accepted operational limitation. No production deletion path exists
  for jail roots, workspaces, stdout/stderr or locators; immediate deletion
  without a durable positive safety witness would be unsafe. Not a blocker.
  Tracking: `D-ARTIFACT-GC` in [spec 17](../../specs/v0.2/17-deferred-open-questions.md).
- **F3** — accepted liveness limit. Equal-continuity selection continues to
  defer as `SelectionAmbiguous` under ADR-0010; no tie-break, random choice or
  implicit fallback is added. Not a blocker. Tracking: `D-SELECTION-POLICY` in
  [spec 17](../../specs/v0.2/17-deferred-open-questions.md).

The implementation round therefore hands over a **FREEZE READY** record. The
`FROZEN` label is not asserted here: it requires the maintainer's explicit
decision after reviewing this closure PR.

Deployment remains restricted to fresh schema-v8 databases and reopening
existing v8 databases. v7→v8 migration is not implemented. The frozen specs
govern acceptance; interface amendments are justified in
[ADR-0008](../../decisions/0008-m6b-typed-provisioning-acquisition.md) and
[ADR-0011](../../decisions/0011-m6b-acquisition-contract-parity.md).
[ADR-0009](../../decisions/0009-m6b-topology-capacity-seam.md) records the
legacy/typed capacity seam; [ADR-0010](../../decisions/0010-m6b-selection-order-scope.md)
retains the unresolved availability/cost ranking question.

## Implemented path

| Phase | Committed facts and checks |
|---|---|
| Enumerate | `Kernel::list_typed_dispatch_candidates_excluding` performs one authority-free, DB-bounded read: the shared readiness predicate, the runtime-local cooldown exclusion, the frozen `priority DESC, created_at ASC, id ASC` order, and `LIMIT probe_limit` are all applied in SQL, so a large blocked or cooling head does not force an O(N) load/sort. The cooldown set is one JSON-array parameter evaluated by `json_each`, so the bind-parameter count stays fixed however many Tasks are cooling. Core's `order_ready_tasks` re-checks readiness and order on the returned bounded page. It returns no claim and mutates nothing |
| Resolve | Validate partition target/profile; apply known hard filters before bounded, read-only source-local attestation; rank eligible candidates and capture the catalog frontier |
| Acquire | Transactionally reload and re-prove Task, actual bound AgentType, placement, source/config/policy, realized retention and exact imported safety; commit Attempt/Lease/assignment and per-Incarnation ProvisioningBinding; leave `Attempt.incarnation_id` unset |
| Prepare | After winner selection/commit, create the winner's **fresh** prepare deadline; re-resolve the explicitly registered exact source; fence claim authority and integration protocol before/after pure preparation; require the attested digest; return a secret-free descriptor |
| Commit Execution | Requalify target and current exact adapter; retain that adapter instance/deadlines; atomically create Execution, Attempt→Incarnation association and immutable BindingSnapshot |
| Physical start | `Dispatcher::start_prepared` calls the retained adapter instance's `start_execution` while still holding the `DispatchGate` permit; the adapter creates the per-execution jail under the committed descriptor and OS-enforces the M5 workspace mode |
| Observe / recover | Existing M5 observation, deadline, writer-safety and exact `(adapter_kind, adapter_binding_key)` recovery own the physical lifecycle; no adapter re-selection occurs after commitment |
| Result ACK | The durable Kernel Result-ACK command (harness-injected in E2E; no production worker transport) validates lease epoch and writer safety; a late physical end is history only |

The production daemon enables this only through
`SchedulerDaemonBuilder::typed_dispatch(integrations, config)`. Without that
call typed Tasks stay QUEUED: the legacy claim path excludes them, which is the
fail-closed default.

### Physical provisioning capability

`JailedProcessAgentAdapter` (`adapter_kind = jailed_process`, protocol
`agentype.workspace-jail/v1`) reports an honest matrix:
`{ReadOnly, Write} × {Enabled}`, `attempt_isolation = false`. It cannot claim
attempt isolation, so an isolating Task is ineligible rather than weakly
provisioned.

- Windows: `CreateRestrictedToken` with `WRITE_RESTRICTED` and a
  **per-Execution** restricting SID derived deterministically from the
  RequestId (`S-1-5-21-<a>-<b>-<c>-<d>`), then `CreateProcessAsUserW`. The
  process-global `S-1-5-12` is never used. The restricted token deletes every
  host privilege except `SeChangeNotifyPrivilege`; shutdown, undock,
  time-zone, working-set and any other inherited privilege are absent, and the
  spawn fails closed if the privilege set cannot be proven minimal. Token
  information is read from a word-aligned allocation whose returned length,
  array count and allocation bounds are validated before any `#[repr(C)]`
  structure or slice is formed; a count or length outside the written region is
  an error, not an unchecked cast.
  Each Execution runs on a per-Execution desktop
  (`agentype-desktop-<request_id>`) whose DACL is extended with
  `DESKTOP_ALL_ACCESS` for that Execution's restricting SID; the creating
  user's default ACEs remain, so this is per-Execution desktop confinement, not
  a same-user security boundary. The adapter **never modifies the shared window
  station DACL**. A per-Execution ACE there could not be revoked safely while
  other workers may still be running, and it would accumulate for the
  scheduler's lifetime; the desktop is therefore the only object granted to the
  restricting SID. Because the token is `WRITE_RESTRICTED`, write-shaped station
  access is evaluated against the restricting SIDs, which hold no station ACE,
  so the worker is denied `WINSTA_CREATEDESKTOP`, `WINSTA_WRITEATTRIBUTES`,
  `WINSTA_ACCESSCLIPBOARD`, `STANDARD_RIGHTS_REQUIRED`, and `WINSTA_ALL_ACCESS`
  by real `OpenWindowStationW` access checks inside the worker. Read-shaped
  rights (`ENUMDESKTOPS`, `READATTRIBUTES`, `ENUMERATE`, `ACCESSGLOBALATOMS`,
  `EXITWINDOWS`, `READSCREEN`) are evaluated with the token's normal SIDs
  against WinSta0's pre-existing DACL; the adapter neither grants nor removes
  them, and the probe reports them honestly rather than claiming they are
  denied. This adapter
  therefore claims per-Execution write containment and desktop confinement, not
  same-user read confidentiality or a hostile-code sandbox.
  A new window station is attempted only if the host privilege exists;
  `CreateWindowStationW` returns `ERROR_ACCESS_DENIED` for ordinary interactive
  users, so the adapter fails closed rather than silently using the interactive
  desktop. `Write`
  grants one modify ACE for that Execution's SID on the per-execution jail root
  and workspace via direct
  `GetNamedSecurityInfoW`/`SetEntriesInAclW`/`SetNamedSecurityInfoW`; `icacls`
  is not used because it cannot resolve an arbitrary non-account SID
  (Windows error 1332). `ReadOnly` grants no such ACE. An ACE for Execution A's
  SID cannot authorize Execution B's token, which carries a different SID.
  A restricted child needs an inherited console, so the adapter fails closed as
  Unavailable when the host has none (rather than spawning a process that dies
  with `STATUS_DLL_INIT_FAILED`). This is a workspace/desktop confinement
  mechanism, not a general malicious-code sandbox.
- Linux: Landlock in `pre_exec` with `PR_SET_NO_NEW_PRIVS`. The adapter probes
  the Landlock ABI and **requires ABI >= 3**: ABI 1/2 cannot deny `truncate(2)`,
  which bypasses `WRITE_FILE`. `start_execution` and the child ruleset both
  reject older ABIs rather than claiming enforcement. `ReadOnly` installs no
  path rule; `Write` installs one path-beneath rule for the per-execution jail
  root covering write-class rights including `TRUNCATE`. This is a
  **content-write boundary**: Landlock does not mediate metadata syscalls, so
  `chmod`, `utimensat`, and `setxattr` outside the jail are explicitly not
  confined. The two Linux counterexample tests assert both the denied content
  write and the succeeding metadata change in `ReadOnly` and `Write`; no claim
  here says the jail makes the rest of the filesystem immutable.
- Other hosts: the enforceable-workspace set is empty, so B.4 rejects the
  adapter; `start_execution` also fails closed.

Both platforms construct an explicit minimal worker environment instead of
inheriting the daemon environment. Windows builds a sorted UTF-16 environment
block and passes it with `CREATE_UNICODE_ENVIRONMENT`; Linux uses `env_clear()`
plus the same non-sensitive allowlist. `HOME`/`TEMP`/`TMP` (Windows) and
`HOME`/`TMPDIR`/`TMP` (Linux) point into the jail workspace. Credential-like
daemon variables are absent; the sentinel-secret test proves a daemon-only
secret does not reach the worker while required non-sensitive variables remain.

The source-config grammar is deliberately secret-free and carries no
environment, credential, network, or workspace-mode field:
`{format, command, args, jail_root}`. Core never interprets the body; the
adapter enforces the mode/network of the M5 start request. The attested digest
binds the canonical descriptor to the exact
`(adapter_kind, adapter_binding_key)`, so a descriptor cannot be replayed
against another physical installation.

### Durable physical locator (P1-2)

`spawn()` returns a process identity only after a birth-time probe, so a crash
between OS spawn and that probe previously left no durable physical evidence.
The adapters now write `locator.json` **immediately after spawn and before the
identity probe** under
`<locator_root>/agentype-jail-exec-<request_id>/locator.json` (jail) or
`<locator_root>/agentype-exec-<request_id>/locator.json` (legacy local process).
The record carries `v`, `kind`, `pid`, `inst`, `request_id`, stdout/stderr paths,
optional `birth`, optional `spawn_after_ms`, plus
`identity_complete=false` and `reconnectable=false`.

`locator_root` is `std::env::temp_dir()` canonicalized exactly once per process
and is bound into the binding key as `:locroot=<path>`. A restarted Scheduler
that resolves a different `TMPDIR`/`TEMP` therefore computes a different
`adapter_binding_key` and cannot mistake the old physical domain for a domain it
can still recover; the cross-process test
`domain_key_binds_the_locator_root_across_processes` proves the key changes and
stays stable per root.

Persistence is not best effort:

- The record is written to a unique `.locator.<pid>.<n>.tmp` file with
  `create_new`, `write_all`, and `sync_all`, then atomically renamed over
  `locator.json`; Unix also fsyncs the containing directory. An interrupted
  overwrite leaves either the previous whole record or the new whole record,
  never a truncated one. `interrupted_locator_rewrite_cannot_destroy_the_previous_record`
  and the Unix `failed_locator_write_preserves_the_previous_record` cover this.
- A failed initial or upgrade write after a real spawn is not reported as a
  clean start and not retried blindly. The adapter returns `Unavailable` with
  the in-memory locator as `runtime_handle_hint`; the runtime records genuinely
  unknown physical state, and
  `locator_write_failure_leaves_the_started_process_unknown_but_controllable`
  proves a fresh adapter can still observe and terminate that exact process
  from the hint.
- `reconcile_start` accepts the locator and, when `birth` is absent, verifies a
  non-impersonating physical witness before adopting a numeric pid. On Windows
  the witness is the per-Execution restricting SID on the same open process
  handle that yields the birth, and that handle is retained for control; on
  Linux it is birth/instance-token verification. A locator cannot impersonate an
  unrelated pid, and a failed witness stays `Unknown` rather than becoming a
  PID-derived identity.
- `observe_execution`, `interrupt_execution`, `terminate_execution`, and
  `collect_outcome` all use the same resolution flow, so the process remains
  controllable after the daemon restarts without a persisted pinned handle and
  no entry point can upgrade a locator with a weaker proof than
  `reconcile_start`. A Windows birthless locator that fails the restricting-SID
  proof is `unproven`: observation is `Unknown`, collect is
  `PhysicalState::Unknown` with no artifacts, and interrupt/terminate do not act.
  The reused `LocalProcessAgentAdapter` has no OS witness on Windows and
  therefore treats every birthless Windows locator as unproven.
- Once the birth identity is known, the locator is upgraded atomically. A crash
  after spawn but before the upgrade still yields a recoverable locator, never a
  blind respawn.
- No path treats "kill was called" as quiescence. Absence of a process produces
  a terminal physical candidate; a terminal candidate without a quiescence
  witness keeps writer safety closed.

### Windows token parsing, minimum privilege and private desktop (P1-1, P1-2, P1-3)

The Windows child token keeps only `SeChangeNotifyPrivilege`. Token information
from `GetTokenInformation` (`TOKEN_RESTRICTED_SIDS` and `TOKEN_PRIVILEGES`) is
read from an explicitly word-aligned allocation, not from `Vec<u8>`. Before any
pointer cast, the code validates that the returned length fits the allocation
and that `count * element_size + header_offset` fits the returned bytes; the
restricted-SID witness additionally checks that each SID pointer lies inside
the written region. `token_information_buffer_is_aligned_and_bounds_checked`
covers alignment, over-count, over-length, overflow and out-of-buffer pointers.
The real-locator tests still exercise the SID witness against a live process.

A per-Execution desktop is created and its DACL is extended with
`DESKTOP_ALL_ACCESS` for that Execution's restricting SID; the creating user's
default ACEs remain, so this is per-Execution desktop confinement, not a
same-user security boundary. The adapter **does not modify the shared window
station DACL at all**; granting a per-Execution ACE there had no safe revocation
point while other workers could still be running and grew the station ACL for
the scheduler's lifetime. The `WRITE_RESTRICTED` access check therefore denies
the restricting SID every write-shaped station right, while read-shaped rights
remain governed by the station's pre-existing DACL. The probe records both
groups, so the report does not overclaim.
`private_desktops_do_not_accrue_window_station_aces` creates and drops real
private desktops for several distinct SIDs and compares the live WinSta0 DACL
before/after, including an assertion that no restricting SID appears in it; the
E2E-level
`shared_window_station_acl_does_not_grow_across_execution_lifecycles` runs three
complete Execution lifecycles and compares the real station ACE count. The
parent-side desktop handle is retained until `collect_outcome` observes process
exit, because closing it while the child is still initializing user32 causes
`STATUS_DLL_INIT_FAILED`. If either the privilege reduction or the private
desktop/ACL step fails, the spawn fails closed; there is no fallback to the
interactive `Default` desktop. These are declared as workspace/desktop
confinement properties, not as a general sandbox for arbitrary malicious code.

## Invariant checklist

| Invariant | Status | Evidence |
|---|---|---|
| AgentType is never SpawnSource/adapter/model/provider | Preserved | `AgentTypeRef`/`SpawnSourceRef` distinct in B.3/B.4 tests; E2E publishes a type revision and a separate source |
| Core does not interpret `SourceConfig` payload | Preserved | Only `WorkspaceJailSourceIntegration` parses it; Core/storage see opaque bytes |
| `attest`/`prepare` pure and read-only | Preserved | No FS/spawn in both methods; deadline-before-invocation tests prove no call |
| Physical creation only in `start_execution` | Preserved | Jail/ACL/process creation occurs in the adapter; pure-prepare tests observe no side effect |
| Every eligibility conjunct re-proved in the authority transaction | Preserved | B.4 56-test suite (`acquisition_reproves_the_mandatory_conjuncts`, readiness/frontier races) |
| No fallback after Execution commitment | Preserved | `prepare_typed_execution_launch` phases; post-commit failures go to settlement/recovery, never candidate fallback |
| `Attempt.incarnation_id` only at Execution commitment | Preserved | `typed_acquisition_and_atomic_binding_snapshot_round_trip` |
| No authority/NACK for typed unavailability before Attempt | Preserved | E2E `unprovisionable_typed_task_creates_no_authority_and_no_nack`, `typed_task_requires_the_exact_adapter_binding_alias`; zero attempts/leases/executions/bindings/failures |
| Bounded re-probe fairness | Preserved | `TypedDispatchState` cooldown per Task, invalidated on catalog-frontier change; cooling ids are excluded inside `list_typed_dispatch_candidates_excluding` **before** `probe_limit`, and `cooling_typed_tasks_do_not_consume_the_probe_window` proves Task N+1 stays visible when the first N are cooling |
| Absolute deadlines across physical phases | Preserved | One `AdapterDeadline` is carried through jail creation, ACL, spawn, identity probe and cleanup; `expired_start_deadline_creates_no_jail_directory`, `expired_prepare_deadline_creates_no_directory`, `deadline_expiring_between_phases_stops_before_the_next_phase`, and Linux `cleanup_never_blocks_past_an_expired_deadline` |
| Worker environment isolation | Preserved | Explicit allowlist on both platforms; `worker_does_not_inherit_daemon_secrets_but_keeps_runtime_variables` |
| Per-Execution write authorization | Preserved | RequestId-derived restricting SID granted only on that jail; Windows `write_a_cannot_write_read_only_b_jail` and `write_a_cannot_write_write_b_jail` |
| Recoverable locator after spawn | Preserved | `locator.json` written before the birth probe with an atomic temp-file + fsync + rename (+ Unix directory fsync); locator root bound into the binding key; `born_less_locator_is_recovered_after_the_spawn_identity_window`, `durable_locator_controls_the_process_after_adapter_restart`, `interrupted_locator_rewrite_cannot_destroy_the_previous_record`, `locator_write_failure_leaves_the_started_process_unknown_but_controllable`, `domain_key_binds_the_locator_root_across_processes`, impersonation negative |
| Windows minimum privilege and private desktop | Preserved | `worker_token_is_minimal_and_desktop_is_private`; only `SeChangeNotifyPrivilege` survives; `Default`/input desktop opens fail; real `OpenWindowStationW` checks deny `CREATEDESKTOP`, `WRITEATTRIBUTES`, `ACCESSCLIPBOARD`, `STANDARD_RIGHTS_REQUIRED`, `ALL_ACCESS` |
| Windows token buffer alignment and bounds | Preserved | `token_information_buffer_is_aligned_and_bounds_checked` proves word alignment plus returned-length, count, overflow and in-buffer SID-pointer validation; the real restricted-SID witness still runs in the locator tests |
| Shared window station DACL has no per-Execution growth | Preserved | `private_desktops_do_not_accrue_window_station_aces` compares the live WinSta0 DACL before/after several distinct SIDs; `shared_window_station_acl_does_not_grow_across_execution_lifecycles` compares the ACE count across three complete Executions |
| Real daemon-crash restart with same physical instance | Preserved | `typed_daemon_restart_recovers_the_same_physical_instance` (separate child process aborts; parent reopens same DB) |
| Legacy and typed queues share the control loop without starvation | Preserved | `legacy_and_typed_tasks_are_both_dispatched_from_a_shared_queue` |
| Linux truncate enforcement | Preserved | Landlock ABI >= 3 gate; `legacy_landlock_abis_are_not_enforceable`; Linux `ftruncate`/`open(O_TRUNC)` denial outside the jail and truncate allowance inside a Write jail |
| Linux enforcement boundary is content writes, not metadata | Contracted | `linux_read_only_landlock_is_a_content_write_boundary_not_metadata_immutability` and `linux_write_landlock_is_a_content_write_boundary_not_metadata_immutability` assert the outside content write is denied while `chmod`/`utimensat` (and `setxattr` when supported) succeed |
| Exact `(adapter_kind, adapter_binding_key)` recovery | Preserved | M5 recovery tests; retention of resolved adapter instance in typed handoff |
| M5 writer safety and absolute deadlines | Preserved | Write E2E refuses ACK without quiescence; runtime deadline/writer suites |
| Accepted scope is typed physical execution → durable ACK interface | Normative | [ADR-0012](../../decisions/0012-m6b4-scope-typed-physical-execution-ack.md); E2E ACK remains harness-injected and no worker transport/auth/retry is claimed |
| Process lock, ReadyPermit, recover-before-dispatch | Preserved | Daemon startup unchanged; full process-lock/supervision suites |
| WARM replacement blocked without physical witness | Preserved | B.4 `warm_resident_provenance_change_requires_physical_lifecycle_evidence`; no new replacement path |
| v7→v8 migration out of scope | Preserved | schema remains fresh-v8 / existing-v8 only |
| B.5 credentials / cold/revivable / typed topology fail closed | Preserved | credential-ref acquisition rejection; no new code path |

## Post-merge acceptance matrix

Baseline: merge commit `5808783` (PR #24), schema v8. `PASS (local)` means the
named Rust test was executed on this Windows host; `PASS (CI)` means the named
platform's CI job executed it. Linux-only enforcement is never claimed from a
Windows compile.

| Contract / invariant | Production code | Test witness | Windows | Linux | Result | Limit / owner |
|---|---|---|---|---|---|---|
| Typed and legacy tasks share the control loop without starvation | `agentype-runtime/src/control.rs`, `lib.rs` | `legacy_and_typed_tasks_are_both_dispatched_from_a_shared_queue` | PASS (local, CI) | PASS (CI) | PASS | — |
| Pre-acquisition unavailability creates no authority and no NACK | `runtime/src/lib.rs` (`defer_or_fail_typed`), `provisioning_resolver.rs` | `unprovisionable_typed_task_creates_no_authority_and_no_nack`, `typed_task_requires_the_exact_adapter_binding_alias` | PASS (local, CI) | PASS (CI) | PASS | Credentials are B.5; fail closed |
| `attest`/`prepare` are pure; no physical creation before Execution | `agentype-source-workspace-jail`, `provisioning_resolver.rs` | `expired_prepare_deadline_creates_no_directory`, `deadline_expiring_between_phases_stops_before_the_next_phase`, `acquisition_reproves_the_mandatory_conjuncts` | PASS (local, CI) | PASS (CI) | PASS | — |
| Attempt/Lease/ProvisioningBinding and Execution/BindingSnapshot commit atomically | `agentype-storage-sqlite/src/kernel.rs` | `typed_acquisition_and_atomic_binding_snapshot_round_trip`, `typed_acquisition_handoff_commits_snapshot_bearing_execution` | PASS (local, CI) | PASS (CI) | PASS | — |
| Exact `(adapter_kind, adapter_binding_key)` binding; no fallback after commitment | `provisioning_resolver.rs`, `runtime/src/lib.rs` | `physical_composition_never_falls_back_to_another_installed_adapter`, `dispatch_missing_adapter_creates_no_writer_ambiguity` | PASS (local, CI) | PASS (CI) | PASS | — |
| Prepare failure settles; confirmed fatal preparation fault escalates once | `provisioning_resolver.rs`, `kernel.rs` | `post_commit_prepare_failure_settles_unavailable`, `confirmed_fatal_preparation_survives_restart_without_execution_lost_retry`, fatal-prep E2E | PASS (local, CI) | PASS (CI) | PASS | — |
| No blind restart after an ambiguous start or missing evidence | `runtime/src/lib.rs`, `recovery.rs` | `dispatch_ambiguous_start_is_persisted_and_never_restarted`, `reconcile_without_any_physical_evidence_is_ambiguous_not_respawned` | PASS (local, CI) | PASS (CI) | PASS | — |
| Daemon crash/restart adopts the same physical instance | `typed_e2e.rs` | `typed_daemon_restart_recovers_the_same_physical_instance` | PASS (local, CI) | PASS (CI) | PASS | — |
| Birthless locator cannot impersonate or act without the SID witness | `agentype-adapter-local-process/src/lib.rs` | `born_less_locator_cannot_impersonate_an_unrelated_pid`, `born_less_locator_without_the_restricting_sid_has_no_control_authority`, `sid_and_birth_proof_come_from_the_same_open_handle` | PASS (local, CI) | PASS (CI) | PASS | — |
| Windows restricting SID/token/desktop and cross-jail write denial | `adapter-local-process/src/jail.rs` | `worker_token_is_minimal_and_desktop_is_private`, `write_a_cannot_write_read_only_b_jail`, `token_information_buffer_is_aligned_and_bounds_checked` | PASS (local, CI) | n/a | PASS | Same-user read confidentiality not claimed |
| Shared window station DACL does not accumulate per-Execution ACEs | `jail.rs` | `private_desktops_do_not_accrue_window_station_aces`, `shared_window_station_acl_does_not_grow_across_execution_lifecycles` | PASS (local, CI) | n/a | PASS | Read-shaped station rights remain |
| Linux Landlock ABI gate and content-write boundary | `jail.rs` | `legacy_landlock_abis_are_not_enforceable`, `linux_read_only_landlock_is_a_content_write_boundary_not_metadata_immutability`, `linux_write_landlock_is_a_content_write_boundary_not_metadata_immutability` | n/a | PASS (CI) | PASS | Metadata syscalls unconfined |
| ReadOnly durable ACK succeeds | `kernel.rs` | `typed_read_only_task_runs_end_to_end_and_acks_through_the_daemon` | PASS (local, CI) | PASS (CI) | PASS | ACK is harness-injected (F1) |
| Write without quiescence refuses ACK | `kernel.rs` | Write E2E, `writer_safety_suspension_is_distinct_from_task_completed` | PASS (local, CI) | PASS (CI) | PASS | — |
| Physical termination creates no Result and no quiescence | `runtime/src/lib.rs`, `kernel.rs` | `dispatch_ambiguous_write_start_never_gains_quiescence`, `unisolated_writer_expiry_without_quiescence_suspends` | PASS (local, CI) | PASS (CI) | PASS | Direct process only; no process tree |
| v7 store rejected; existing v8 store reopens | `storage-sqlite/src/store.rs` | `schema_v7_database_is_rejected_after_b4_tables`, `prior_v8_database_reopen_installs_additive_preparation_fault_guards` | PASS (local) | PASS (closure CI) | PASS | No v7→v8 migration (`D-DB-MIGRATE`) |
| Worker-originated Result transport/auth/retry/ingress (F1) | none — excluded by ADR-0012 | Harness calls `Kernel::ack_success`; no worker transport | n/a | n/a | DEFERRED | `D-WORKER-RESULT-DP` |
| Artifact retention and safe GC (F2) | none — no production deletion authority | `prepare_jail` retains jail/workspace/stdout/stderr/locator; only test code deletes | n/a | n/a | DEFERRED | `D-ARTIFACT-GC` |
| Equal-continuity selection policy (F3) | `provisioning_resolver.rs` | `ambiguous_selection_fails_closed`, `a_new_equal_continuity_config_blocks_warm_reuse_until_the_tie_is_removed` | PASS (local, CI) | PASS (CI) | DEFERRED (liveness) | `D-SELECTION-POLICY` / ADR-0010 |

`PASS (CI)` for Linux refers to the ubuntu job of run `42404395365` (20 jail
enforcement tests, 10 typed E2E, 35 public-API probes). The newly added v7
schema-gate test is not part of that run; its Linux execution is the closure
PR's ubuntu job, and the PR record binds the result to the exact closure SHA.

## E2E trace (real run, Windows)

Command:

```text
cargo test -p agentype-adapter-local-process --test typed_e2e \
  typed_read_only_task_runs_end_to_end_and_acks_through_the_daemon \
  -- --test-threads=1 --nocapture
```

Captured:

```text
typed-e2e trace: task=task_1eb730a572a64f1280b95e953de1c640
  execution=exec_a56a49b407d14cb0815fe80fdf4a201b
  request=request_78a0ac7e64924981926597c1c75ffb46
  adapter=jailed_process/win:UFO:79922a1d-4f10-11f0-b487-b0f7f68c8b54:locroot=\\?\C:\Users\hanni\Desktop\agentype\target\test-tmp
  protocol=agentype.workspace-jail/v1
  jail=...\jail-base\agentype-jail-request_78a0ac7e64924981926597c1c75ffb46
  agent=agent_46b385b4deb042869d6fc6faa1de9dad
typed-e2e ack: task=Completed result=result_184bb188081545468e1f00d4386448a8 physical_state=Succeeded
test result: ok. 1 passed; 0 failed; 0 ignored; 0 filtered out
```

The assertions in that test prove, from durable Scheduler state and the
filesystem, that:

- the Task was dispatched by the production `ControlLoopService`, not a test
  helper;
- the Execution's frozen adapter identity equals the imported
  `(jailed_process, win:...)` and the BindingSnapshot carries the committed
  descriptor and source digest;
- a real per-execution jail directory and `workspace` child exist under the
  request-derived root, and the runtime handle names `jailed_process`;
- the ProvisioningBinding is immutable and names the same physical domain;
- the durable Result ACK boundary is valid: the harness invokes
  `Kernel::ack_success` with the committed Attempt, lease epoch and Execution,
  produces a Result and moves the Task to `Completed`;
- the subsequent physical end becomes `Succeeded`/terminal history and does not
  replace the Result.

### Harness-injected ACK boundary (P2-1)

The E2E suite does **not** implement or exercise production Worker Result
delivery. The harness reads the committed `ProvisioningBinding`/Execution and
calls the durable Kernel command `ack_success` directly. There is no worker
transport, worker authentication, delivery retry, queue, or network ingress in
this scope; B.5 credentials and a worker data plane are deferred. The evidence
therefore covers the **durable Kernel ACK contract** (lease-epoch validation,
ReadOnly completion, Write quiescence refusal, late-physical-end history), not a
complete worker-originated ACK path. Any wider claim would be unsupported. The
accepted scope label is therefore **typed physical execution → durable ACK
interface**; this record does not claim worker-originated end-to-end execution.
[ADR-0012](../../decisions/0012-m6b4-scope-typed-physical-execution-ack.md)
records that boundary and the deferred worker data plane as a normative scope
decision. The fifth independent audit grades this as **F1: a scope gap, not an
implementation fault** — the durable ACK interface is proven, the
worker-originated Result loop is explicitly not.

Additional real-process evidence in this revision:

- `typed_daemon_restart_recovers_the_same_physical_instance` starts the
  production daemon in a **separate child test process**, waits until one typed
  Execution is RUNNING and the jailed worker has written a ready marker, then
  calls `abort()`. The parent reopens the same SQLite store, starts a fresh
  daemon, and proves that recovery adopts the **same pid**, that a fresh adapter
  observes the still-live process, and that no second Execution is created.
- `legacy_and_typed_tasks_are_both_dispatched_from_a_shared_queue` queues a real
  legacy Task (local-process target) and a real typed Task (jail target) at the
  same time. The alternating control loop dispatches both, proving neither queue
  starves the other.

The full 10-test E2E suite also proves the newborn-agent path, a real in-jail
write with a refused unquiescent Write ACK, unprovisionable and missing-alias
Tasks with no authority, post-commit digest drift with no Execution, and a
fatal preparation fault that fails the daemon and produces exactly one
`PREPARATION_FAULT` Root decision after recovery.

## Crash-window and counterexample evidence

| Window / counterexample | Witness |
|---|---|
| Post-Attempt / pre-prepare crash | `interrupted_acquisition_before_execution_recovers_as_execution_lost`; `post_commit_prepare_failure_settles_unavailable` |
| Post-prepare / pre-Execution | `malformed_successful_prepare_is_fatal_without_nack_or_retry`; `prepare_protocol_drift_is_fatal_before_and_after_the_call`; fatal-prep E2E |
| Post-Execution-commit / pre-start | `typed_acquisition_handoff_commits_snapshot_bearing_execution`; typed handoff recovery tests; M5 `execution_commitment_freezes_adapter_kind` |
| `COMMIT → start` real crash | `typed_daemon_restart_recovers_the_same_physical_instance` (child daemon aborts; parent reopens the same store) |
| `spawn → identity` crash window | `born_less_locator_is_recovered_after_the_spawn_identity_window`; `reconcile_without_persisted_handle_recovers_running_instance` |
| `identity → observation persistence` crash window | `durable_locator_controls_the_process_after_adapter_restart`; locator-aware `observe`/`terminate`/`collect` |
| Locator impersonation (all entry points) | `born_less_locator_cannot_impersonate_an_unrelated_pid` and `born_less_locator_without_the_restricting_sid_has_no_control_authority` (observe/interrupt/terminate/collect leave the target alive and report `Unknown`) |
| SID/birth proof substitution | `sid_and_birth_proof_come_from_the_same_open_handle` (test-only probe records the handle each phase used; inequality fails deterministically) |
| Blocked typed queue head | `bounded_typed_enumeration_skips_a_large_blocked_head` (30 ineligible high-priority Tasks, bounded page still finds the ready Tasks) |
| Missing locator/handle after restart | `reconcile_without_any_physical_evidence_is_ambiguous_not_respawned` |
| Unknown Execution COMMIT | `uncertain COMMIT` counterparts in the resolver suite; `kernel_faults_are_never_classified_as_authority` |
| Ambiguous process start | `dispatch_ambiguous_start_is_persisted_and_never_restarted`; `dispatch_ambiguous_write_start_never_gains_quiescence` |
| Missing exact binding after restart | `dispatch_missing_adapter_creates_no_writer_ambiguity`; `physical_composition_never_falls_back_to_another_installed_adapter`; E2E missing-alias case |
| Same-key adapter replacement | `recovery_follows_persisted_isolation_fact_despite_registry_reconfiguration`; `stale_safety_proof_cannot_authorize_later_attempt_after_registry_reconfiguration`; B.4 `execution_cannot_freeze_target_isolation_beyond_the_committed_capability` |
| Expired lease with possible writer | `supervision_loss_then_expiry_suspends_unisolated_writer`; `unisolated_writer_expiry_without_quiescence_suspends`; Write E2E |
| Fatal prepare escalation | `confirmed_fatal_preparation_survives_restart_without_execution_lost_retry`; `preparation_fault_is_immutable_and_fences_execution_and_worker_settlement`; fatal-prep E2E |
| Root retry / cancel races | `retry_wait_is_promoted_then_redispatched`; `cancel_task_releases_a_prepared_claim`; `cancel_batch_releases_a_prepared_claim` |
| Typed unavailable / ambiguous before Attempt | `target_isolation_filters_incapable_source_before_attest_and_selection`; `credential_config_is_filtered_before_attestation_and_cannot_hide_valid_config`; E2E negative cases |
| Windows token parse boundary | `token_information_buffer_is_aligned_and_bounds_checked` and `variable_length_token_header_is_validated_before_any_typed_access` (zero entries, short header, huge count, length beyond allocation all fail before any typed access); the real restricted-SID witness still runs in the locator tests; neither the adapter nor `fake-agent` forms a variable-length typed reference before validating the returned length |
| Cooldown filter size | `large_cooldown_set_cannot_exhaust_sqlite_parameters` (a probe proves this build rejects a 40,000-variable statement; the Kernel query with a 40,000-id cooldown set still returns the ready page and still applies exclusion) |
| Execution artifact lifetime | No production deletion/GC path exists: `prepare_jail` creates the per-RequestId jail root/workspace, stdout/stderr and locator, and termination/collection retain them; only test code calls `remove_dir_all`. Retention, safe-GC ownership and disk-growth control are recorded as an explicit remaining limit (fifth audit F2), not silently handled |
| Repeated Execution lifecycles | `shared_window_station_acl_does_not_grow_across_execution_lifecycles` (three real Executions) and `private_desktops_do_not_accrue_window_station_aces` (several distinct SIDs) |
| Write success without quiescence | `writer_safety_suspension_is_distinct_from_task_completed`; Write E2E (`ack_success` returns `None`) |

## Verification

Local checks on 2026-11-08 (Windows):

| Check | Result |
|---|---|
| `cargo fmt --all --check` | passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | passed |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary` | passed (adapter-local-process lib 39, conformance 26, daemon acceptance 4, jail 19, typed E2E 10, runtime 298, storage m4 72, B.3 42, B.4 56, frontier 49, catalog 32) |
| `cargo test -p agentype-public-api-boundary` | 35 passed |
| `cargo test -p agentype-adapter-local-process --test jail_enforcement -- --test-threads=1` | 19 passed (real OS enforcement, including token/desktop, station-DACL lifecycle, locator recovery and the birthless-locator negative control tests) |
| `cargo test -p agentype-adapter-local-process --test typed_e2e -- --test-threads=1` | 10 passed (including real daemon restart and mixed legacy/typed queue) |
| `cargo test -p agentype-adapter-local-process --lib` | 39 passed (including the variable-length token header negative cases, the deterministic SID/birth same-handle probe, token-buffer alignment/bounds and station-DACL unit tests) |
| `cargo test -p agentype-storage-sqlite --all-features --test m4_kernel schema_v7_database_is_rejected_after_b4_tables` | passed (a v7 fixture is rejected by the schema-version gate) |
| `cargo test -p agentype-storage-sqlite --all-features --test b3_typed_admission bounded_typed_enumeration_skips_a_large_blocked_head` | passed (30 blocked high-priority Tasks, bounded page still finds the ready ones) |
| `cargo test -p agentype-storage-sqlite --all-features --test b3_typed_admission large_cooldown_set_cannot_exhaust_sqlite_parameters` | passed (40,000 cooling ids exceed this build's variable limit yet the query stays bounded) |

The local Windows run needed `C:\ProgramData\agentype` to be a junction to a
workspace directory because the sandboxed test process cannot create the
production machine-lock directory. This is a local harness constraint; no
production code was changed for it, and CI does not use that workaround.

Remote CI is the acceptance gate.

```text
push @ ec093753f30e81771d36fe555fbf1c509a6c77ee
    https://github.com/Mr-enthalpy/agentype/actions/runs/42402482506  PASS
pull_request @ ec093753f30e81771d36fe555fbf1c509a6c77ee
    https://github.com/Mr-enthalpy/agentype/actions/runs/42402487742  PASS
push @ 58087836ea690c010bc2d7152b646a16ca1066be (main, PR #24 squash merge)
    https://github.com/Mr-enthalpy/agentype/actions/runs/42404395365  PASS 6/6
```

This closure PR changes documentation and adds one schema-gate test only; it
must pass its own push and pull-request CI before merge. A green run on
`5808783` is **not** cited as proof for a later closure HEAD.

## Freeze gate resolution

ADR-0012 Decision 5 requires an independent re-audit at an exact final
implementation SHA with green Linux and Windows CI.

| Gate | Evidence |
|---|---|
| Independent audit | Fifth audit returned `CLOSED WITH STATED SCOPE`, no new P0/P1 |
| Audit subject | `ec093753f30e81771d36fe555fbf1c509a6c77ee` (PR #24 head), **not** a distinct audit of `5808783` |
| Merge | `58087836ea690c010bc2d7152b646a16ca1066be` (squash merge of PR #24) |
| Audited → merge delta | `git diff --stat ec09375 5808783` = one file, this report; documentation only, no implementation change |
| CI at the audited head | push `42402482506` PASS, PR `42402487742` PASS |
| CI at the merge commit | `42404395365` PASS, 6/6 jobs |
| Closure PR | documentation + one schema-gate test; requires its own green CI |

The audit evidence therefore covers the implementation content that is present
at the merge commit. It is not described as an audit executed against the merge
SHA itself, and no later closure commit inherits that evidence without the
documentation-only delta being stated.

## Baseline and evidence mapping

```text
SCHEMA_VERSION = 8
Frozen base (main before M6-B.4):  cbd5a7518395247dc7abfef996eb307e7a526f25
Audited implementation head:       ec093753f30e81771d36fe555fbf1c509a6c77ee
Merge commit (PR #24):             58087836ea690c010bc2d7152b646a16ca1066be
CI @ audited head:                 push 42402482506 / PR 42402487742   PASS
CI @ merge commit:                 42404395365                        PASS 6/6
Closure branch:                    agent/m6b4-freeze-closure
```

The third, fourth and fifth audit revisions and their findings remain recorded
in the sections above; no failed revision is removed to manufacture a pass.

## Post-merge document reconciliation

| Document | Statement at merge | Reconciliation |
|---|---|---|
| `docs/README.md` | M6-B.4 "incomplete and not frozen" because the worker data plane is deferred | Corrected: that data plane is outside ADR-0012; the milestone is FREEZE READY within the stated scope |
| This record | Verdict `NOT FROZEN` | Updated to FREEZE READY with the evidence mapping above |
| [ADR-0008](../../decisions/0008-m6b-typed-provisioning-acquisition.md) | Status: "not a milestone freeze" | Describes the ADR's own status; the milestone state is owned here |
| [ADR-0010](../../decisions/0010-m6b-selection-order-scope.md) | Equal-continuity selection fails closed | Unchanged and still governs F3 |
| [ADR-0011](../../decisions/0011-m6b-acquisition-contract-parity.md) | Status: "not a milestone freeze"; "unfrozen v8 schema" | Describes the correction's own status and the pre-freeze state; the v8 shape is recorded here |
| [ADR-0012](../../decisions/0012-m6b4-scope-typed-physical-execution-ack.md) | Freeze requires independent re-audit at an exact SHA with green CI | Condition satisfied by the mapping above; the ADR text is not rewritten |
| [spec 13](../../specs/v0.2/13-storage-and-transactions.md) | "During this unfrozen PR" | Editorial change to bring-up wording; behavior unchanged |
| [contract parity report](riir-m6b.4-contract-parity.md) | "milestone freeze remain separate gates" | True when written; the gate is now tracked here and its witnesses are unchanged |

## Remaining limits

- **B.5 credentials / security import**: out of scope; acquisition rejects
  credential-bearing configs.
- **Cold / revivable matching**: out of scope.
- **Full typed topology**: only the ADR-0009 capacity seam exists.
- **Executed-WARM replacement**: blocked without a real stop/isolate witness;
  durable fencing is not physical termination. WARM equal-continuity priority
  remains unresolved (ADR-0010).
- **v7→v8 migration**: not implemented; fresh-v8 or existing-v8 only.
- **Worker Result transport/auth/retry (F1)**: not implemented. E2E invokes the
  durable `Kernel::ack_success` command from the harness; there is no production
  worker data plane, worker authentication, delivery retry, or worker-originated
  ACK claim. See *Harness-injected ACK boundary*. Disposition: **accepted scope
  exclusion, not an M6-B.4 freeze blocker**. Owner/tracking:
  `D-WORKER-RESULT-DP` in [spec 17](../../specs/v0.2/17-deferred-open-questions.md);
  a later milestone or ADR must define the transport/auth/retry/ingress contract.
- **Execution artifact retention / GC (F2)**: per-RequestId jail roots and
  workspaces, stdout/stderr files and the recoverable locator are created on the
  start path and are retained after terminate, collect and restart. There is no
  durable retention owner, no safe-GC authority and no disk-growth control yet.
  No production deletion path exists: immediate deletion is deliberately not
  attempted because evidence, unresolved writers and restart reconciliation may
  still depend on those artifacts. Disposition: **accepted operational
  limitation, not an M6-B.4 freeze blocker**. Owner/tracking: `D-ARTIFACT-GC` in
  [spec 17](../../specs/v0.2/17-deferred-open-questions.md); until that design
  exists, long-running deployments must treat jail-root growth as an operational
  responsibility and monitor disk usage.
- **Platform coverage**: Windows enforcement is executed locally. Linux
  Landlock is implemented and compiled here but must be executed by the Linux CI
  runner; ABI 1/2 rejection and `ftruncate`/`open(O_TRUNC)` counterexamples are
  Linux-only tests. Other platforms honestly report no enforceable workspace
  mode and are ineligible.
- **Windows console dependency**: a restricted-token child requires an
  inherited console; hosts without one get a fail-closed Unavailable, not an
  unconfined launch.
- **Windows window station**: a private per-Execution desktop is created, but a
  new window station requires a privilege ordinary interactive users do not
  have (`CreateWindowStationW` → `ERROR_ACCESS_DENIED`). The adapter therefore
  fails closed on any private-desktop/ACL failure and never falls back to the
  interactive desktop. The shared station DACL is never modified, so the adapter
  owns no station ACE and has nothing to revoke after a crash or a normal
  Execution end; several Execution lifecycles were shown not to grow the ACL.
  `WRITE_RESTRICTED` denies the restricting SID every
  write-shaped station right, but read-shaped rights (`ENUMDESKTOPS`,
  `READATTRIBUTES`, `ENUMERATE`, `ACCESSGLOBALATOMS`, `EXITWINDOWS`,
  `READSCREEN`) are evaluated against WinSta0's pre-existing DACL and are not
  removed by this adapter. They are recorded by `isolation_probe`; the claim is
  workspace/desktop write confinement, not same-user read confidentiality or a
  general malicious-code sandbox.
- **Linux metadata**: Landlock does not mediate `chmod`, `utimensat`, or
  `setxattr`. The Linux claim is therefore a content-write boundary, proven by
  the two metadata counterexample tests, not filesystem immutability.
- **Windows ACL mechanism**: direct security APIs replace `icacls`, which
  cannot map an arbitrary per-execution SID (error 1332). The ACE is applied to
  the jail root and workspace, not recursively to pre-existing content.
- **Process-tree lifecycle**: `terminate_execution` acts on the proven direct
  process only. The jail does not create a Linux process group/cgroup or a
  Windows Job Object, so descendants spawned by the worker or harness can
  outlive the direct child. This is not a quiescence claim — M5 never derives
  `quiescent_confirmed` from process death — and it does not reopen writer
  authority. Process-tree lifetime management remains a follow-up capability; the
  adapter docs, ADR-0012 non-claims and spec 16's conformance list state the
  boundary explicitly.
- **Network enforcement**: the jail supports `Enabled` only; other policies are
  rejected as Unavailable rather than weakened.
- **Attempt isolation**: not provided by a per-execution workspace and never
  overclaimed; isolating Tasks remain ineligible.
