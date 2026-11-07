# ADR-0015 — M6-B.5 credential observations, feature gates, and the scoped freeze

Status: Proposed (M6-B.5 / B5.0 normative design; pending maintainer audit; not a milestone freeze)
Date: 2026-11-08
Canonical path: `docs/decisions/0015-m6b5-credentials-feature-gates-scope-freeze.md`

## Context

M6-B.4 rejects every credential-bearing SourceConfig. It has no trusted,
source-bound availability proof and no delivery mechanism, and it correctly
refuses to accept a caller-supplied `available=true`. [ADR-0012](0012-m6b4-scope-typed-physical-execution-ack.md)
leaves credentials for B.5.

Two facts shape the design:

1. Credential availability is externally mutable. It is not deterministic like
   catalog selection, and it cannot be made atomic with a SQLite commit.
2. Secret delivery is the largest physical risk in B.5. A mechanism that works
   on one platform and one source is not automatically a cross-platform
   contract.

B5.0 therefore freezes the qualification boundary, the failure taxonomy, the
feature gates, and the freeze scope before implementation. It does not claim a
production secret-delivery mechanism.

## Decision

### 1. Three credential objects

| Object | Meaning | Durable? |
|---|---|---|
| `CredentialEligibilityEvidence` | non-secret, non-authorizing observation that a credential is currently eligible for an exact subject | transient; its non-secret identity fields are carried into the Execution snapshot |
| `CredentialAccessPlan` | secret-free use plan committed with the Execution security snapshot | yes |
| `CredentialDeliverySession` | short-lived, execution-bound delivery object created after commitment | no, never in SQLite or the public control API |

The eligibility subject binds broker identity/registration epoch, exact
source/config/domain, the credential-reference set digest, credential version
identifiers, requested scope digest, delivery mode, and observation validity. At
this stage there is no ExecutionId, so no Execution-bound grant may be invented.

`CredentialEligibilityEvidence` is a transient observation, not a durable
standalone row. At commitment, the selected proof's non-secret identity is copied
into the `CredentialAccessPlan` and therefore into the
`ExecutionSecuritySnapshot`: proof identity, broker identity plus observation
epoch, issuer/protocol version, validity/expiry bound, subject digest, and
requested-scope digest. Core never persists the broker's raw response. A field
not carried into that snapshot has no durable meaning and MUST NOT be cited as
durable evidence.

The access plan binds broker identity, credential-reference digest, pinned
version identifiers, allowed use/audience/delivery mode, effective security
digest, and Execution/Request association. It contains no secret value and no
usable bearer token.

A delivery session targets the exact execution request and the proven recipient
environment, never merely "this PID" or "this SourceConfig".

### 2. Broker interface

```text
probe_availability(subject, deadline)
    → eligibility evidence | unavailable

authorize_and_deliver(committed_launch_authority, access_plan,
                      proven_recipient, deadline)
    → delivery session / receipt

reconcile_delivery(request_identity, deadline)
    → observed delivery state

revoke_or_close(delivery_identity, deadline)
    → closure observation
```

`probe_availability` is read-only. The other three belong to post-commit
physical start and recovery.

Broker routes bind the exact SpawnSource revision; a new source revision does
not automatically inherit an old route. Registration/installation belongs to
provisioning administration, not the Root semantic API.

### 3. One observation epoch per acquisition, with an honest scope

Each acquisition creates one logical observation epoch. Probe results are
cached by broker subject within that round, so candidates do not see different
worlds inside one selection.

This is only a Scheduler observation-consistency boundary. It is **not** a claim
that multiple independent brokers share one atomic snapshot. Every proof carries
its own issuer version, validity period, and exact subject.

Failure taxonomy:

| Result | Disposition |
|---|---|
| `Available` | participates in selection within its valid subject/version/freshness |
| `Absent` / confirmed no authorization | exclude that candidate |
| `Unreachable` / timeout | control-plane indeterminacy; MUST NOT be treated as `Absent` |
| `ProtocolFault` / invalid proof | abort resolution and escalate |
| `StaleObservation` | re-acquire the whole round or defer; never commit a stale fact |

An unreachable broker must not be allowed to push credential-bearing candidates
out of the set by pretending they are absent.

### 4. Unknown-broker candidates are judged by continuity dominance, not enumeration position

Selection in B.4 is a continuity comparison, not a priority order over
enumeration positions. When the highest continuity tier contains more than one
eligible candidate, the result is `SelectionAmbiguous`, not the first one
listed. An unknown candidate that appears later in enumeration but has the same
continuity as the current winner can therefore still change the outcome.

The rule is:

- an unknown candidate may be ignored only when trusted static eligibility and
  its continuity upper bound prove that its resolution cannot change the
  selected result;
