# M6-B.4 — Frozen-contract counterexample witness

Status: Historical Report
Date: 2026-10-11
Applies to: PR #24 against frozen M4–M6-B.3
Canonical path: `docs/reports/v0.2/riir-m6b.4-contract-parity.md`
Not a specification.

## Current acceptance evidence

The [implementation record](riir-m6b.4-provisioning-binding-freeze.md) owns scope
and validation. [ADR-0011](../../decisions/0011-m6b-acquisition-contract-parity.md)
explains shared eligibility, phase boundaries and interface reasons. This is one
matrix of final observable behavior; earlier audit narratives are not competing
acceptance rules. Resolver passes 50 tests; B.4 storage passes 56; default API
passes 35 probes and runtime rustdoc passes 11.

| Boundary | Authoritative consequence and valid counterpart |
|---|---|
| Task/Batch readiness | QUEUED sibling in SUSPENDED Batch creates no Attempt/Lease or newborn side rows on either typed path; explicit Root resolution restores ACTIVE and permits the same Task |
| Readiness before source I/O | Task state, Batch suspension, inactive partition and future backoff reject before any attest/prepare call, including direct enumeration and a fatal producer probe; due-backoff existing/new counterparts acquire. Separate controller changes during attestation still reject/roll back with no prepare, authority or newborn rows |
| Root decision transaction | Fatal recovery atomically suspends Task/Batch with one preparation-cause Escalation/event; outbox failure rolls back closure/release/suspension; repeat recovery and prior-v8 repair are idempotent, explicit retry/cancel works, and resolved causes never reopen |
| Required continuity | B.3/existing/new placement agree; absent concrete matching workstream creates no authority, binding or newborn agent; concrete matching continuity succeeds |
| Target/importer isolation | Independent target/importer combinations reject unenforceable target policy before authority on both paths; imported capability alone does not enable policy |
| Snapshot commitment | An isolation overclaim rolls back Execution, association and snapshot; coherent exact binding commits atomically |
| Pre-Execution closure | A stored resident Execution/ACK produces WARM; config failure, Task/Batch cancellation, expiry, restart, NACK and ACK before another Execution preserve its identity/state |
| Pure reservation reuse/rollover | STARTING with validated binding and no Execution history reuses its source, or changes source/config after pure failure/cancellation and same-Task retry; old provenance stays immutable, one reservation remains active, no adapter starts |
| Physical-history guard | ACTIVE agent claims with NULL association and STARTING with any Execution history cannot be replaced; executed WARM provenance change fails before a new claim and preserves the host |
| Common typed-host cutover | READY/assigned WARM cross-target MERGE rolls back Task/topology changes, including redirected pending destinations; old pending release after failure/cancel/recovery preserves host and suspends pending agent; same-target/unstarted movement succeeds; missing child cannot select legacy behavior |
| Known ineligibility before I/O | Poisoned credential/wrong-kind attestations are never invoked; a later eligible candidate remains usable while corruption on an actually performed read is fatal |
| Integration fault dispositions | Availability settles RESOURCE_UNAVAILABLE; authority loss remains recovery-owned; storage/invariant/recovery-required/missing-revision/unknown faults retain claim rows plus a durable immutable fault fence and write no worker failure |
| Confirmed fault across recovery | Reopen after fatal output/protocol/storage/invariant observation: suspend with one actionable Root decision/event, without ExecutionLost, worker failure, new Attempt or physical obligation; reopen before prepare: ordinary ExecutionLost retry succeeds. Failed fault persistence propagates without Task NACK |
| Pure handoff / COMMIT boundary | Confirmed plan/candidate/snapshot/settlement faults become durable Root decisions with no ExecutionLost retry; failed fact writes remain fatal. A real committed Execution/RequestId survives simulated unknown-result errors without a pre-Execution fact; authority loss/availability retain their outcomes |
| Fault authority and v8 reopening | Exact identity/epoch ownership, immutable/replace/delete guards, elapsed-but-owned fact recording, blocked worker settlement/Execution and explicit cancellation; prior PR-era v8 initializes additive guards, and executed WARM remains unchanged |
| Attestation fallback scope | Config A ConfigurationUnavailable / B valid chooses and commits B; A StorageFailure/InvariantViolation aborts before B and leaves no claim |
| Committed descriptor | Snapshot A rejects explicit B, empty/whitespace and legacy typed construction; exact A and normal legacy requests succeed |
| Private diagnostics | Attest/prepare sentinel matrices, including nested storage/contract faults, expose no private text in public Display/Debug or any durable text/blob cell; stage-specific candidate/global/fatal semantics remain unchanged |
| Attestation SPI | None explicitly means unavailable; Some is a validated canonical MaterializationDigest; runtime rustdoc checks the actual signature and default API rejects successful raw-string output |
| Preparation output/protocol | Malformed digest and empty/whitespace descriptor are fatal, with no automatic retry, Execution or snapshot; protocol drift before prepare prevents the call, and drift during success/availability-error returns is fatal |
| Exact adapter instance | Drop the original registry after commitment and install another instance at the same kind/key; consuming the handoff starts only the original captured instance with its retained deadline/request identity |
| Exact source routing | A new source revision has no inherited route; shared integrations need explicit registrations; default production API cannot call the wildcard helper |
| WARM equal-continuity tie | A is reusable before equally continuous B is published; then B.3 still returns the existing agent but acquisition leaves the Task QUEUED with no new authority or host replacement; disabling B restores A reuse |

