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

For B.5 v1, one `CredentialAccessPlan` MUST be fulfilled entirely by one trusted
broker delivery domain. That domain may sub-route internally across backends, but
it is the single accountable authority for qualification, the canonical request,
delivery and recovery, so the single registration epoch, probe subject and
complete-delivery receipt have one meaning. A plan whose references require
multiple independent broker authorities is unsupported in v1: it has no single
qualification proof or recovery contract, and MUST NOT be split into an implicit
fan-out. Supporting independent brokers later requires a versioned per-broker
evidence vector and a composite delivery protocol, not a reinterpretation of this
plan.

A delivery session targets the exact execution request and the proven recipient
environment, never merely "this PID" or "this SourceConfig".

The canonical `CredentialRequest` (section 2) is an input derivation, not a
fourth credential object: it is secret-free, never authorizes anything on its own,
and only its digest is carried into the eligibility evidence and the access plan.

### 2. Broker interface

```text
probe_availability(subject, deadline)
    → eligibility evidence | unavailable

authorize_and_deliver(committed_launch_authority, access_plan,
                      proven_recipient, deadline)
    → delivery session + non-secret delivery result

reconcile_delivery(request_identity, deadline)
    → observed delivery state

revoke_or_close(delivery_identity, deadline)
    → closure observation
```

`probe_availability` is read-only. The other three belong to post-commit
physical start and recovery.

The broker never invents the request. The request is derived before the probe,
from the exact `SourceConfig` revision, by the trusted source-local resolver:

```text
SourceConfig exact revision
    → trusted source-local request resolver
    → canonical secret-free CredentialRequest
    → validate_within(P_eff)
    → broker probe
```

`CredentialRequest` is canonical and secret-free. It fixes at least the
credential-reference set, use, scope, audience, delivery mode, and a canonical
digest. It can only request; it never grants. Runtime validates the whole request
against `P_eff` (the AgentType grant and any narrowing Generation/source ceilings)
before any broker probe, so a scope cannot be learned from the broker and then
authorized. The request digest enters winner qualification and the
`CredentialAccessPlan`; a request that changes after qualification belongs to a
new observation epoch and cannot be merged into the committed plan.

`resolve_credential_request` is a pure, read-only, secret-free resolution that runs
during candidate qualification, after the exact `SourceConfig` revision is
resolved and attested, before any broker probe, and before acquisition authority
commitment. It does not participate in M6-A semantic admission: at semantic
admission no concrete source/config exists yet. Its input MUST bind the exact
`SourceConfig` revision; its output MUST be a deterministic canonical secret-free
`CredentialRequest`. It MUST NOT read, fetch, decrypt or hold secret material, and
MUST NOT create physical resources, contact a broker, or produce any
credential-delivery side effect. It runs under a fixed absolute deadline; a timeout
is control-plane indeterminacy, never an empty or partial request. This keeps the
new resolver inside the B.4 pure-preparation boundary.

For B.5 v1 the resolver is not free to choose the set. Over the canonicalized
references (sorted, deduplicated), the invariant is

```text
Refs(CredentialRequest) == Refs(SourceConfig)
```

The resolver MUST NOT omit, add, or deduplicate away a distinct reference, and a
request produced from a different revision is invalid. Ordering is not significant
after canonicalization, so a reordered request is still equal only when its
canonical set is identical. An empty request for a non-empty `credential_refs` is
a hard failure, never a credential-free Execution: it would silently bypass the
brokered feature gate. A request that names a reference absent from the exact
revision fails closed the same way.

A future revision may distinguish source-side provisioning references from
worker-delivered references, but only by an explicit typed classification that
accounts for every reference and proves each one is authorized for its use; no
reference may disappear by resolver discretion. Until such a classification
exists, the equality above is the only legal rule.

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

Credential authorization is positive and deny-by-default, and it is never
interchangeable with broker availability:

- the bound AgentType `SECURITY_V1` ceiling MUST explicitly grant the exact
  `CredentialRef` together with its use, scope, audience, and delivery mode; an
  omitted secret-scope dimension in that ceiling grants nothing and is never
  normalized to `Top`;
- a `Generation` or source ceiling that declares the dimension may narrow that
  grant, but cannot create or widen one;
- `SourceConfig.credential_refs` remains a request; broker `Available` evidence
  and adapter brokered-delivery support are facts, not authorizations;
- a ref/use/scope/audience/delivery mode outside the granted set makes the
  candidate ineligible before any broker call, and it is never truncated,
  widened, or satisfied by a later gate.

