# M6-B.4 — Frozen-contract counterexample witness

Status: Historical Report
Date: 2026-10-10
Applies to: PR #24 against frozen M4–M6-B.3
Canonical path: `docs/reports/v0.2/riir-m6b.4-contract-parity.md`
Not a specification.

## Purpose

This report records discriminating regression evidence against frozen M4–M6-B.3.
The [implementation record](riir-m6b.4-provisioning-binding-freeze.md) owns current
scope and verification. [ADR-0011](../../decisions/0011-m6b-acquisition-contract-parity.md)
owns the shared eligibility and physical-phase rationale.

## Original implementation versus corrected implementation

The five added storage tests were run with the original `515a25ca` provisioning
implementation and the new tests retained. All five failed while all 34 existing
B.4 tests passed. Restoring the correction made all 39 B.4 tests pass.

| Regression test | Frozen-contract consequence asserted |
|---|---|
| required_continuity_has_the_same_gate_for_existing_and_new_acquisition | Existing/new and B.3 placement agree; Required without a concrete workstream creates no authority, binding or newborn agent |
| target_isolation_is_reproved_before_both_acquisition_paths | Independent target/importer isolation combinations reject unenforceable target policy before authority on both paths |
| execution_cannot_freeze_target_isolation_beyond_the_committed_capability | A snapshot cannot overclaim capability; rejected Execution creation rolls back its association and snapshot |
| pre_execution_settlement_preserves_a_reused_resident_host_across_all_closures | A genuine earlier Execution and ACK produce a reusable WARM host; config failure, Task/Batch cancellation, expiry, restart, NACK and ACK before a new Execution preserve its identity/state |
| fresh_pre_execution_reservation_remains_starting_and_can_be_reused | Pure pre-Execution closure leaves a fresh STARTING reservation reusable with the same immutable provisioning identity |

These tests pair rejected scenarios with valid counterparts and check durable
consequences, including rollback, authority absence and unchanged physical host
state. They do not infer correctness from an error return alone.

## Source-private I/O and integration evidence

The resolver suite passes 34 tests. Added cases place a credential config ahead
of a valid one and install poisoned attestations for credential/wrong-kind
candidates; those integrations are never called. An isolated target rejects an
incapable domain before attestation while admitting a capable source. An actual
pure preparation failure after a prior committed resident Execution preserves
the WARM host.

Reservation and fault regressions retained against `ecc2990` reproduce the next
uncovered phase boundaries: storage reports 41 passed / 1 failed and resolver
reports 27 passed / 3 failed. The failures are source replacement after closure,
fatal integration errors being settled as availability, and authority-loss
errors producing Task failure. The availability counterpart and physical-history
guards pass on both implementations. After correction, B.4 passes all 42 tests
and resolver passes all 30.

Against audited `149edce`, the producer-boundary tests report resolver 30 passed
/ 2 failed: malformed successful output is downgraded to retryable failure, and
protocol drift is accepted. A separate resident Execution/ACK WARM replacement
counterexample also fails because SQL fencing permits provenance change without
physical lifecycle evidence. The corrected implementation passes resolver 34,
B.4 storage 43, complete storage 301 plus 1 doc probe, and production API 34
probes. These are retained witnesses, not additional audit-round acceptance rules.

| Producer/physical boundary | Authoritative consequence asserted |
|---|---|
| Successful output grammar | Malformed digest, empty and whitespace descriptors propagate fatal InvalidRef with redacted messages; explicit retry policy produces no failure/retry, Execution or snapshot |
| Protocol drift | Drift before prepare prevents the call; drift during prepare is fatal on success and availability-error returns; claim/Lease/provenance survive and no adapter starts |
| Executed WARM replacement | A real stored resident Execution/ACK produces WARM; different provenance is rejected before a claim, preserving host state and immutable binding; same-provenance reuse and pure-reservation rollover still pass |
| Exact adapter instance | Drop the original registry after commitment and install another instance at the same kind/key; the consumed handoff starts only the original instance with its retained deadlines/request identity |
| Exact source routing | A new source revision has no inherited route; shared integrations require explicit registration for both; production API cannot call the wildcard constructor |

The replacement matrix covers pure preparation failure and cancellation, a new
config on the same source and a new source, same-Task retry, immutable old
provenance, one active reservation and zero physical starts. Guard cases retain
the restriction for an ACTIVE claim with NULL Incarnation association and a
STARTING Incarnation with Execution history. Fault matrices assert exact fatal
propagation, unchanged claim/Lease and no failure rows, alongside an actual
RESOURCE_UNAVAILABLE settlement for availability errors.

Together these cases exercise placement, target/importer qualification,
reservation/Execution separation and source-private I/O ordering. Full daemon
integration, runtime/CI acceptance and milestone freeze remain the distinct
gates documented in the implementation record.
