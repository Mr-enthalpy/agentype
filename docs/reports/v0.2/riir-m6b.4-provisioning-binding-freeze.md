# RIIR M6-B.4 — Provisioning Binding and End-to-End Typed Execution Record

Status: Implementation and acceptance record
Implementation status: M6-B.4 end-to-end typed execution loop implemented on PR #24
Date: 2026-11-06
Canonical path: `docs/reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md`
Not a specification. The retained filename does not assert a wider freeze.

Verdict: **CLOSED WITH STATED SCOPE**. The scope is the typed Task → B.3
matching → B.4 physical eligibility → Attempt/Lease/ProvisioningBinding → pure
prepare → Execution/BindingSnapshot → exact adapter start → observation/recovery
→ Worker Result ACK path, with a real source integration and a real OS-enforced
workspace jail. B.5 credentials, cold/revivable matching, full typed topology,
executed-WARM replacement and v7→v8 migration remain out of scope and fail
closed (see *Remaining limits*).

## Scope and acceptance baseline

PR #24 implements the authority-bearing typed acquisition above the frozen
baselines M4, M5, M6-A, M6-B.1/B.2/B.3
(`cbd5a7518395247dc7abfef996eb307e7a526f25`, schema v7). This revision starts
from the previous PR head `c8ed98f78ec599976903eae1e141849cfb361c52` and closes
the three gaps recorded there:

1. production `ControlLoopService` typed dispatch (not a test-only helper),
2. a concrete `SourceConfigIntegration` (`agentype-source-workspace-jail`),
3. a provisioning-capable real adapter with real OS enforcement
   (`JailedProcessAgentAdapter`).

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
| Enumerate | `Kernel::list_typed_dispatch_candidates` performs one authority-free read using the shared Core readiness predicate and the frozen `priority DESC, created_at ASC, id ASC` order; it returns no claim and mutates nothing |
| Resolve | Validate partition target/profile; apply known hard filters before bounded, read-only source-local attestation; rank eligible candidates and capture the catalog frontier |
| Acquire | Transactionally reload and re-prove Task, actual bound AgentType, placement, source/config/policy, realized retention and exact imported safety; commit Attempt/Lease/assignment and per-Incarnation ProvisioningBinding; leave `Attempt.incarnation_id` unset |
| Prepare | Re-resolve the explicitly registered exact source; fence claim authority and integration protocol before/after pure preparation; require the attested digest; return a secret-free descriptor |
| Commit Execution | Requalify target and current exact adapter; retain that adapter instance/deadlines; atomically create Execution, Attempt→Incarnation association and immutable BindingSnapshot |
| Physical start | `Dispatcher::start_prepared` calls the retained adapter instance's `start_execution` while still holding the `DispatchGate` permit; the adapter creates the per-execution jail under the committed descriptor and OS-enforces the M5 workspace mode |
| Observe / recover | Existing M5 observation, deadline, writer-safety and exact `(adapter_kind, adapter_binding_key)` recovery own the physical lifecycle; no adapter re-selection occurs after commitment |
| Result ACK | Worker Result ACK goes through the durable Kernel authority; a late physical end is history only |

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

- Windows: `CreateRestrictedToken` with `WRITE_RESTRICTED` and restricting SID
  `S-1-5-12`, then `CreateProcessAsUserW`. `Write` grants
  `*S-1-5-12:(OI)(CI)(M)` on the per-execution jail root only; `ReadOnly` grants
  no such ACE. A restricted child needs an inherited console, so the adapter
  fails closed as Unavailable when the host has none (rather than spawning a
  process that dies with `STATUS_DLL_INIT_FAILED`).
- Linux: Landlock in `pre_exec` with `PR_SET_NO_NEW_PRIVS`. `ReadOnly` installs
  no path rule; `Write` installs one path-beneath rule for the per-execution
  jail root.
- Other hosts: the enforceable-workspace set is empty, so B.4 rejects the
  adapter; `start_execution` also fails closed.

The source-config grammar is deliberately secret-free and carries no
environment, credential, network, or workspace-mode field:
`{format, command, args, jail_root}`. Core never interprets the body; the
adapter enforces the mode/network of the M5 start request. The attested digest
binds the canonical descriptor to the exact
`(adapter_kind, adapter_binding_key)`, so a descriptor cannot be replayed
against another physical installation.

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
| Bounded re-probe fairness | Preserved | `TypedDispatchState` cooldown per Task, invalidated on catalog-frontier change; typed/legacy alternation in `ControlLoopService` |
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
- the ACK occurs while the Execution is still `Running`, produces a Result and
  moves the Task to `Completed`;
- the subsequent physical end becomes `Succeeded`/terminal history and does not
  replace the Worker Result.

The full 7-test E2E suite also proves the newborn-agent path, a real in-jail
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
| `cargo test -p agentype-adapter-local-process --test jail_enforcement` | 6 passed (real OS enforcement) |
| `cargo test -p agentype-adapter-local-process --test typed_e2e -- --test-threads=1` | 7 passed |

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
- **Platform coverage**: Windows enforcement is executed locally; Linux
  Landlock is implemented but only exercised by remote CI on a Linux runner.
  Other platforms honestly report no enforceable workspace mode and are
  ineligible.
- **Windows console dependency**: a restricted-token child requires an
  inherited console; hosts without one get a fail-closed Unavailable, not an
  unconfined launch.
- **Network enforcement**: the jail supports `Enabled` only; other policies are
  rejected as Unavailable rather than weakened.
- **Attempt isolation**: not provided by a per-execution workspace and never
  overclaimed; isolating Tasks remain ineligible.
