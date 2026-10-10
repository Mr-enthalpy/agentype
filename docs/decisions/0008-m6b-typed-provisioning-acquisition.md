# ADR-0008 — Typed provisioning acquisition and the evidence-authority fence

Status: Accepted (M6-B.4; not a milestone freeze)
Date: 2026-10-06
Consolidated: 2026-10-10
Canonical path: `docs/decisions/0008-m6b-typed-provisioning-acquisition.md`

## Context

Frozen M6-B.3 commits typed semantic identity and performs non-authoritative
existing-agent matching. Neither a `TaskAgentRequirement` nor a
`LogicalAgentTypeBinding` proves physical eligibility. [ADR-0007](0007-m6b-can-provision-task-staging.md)
places `can_provision_task` and the remaining physical conjuncts at the first
authority-bearing typed acquisition, implemented in M6-B.4.

The acceptance baseline is frozen M4–M6-B.3. An interface amendment must explain
its reason and preserve that baseline's authority and physical-safety contracts.
This document records the final acquisition design. Superseded audit narratives
remain in Git history; they do not define competing acceptance rules.

## Authority and qualification

The Scheduler derives a winner from validated catalog revisions and imported
physical facts. The authority transaction reloads the current Task, actual bound
AgentType, logical type binding, source/config/policy revisions and capability
catalog, then re-proves `can_execute` and `can_provision_task` before creating an
Attempt or Lease. A compatible B.3 AgentType may differ from the Task pin;
another revision of the same `type_id` remains excluded while revision
compatibility is deferred.

Legacy selection, B.3 matching and B.4 acquisition share the pure Core
`claim_placement_eligible` decision: exact partition, tag superset and Required
continuity with a concrete matching Task workstream. Both existing and newborn
acquisition satisfy the same placement predicate.

Physical target/profile come from the M5 partition or anchor. The target kind
must match the active policy's kind; `binding_ref` resolves the exact installed
`AdapterKind` + `AdapterBindingKey`. Policy `required_safety`, realized M4
retention, source/type compatibility and imported enforcement must all qualify.
Evidence is bound to that exact physical domain. Catalog declarations cannot
substitute for imported enforceability.

The resolver validates target/profile and all known hard filters before
source-private attestation: policy activity/kind, protocol support, credentials,
retention, contract compatibility and enforceability. Only otherwise eligible
revisions enter read-only I/O. The transaction independently re-proves its
mandatory predicates; preflight grants no authority.

Credential-bearing configs fail closed until B.5 supplies trusted, source-bound
availability and brokering. A caller-provided boolean is not such proof. Core
never receives a secret value; snapshots contain a secret-free credential-reference
digest.

The deterministic catalog-frontier digest fences the selected candidate set
through commitment. Candidate-local ineligibility skips that candidate;
corruption aborts resolution. Among hard-eligible candidates, continuity ranking
and unresolved ties follow [ADR-0010](0010-m6b-selection-order-scope.md).
`SelectionAmbiguous` fails closed while availability/cost ranking is deferred.
A stale frontier requires fresh resolution of the whole selection.

## Reservation, Execution and immutable provenance

Typed acquisition atomically commits Attempt/Lease/assignment and the selected
Incarnation's `ProvisioningBinding`. It leaves `Attempt.incarnation_id` unset.
Assignment and the one-active-Incarnation constraint fence concurrent acquisition.
The binding reserves exact provisioning provenance without claiming that this
Attempt has a physical Execution.

The frozen M5 Execution transaction establishes Attempt→Incarnation association.
`create_execution_with_snapshot` writes Execution and `BindingSnapshot`
atomically and proves that the selected Incarnation owns the exact reserved
binding. Incarnation/provenance drift rolls back the whole transaction. The
legacy Execution API rejects typed Tasks, so it cannot bypass the snapshot.
[ADR-0011](0011-m6b-acquisition-contract-parity.md) explains this phase boundary
and the shared eligibility decisions.

`ProvisioningBinding` is stable per-Incarnation provenance: exact AgentType,
source/config/policy revisions, physical domain, imported enforceability,
provisioning protocol and attested materialization digest. It carries no
per-Task requirement. Reuse requires exact qualification; changed provenance
requires a new Incarnation. A legacy Incarnation can be adopted only if it has
no Execution history, because legacy physical history cannot prove opaque
SourceConfig provenance.