The last row proves an accepted liveness limitation, not a completed reuse-priority
policy. [ADR-0010](../../decisions/0010-m6b-selection-order-scope.md) explicitly
leaves that selection relation unresolved. Assertions cover durable rows,
rollback, unchanged host identity/state, absence of automatic retry and adapter
call counts rather than merely returned errors.

## Discriminating code baselines

The regression tests were retained while running the relevant prior code:

| Code under test | Counterexample result |
|---|---|
| Original provisioning `515a25ca` | Five placement/isolation/association regressions fail while its 34 existing B.4 tests pass |
| `ecc2990` | Storage 41 pass / 1 fail and resolver 27 pass / 3 fail: closed reservation cannot change source; fatal and authority-loss errors produce Task failures |
| `149edce` | Resolver 30 pass / 2 fail: malformed successful output is downgraded and protocol drift is accepted; the separate executed-WARM replacement counterexample also fails |
| Audited `bd7019c` | Both retained private-diagnostic sentinel tests fail at the outward-error boundary; the WARM tie limitation is reproduced without changing its policy |
| Audited `2ab94c0` | Retained restart-fatal, two-config unavailable and descriptor-substitution counterexamples fail; ordinary pre-prepare orphan retry passes |
| Audited `32377e1d` | Retained Batch-readiness and missing-Root-decision counterexamples fail (46 pass / 2 fail) |
| Audited `a12575a3` | Focused typed WARM MERGE and target-drift fatal handoff counterexamples each fail before correction |
| Before readiness preflight, with cutover/handoff corrections retained | Focused unready-Task probe fails: attestation is called once instead of zero |
| Corrected library slice | Resolver 50 pass; B.4 storage 56 pass; full storage 314 plus 1 doc probe; default API 35 probes; runtime rustdoc 11 pass |

Complete storage verification retains frozen M4 writer-safety/recovery, M6-A
frontier and B.3 admission/matching. Default API probes supplement transactional
re-proof; feature visibility is not an authority token. Local full Windows
runtime remains limited by ProcessLock setup, so full remote acceptance must be
checked for the exact pushed head.

These witnesses establish the library boundaries above. Actual typed daemon
start/recovery, real source lifecycle and security conformance, v7 migration and
milestone freeze remain separate gates. Stored WARM state and fake-adapter calls
are not claims of production physical cleanup or enforcement.