- otherwise the whole round defers;
- enumeration position is not evidence of preference and MUST NOT be used as a
  tie-break;
- retries are bounded, with backoff and diagnostics.

For example, an unknown candidate already known to have strictly lower
continuity than a confirmed winner can be proven non-decisive once all other
qualifications are complete. An unknown candidate in the winner's continuity
tier is decisive regardless of where it appears in the enumeration.

This deliberately trades availability for fail-closed behavior and preserves
[ADR-0010](0010-m6b-selection-order-scope.md).

### 5. Transaction validation and post-commit authorization

The authority transaction validates only the subject, identity, version, and
freshness of proofs that were already obtained. External broker state cannot be
made atomic with SQLite, so a final delivery-authorization gate remains after
commit.

A `CredentialRef`, its requested use, and its requested scope MUST already belong
to the secret scope of `P_eff` and stay within the exact Source ceiling. Listing
a reference in `SourceConfig` is a request, not an authorization. A requested
scope outside `P_eff` or outside the source ceiling makes the candidate
ineligible; it MUST NOT be silently truncated or widened.

If the winner's eligibility becomes invalid before commit, the default is defer,
not reselection. Only a new complete observation epoch may select again, with a
bounded retry/backoff limit.

### 6. Delivery-mode requirement: v1 is `BrokeredAccess` only

The two delivery modes are:

- `BrokeredAccess`: the raw secret stays in the source-local broker; the worker
  receives a constrained access capability.
- `TransientRawDelivery`: the raw value is handed to an explicitly trusted
  receiving environment; the security claim excludes preventing that recipient
  from leaking it.

`TransientRawDelivery` MUST NOT be an implicit fallback for `BrokeredAccess`.

B.5 v1 production enables only `BrokeredAccess`. The
`TransientRawDelivery` interface is defined but marked unsupported and is never
selected. The credential eligibility evidence includes the delivery-mode
requirement in its subject, so a raw-capable source cannot satisfy a
brokered-only policy.

If a future version enables transient raw delivery, delivery mode must become an
AgentType `SECURITY_V1` ceiling, not a `TaskSpec` field, and a v1 Task cannot
independently forbid raw delivery. That change needs its own decision.

### 7. Secret protection means exactly what it says

Agentype's own control path MUST NOT place secret values in the catalog,
`BindingSnapshot`, the Execution security snapshot, the launch descriptor,
command-line arguments, control errors, or ordinary control logs. Ambient daemon
environment inheritance is forbidden.

Once a raw secret is delivered to a worker, Agentype cannot guarantee that the
worker never copies, prints, or exfiltrates it. That is why `TransientRawDelivery`
carries the explicit limitation above and is not the v1 production mode.

A source may use a private pipe, a restricted handle, or its own remote session
mechanism, but the audience and exact Execution binding must be enforced and
tested. Broker-private error text MUST NOT enter `RuntimeError`; control errors
keep only fixed classifications. Trust in the raw recipient is a deployment and
acceptance condition, not a string-redaction afterthought.

### 8. Revocation and versioning are distinct events

At least these are different:

```text
availability observation expires
launch authorization expires
credential material expires
broker denies future use
already-delivered raw secret remains in recipient memory
```

`BrokeredAccess` can stop future access at the broker. `TransientRawDelivery`
cannot recall bytes already delivered. Scheduler Lease expiry and an external
service rejecting an old credential have no natural atomicity; a contract that
needs strong revocation must require a broker or external service that actually
enforces epoch/scope checks. Without that ability the combination is
unsupported.

A committed Execution pins the credential reference, verifiable version
identifier, and authorized scope. A version change or revocation between probe
and delivery MUST NOT silently switch to "latest"; it returns unavailability or
start failure and is handled by the existing flow. A new Execution re-resolves
the current version and re-qualifies. Automatic hot refresh across versions is
not part of v1. A source that mints short-lived credentials at authorization
time needs an explicit issuer contract (issuer, audience, scope, maximum
validity), not a fictional static secret version.

### 9. Three independent imported capability gates

```text
security_execution_v1.policy
security_execution_v1.credential_brokered
security_execution_v1.credential_transient_raw
```

These are not operator booleans. Each is a trusted, versioned imported
capability on the exact binding, default-off, with independent positive and
negative tests. `credential_brokered` and `credential_transient_raw` depend on
the complete policy commit/start/receipt/recovery protocol.

| Support | Allowed behavior |
|---|---|
| no `policy` | `SECURITY_V1` Task is ineligible |
| `policy` only | credential-free `SECURITY_V1` may execute within `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` |
| `policy` + `credential_brokered` | credential tasks satisfying the brokered contract may execute |
| `policy` + `credential_transient_raw` | raw delivery only where explicitly allowed (not enabled in v1 production) |
| config has credential refs but no delivery capability | B.4 fail-closed behavior; no Attempt is created |

