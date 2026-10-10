# RIIR M6-B.4 — Provisioning Binding and Exact Launch Implementation Record

Status: Historical Report
Implementation status: M6-B.4 implemented, not frozen
Date: 2026-10-10
Canonical path: `docs/reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md`
Not a specification. The retained filename does not assert milestone freeze.

## Scope and acceptance baseline

PR #24 implements the first authority-bearing typed acquisition above frozen
M4–M6-B.3 (`cbd5a7518395247dc7abfef996eb307e7a526f25`, schema v7). It introduces
schema v8 provisioning provenance, pure source preparation and a snapshot-bearing
Execution handoff. Typed daemon dispatch and production source integration remain
pending; this is a library/composition boundary, not an end-to-end production
typed dispatch claim.
Deployment is restricted to fresh schema-v8 databases and reopening existing
v8 databases. Existing v7 databases are rejected; this PR offers no upgrade path.

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
- Known hard filters precede source-private I/O. Credentials fail closed for
  B.5; ambiguous continuity ties fail closed while availability/cost ranking is
  deferred. Corruption is fatal, never silently skipped.
- Preparation preserves integration-returned fault kinds. Explicit availability
  settles as RESOURCE_UNAVAILABLE; authority loss produces no Task NACK; storage,
  invariant, recovery-required, missing-revision and unknown faults propagate
  without writing a Task failure or closing the committed claim.
- Malformed successful producer output and observed integration-protocol drift
  are fatal too. They preserve authority and produce no failure, retry,
  Execution or snapshot. Grammar checks precede post-call availability checks.
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
| Descriptor/effective network on start request; attested digest on durable records | Persist and deliver the environment identity/policy that the exact adapter must consume; the digest describes expected identity, not prior physical creation |
| Resolver requires ExecutionRegistry | Authoritative target/profile are needed before attestation, so irrelevant candidates cannot spend the shared budget or hide eligible sources |
| Shared placement/isolation decisions | Preserve frozen gates across preselection, authority, snapshot and handoff instead of maintaining inconsistent copies |
| Attempt association deferred to Execution | Restore frozen M5 physical-phase assumptions and preserve earlier WARM hosts across every pre-Execution closure; settlement APIs and schema need no additional amendment |
| Source registry wildcard restricted to private unit tests | A new source revision must not inherit opaque-config interpretation without an explicit composition-root routing decision; shared integrations use multiple registrations |
| Internal PreparedTypedExecutionLaunch owns exact binding | Retain the validated instance and deadlines through dispatch, preventing stale-binding re-selection; public legacy PreparedExecutionLaunch remains unchanged |
| Executed WARM provenance replacement fails closed | Terminal Execution rows cannot prove cleanup of external resources; preserve the host until M5 lifecycle closure or a future verified safety witness |
| Typed population excluded from legacy capacity | Legacy desired population cannot retire semantically bound agents; full typed topology remains deferred (ADR-0009) |

## Verification

Local checks on 2026-10-10:

| Check | Result |
|---|---|
| Storage with test-support | 301 tests and 1 doc probe passed, including B.4 43, B.3 40, catalog 32, M6-A frontier 49, M4 kernel 71 and all storage recovery/topology/supervision/outbox suites |
| Provisioning resolver unit target | 34 passed, including producer-output/protocol/fault matrices, exact-instance handoff, explicit revision routing, source rollover and WARM preservation |
| Default-feature public API boundary | 34 probes passed, including the production wildcard-constructor compile-fail probe |
| Workspace all-target compilation, default/all-feature clippy | Passed; warnings denied |
| Formatting and whitespace | cargo fmt --all --check and git diff --check passed |
| Full Windows workspace runtime gate | Local ProcessLock setup is restricted; full runtime acceptance is checked against the pushed head's remote CI |

The five new B.4 storage regressions also fail against the original provisioning
implementation while its 34 existing tests pass, then pass with the correction.
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

M6-B.4 remains IMPLEMENTED, NOT FROZEN. Full runtime/CI acceptance and independent
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