A STARTING provisioning reservation may roll to different source/config
provenance after its claim closes. The rollover transaction must positively
validate its ProvisioningBinding, prove that it has no Execution history and
exclude any ACTIVE claim on its LogicalAgent (including one with no Incarnation
association). STARTING with Execution history and every active physical
Execution remain protected. Durable LOST is a provenance fence, not a claim of
physical resource termination.

A WARM Incarnation with any Execution history cannot change provenance merely
because all its Executions are terminal. B.4 has no durable stop/isolation/safe
retention witness, so that replacement fails closed as pre-acquisition
ConfigurationUnavailable, preserving the old host and creating no new claim.
Same-provenance reuse remains valid. Enabling replacement requires the actual
M5 lifecycle to close the host or a separately specified and conformant physical
safety witness; a SQL LOST update supplies neither.

`BindingSnapshot` is the per-Execution commitment. It records exact binding
provenance, target/profile, admitted capabilities, imported capability and
effective execution policy separately, credential-reference digest and the
prepared launch descriptor. Its effective isolation equals the frozen Execution
and satisfies the admitted requirement.

Schema v8 uses parent markers, write-once rows and replacement guards for primary
and secondary/partial unique identities. Authoritative reads re-encode canonical
documents, compare relational columns and validate cross-record coherence.
Missing or incoherent children of marked parents are corruption. Reconciliation
and restart recovery use the same validated reads. Resolver-version validation
accepts the `agentype-resolver/*` family so a producer bump does not invalidate
immutable snapshots.

## Source preparation and exact physical handoff

The composition root routes each exact `SpawnSourceRef` through a
`SourceIntegrationRegistry` to the integration that understands its config.
Unrouted sources are ineligible. Core persists and forwards opaque config
identity and descriptors without interpreting provider/model/config grammar.
Every source revision must be registered explicitly, even when several sources
share an integration. There is no production wildcard route; `uniform` is a
private unit-test fixture only. Publishing a new revision cannot implicitly
grant it access to an existing integration.

`SourceConfigIntegration::attest` and `prepare` are pure/read-only. Attestation
checks an exact validated revision and physical domain; an `ExternalRef` must be
attested, while an `OpaqueJson` body digest is Core-validated. Preparation of the
committed winner re-derives the expected digest and returns an opaque,
secret-free launch descriptor. Its canonical `sha256:<64 hex>` digest must match
the committed `attested_materialization_digest`; mismatch diagnostics are redacted.

Attestation returns `Result<Option<MaterializationDigest>, ProvisioningResolutionError>`.
`Ok(None)` explicitly means unavailable; `Ok(Some(digest))` is a validated
canonical value. A malformed digest cannot be a successful output and must
surface as a producer error. This replaces the ambiguous `Ok("")` convention;
preparation still rejects malformed successful output as fatal.

Enumeration and winner preparation receive distinct absolute deadlines. No call
renews its endpoint. The full committed-claim authority fence is checked before
and after preparation, and deadlines/digests are checked on return.

The integration's protocol must equal the exact adapter's imported provisioning
protocol at eligibility and handoff. Preparation checks the integration against
the committed protocol before and after the call, including error returns.
Observed drift is a fatal producer invariant violation, with redacted diagnostics
and no Task NACK. Integrations must keep their protocol stable throughout every
call, including concurrent calls; endpoint checks do not prove unobservable
transient drift. `import_provisioning_protocol` defaults to
`None`; an ordinary adapter remains usable for M5 but is ineligible for typed
provisioning until it consumes the descriptor. The reference local-process
adapter declares `None`.

Every fallible handoff composition check precedes Execution commitment. The
prepared descriptor then reaches the exact adapter through
`EnvironmentStartRequest::from_launch_with_descriptor`. The adapter creates the
physical environment inside `start_execution`. M5 Execution/RequestId,
`reconcile_start`, observation and termination own the complete physical
lifecycle; M6-B introduces no pre-Execution physical operation.

`TypedLaunchPlan` retains the resolved `ResolvedAdapterBinding` alongside the
validated environment. The internal `PreparedTypedExecutionLaunch` keeps that
same instance and its operation deadlines with the committed launch request;
dispatch consumes them together instead of resolving from a mutable registry
again. The public legacy `PreparedExecutionLaunch` contract is unchanged.

