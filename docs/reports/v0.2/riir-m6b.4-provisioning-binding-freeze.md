# RIIR M6-B.4 — Provisioning Binding and Exact Launch Implementation Record

Status: Historical Report
Implementation status: M6-B.4 library slice implemented; milestone incomplete, not frozen
Date: 2026-10-11
Canonical path: `docs/reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md`
Not a specification. The retained filename does not assert milestone freeze.

## Scope and acceptance baseline

PR #24 implements the first authority-bearing typed acquisition above frozen
M4–M6-B.3 (`cbd5a7518395247dc7abfef996eb307e7a526f25`, schema v7). It introduces
schema v8 provisioning provenance, pure source preparation and a snapshot-bearing
Execution handoff. Typed daemon dispatch and production source integration remain
pending; this is a library/composition boundary, not an end-to-end production
typed dispatch claim. It is not a deployable production typed runtime or a
drop-in upgrade of existing v7 installations.
Deployment is restricted to fresh schema-v8 databases and reopening existing
v8 databases. Idempotent initialization installs additive preparation-fault
and decision guards on prior PR-era v8 stores; recovery repairs a latest
already-suspended preparation fault missing its Root decision once. Existing v7
databases are rejected; this PR offers no upgrade path.

The frozen specs govern acceptance. Interface amendments are justified in
[ADR-0008](../../decisions/0008-m6b-typed-provisioning-acquisition.md) and
[ADR-0011](../../decisions/0011-m6b-acquisition-contract-parity.md).
[ADR-0009](../../decisions/0009-m6b-topology-capacity-seam.md) records the explicit
legacy/typed capacity seam; [ADR-0010](../../decisions/0010-m6b-selection-order-scope.md)
records unresolved availability/cost ranking. This report consolidates the final
implementation and evidence; superseded audit narratives remain in Git history.

## Implemented path

| Phase | Committed facts and checks |
|---|---|
| Resolve | Validate partition target/profile; apply known hard filters before bounded, read-only source-local attestation; rank eligible candidates and capture the catalog frontier |
| Acquire | Transactionally reload and re-prove Task, actual bound AgentType, placement, source/config/policy, realized retention and exact imported safety; commit Attempt/Lease/assignment and per-Incarnation ProvisioningBinding; leave Attempt.incarnation_id unset |
| Prepare | Re-resolve the explicitly registered exact source; fence claim authority and integration protocol before/after pure preparation; treat malformed output as fatal, require the attested digest and return a secret-free descriptor |
| Commit Execution | Requalify target and current exact adapter; retain that adapter instance/deadlines; perform composition checks; atomically create Execution, Attempt→Incarnation association and immutable BindingSnapshot; carry the binding with the request |
| Physical start | The exact ExecutionAdapter consumes the frozen descriptor inside start_execution; existing M5 recovery, observation, deadlines and writer safety own its physical lifecycle |

The implemented handoff assembles the exact start request. Wiring that request
into running typed daemon dispatch is a remaining step.

## Structural guarantees

- One Core placement predicate is shared by legacy selection, B.3 matching and
  both B.4 acquisition paths. Required continuity needs a concrete matching
  workstream, including newborn agents.
- One pure isolation decision separates semantic requirement, authoritative
  target policy and imported enforceability. A target that enables isolation
  cannot launch through an incapable domain, even when Task/Type do not require it.
- Acquisition reserves provenance. Execution commitment establishes physical
  association. Pure preparation failure, cancellation, expiry or recovery before
  a new Execution cannot mark an older resident WARM host dead.
- An idle STARTING reservation with validated provenance and no Execution
  history can roll to another source/config. ACTIVE claims on its LogicalAgent,
  any Execution history on STARTING, and active physical Executions prevent that
  rollover. A durable LOST fence does not assert physical resource termination.
  WARM hosts with Execution history reject provenance replacement before a new
  claim; SQL-only fencing cannot supply missing physical cleanup evidence.
- Per-Incarnation bindings are immutable exact provenance; per-Execution
  snapshots freeze the admitted requirement and effective launch policy. Reuse
  requires exact qualification; changed provenance requires a new Incarnation
  and cannot replace an executed WARM host without physical lifecycle closure.
- Parent markers, write-once/replacement guards and coherent authoritative reads
  protect both records. Execution/recovery cannot bypass required snapshots.
- The authority transaction re-proves imported facts and the catalog frontier.
  No public control API accepts caller-supplied enforcement evidence. Default
  producer visibility is a supported-surface fence, not an authority token.
- Read-only Task/Batch/partition/backoff preflight precedes semantic matching
  and direct source enumeration. Known unreadiness spends no attest/prepare
  budget; success grants no authority and concurrent changes still reject or
  roll back in the final transaction. Known hard filters precede private I/O.
  Credentials fail closed for
  B.5; ambiguous continuity ties fail closed while availability/cost ranking is
  deferred. Corruption is fatal, never silently skipped.
