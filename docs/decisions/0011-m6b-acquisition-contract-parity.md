# ADR-0011 — Acquisition preserves frozen placement and physical-phase contracts

Status: Accepted (M6-B.4 correction; not a milestone freeze)
Date: 2026-10-08
Updated: 2026-10-10
Canonical path: `docs/decisions/0011-m6b-acquisition-contract-parity.md`

## Context

The independent review of PR #24 at `515a25ca` found three correctness gaps and
one candidate-enumeration gap against the frozen M4–M6-B.3 baseline. Repeated
local corrections had left two structural sources of drift: duplicated
eligibility decisions and an acquisition-time Attempt→Incarnation association
that changed the assumptions of M5 settlement without an Execution.

This decision supplies the rationale for the shared gates and phase boundary
in ADR-0008's consolidated prepare-only model. Frozen specs and observable
authority/physical-state behavior remain the acceptance oracle;
an existing interface may change with a stated reason, but its invariants must
not silently change. B.5 and daemon wiring remain outside this correction.

## Decision

### Shared eligibility, separate authority boundaries

Legacy claim selection, B.3 semantic preselection and B.4 transactional placement
use the same pure Core `claim_placement_eligible` decision. Required continuity
requires a concrete Task workstream equal to the agent's workstream. `None ==
None` is not continuity. Both existing and newborn acquisition must pass it.

Isolation qualification has two independent implications:

```text
(AgentType.requires OR Task.requires) -> ExecutionTarget.isolation
ExecutionTarget.isolation            -> exact adapter.enforceable_isolation
```

The pure execution-configuration decision is reused at candidate enumeration,
authority commitment, launch requalification and snapshot validation. Imported
capability does not turn isolation on; M4 target configuration remains the
effective execution policy. Handoff independently checks current installed
capability against the committed capability, including non-isolation dimensions.

### A provisioning reservation is not a physical execution association

Acquisition atomically creates Attempt/Lease/assignment and immutable
ProvisioningBinding, but leaves `Attempt.incarnation_id` unset. The binding
records the selected Incarnation's exact provisioning provenance. Assignment and
the frozen one-active-Incarnation rule fence concurrent acquisition.

The Attempt→Incarnation association is established with Execution creation,
as in frozen M5. `create_execution_with_snapshot` must atomically prove that
the Execution's selected Incarnation owns the exact ProvisioningBinding named
by its snapshot. If the selected Incarnation or provenance changed, the entire
transaction rolls back; it cannot silently select another binding.

This removes the erroneous early association instead of adding special cases
to NACK, cancellation, expiry, recovery and ACK. Pre-Execution closure cannot
infer physical death or quiescence for an earlier WARM host. A fresh, never
materialized STARTING reservation remains reusable. After Execution commitment,
the unchanged M5 physical presence and writer-safety rules apply. Semantic
retirement still fences an agent's reusable Incarnations in its own transaction.

Reuse and replacement obey the same phase boundary. A validated STARTING
provisioning reservation with no Execution history may be fenced and rolled to
new provenance once its claim closes. The transaction checks ACTIVE claims by
LogicalAgent, because a pre-Execution Attempt deliberately has no Incarnation
association. A STARTING Incarnation with any Execution history, or one whose
agent still has an ACTIVE claim, is not such a reservation. Active physical
Executions remain protected. This permits legal source/config replacement
without reopening a physical-start or concurrent-writer exception.

Previously executed WARM hosts are a different phase: terminal Execution rows
do not prove external-resource cleanup. Provenance replacement now fails closed
before acquisition and leaves the host WARM. This narrows the B.4 replacement
path because SQL fencing cannot satisfy M5 physical safety; same-provenance reuse
and the frozen settlement/termination APIs retain their behavior.

The association correction requires no settlement API or schema changes. There
is no new physical provisioning lifecycle and no generic `execution.is_none()`
exception to the writer-safety gate.

### Complete known hard filters before source-private I/O

Candidate enumeration validates target/profile and filters inactive policy,
wrong target kind, target/imported isolation mismatch, credential-bearing
config, realized retention and static contract/enforcement failures before
calling `attest`. These facts are already known; they must not consume the
shared deadline or introduce irrelevant integration failures.

Only an otherwise eligible revision enters bounded attestation. Unknown config
availability still requires attestation; corruption on a read that is performed
still fails closed. Filtering does not grant authority: the commitment
transaction re-loads and re-proves its mandatory conjuncts. Ranking receives a
hard-eligible set and retains ADR-0010's ambiguity behavior.

The internal resolver signature now requires the authoritative execution
registry. Its former signature could enumerate candidates without the target
policy and only filter them after source-private I/O; that is the reason for
the interface change. This grants no new public control-plane authority.

### Preserve fault kinds across the integration boundary

The prepare Result includes control-plane failures as well as source
availability failures. Runtime classifies errors by kind regardless of which
component returned them. Only explicit availability errors settle as
RESOURCE_UNAVAILABLE; authority loss remains recovery-owned. Durable/storage
faults, missing revisions and unsupported errors default to Fatal and leave the
committed claim rows intact, without a Task NACK. Before returning a confirmed
Fatal, runtime commits an immutable, payload-free preparation fault category for
that exact Attempt. This fences subsequent claim operations and survives restart.
Recovery closes Attempt/Lease, suspends the Task and releases assignment without
recording ExecutionLost, consulting retry policy, changing the reserved host or
creating worker-failure/quiescence escalation. A crash before a confirmed fault
still follows ordinary M5 orphan recovery. Fault persistence errors propagate
fatally; an unsuccessful write cannot claim a durable disposition. Suspended
faults require control-plane repair and explicit cancellation/re-admission; this
slice does not offer automatic retry or a fault-resume control API.
The existing Result signature is retained; its fault semantics are honored.

The same rule applies to successful producer output: malformed digests and blank
descriptors are fatal contract faults, not Task availability. Integration protocol
is checked against the committed binding before/after prepare, including failed
calls; observed drift is fatal. Integrity faults cannot be hidden by an
availability error or post-call deadline. Valid but changed content remains an
availability failure. Diagnostics never echo malformed producer content.

At the integration boundary, successful attestation now returns an optional
validated MaterializationDigest: None explicitly means unavailable and Some
cannot contain the former empty-string sentinel. This is a scoped SPI amendment
to separate availability from producer output validity, without changing the
authority transaction or M5 adapter interfaces. Attestation may also return
ConfigurationUnavailable as an error: it has the same candidate-local outcome
as None in enumeration and facade fallback. StorageFailure/InvariantViolation
remain global, and deadlines/authority faults retain their stage semantics.

Source-returned private diagnostics are discarded before propagation. A sanitized
IntegrationFailure exposes a fixed category and carries runtime-only routing
metadata computed from the original error, preserving candidate fallback,
authority loss and fatal settlement rules. Nested storage/contract errors lose
their private strings too; preparation settlement uses category messages instead
of rewrapping arbitrary producer text. This changes outward diagnostic payloads,
not fault dispositions or Task retry policy.

Production source routing requires an explicit registration for each exact
SpawnSourceRef; the wildcard helper is private and unit-test-only. This prevents
a new revision from gaining config interpretation without a composition-root
routing decision. Shared integration implementations remain supported through
multiple explicit registrations.

The internal typed launch result now owns the resolved adapter instance and its
deadline policy alongside the request. Dispatch consumes this pair without
registry re-selection, so keeping kind/key while swapping an instance cannot
change a previously validated launch. The public legacy launch interface is
unchanged; actual typed daemon dispatch remains pending.

ExecutionLaunchSnapshot now carries the optional committed descriptor. The
trusted unchecked constructor gains this argument because an Execution-only
capability could not constrain a later free String to immutable provenance.
Kernel returns Some only after coherent snapshot commitment and None for legacy.
EnvironmentStartRequest::from_committed_launch consumes that value; the retained
explicit-descriptor constructor accepts only exact equality, and legacy
from_launch rejects typed Executions. Legacy requests still use an empty
descriptor. The raw unsafe constructor remains a procedural trust contract,
not an access-control token.

Fault facts add a small table and guards to the unfrozen v8 schema. Idempotent
initialization installs them on prior PR-era v8 databases, without changing
existing records or admitting v7. Recording a late confirmed fault checks exact
ACTIVE claim ownership even after elapsed time; it grants no work authority.
Core validate_authority still requires an unexpired lease for execution,
renewal and settlement. Closed/replaced claims cannot acquire a fault fact.

## Acceptance and consequences

The acceptance matrix covers behavior across boundaries, not only the newest
finding's line: B.3/existing/new placement parity; target/importer isolation
combinations; reservation versus committed Execution; configuration failure,
Task/Batch cancellation, expiry, restart, physical-history-preserving NACK and
ACK; immutable binding identity; and statically ineligible candidates whose
attestation would otherwise fail the entire search.

It also covers source/config rollover after pure prepare failure and cancellation,
same-Task mechanical retry, ACTIVE claims with no Incarnation association,
STARTING with physical history, and integration-returned availability versus
authority-loss versus fatal faults. Assertions include immutable old provenance,
one active reservation, zero adapter starts, and failure-row absence for fatal
and authority-loss cases.

Producer-output and protocol-drift matrices also assert unchanged claim/Lease,
no Execution/snapshot/failure and no automatic retry. A real stored resident
Execution/ACK proves the WARM replacement guard; a dispatch-handoff test replaces
the registry with a different instance at the same kind/key and proves only the
captured instance receives start. Exact source revisions and the production
wildcard API boundary are checked separately.

Attest/prepare sentinel matrices check Display, Debug and all durable text/blob
cells while asserting availability settlement versus preserved fatal/authority
claims. A stage-specific matrix preserves distinctions such as global deadline
versus candidate-local unavailability. A real stored WARM host demonstrates that
an added equally continuous config blocks acquisition, without creating authority
or replacing the host; removing the tie restores reuse. ADR-0010 explicitly
retains that liveness limit rather than inventing a hidden preference rule.

Tests must include valid counterparts and assert authoritative consequences
(Attempt/Lease absence, atomic rollback, unchanged host identity/state), not
merely a returned error. Existing M4 writer-safety/recovery and M6-A/B.3 suites
remain required, alongside default-feature API boundary probes. Passing a new
regression suite is not proof of full daemon integration or milestone freeze.

Paired restart tests allow ExecutionLost retry and prove confirmed fatal output,
protocol drift and storage/invariant faults suspend without a worker failure,
while a crash before prepare retries normally. Guard tests cover identity
forgery, immutable facts, late elapsed leases, blocked commitment/settlement,
explicit cancellation, prior-v8 reopening and preservation of an executed WARM
host. Two-config attestation tests distinguish local unavailability from global
integrity faults. Descriptor tests reject committed A replaced by B, empty
typed requests and the legacy typed constructor, while exact A succeeds.

Evidence: [frozen-contract counterexample witness](../reports/v0.2/riir-m6b.4-contract-parity.md).