A gate does not mean every policy dimension is executable; the
`EnforcementSupport` matrix still applies, as fixed in
[ADR-0014](0014-m6b5-enforcement-compilation-receipt.md).

### 10. Delivery idempotency and crash behavior

The delivery identity is:

```text
ExecutionId
+ RequestId
+ credential access plan digest
+ proven recipient identity
```

A repeated call MUST NOT hand the same authorization to another process
instance. B.5 does not claim exactly-once delivery. If delivery may have
happened but the receipt was not persisted, the broker's reconciliation contract
decides; the runtime MUST NOT unconditionally re-issue and MUST NOT start a
second worker to escape uncertainty. A broker may keep a non-secret delivery
ledger under its own recovery responsibility; Core does not become a secret
store.

Delivery is ordered against the secured start phase. A credential-bearing
Execution MUST establish the delivery session while the physical recipient is
`HELD` and before `RELEASED`:

```text
HELD physical recipient
    → credential authorization/delivery
    → applied-security + delivery receipt durable
    → authority revalidation
    → RELEASED
```

The secret-free delivery scope in the applied-security receipt MUST describe the
same session that was established. A later asynchronous delivery with a different
scope MUST NOT be treated as covered by the earlier receipt; it needs its own
recoverable receipt chain, and the applied-security receipt cannot be retroactively
widened. A missing or mismatched delivery receipt keeps the phase at most
`HELD`/`UNKNOWN`.

### 11. Scoped freeze ladder

The freeze status ladder is:

```text
IMPLEMENTED → CLOSED WITH STATED SCOPE → FREEZE READY → FROZEN
```

`FROZEN` requires maintainer authorization at the exact final SHA, green Linux
and Windows CI, and no unresolved P0/P1 in the declared scope. A freeze
statement covers only the proved subset.

The milestone ladder is:

```text
M6-B.4
    FROZEN
    typed physical execution → durable ACK interface (ADR-0012)

M6-B.5 / policy-only
    independent restricted freeze point after its acceptance
    scope = SECURITY_V1_POLICY_ONLY_REFERENCE_V1

M6-B.5 / credential-bound
    independent restricted freeze point after its acceptance

M6-B aggregate
    frozen only after every item defined as an aggregate blocker is closed
```

If only policy-only is completed, the record MUST freeze the policy-only slice
and MUST NOT describe credential-bound acquisition as complete.

`D-TOPOLOGY` remains `BLOCKS_SEMANTIC_LAYER (remainder)` and is not silently
reclassified as an M6-B aggregate blocker. Whether full typed topology belongs
in the aggregate freeze is a maintainer decision requiring an ADR change, not an
inference from this ADR.

### 12. Registry wording

`D-SANDBOX-ORDER` and `D-SANDBOX-INTERSECTION` are recorded as "relation defined
in B.5; backend feature coverage tracked in the B.5 enforcement matrix", not as
unconditional `RESOLVED`. This prevents the decision from being read as "every
policy is enforceable on every adapter". The matrix is a B5.5 deliverable and is
not a claim about any backend at this baseline.

`D-DB-MIGRATE` records v9 as a fresh-database stage with no v8 → v9 in-place
upgrade. The receipt artifact class is added to the `D-ARTIFACT-GC` limitation.

The following remain outside B.5: `D-WORKER-RESULT-DP`, `D-ARTIFACT-GC`,
`D-SELECTION-POLICY`, full typed topology, cold/revivable agent matching,
MemoryCapsule/Transform, DB in-place migration, and a general malicious-code
sandbox.

### 13. Mandatory conformance additions

B5.5 MUST treat these as mandatory regression categories:

| Category | Assertion |
|---|---|
| Policy algebra | `meet` is the greatest lower bound; order is transitive; normalize is idempotent; digest is permutation-stable |
| Equality regression | every contract accepted by the old exact-equality relation is still accepted by the pure policy order; new physical eligibility is not thereby granted |
| Coarse projection | every `SECURITY_V1` coarse/fine conflict is rejected |
| Task-local consistency | a Task coarse/fine conflict is rejected locally even when another ceiling would hide it; `sandbox_policy = None` follows the `SECURITY_V1` rule, and validation runs before normalization |
| Imported coverage | a policy using an unsupported dimension is ineligible even when the protocol gate is open; supporting A and B does not imply supporting `A ∩ B` |
| Enforcement matrix completeness | every spec-10 dimension is classified proven/limited/unsupported for the exact binding, and an unclassified dimension fails the suite |
| Canonical golden vectors | each schema/algorithm version pins byte-level input/output and digest |
| Compile budget | exhausting the budget mid-round, or an unknown candidate not proven continuity-non-decisive, cannot produce partial selection |
| Credential probe | broker down is not absent; different epochs and wrong scope are not mixed; an unknown candidate in the winner's continuity tier defers, and only a proven non-decisive candidate may be ignored |
| Secret sentinel | no raw secret sentinel in durable JSON, logs, control errors, snapshots, or descriptors |
| Recipient identity | wrong SID/birth, wrong pinned instance, and cross-Execution delivery are rejected |
| Receipt replay | Execution A's receipt cannot confirm B; a missing receipt cannot enter RUNNING |
| Hold/release | worker code does not run before receipt durability; recovery distinguishes awaiting-release from actually started; a receipt alone does not confirm RUNNING |
| Secured start phase | `HELD`/`RELEASED`/`UNKNOWN`/`TERMINAL` transitions are explicit; `HELD`/`RELEASED` require a verified receipt, `UNKNOWN` permits a missing receipt and never enters RUNNING; release requires a Kernel authority proof for the exact RequestId and instance; PID reuse cannot authorize release; uncertain release cannot create a second worker |
| Cross-mode exclusion | a `SECURITY_V1` AgentType cannot execute a `COARSE_V1` Task; its coarse projection cannot replace the authored fine contract |
| Admission mode | the mode is explicit and durable, never inferred from policy presence; a `SECURITY_V1` Task cannot downgrade at matching, provisioning or recovery; a `COARSE_V1` AgentType cannot execute it, and a `SECURITY_V1` AgentType cannot execute a `COARSE_V1` Task |
| Stage separation | `PlacementConstraints` are never evaluated at admission; `Pending` obligations are recorded and discharged at the candidate stage |
| `R_min` typing | permission floors use `RequiredAccess ⊆ ProjectAccess`; workspace/network use `CoarseExecutionModes` predicates; guarantees and placement use `satisfies_requirement`, never set inclusion |
| Evidence durability | credential proof identity, epoch, issuer version, validity bound and subject/scope digests reach the Execution snapshot; the raw broker response never does |
| Secret scope containment | a requested credential scope outside `P_eff` or the Source ceiling is rejected, not truncated |
| Reference profile | every spec-10 dimension is classified against `SECURITY_V1_POLICY_ONLY_REFERENCE_V1`; a non-claimed dimension makes the candidate ineligible; a capability is claimed only when both platforms prove it, and B5.4 must prove the same-user receipt-directory isolation mechanism |
| Credential ordering | delivery is established while `HELD`, its receipt is durable before `RELEASED`, and a later different-scope delivery cannot retroactively satisfy the earlier receipt |
| RUNNING atomicity | observation, receipt, RUNNING, and first renewal all commit or all roll back |
| Schema gate | v8 cannot open as v9; no implicit upgrade |
| Feature gating | without a declared policy/credential capability, behavior stays at B.4 fail-closed |
| Crash recovery | an uncertain physical start keeps the original RequestId and does not create a second security fact |

Equality regression is limited to the pure policy relation. A candidate that ran
under a coarse contract but lacks a newly required enforcement proof may become
ineligible; that is a legitimate security tightening, not a regression.

## Non-claims

- No B.5 implementation, no production credential broker, and no production
  secret-delivery mechanism exists at this baseline.
- No global provider credential pool, no provider/model interpretation in Core,
  and no caller-supplied availability boolean.
- No claim that a delivered raw secret can be recalled, or that Lease expiry and
  external revocation are atomic.
- No claim that the policy-only slice completes credential-bound provisioning or
  the whole M6-B aggregate.
- No claim that `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` covers a dimension outside
  its positive list; such a dimension makes the candidate ineligible.
- No reclassification of `D-TOPOLOGY` or other deferred semantic-layer items.

## Consequences

- B.5 can freeze a policy-only slice without waiting for cross-platform secret
  delivery, but must state that boundary explicitly.
- Credential-bearing configs stay fail-closed until both their qualification and
  delivery gates are proven for the exact binding.
- Broker outage cannot be converted into candidate absence, and an unknown
  candidate not proven continuity-non-decisive forces a whole-round defer.
- The registry no longer implies that defining the relation means every backend
  implements it.

## Relationship to other decisions

- [ADR-0010](0010-m6b-selection-order-scope.md) owns the deterministic selection
  order used by the unknown-broker rule.
- [ADR-0012](0012-m6b4-scope-typed-physical-execution-ack.md) owns the frozen
  B.4 boundary and the deferred worker data plane.
- [ADR-0013](0013-m6b5-policy-authority-algebra-identity.md) owns policy
  authority and algebra.
- [ADR-0014](0014-m6b5-enforcement-compilation-receipt.md) owns enforcement
  compilation and the applied-security receipt.
- [Spec 17](../specs/v0.2/17-deferred-open-questions.md) is the registry updated
  by §12.