- Preparation preserves integration-returned fault kinds. Explicit availability
  settles as RESOURCE_UNAVAILABLE; authority loss produces no Task NACK; storage,
  invariant, recovery-required, missing-revision and unknown faults propagate
  without writing a Task failure. A confirmed fatal category commits as an
  immutable Attempt fact and fences further claim operations. Recovery closes
  Attempt/Lease, releases assignment and atomically suspends Task/Batch with a
  preparation-cause Escalation and DECISION_REQUIRED outbox event. No worker
  Failure, automatic retry or quiescence is invented. Root explicitly chooses
  retry/cancel after repair through the existing resolution primitive. A crash
  before confirmed fault keeps ordinary orphan recovery. Failed fault writes
  cannot claim a durable fence; failed notification writes roll back suspension.
  Confirmed pure handoff and definite snapshot/settlement faults use that same
  mechanism; uncertain Execution COMMIT errors preserve M5 reconciliation.
- Malformed successful producer output and observed integration-protocol drift
  are fatal too. They preserve claim rows, fence activity and produce no worker failure, retry,
  Execution or snapshot. Grammar checks precede post-call availability checks.
- Source-returned errors lose every private diagnostic payload before control
  propagation, including nested storage/contract errors. Public Display/Debug and
  settlement use fixed categories while retaining original fault dispositions.
- Attestation has an explicit optional validated digest: None is unavailable;
  Some cannot be empty/noncanonical text. Attestation ConfigurationUnavailable
  skips only that config, consistently with candidate-local fallback; storage
  and invariant faults abort enumeration. Adding an equally continuous config
  can still block an existing WARM host before acquisition; removing the tie
  restores reuse. ADR-0010 retains this liveness limit as an open policy question.
- Production routing has no wildcard API; each source revision needs an explicit
  registration. Typed dispatch handoff keeps the same validated adapter instance
  and operation policy even if a new registry reuses its kind/key.
- Attest/prepare are read-only, have separate absolute budgets and bind the
  descriptor to an exact protocol/domain/digest. Post-commit recovery resolves
  the exact physical binding and cannot choose a different source.

The M4–M5 authority/writer-safety/recovery contracts, M6-A admission and B.1–B.3
semantic relations/catalog/matching remain the baseline. The documented V0.1
capacity seam excludes BOUND agents from legacy desired-capacity math; untyped
capacity and retirement fencing retain their behavior.

## Interface adjustment reasons

| Adjustment | Reason and compatibility boundary |
|---|---|
| Adapter workspace/network and optional provisioning-protocol import | Typed eligibility requires actual enforcement and descriptor support. Conservative defaults preserve ordinary M5 adapters and make unsupported typed acquisition ineligible |
| Pure source-local attest/prepare, separate deadlines | Opaque config interpretation belongs to its source integration. Physical creation remains inside M5 adapter start; candidate probes cannot consume the winner's preparation budget |
| Committed descriptor on ExecutionLaunchSnapshot and start constructors | The trusted constructor gains an optional descriptor to close the committed-provenance → request boundary. Typed request construction consumes committed A and rejects B/empty/legacy construction; legacy remains empty. The raw unsafe constructor is still a procedural trust contract |
| Durable preparation-fault category and ownership-only fact fence | Confirmed producer faults must survive recovery without becoming ExecutionLost. No worker FailureClass or physical lifecycle is introduced. An elapsed but still-owned ACTIVE claim may record a late fact; activity authority still requires an unexpired lease |
| Resolver requires ExecutionRegistry | Authoritative target/profile are needed before attestation, so irrelevant candidates cannot spend the shared budget or hide eligible sources |
| Shared readiness/placement/isolation decisions and read-only readiness check | Legacy and both typed authority paths share QUEUED/ACTIVE Batch/active partition/due backoff. Preflight rejects before irrelevant private I/O or producer faults, without granting authority; concurrent changes still require final recheck. Placement/isolation preserve frozen gates across matching, commitment and handoff |
| EscalationCause replaces mandatory failure_class | A new preparation domain needs Root notification/resolution without a worker FailureClass. TaskFailure keeps legacy safety rules; PreparationFault names the immutable Attempt fact. The optional failure_class() getter and storage kind re-export preserve meaningful legacy access |
| Attempt association deferred to Execution | Restore frozen M5 physical-phase assumptions and preserve earlier WARM hosts across every pre-Execution closure; settlement APIs and schema need no additional amendment |
| Source registry wildcard restricted to private unit tests | A new source revision must not inherit opaque-config interpretation without an explicit composition-root routing decision; shared integrations use multiple registrations |
| Internal PreparedTypedExecutionLaunch owns exact binding | Retain the validated instance and deadlines through dispatch, preventing stale-binding re-selection; public legacy PreparedExecutionLaunch remains unchanged |
| Executed WARM provenance replacement and common cutover fail closed | Terminal Execution rows cannot prove host cleanup. MERGE preflights READY/assigned typed hosts; commit rechecks. Prior unsafe pending moves defer at release with original host/partition/retention and a suspended agent. Same-target/unstarted movement remains valid |
| Pure typed handoff shares the durable fault fence | A confirmed compositor or definite snapshot/settlement fault is control-plane evidence, not an orphan worker failure. Existing fault/Root machinery applies before Execution; uncertain COMMIT and committed RequestId remain M5-owned |
| Attestation returns Option<MaterializationDigest> | Separate explicit unavailability from valid successful output; replace the empty-string SPI convention without changing acquisition authority |
| Sanitized integration failure categories | Private source diagnostics do not become authorized public/log output; discard payloads and preserve existing stage-specific candidate/fatal/authority semantics |
| Typed population excluded from legacy capacity | Legacy desired population cannot retire semantically bound agents; full typed topology remains deferred (ADR-0009) |

