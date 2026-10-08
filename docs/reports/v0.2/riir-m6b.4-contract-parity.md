# M6-B.4 — Frozen-contract counterexample witness

Status: Historical Report
Date: 2026-10-08
Applies to: PR #24 correction after `515a25ca`
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

The resolver suite passes 26 tests. Added cases place a credential config ahead
of a valid one and install poisoned attestations for credential/wrong-kind
candidates; those integrations are never called. An isolated target rejects an
incapable domain before attestation while admitting a capable source. An actual
pure preparation failure after a prior committed resident Execution preserves
the WARM host.

Together these cases exercise placement, target/importer qualification,
reservation/Execution separation and source-private I/O ordering. Full daemon
integration, runtime/CI acceptance and milestone freeze remain the distinct
gates documented in the implementation record.