Pure pre-Execution configuration/liveness failures settle as
`RESOURCE_UNAVAILABLE`. Authority loss is left to recovery; corruption,
persistence and settlement faults propagate as fatal. A pre-Execution closure
preserves an older reusable WARM host and leaves a fresh STARTING reservation
reusable. After Execution commitment, frozen M5 physical presence and writer
safety govern settlement. Semantic retirement retains its own fencing.
Committed snapshots/recovery always use the exact binding; missing bindings
cannot cause source fallback.

Integration-returned errors preserve their fault kind: explicitly enumerated
availability errors may settle as RESOURCE_UNAVAILABLE; authority loss is left
to recovery; StorageFailure, InvariantViolation, RecoveryRequired, missing
revisions and all unrecognized errors propagate without writing a Task failure.
The broad prepare Result type does not imply that every error is availability.
At both integration returns, runtime discards every producer-controlled error
payload, including nested storage/contract diagnostics and identifiers. The
public `IntegrationFailure` exposes only a fixed category; private routing
metadata retains the original candidate/fatal/authority classification.
Preparation settlement and its outward error use fixed category messages,
never a raw producer `err.to_string()`. Display and Debug cannot recover the
discarded diagnostic; private logging remains the integration's responsibility.
Successful output is also a producer boundary: a malformed canonical digest or
blank descriptor propagates as fatal, preserving Attempt/Lease/provenance and
writing no failure or retry. Output grammar is checked before post-call deadline
and digest-equality availability checks. A valid digest whose content no longer
matches attestation remains a distinct availability/TOCTOU rejection.

## Capability and effective policy

One pure execution-configuration decision enforces both implications:

```text
(AgentType.requires OR Task.requires) -> target.attempt_isolation
target.attempt_isolation              -> exact adapter.enforceable_isolation
```

Enumeration, acquisition, snapshot validation and handoff reuse that decision.
Imported isolation capability does not enable the target's effective policy.
Handoff additionally rejects any weakening of the committed imported capability.
Workspace enforceability is exact-set membership; one mode implies no other
mode. Effective workspace and network policy are carried to the start request.

## Interface amendments and consequences

- `ImportableAdapter` adds workspace/network enforcement import and optional
  provisioning protocol import. Conservative defaults prevent older adapters
  from inventing enforcement or descriptor support.
- `EnvironmentStartRequest` carries the descriptor and effective network policy
  so the adapter receives the exact environment and policy that were committed.
- Resolver entry points require the execution registry to apply authoritative
  target/profile filters before attestation. Separate deadlines preserve the
  winner's preparation budget.
- Shared pure placement/isolation decisions remove duplicated eligibility rules.
  Moving Attempt association to Execution restores M5 settlement assumptions
  without changing settlement APIs or adding a physical lifecycle.
- Exact-source registration replaces the production wildcard helper to preserve
  source-local config ownership across new catalog revisions. The internal typed
  handoff gains an owned exact adapter binding to preserve the validated instance
  through dispatch; this does not widen the legacy launch API or grant new
  Scheduler authority.
- The attestation SPI uses optional validated digests instead of successful
  empty strings, making availability explicit. Source-returned errors become
  sanitized public categories while retaining stage-specific dispositions;
  arbitrary private diagnostic text never grants control-plane disclosure.
- The internal `provisioning-producer` feature narrows the default contract-crate
  surface. Cargo feature unification means it is not an unforgeable authority
  token; transactional re-proof and control APIs that accept no caller-supplied
  enforcement facts remain the authority boundary.

[ADR-0009](0009-m6b-topology-capacity-seam.md) separately documents the deliberate
V0.1 capacity amendment: only UNBOUND population counts toward legacy capacity;
BOUND agents cannot be chosen as legacy excess. The full typed topology model
remains deferred.

M6-B.4's library slice is implemented; the milestone is incomplete, not frozen.
Typed daemon dispatch, concrete production
source integration, B.5 credential/security import, cold/revivable matching,
full typed topology and database migration remain pending. The
[implementation record](../reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md)
owns scope and verification evidence.
This PR's deployment scope is explicitly fresh schema-v8 databases only, plus
reopening databases already created at v8. Existing v7 databases are rejected;
no upgrade compatibility is claimed until migration is implemented and verified.
