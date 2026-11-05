# RIIR M6-B.4 — Provisioning Binding and Typed Physical Execution Record

Status: Implementation and acceptance record
Implementation status: M6-B.4 typed physical execution → durable ACK interface implemented on PR #24
Date: 2026-11-06
Canonical path: `docs/reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md`
Not a specification. The retained filename does not assert a wider freeze.

Verdict: **NOT FROZEN — revised implementation awaiting independent re-audit and
final-SHA CI**. The accepted scope is **typed physical execution → durable ACK
interface**: typed Task → B.3 matching → B.4 physical eligibility →
Attempt/Lease/ProvisioningBinding → pure prepare → Execution/BindingSnapshot →
exact adapter start → observation/recovery → durable Result ACK boundary, with a
real source integration and a real OS-enforced workspace jail. A production
worker-originated Result transport, worker authentication, delivery retry, and
network ingress are **not** part of this scope (see *Harness-injected ACK
boundary*); the suite does not claim worker-originated end-to-end ACK. B.5
credentials, cold/revivable matching, full typed topology, executed-WARM
replacement and v7→v8 migration remain out of scope and fail closed (see
*Remaining limits*).

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
`12a7c0374654a34fb0788023063d538ed4a5fbc1` and closes P1-1, P1-2, P1-3, P2-2
and P2-3 of that audit: the accepted scope is contracted to **typed physical
execution → durable ACK interface**, physical locator evidence survives the
`spawn → identity` window, Windows children keep only
`SeChangeNotifyPrivilege` and run on a per-Execution desktop, the typed daemon
restart test uses a real process crash over the same store, a mixed legacy/typed
queue test proves no starvation, and the PR description is re-synchronized with
the actual head.

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
| Enumerate | `Kernel::list_typed_dispatch_candidates_excluding` performs one authority-free read using the shared Core readiness predicate and the frozen `priority DESC, created_at ASC, id ASC` order. A runtime-local cooldown exclusion is applied to the ordered candidates **before** `probe_limit`/`take`, so cooling Tasks cannot occupy the probe window and permanently starve Task N+1; it returns no claim and mutates nothing |
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
  spawn fails closed if the privilege set cannot be proven minimal. Each
  Execution runs on a per-Execution desktop (`agentype-desktop-<request_id>`)
  whose DACL is extended with `DESKTOP_ALL_ACCESS` for that Execution's
  restricting SID; the creating user's default ACEs remain, so this is
  per-Execution desktop confinement, not a same-user security boundary. The
  current window station receives the access required for Win32 initialization.
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
  root covering write-class rights including `TRUNCATE`.
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
`<temp>/agentype-jail-exec-<request_id>/locator.json` (jail) or
`<temp>/agentype-exec-<request_id>/locator.json` (legacy local process). The
record carries `v`, `kind`, `pid`, `inst`, `request_id`, stdout/stderr paths,
optional `birth`, optional `spawn_after_ms`, plus
`identity_complete=false` and `reconnectable=false`.

- `reconcile_start` accepts the locator and, when `birth` is absent, verifies a
  non-impersonating physical witness before adopting a numeric pid. On Windows
  the witness is the per-Execution restricting SID on the process token; on
  Linux it is birth/instance-token verification. A locator cannot impersonate an
  unrelated pid.
- `observe_execution`, `interrupt_execution`, `terminate_execution`, and
  `collect_outcome` resolve a locator directly, so the process remains
  controllable after the daemon restarts without a persisted pinned handle.
- Once the birth identity is known, the locator is upgraded in place. A crash
  after spawn but before the upgrade still yields a recoverable locator, never a
  blind respawn.
- No path treats "kill was called" as quiescence. Absence of a process produces
  a terminal physical candidate; a terminal candidate without a quiescence
  witness keeps writer safety closed.

### Windows minimum privilege and private desktop (P1-3)