The grant is over namespaced authorization identities, not bare strings.
`CredentialRef` is source-local; the AgentType ceiling grants a stable
`CredentialAuthorizationId` (explicit authority namespace plus logical
reference), and the trusted source integration resolves the source-local ref to
that identity under the exact revision. The resolved identity is part of the
eligibility subject and the `CredentialAccessPlan`. Two distinct credential
authorities using the same string are different identities, and a source-local ref
MUST NOT acquire AgentType authorization by string equality alone. The B5.1
encoding MUST carry the namespace explicitly in the AgentType grant and in the
resolved request/plan identity; a bare string is never a valid authorization key.

The request validated here is the canonical `CredentialRequest` produced by the
trusted source-local resolver from the exact `SourceConfig` revision (section 2).
B5.3 MUST NOT derive scope, audience, use, or delivery mode from a broker
response, and MUST NOT treat a successful probe as authorization.

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

B5.3 has two acceptance levels. **Component-ready** means the broker
qualification and delivery API, the canonical request/plan path and the component
conformance tests pass; it is not an end-to-end claim and does not by itself accept
B5.3. **End-to-end accepted** additionally requires one real reference integration,
not only a broker API and mock tests. On the B5.4 policy-only secured
start/hold/receipt/release path, an existing external agent runtime spawned
through a real `SpawnSource` integration MUST complete an end-to-end constrained
`BrokeredAccess` use: the source-local broker supplies a capability that the
harness can actually consume, and Scheduler Core never interprets the provider or
the secret. A harness that can consume credentials only as a raw environment
variable or from its own local store is a genuine integration gap; it MUST be
recorded as a B5.3 blocker rather than declared brokered-capable, unless an
explicitly narrower scope is agreed. The reference integration need not cover
every harness, but it must prove one real end-to-end path. This condition does not
change the policy-only slice or the brokered-only security rule.

The end-to-end gate runs on the policy-only handoff, not on the credential-bound
handoff, so B5.3 acceptance does not wait for B5.4 credential-bound integration;
the credential-bound handoff follows B5.3 acceptance. Conversely, B5.3 cannot be
accepted end-to-end before the policy-only handoff exists, so the work-item numbers
do not imply that B5.3 finishes before B5.4.

`BrokeredAccess` is only a real boundary if the worker cannot read the broker's raw
secret store and cannot bypass the capability. The policy-only reference profile
explicitly does not claim same-user read confidentiality, so the workspace jail
proves content-write confinement, not broker secret isolation. B5.3's real
integration MUST therefore additionally prove that:

- the broker's raw secret storage is not readable through the worker's declared
  launch environment, inherited handles or reachable filesystem paths;
- the worker can use only the declared scope/audience and cannot mint, widen or
  replay the capability outside its audience; and
- expiry, revocation and version fencing are enforced by the trusted broker rather
  than by the harness.

This is a condition on the `BrokeredAccess` trust boundary, not a claim of general
host read isolation or of secret-free worker reachability. Failing it is a recorded
B5.3 blocker, not a policy-only narrowing that silently drops the brokered claim.

Delivery mode is already an authorization dimension of the v1 AgentType
`SECURITY_V1` ceiling (section 5): the grant must name the exact mode, and B5.1
MUST encode it in that grant now rather than defer it to a future schema. B.5 v1
production admits only `BrokeredAccess`; `TransientRawDelivery` is present in the
versioned mode set but marked unsupported and is never selected. A v1 Task cannot
independently forbid raw delivery, because the delivery-mode dimension is
**outside the v1 Task policy grammar**.

Enabling `TransientRawDelivery` later therefore does not create a new
authorization dimension. It expands the versioned set of allowed delivery modes
and, if a Task must be able to narrow below its AgentType ceiling, adds the
delivery-mode dimension *and* its `meet`/conflict rule to a new Task policy
grammar. A v1 Task MUST NOT be reinterpreted as carrying that narrowing. Until
then, the requirement is derived from the AgentType ceiling and the credential
eligibility subject only, with no second implicit authority competing with the
ceiling.

### 7. Secret protection means exactly what it says

Agentype's own control path MUST NOT place secret values in the catalog,
`BindingSnapshot`, the Execution security snapshot, the launch descriptor,
command-line arguments, control errors, or ordinary control logs. Ambient daemon
environment inheritance is forbidden. The bootstrap manifest persisted in or
reconstructed for the security snapshot follows the same rule structurally: its
typed fields have no raw-credential or usable-bearer-capability slot, and delivery
material arrives only through the post-commit broker path (ADR-0014 §6).