## Verification

Local checks on 2026-10-11:

| Check | Result |
|---|---|
| Storage with test-support | 314 tests and 1 doc probe passed, including B.4 56, B.3 40, catalog 32, M6-A frontier 49, M4 kernel 71 and all storage recovery/topology/supervision/outbox suites |
| Provisioning resolver unit target | 50 passed, including zero-I/O unreadiness, due-backoff existing/new counterparts, separate-controller readiness races, fatal handoff/snapshot/settlement restart, uncertain-COMMIT counterpart, failed fact persistence and actionable Root recovery and restart fault/orphan counterparts, two-config fault routing, descriptor substitution, fault-write failure and private-diagnostic sentinel/fallback matrices, WARM equal-continuity tie and restored reuse, typed attestation, producer/protocol faults and exact-instance handoff |
| Default-feature public API boundary | 35 probes passed, including wildcard-constructor and raw-string attestation compile-fail probes |
| Runtime rustdoc | 11 passed, including the actual optional validated attestation SPI signature |
| Workspace all-target compilation, default/all-feature clippy | Passed; warnings denied |
| Formatting and whitespace | cargo fmt --all --check and git diff --check passed |
| Full Windows workspace runtime gate | Local ProcessLock setup is restricted; full runtime acceptance is checked against the pushed head's remote CI |

The two Task/Batch readiness and Root-decision counterexamples retained on
`32377e1d` fail (46 pass / 2 fail), then pass with the correction. The complete
suite also verifies notice-failure rollback, cause provenance, idempotent old-v8
repair and explicit Root resolution. Focused typed WARM MERGE and fatal
handoff target-drift regressions also fail on `a12575a3`; corrected counterparts
cover shared cutover, old pending release and confirmed versus uncertain
pre-Execution outcomes. Earlier witnesses remain consolidated.
The unready-Task probe failed before preflight, with other corrections retained:
attestation ran once instead of zero. It now passes alongside ready and concurrent
change counterparts; transactional recheck remains the authority boundary.
The focused [counterexample witness](riir-m6b.4-contract-parity.md) records the
discriminating cases. Passing test counts alone are not the acceptance argument.

The four locally blocked targets are runtime unit tests, runtime process_lock,
runtime semantic_control_contract and adapter-local-process daemon_acceptance.
The prior `ecc2990` local full run reported 262 passed / 10 failed in the runtime
unit target, with failures at process-lock setup before the acquisition path.
This records the environmental limit and does not count those targets as locally
passing. Its remote CI passed; remote status for subsequent changes must be
checked against their pushed head, not that earlier green result.

## Remaining acceptance and scope

M6-B.4's library slice is implemented; the milestone remains INCOMPLETE, NOT FROZEN.
Full runtime/CI acceptance and independent
freeze review remain separate gates. Remaining implementation includes typed
ControlLoopService dispatch, a concrete production SourceConfigIntegration,
B.5 credentials/security import, cold/revivable matching, full typed topology
and v7→v8 migration. The reference local-process adapter has no provisioning
protocol and cannot launch typed descriptors.

The internal handoff now retains the exact adapter instance, but running typed
daemon wiring must actually consume it without registry re-selection and prove
start/recovery conformance. Deployment is explicitly limited to new schema-v8
databases and reopening those v8 databases; v7 upgrade compatibility requires a
verified migration. Resident WARM source replacement is currently blocked when
there is Execution history. Enabling it requires real-adapter conformance for
stopping, isolating or safely retaining old resources and an explicit lifecycle
witness; durable fencing alone is not physical termination.

WARM provenance priority among equal-continuity candidates is not implemented.
The current fail-closed tie can stall a previously reusable host; defining a
positive reuse-priority relation needs an explicit policy amendment and tests,
not implicit source/config identity ordering. Real typed daemon acceptance must
cover crash between Execution commitment and start, missing exact binding after
restart, same-key instance replacement, ambiguous writer start with expiry/retry,
and actual workspace/network/protocol enforcement. Current library tests do not
prove those production physical scenarios.