The Windows child token keeps only `SeChangeNotifyPrivilege`. A per-Execution
desktop is created and its DACL is extended with `DESKTOP_ALL_ACCESS` for that
Execution's restricting SID; the creating user's default ACEs remain, so this
is per-Execution desktop confinement, not a same-user security boundary. The
current window station is extended with the access required for Win32
initialization. The parent-side desktop handle is retained
until `collect_outcome` observes process exit, because closing it while the
child is still initializing user32 causes `STATUS_DLL_INIT_FAILED`. If either
the privilege reduction or the private desktop/ACL step fails, the spawn fails
closed; there is no fallback to the interactive `Default` desktop. These are
declared as workspace/desktop confinement properties, not as a general sandbox
for arbitrary malicious code.

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
| Recoverable locator after spawn | Preserved | `locator.json` written before the birth probe; `born_less_locator_is_recovered_after_the_spawn_identity_window`, `durable_locator_controls_the_process_after_adapter_restart`, impersonation negative |
| Windows minimum privilege and private desktop | Preserved | `worker_token_is_minimal_and_desktop_is_private`; only `SeChangeNotifyPrivilege` survives; `Default`/input desktop opens fail |
| Real daemon-crash restart with same physical instance | Preserved | `typed_daemon_restart_recovers_the_same_physical_instance` (separate child process aborts; parent reopens same DB) |
| Legacy and typed queues share the control loop without starvation | Preserved | `legacy_and_typed_tasks_are_both_dispatched_from_a_shared_queue` |
| Linux truncate enforcement | Preserved | Landlock ABI >= 3 gate; `legacy_landlock_abis_are_not_enforceable`; Linux `ftruncate`/`open(O_TRUNC)` denial outside the jail and truncate allowance inside a Write jail |
| Exact `(adapter_kind, adapter_binding_key)` recovery | Preserved | M5 recovery tests; retention of resolved adapter instance in typed handoff |
| M5 writer safety and absolute deadlines | Preserved | Write E2E refuses ACK without quiescence; runtime deadline/writer suites |
| Process lock, ReadyPermit, recover-before-dispatch | Preserved | Daemon startup unchanged; full process-lock/supervision suites |
| WARM replacement blocked without physical witness | Preserved | B.4 `warm_resident_provenance_change_requires_physical_lifecycle_evidence`; no new replacement path |
| v7→v8 migration out of scope | Preserved | schema remains fresh-v8 / existing-v8 only |
| B.5 credentials / cold/revivable / typed topology fail closed | Preserved | credential-ref acquisition rejection; no new code path |

## E2E trace (real run, Windows)

Command:

```text
cargo test -p agentype-adapter-local-process --test typed_e2e \
  typed_read_only_task_runs_end_to_end_and_acks_through_the_daemon \
  -- --test-threads=1 --nocapture
```

Captured:

```text
typed-e2e trace: task=task_4437df37a9994db6b05f33389be0786b
  execution=exec_6f252b30b8ca468c9dad77ad6610d6c8
  request=request_53696691a4e647a597f3259309e3e99d
  adapter=jailed_process/win:UFO:79922a1d-4f10-11f0-b487-b0f7f68c8b54
  protocol=agentype.workspace-jail/v1
  jail=...\jail-base\agentype-jail-request_53696691a4e647a597f3259309e3e99d
  agent=agent_02c38861449d4209ac0e97e30758858e
typed-e2e ack: task=Completed result=result_7ce9214bc9a24769b2120db395535e3b physical_state=Succeeded
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
| Locator impersonation | `born_less_locator_cannot_impersonate_an_unrelated_pid` (restricting-SID witness) |
| Missing locator/handle after restart | `reconcile_without_any_physical_evidence_is_ambiguous_not_respawned` |
| Unknown Execution COMMIT | `uncertain COMMIT` counterparts in the resolver suite; `kernel_faults_are_never_classified_as_authority` |
| Ambiguous process start | `dispatch_ambiguous_start_is_persisted_and_never_restarted`; `dispatch_ambiguous_write_start_never_gains_quiescence` |
| Missing exact binding after restart | `dispatch_missing_adapter_creates_no_writer_ambiguity`; `physical_composition_never_falls_back_to_another_installed_adapter`; E2E missing-alias case |
| Same-key adapter replacement | `recovery_follows_persisted_isolation_fact_despite_registry_reconfiguration`; `stale_safety_proof_cannot_authorize_later_attempt_after_registry_reconfiguration`; B.4 `execution_cannot_freeze_target_isolation_beyond_the_committed_capability` |
| Expired lease with possible writer | `supervision_loss_then_expiry_suspends_unisolated_writer`; `unisolated_writer_expiry_without_quiescence_suspends`; Write E2E |
| Fatal prepare escalation | `confirmed_fatal_preparation_survives_restart_without_execution_lost_retry`; `preparation_fault_is_immutable_and_fences_execution_and_worker_settlement`; fatal-prep E2E |
| Root retry / cancel races | `retry_wait_is_promoted_then_redispatched`; `cancel_task_releases_a_prepared_claim`; `cancel_batch_releases_a_prepared_claim` |
| Typed unavailable / ambiguous before Attempt | `target_isolation_filters_incapable_source_before_attest_and_selection`; `credential_config_is_filtered_before_attestation_and_cannot_hide_valid_config`; E2E negative cases |
| Write success without quiescence | `writer_safety_suspension_is_distinct_from_task_completed`; Write E2E (`ack_success` returns `None`) |

## Verification

Local checks on 2026-11-06:

| Check | Result |
|---|---|
| `cargo fmt --all --check` | passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | passed |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary` | passed |
| `cargo test -p agentype-public-api-boundary` | 35 passed |
| `cargo test -p agentype-adapter-local-process --test jail_enforcement -- --test-threads=1` | 16 passed (real OS enforcement, including token/desktop and locator recovery) |
| `cargo test -p agentype-adapter-local-process --test typed_e2e -- --test-threads=1` | 10 passed (including real daemon restart and mixed legacy/typed queue) |

The local Windows run needed `C:\ProgramData\agentype` to be a junction to a
workspace directory because the sandboxed test process cannot create the
production machine-lock directory. This is a local harness constraint; no
production code was changed for it, and CI does not use that workaround.

Remote CI is the acceptance gate. The final PR head check run (fmt, clippy,
workspace tests, public-API boundary) is recorded on PR #24; the head SHA is
reported with the completion summary so the evidence is tied to the exact
tested commit. If remote CI fails, this record is not a freeze and the verdict
reverts to NOT FROZEN.

## Remaining limits

- **B.5 credentials / security import**: out of scope; acquisition rejects
  credential-bearing configs.
- **Cold / revivable matching**: out of scope.
- **Full typed topology**: only the ADR-0009 capacity seam exists.
- **Executed-WARM replacement**: blocked without a real stop/isolate witness;
  durable fencing is not physical termination. WARM equal-continuity priority
  remains unresolved (ADR-0010).
- **v7→v8 migration**: not implemented; fresh-v8 or existing-v8 only.
- **Worker Result transport/auth/retry**: not implemented. E2E invokes the
  durable `Kernel::ack_success` command from the harness; there is no production
  worker data plane, worker authentication, delivery retry, or worker-originated
  ACK claim. See *Harness-injected ACK boundary*.
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
  interactive desktop. The result is workspace/desktop confinement, not a
  general malicious-code sandbox or a claim of resistance to every adversarial
  window-message attack.
- **Windows ACL mechanism**: direct security APIs replace `icacls`, which
  cannot map an arbitrary per-execution SID (error 1332). The ACE is applied to
  the jail root and workspace, not recursively to pre-existing content.
- **Network enforcement**: the jail supports `Enabled` only; other policies are
  rejected as Unavailable rather than weakened.
- **Attempt isolation**: not provided by a per-execution workspace and never
  overclaimed; isolating Tasks remain ineligible.