The catalog trust boundary is unchanged: `SourceConfigBody::OpaqueJson` is opaque
to Core, and the source publisher is responsible for ensuring its payload contains
no secret values. B.5 keeps that boundary rather than promising that Core can
recognise a secret inside arbitrary source configuration. Credential material must
be referenced through `CredentialRef`/the broker, never embedded in an opaque body.
The secret-sentinel tests cover Agentype-produced artifacts and control surfaces,
not the internal content of a publisher-supplied opaque body; a secret found there
is a source-publisher contract violation, not evidence that Core can mechanically
classify arbitrary configuration.

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

A gate does not mean every policy dimension is enforceable; the
`EnforcementSupport` matrix still applies to the derived `FeatureObligations`, as
fixed in [ADR-0014](0014-m6b5-enforcement-compilation-receipt.md).

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
SandboxAppliedReceipt durable
    → HELD
    → credential authorization/delivery
    → CredentialDeliveryReceipt durable, binding the sandbox receipt digest
    → authority revalidation
    → RELEASED
```

The sandbox receipt MUST NOT claim a delivery scope, because the delivery has not
happened yet. The credential receipt records the scope actually established. B.5
v1 allows at most one credential delivery per Execution: one immutable
`CredentialAccessPlan` produces at most one `CredentialDeliveryReceipt`. A request
for a different scope under the same Execution is rejected before any broker call
and requires a new Execution with its own qualification, snapshot, and chain. No
earlier receipt is ever rewritten, and a second row of the same receipt kind is
corruption rather than an append. Multi-delivery inside one Execution would need
a versioned delivery identity and a new durable protocol in a later ADR. A missing
or mismatched delivery receipt keeps the phase at most `HELD`/`UNKNOWN`. A
policy-only Execution has no credential receipt and does not need one.

The single receipt covers the whole plan, not one reference at a time. A plan may
request several references; its receipt MUST prove the entire requested set was
delivered exactly, with the committed use/scope/audience/delivery mode. Partial
delivery is not a successful receipt and never reaches `RELEASED`. If some
references succeed and another fails, the Execution stays at most
`HELD`/`UNKNOWN`; recovery calls `reconcile_delivery`, closes the partial
capabilities with `revoke_or_close`, and if closure cannot be proven records
`UNKNOWN` and follows M5 recovery instead of assuming the delivered subset is
harmless. The next attempt is a new Execution with a new plan and chain.

That replacement gate is independent of physical worker recovery. When broker
capability closure cannot be proven, a later Execution MUST NOT be authorized
merely because the previous physical worker was quiesced or its process recovered:
M5 physical quiescence and invalidation of an external broker capability are
different facts. Before the replacement plan can be authorized, the broker must
supply an independent epoch/scope-fencing proof for the prior delivery identity —
for example a new broker epoch that supersedes the old capability, or explicit
closure/denial evidence — and the new plan must bind that evidence. Without it the
replacement candidate is ineligible or deferred. This is a B5.3/B5.4 acceptance
condition, not a new Core credential lifecycle manager; Core still keeps no secret
and does not track provider capabilities on its own.

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

The aggregate blocker set is exactly enumerable, not inferred:

```text
1. M6-B.4 remains FROZEN
2. M6-B.5 / policy-only is FROZEN
3. M6-B.5 / credential-bound is FROZEN
4. every registry item classified BLOCKS_M6B_FREEZE is closed
```

A slice at `FREEZE READY` is not sufficient for the aggregate: the aggregate
requires each slice's own maintainer authorization at the exact final SHA, not
only that its evidence is ready. If the maintainer authorizes both slices and the
aggregate at the same exact SHA, the freeze record MUST establish each slice as
`FROZEN` at that SHA before describing the aggregate as frozen. One authorization
event may cover all three, but it cannot skip the per-slice status.

Credential-bound is therefore a mandatory prerequisite of the M6-B aggregate
freeze, not an optional follow-up: an aggregate freeze while credential-bound is
open is not permitted. `BLOCKS_M6B_B5_FREEZE` items block only their named slice
and are not aggregate blockers by themselves. `D-TOPOLOGY` keeps its existing
`BLOCKS_SEMANTIC_LAYER (remainder)` classification and is not added to this set by
inference; moving it would require the maintainer ADR change described below.
When only a slice freezes, the record MUST name that slice and MUST NOT describe
the aggregate as frozen.

If only policy-only is completed, the record MUST freeze the policy-only slice
and MUST NOT describe credential-bound acquisition as complete.

B5.4 includes a **physical feasibility spike** whose OS risks start early, not
after the full policy compiler. The slice numbers are work-item identifiers, not a
chronological order; the implementation gate is dependency-based:

```text
B5.0 accepted
    ↓
B5.1 policy kernel
    ├── B5.4 physical feasibility spike (both Linux and Windows)
    └── B5.2 semantic enforcement qualification
                  ↓
             B5.4 policy-only physical handoff
                  ↓
             B5.3 real broker integration
                  ↓
             B5.4 credential-bound physical handoff
                  ↓
             B5.5 end-to-end conformance
```

The spike is not gated on completing B5.2 and may begin with a minimal OS prototype
alongside B5.1. B5.2 may proceed in parallel; the spike's result, not the policy
compiler, decides whether the required physical mechanism is attainable. Neither
B5.3 end-to-end acceptance nor a policy-only freeze may complete before the spike
gate passes on both platforms. B5.3 component work may begin after B5.2, but its
end-to-end acceptance runs on the B5.4 policy-only secured handoff rather than on
the credential-bound handoff, so the B5.3 gate does not depend on finishing
credential-bound B5.4.

The spike proves:

```text
hold boundary and worker cannot run before receipt durability
receipt publication and namespace durability
crash / control-channel-loss / OS-restart recovery
restart-time classification of HELD / RELEASED / UNKNOWN
physical instance identity and release denial on PID/handle reuse
conservative secured-start budget fits the remaining Lease window
```

The spike MUST also show that `B_secured` — a conservative admission budget, not a
mathematical worst-case bound on fsync, OS scheduling or IPC — fits a realistic
initial Lease grant on the reference profile, using measurements under the stated
reference platform and load conditions rather than an assumed figure. The spike
validates feasibility, timeout handling and recovery correctness; it does not
prove that an OS operation can never exceed the measured value, and actual
overrun still fails closed as `UNKNOWN`. If the budget does not fit, the slice
records a concrete blocker and narrows or defers the profile; it MUST NOT add a
pre-RUNNING Lease renewal path or rely on repeating the failing chain.

Restart-time classification is the first implementation gate of the policy-only
profile, not a B5.5 finding. The spike MUST prove how a restart distinguishes
`HELD`, `RELEASED` and `UNKNOWN`, including the window where release was reached
but the RUNNING transaction was not committed. If the release state cannot be
proven after restart, the phase MUST remain `UNKNOWN`; the spike MUST NOT infer
`RELEASED` to improve recovery success. Policy-only `SECURITY_V1` is not claimed
until this gate passes on both platforms.

The spike MUST first prove that hold, publication and recovery satisfy this
contract on both platforms, or record the concrete gap that prevents it. Broker
and credential-delivery work MUST NOT be treated as the critical path ahead of
that proof: a credential protocol built on an unproven hold/receipt mechanism
would restate the same risk with more machinery. A spike that shows the contract
is unattainable on a platform narrows the reference profile rather than relaxing
the contract. After the spike, B5.2 and B5.3 component work may proceed, and B5.4
first integrates the policy-only physical handoff. B5.3 is component-ready at that
point but is accepted end-to-end only when its real integration passes on that
policy-only handoff; the credential-bound handoff follows B5.3 acceptance. The
numbering does not authorize end-to-end B5.3 acceptance before the policy-only
handoff exists, and it does not make B5.4 credential-bound handoff a prerequisite
of B5.3 acceptance.

Narrowing the profile never means deleting a condition that `SECURITY_V1` needs
in order to be trustworthy. If the gap is in the fail-closed hold, receipt
integrity/non-replaceability/identity binding, or crash and
control-channel-loss recovery, the spike result MUST NOT be converted into a
policy-only freeze by dropping that condition. Only non-core enforcement features
may be removed from the claimed set. Otherwise the platform is recorded as not
supporting `security_execution_v1.policy`, or a new ADR redefines the supported
scope. A profile without a trustworthy receipt MUST NOT be described as
supporting policy-only `SECURITY_V1`, because that would turn an engineering
failure in B5.4 into a scope reinterpretation in B5.5.

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
upgrade. The receipt-chain artifact class is added to the `D-ARTIFACT-GC`
limitation.

The following remain outside B.5: `D-WORKER-RESULT-DP`, `D-ARTIFACT-GC`,
`D-SELECTION-POLICY`, full typed topology, cold/revivable agent matching,
MemoryCapsule/Transform, DB in-place migration, and a general malicious-code
sandbox.

### 13. Mandatory conformance additions

B5.5 MUST treat these as mandatory regression categories:

| Category | Assertion |
|---|---|
| Policy algebra | `meet` returns `Result<NormalizedPolicy, PolicyConflict>`; the greatest-lower-bound property holds only for `Ok`, a conflict is deterministic unsatisfiability (never a fault, transient unavailability or `RESOURCE_UNAVAILABLE`), and no `Bottom` policy is persisted; an empty allowed-set intersection is `Ok` as a deny-all dimension, distinct from a required launch/structural clash; order is transitive; normalize is idempotent; digest is permutation-stable |
| Equality regression | every contract accepted by the old exact-equality relation is still accepted by the pure policy order; new physical eligibility is not thereby granted |
| Coarse projection | every `SECURITY_V1` coarse/fine conflict is rejected |
| Task-local consistency | a Task coarse/fine conflict is rejected locally even when another ceiling would hide it; `sandbox_policy = None` follows the `SECURITY_V1` rule, and validation runs before normalization |
| Imported coverage | `RequiredEnforcementFeatures(normalized policy, effective guarantees)` emits an obligation only for an enforceable restriction or positive guarantee; a `Top`/unrestricted value emits none and is not itself a reason for ineligibility, while a concrete ceiling, disabled capability or required guarantee emits one; a candidate is ineligible when `FeatureObligations ⊄ EnforcementSupport(exact_binding)` even with the protocol gate open; the check is over the complete composite policy, and supporting A and B does not imply supporting `A ∩ B` |
| Enforcement matrix completeness | every spec-10 dimension is classified proven/limited/unsupported for the exact binding, and an unclassified dimension fails the suite |
| Canonical golden vectors | each schema/algorithm version pins byte-level input/output and digest |
| Compile budget | exhausting the budget mid-round, or an unknown candidate not proven continuity-non-decisive, cannot produce partial selection; repeated identical over-budget rounds surface a durable operator-actionable diagnostic and an explicitly blocked-for-operator defer instead of unbounded indistinguishable retries, without auto-enlarging the budget, silently shrinking the candidate set or choosing among the compiled subset |
| Credential probe | broker down is not absent; different epochs and wrong scope are not mixed; an unknown candidate in the winner's continuity tier defers, and only a proven non-decisive candidate may be ignored |
| Secret sentinel | no raw secret sentinel in Agentype-produced durable JSON, logs, control errors, snapshots, or descriptors; `SourceConfigBody::OpaqueJson` remains publisher-opaque, so the sentinel does not claim Core can identify secrets inside arbitrary publisher payload — that is a source-publisher contract violation |
| Compile cache | the cache key includes `effective_policy_digest`, `normalization_version`, `meet_algorithm_version`, adapter identity, enforcement profile and compiler protocol; the policy digest does not implicitly commit to either algorithm version, and a version change cannot reuse an older compile result |
| Recipient identity | wrong SID/birth, wrong pinned instance, and cross-Execution delivery are rejected |
| Receipt replay | Execution A's receipt cannot confirm B; a missing receipt cannot enter RUNNING |
| Hold/release | worker code does not run before receipt durability; recovery distinguishes awaiting-release from actually started; a receipt alone does not confirm RUNNING |
| Secured SPI deadline | the three secured operations take one absolute deadline, the hold/receipt-publication/release stages share it with no per-stage reset, an expired deadline with unknown physical state yields `UNKNOWN` and M5 recovery instead of `RELEASED`/RUNNING, and a later `reconcile_secured_start` gets a new bounded deadline without changing the `RequestId` or creating a second instance |
| Secured launch window | the usable window is the earlier of the absolute deadline and remaining Lease validity; a controlled instance is created only when that window covers the declared `B_secured` budget for the exact binding, where `B_secured` is a conservative admission budget derived from the enforcement profile, binding and reference-platform measurements rather than a mathematical OS upper bound, and overrun still fails closed; the failure action is aligned with the committed stage — before acquisition commit it defers without creating an Attempt, after acquisition commit but before Execution commit it creates no instance and closes the committed reservation through the frozen pre-Execution settlement path, and after Execution commit it follows the M5 post-Execution failure/recovery path rather than a no-Attempt defer; a window lost after materialization forbids release dispatch and leaves the instance to M5 recovery; repeated identical acquisition-stage infeasibility is a durable launch-window deferral, the adapter cannot extend the window, and no pre-RUNNING Lease renewal path is added beyond the frozen RUNNING-plus-first-renewal transaction |
| Secured start phase | `HELD`/`RELEASED`/`UNKNOWN`/`TERMINAL` transitions are explicit; `HELD` requires a verified sandbox receipt, a credential-bound `RELEASED` also requires a credential-delivery receipt; `UNKNOWN` permits absence and never enters RUNNING; release requires a Kernel authority proof for the exact RequestId and instance; PID reuse cannot authorize release; uncertain release cannot create a second worker; the hold stays closed on daemon crash or control-channel loss |
| Release authority proof | the proof carries ExecutionId, RequestId, Attempt/Lease fencing identity, exact instance witness, bounded validity, issuer and a single-use `ReleaseOperationId`; only the trusted Kernel path issues it, callers cannot supply or extend it, and the Kernel rechecks current authority immediately before release; a durable Kernel-owned `ReleaseOperationIntent` records the exact instance, fencing identity, proof digest/validity and `prepared`/`dispatched`/`released`/`unknown`/`aborted` state, with creation, dispatch and result recording in fenced Core transactions; a proof known to be invalid never initiates release; a repeated call after durable `RELEASED` for the same operation returns the recorded state without a second OS release, an unknown outcome is never re-issued, and a new operation is authorized only when no prior intent exists or the prior intent is terminal — a `dispatched`/`unknown` intent keeps the phase `UNKNOWN` and a `HELD` observation alone cannot close it or authorize a second operation; the adapter verifies instance/validity/operation only, and a Lease revocation racing an already-started valid release is owned by M5 fencing/recovery and cannot create a new valid semantic result |
| Release settlement | authorization and historical settlement are separate: `authorize_release` requires current Lease/fencing authority and is the only operation that may dispatch an OS release, while `settle_historical_release_observation` may advance an existing non-terminal intent to `released`/`aborted` after Lease expiry using a narrowly scoped recovery authorization bound to the exact intent, instance and dispatch-time lineage and trusted evidence; settlement cannot create an intent, issue a proof, dispatch a release, renew authority or confirm RUNNING, is idempotent and never downgrades `released`, and an unprovable outcome stays `unknown`, is not retired by timeout and blocks only that exact instance until independent evidence proves it cannot execute |
| Receipt chain | receipts are append-only, write-once, and bounded to one per kind per Execution; the sandbox receipt establishes `HELD` and is never rewritten; a credential-delivery receipt binds the sandbox receipt digest; a second row of the same kind is corruption, not an append; a policy-only Execution has no credential receipt, and a credential-bound Execution has both before `RELEASED` |
| Physical root binding | `ResolveRoot(committed descriptor, RequestId) = receipt.actual_root` is checked against materialized filesystem identity, not path string equality; a digest match with a substituted, symlinked or reparse-point-aliased root cannot confirm RUNNING; root substitution, aliasing and TOCTOU are negative tests |
| Receipt publication | durable contents, atomic no-replace publication, durable namespace update, and re-read verification of the published identity; POSIX `fsync`/`rename` alone is not a cross-platform proof |
| Cross-mode exclusion | symmetric in B.5 v1: a `SECURITY_V1` AgentType cannot execute a `COARSE_V1` Task, a `SECURITY_V1` Generation cannot admit a `COARSE_V1` Task, and every `SECURITY_V1` SpawnSource revision is reachable only through the full `SECURITY_V1` path, including one with `P_source = Top`; in each case the coarse projection cannot replace the authored fine contract or the declared protocol mode, a "supports both modes" label grants no execution, and only an independently authored versioned mode-specific contract can make a revision reachable from both paths |
| Admission mode | the mode is owned solely by `task_agent_requirements.security_contract_mode`; `task_security_bindings` exists exactly when the mode is `SECURITY_V1` and is absent for `COARSE_V1` (conditional biconditional), with no independent mode, and `generation_task_bindings` carries none; a Generation's mode follows `policy_mode`: `POLICY` reads the immutable `generation_policies` row, `NONE` is the derived `COARSE_V1` with no row and cannot admit a `SECURITY_V1` Task; the mode is explicit and durable, never inferred from policy presence; a `SECURITY_V1` Task cannot downgrade at matching, provisioning or recovery |
| Delivery mode vs Task grammar | delivery mode is already an explicit dimension of the v1 AgentType `SECURITY_V1` grant, so B5.1 must encode it now; v1 production admits only `BrokeredAccess` while `TransientRawDelivery` remains defined but unsupported. The dimension is outside the v1 Task policy grammar, so a v1 authored `sandbox_policy` cannot narrow it; enabling raw later expands the versioned allowed-mode set and, if Task narrowing is wanted, adds the dimension plus its `meet`/conflict rule in a new Task grammar, never by reinterpreting a v1 Task |
| Stage separation | `RequiredCapabilities` are not evaluated at admission because they need the actual AgentType; `PlacementConstraints` are never evaluated at admission; `Pending` obligations are recorded and discharged at the candidate stage; at Candidate every `Pending` resolves to `Satisfied`/`Unsatisfied`, and a remaining `Pending` is a whole-round defer or control-plane fault, never `Satisfied` and never an execution-authority commit |
| Provisioning relation scope | `can_provision` proves AgentType plus SpawnSource/SourceConfig type- and source-level eligibility and their applicable security constraints, but not the full four-way `P_eff`, because it takes no Task or Generation; `can_provision_task` and acquisition qualification prove full Task-specific `P_eff`, and Execution commitment re-proves the complete contract against actual bound identities |
| Composite mode re-check | candidate matching and commitment re-evaluate `CoarseExecutionModes` and every positive Task requirement against the complete `P_eff`; an admission-time verdict is not reused, because a later meet can remove every writable path; `Write` requires an allowed write inside the committed logical workspace region, not a writable temporary location |
| Guarantee composition | attempt isolation is a disjunction (`AgentTypeContract.security.requires_attempt_isolation OR TaskAgentRequirement.hard.required_attempt_isolation OR SourceRequirement.requires_attempt_isolation`), never a policy intersection; an `EnforcementSupport` capability that supports isolation does not create a requirement; a required isolation must reach an isolating target, exact-adapter proof and a committed isolation request, or the candidate is ineligible |
| `R_min` typing | `RequiredAccess ⊆ ProjectAccess` only for catalog-mapped security operation requirements; projection accepts only a `Set` whose `polarity` is `Ability` and whose `security_class` is `Authority`/`Sandbox` through the versioned member map, so `Bool(true)`, a `Restriction`, an ineligible class or an unmapped member fails closed; `RequiredCapabilities` use their existing `Bool`/`Set`/`Ordered`/`Quantity`/`Exact` matchers at the candidate stage; workspace/network use `CoarseExecutionModes` predicates; guarantees and placement use `satisfies_requirement`, never set inclusion; a `Restriction` ceiling relation is never converted into a positive floor |
| `P_source` top | a `SECURITY_V1` SpawnSource revision without an authored fine ceiling has `P_source = Top` and adds no restriction, but still requires complete imported-enforcement coverage, the feature gate and credential qualification; `Top` never bypasses eligibility and never makes the single-mode `SECURITY_V1` revision selectable by the coarse path |
| Evidence durability | credential proof identity, epoch, issuer version, validity bound and subject/scope digests reach the Execution snapshot; the raw broker response never does |
| Secret scope containment | credential authorization is positive and deny-by-default over namespaced `CredentialAuthorizationId` values: the AgentType `SECURITY_V1` ceiling must explicitly grant the exact identity plus use, scope, audience and delivery mode; an omitted declaration grants nothing and is never `Top`; Generation and source ceilings may only narrow; a source-local `CredentialRef` cannot gain authorization by string equality across distinct credential authorities; a request outside the grant is rejected before any broker call, never truncated or widened, and broker `Available` plus adapter support cannot substitute for the grant |
| Credential request provenance | the request is produced only during candidate qualification, after the exact `SourceConfig` revision is resolved and attested, before any broker probe and before acquisition authority commitment — never during M6-A semantic admission — by the trusted source-local resolver as a canonical secret-free `CredentialRequest` (ref set, use, scope, audience, delivery mode, canonical digest), with canonical `Refs(request) == Refs(SourceConfig)` so no reference is silently omitted or added and an empty request for a non-empty config fails closed instead of bypassing the brokered gate, validated as a whole against `P_eff` before any probe; the digest enters winner qualification and the `CredentialAccessPlan`; no scope may be derived from a broker response, so "probe then authorize" is a rejected ordering; the resolver is pure, read-only, secret-free, deterministic and deadline-bounded, with no physical or delivery side effect |
| Bootstrap authorization | `validate_bootstrap` proves the committed manifest does not widen worker environment, inherited handles, start exemptions, paths, credentials or any other dimension beyond the compiled plan and `P_eff`; launcher-only authority is non-inheritable and dropped before release; a digest-matching manifest that requests an over-broad exception is rejected before physical work; the typed manifest has no slot for raw credential values or usable bearer capabilities, because delivery material arrives only through the post-commit broker path, and the rule is tested on both the persisted-bytes and deterministic-reconstruction paths; manifest content is recoverable as canonical bytes or by exact deterministic reconstruction, never digest-only |
| Freeze scope | the M6-B aggregate freeze requires M6-B.4 frozen, policy-only `FROZEN`, credential-bound `FROZEN`, and every open `BLOCKS_M6B_FREEZE` item closed; `FREEZE READY` is insufficient for either slice, and a same-SHA authorization must record each slice as `FROZEN` before the aggregate; `BLOCKS_M6B_B5_FREEZE` items block only their named slice, and a slice freeze is never reported as the aggregate |
| Reference profile | every spec-10 enforcement feature is classified against `SECURITY_V1_POLICY_ONLY_REFERENCE_V1`; a non-claimed feature obligation makes the candidate ineligible, while a `Top`/unrestricted value yields no obligation and remains admissible through the full protocol; a capability is claimed only when both platforms prove it; B5.4 must prove receipt integrity, non-replaceability, identity binding, crash recovery and restart-time HELD/RELEASED/UNKNOWN classification, with an unprovable release kept `UNKNOWN`; read confidentiality is a non-claim and is never assumed, and credential-free means only no Agentype-managed delivery or committed credential plan — not that the worker can reach no host secret |
| Credential ordering | delivery is established while `HELD`; the credential-delivery receipt is durable before `RELEASED` and binds the sandbox receipt; one immutable `CredentialAccessPlan` yields at most one `CredentialDeliveryReceipt`, so a different scope is rejected under the same Execution and requires a new Execution with its own qualification, snapshot and chain; the single receipt must prove the whole requested set was delivered exactly, and partial delivery never reaches `RELEASED` |
| Single delivery domain | one `CredentialAccessPlan` is fulfilled entirely by one trusted broker delivery domain, which may sub-route internally but owns qualification, the canonical request, delivery and recovery; a plan requiring independent broker authorities is unsupported in v1 rather than implicitly fanned out or split into multiple receipts |
| B5.3 reference integration | B5.3 has a component-ready level (broker qualification/delivery API, canonical request/plan, component conformance) and an end-to-end accepted level; the latter requires one real external agent runtime through a real `SpawnSource` integration to complete an end-to-end constrained `BrokeredAccess` use on the B5.4 policy-only secured handoff, not the credential-bound handoff, with the broker supplying a capability the harness actually consumes and Core not interpreting the provider; mock-only broker/receipt tests do not satisfy acceptance, a harness limited to raw environment or its own credential store is a recorded B5.3 blocker rather than brokered-capable, and the work-item numbers do not imply that B5.3 ends before B5.4 |
| Broker secret isolation | `BrokeredAccess` additionally requires proof that the broker's raw secret store is not reachable through the worker's launch environment, inherited handles or accessible paths, that the capability cannot be minted, widened or replayed outside its declared scope/audience, and that expiry, revocation and version fencing are broker-enforced; this is not implied by content-write workspace confinement because the policy-only profile does not claim same-user read confidentiality, and a failure is a recorded B5.3 blocker rather than a policy-only narrowing |
| Credential replacement gate | when partial-delivery capability closure cannot be proven, a later Execution is not authorized by physical worker quiescence alone: M5 process recovery and external broker invalidation are different facts, so the replacement plan needs an independent broker epoch/scope-fencing proof for the prior delivery identity; without it the candidate is ineligible or deferred, and this remains a B5.3/B5.4 acceptance condition rather than a Core credential lifecycle manager |
| WARM credential reuse | a fresh per-Execution write jail proves content-write confinement only, not complete cross-Task security-context isolation; credential-bound WARM reuse additionally needs execution-bound broker verification, proof the old capability cannot be inherited or continued, or a physically verified reset witness, and is refused otherwise |
| RUNNING atomicity | observation, receipt chain, RUNNING, and first renewal all commit or all roll back |
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
- No claim that `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` covers an enforcement
  feature outside its positive list; a required `FeatureObligation` outside it
  makes the candidate ineligible. A `Top`/unrestricted value creates no such
  obligation, but never waives the complete compile/receipt/execution protocol.
- No receipt read-confidentiality claim: the security model relies on receipt
  integrity, non-replaceability, execution/instance binding and recoverability,
  never on the receipt path, digest or content being unknown to the worker.
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
  compilation and the applied-security receipt chain.
- [Spec 17](../specs/v0.2/17-deferred-open-questions.md) is the registry updated
  by §12.
