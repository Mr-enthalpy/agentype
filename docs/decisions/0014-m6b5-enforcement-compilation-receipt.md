# ADR-0014 — M6-B.5 enforcement compilation, physical binding, and the applied-security receipt

Status: Proposed (M6-B.5 / B5.0 normative design; pending maintainer audit; not a milestone freeze)
Date: 2026-11-08
Canonical path: `docs/decisions/0014-m6b5-enforcement-compilation-receipt.md`

## Context

[ADR-0013](0013-m6b5-policy-authority-algebra-identity.md) fixes what an
effective security policy *is*. This ADR fixes how that policy is compiled,
proven enforceable for an exact physical domain, committed to an Execution, and
checked against what was actually applied.

The frozen B.4 path already distinguishes pure source `attest`/`prepare` from
physical start. `WorkspaceJailSourceIntegration::prepare` returns a descriptor
containing the jail base and has no physical side effects. The real
`agentype-jail-<request_id>` child, its `workspace`, and the Windows ACL are
created inside `start_execution` → `prepare_jail`. Any compile step that assumed
an existing per-Execution root would therefore either run before the root exists
or have to move after commitment, losing its candidate-filtering value.

[Spec 14](../specs/v0.2/14-recovery-and-reconciliation.md) also requires the
positive RUNNING transition and the first Lease renewal to commit in one fenced
Core transaction. A security receipt cannot be a second supervision authority
or a separate "security confirmed" state machine.

## Decision

### 1. Three phases with distinct powers

| Phase | Inputs | Output | Physical side effects |
|---|---|---|---|
| Candidate resolution | normalized effective policy, exact binding, imported profile | root-relative enforcement plan + digests | none |
| Winner prepare | plan, attested source-private root recipe, launch descriptor | root-relative plan bound to logical root selectors | none |
| Physical start | committed plan, actual Execution root | real paths, applied restrictions, receipt | yes, after commitment |

Only the third phase creates a jail root, `workspace`, ACLs, processes, or
credentials.

### 2. Compilation is root-relative

`compile_execution_security` MUST take only the normalized effective policy, the
exact adapter binding, and the imported enforcement profile. Its output is a
**root-relative** plan: logical path selectors, not a concrete
`agentype-jail-<request_id>` path. It MUST NOT create a directory or require
one.

`prepare` binds the logical selectors to the attested jail base and workspace
mapping and returns a descriptor. It MUST NOT claim to have observed the actual
physical root: at B.4 the per-request child does not exist yet. The adapter
materializes it in `start_execution`, and the receipt records both the
root-relative plan digest and the actual physical identity.

### 3. Fixed candidate-probing order

```text
readiness preflight
    → static hard filters
    → effective policy derivation
    → imported profile coverage check
    → bounded root-relative compile
    → SourceConfig attest (read-only)
    → credential probe (when needed)
    → complete eligible candidate set
    → B.4 deterministic selection
```

`attest` is read-only but may perform external I/O and stays under its own
absolute deadline. Known-ineligible candidates are removed before private I/O.
Credential probing comes after local static/policy/enforcement work so an invalid
config cannot cause an external broker call.

### 4. Compile budget, memoization, and no partial selection

Compilation cache key:

```text
effective_policy_digest
adapter_kind
adapter_binding_key
enforcement_profile_digest
compiler_protocol_version
```

The key is scoped to a verifiable profile version and exact physical domain. The
cache memoizes **only** the pure compile result. It is not an eligibility proof:
source disposition, attestation, credential eligibility, lifecycle and every
other candidate-qualification fact are re-read and revalidated in every
acquisition round. A proof is never reused across a changed profile or binding,
and no cache key is allowed to make a stale disposition look current.

Each acquisition has a bounded candidate count, compile count, and absolute
time budget. If any relevant candidate still has undetermined eligibility,
final selection MUST NOT proceed over the remaining candidates. Budget
exhaustion or temporary inability to qualify the whole set is a whole-round
defer, never a partial selection over the first few successes.

Classification is strict:

- a deterministically unsupported combination excludes that candidate;
- a compiler protocol error or catalog/evidence corruption is a control-plane
  fault;
- a winner whose prepare or plan fails after selection MUST NOT fall back to the
  next candidate. Selection may restart only as a new complete, bounded
  resolution epoch.

### 5. Evidence subject

Acquisition evidence binds at least:

```text
actual AgentType revision
Task security requirement digest
SpawnSource revision
SourceConfig revision + config digest
AdapterBindingPolicy revision
exact adapter kind + binding key
effective policy digest
enforcement profile/protocol + profile digest
compiled plan digest
```

The Task requirement and effective policy are candidate-qualification facts and
MUST NOT be folded into the Incarnation-scoped materialization identity. The
source materialization digest keeps its existing responsibility; the
Execution-scoped `security_plan_digest` is separate.

The authority transaction re-reads and revalidates all durable inputs, including
the actual bound AgentType, candidate frontier, exact binding, policy digest,
profile digest, and evidence subject. External mutable facts are checked only
for subject, version, and validity; they are not pretended to be part of the
SQLite transaction.

### 6. Two time scales, and an explicit security snapshot

`ProvisioningBinding` remains the stable Incarnation-scoped provenance record:
exact source/config/policy, exact adapter binding, repeatable materialization
provenance, and stable enforceability identity. It does not carry a per-Task
policy or a temporary credential grant.

`ExecutionSecuritySnapshot` is the immutable per-Execution commitment, written
in the same transaction as the Execution and `BindingSnapshot`:

```text
inline effective policy bytes + digest
security derivation (normalization and meet algorithm versions)
effective capability requirements
required-access digest
enforcement profile identity + profile digest
security plan bytes + digest
bootstrap manifest digest
credential access plan (secret-free, with proof identity/epoch/issuer/validity)
security protocol version
actual bound AgentTypeRef or digest
Task derivation digest
exact source/binding provenance
```

`BindingSnapshot` currently carries no AgentType; the new snapshot MUST carry
the actual bound AgentType (or its digest) and the Task derivation digest, or a
later reader cannot verify that `P_agent` participated in the derivation. The
receipt and plan subject include the same identity.

A committed Execution's security contract is never changed by a later Task. A
long-lived LogicalAgent may serve different Task policies when continuity and
physical enforceability allow it, but a tightening alone does not mint a new
LogicalAgent; a real source/config/domain/enforceability change does not reuse
the old physical host.

### 7. Secured start observation and receipt

For `SECURITY_V1`, a start can become RUNNING only with a valid
`AppliedSecurityReceipt` and a proven secured phase. The receipt is optional in
the observation because the physical instance may exist before the receipt is
durable:

```text
SecuredStartObservation
    physical StartObservation
    phase: SecuredStartPhase
    receipt: Option<AppliedSecurityReceipt>
```

Phase invariants:

- `HELD` and `RELEASED` require a valid, verified receipt; without it the phase
  is not a proven security phase.
- `UNKNOWN` permits a missing receipt and preserves physical uncertainty; it
  MUST NOT enter RUNNING.
- `TERMINAL` may occur before a receipt exists; a missing receipt MUST NOT be
  used to fabricate success.
- if a physical observation exists but the receipt cannot be recovered, the
  runtime MUST NOT enter RUNNING and MUST NOT use the legacy path to synthesize
  acceptance.

Both `start` and `reconcile_start` MUST return or recover the same Execution's
trusted receipt whenever the phase proves one. The legacy `ExecutionAdapter`
interface remains available for `COARSE_V1`; `SECURITY_V1` uses a versioned
extension instead of changing the frozen B.4 trait:

```text
SecuredExecutionAdapter : ExecutionAdapter
    start_secured_execution(plan, launch)        → SecuredStartObservation
    reconcile_secured_start(request_id, witness) → SecuredStartObservation
    release_held_execution(request_id, witness, authority_proof)
                                                 → SecuredStartObservation
```

`SecuredStartObservation` extends `StartObservation` with the explicit phase and
the optional receipt. An adapter that cannot implement the extension cannot
provide `SECURITY_V1` for that binding. `COARSE_V1` continues to use the
unchanged base trait.

The receipt binds at least:

```text
ExecutionId
RequestId
exact adapter binding
effective policy digest
enforcement plan digest
actual root identity
proven physical instance
enforcement profile/protocol
secret-free credential delivery scope receipt
```

It is a physical fact, not Task success and not a new authority source.

### 8. Secured start phase machine, hold/release, and recovery

The receipt is only useful if it is durable before untrusted worker code can run.
Physical start MUST provide a trusted hold/release barrier:

```text
create controlled instance
    → establish instance witness
    → apply and verify restrictions
    → persist recoverable receipt (temp → fsync → rename)
    → release worker code
```

The worker MUST NOT execute before the receipt is durable. A source whose process
model cannot hold the worker at a pre-exec boundary must provide an equivalent
controlled-start mechanism, or the candidate is unsupported.

A persisted receipt does not by itself mean the worker was released. Secured start
therefore has an explicit phase:

```text
SecuredStartPhase:
    HELD       // controlled instance, restrictions applied, receipt durable, not executing
    RELEASED   // worker authorized to execute
    UNKNOWN    // instance exists, release state cannot be proven
    TERMINAL   // instance ended before or at release; cannot advance
```

The crash window

```text
sandbox applied
receipt persisted
daemon crashed
worker not yet released
```

is the `HELD` state and is a real durable possibility. Recovery MUST distinguish
it from `RELEASED`. `reconcile_start` recovers the phase for the same RequestId
and the same physical instance; it MUST NOT confirm RUNNING from the receipt
alone.

There is also a pre-receipt crash window:

```text
controlled process created
daemon crashed
receipt not persisted
restart finds the instance
```

This is `UNKNOWN`, not `HELD` and not `TERMINAL`. `reconcile_start` MUST NOT
synthesize a receipt, MUST NOT release the worker, and MUST NOT infer RUNNING.
The Execution remains physically uncertain and follows M5 recovery. The
crash-injection suite MUST cover this window explicitly.

A `HELD` instance is released only through an authority-bearing path:

```text
Runtime
    → reconcile exact physical instance
    → phase = HELD?
    → yes
    → Kernel revalidates current Scheduler/Task authority
    → release_held_execution(RequestId, exact instance witness, authority proof)
    → worker executes
```

The adapter performs the mechanical release; it MUST NOT decide whether the Task
still holds authority. That decision belongs to the Kernel and MUST be proven for
the exact `RequestId` and instance before the release call. The release result is
then part of the evidence consumed by the RUNNING transaction; it is not a second
security authority.

When the policy requires credential delivery, the order is:

```text
HELD physical recipient
    → credential authorization/delivery
    → applied-security + delivery receipt durable
    → Kernel authority revalidation
    → RELEASED
```

The applied-security receipt's secret-free credential delivery scope receipt MUST
describe the session that was actually established. A receipt MUST NOT be
persisted as covering a scope that is delivered later or with a different scope.
If delivery changes the scope, the receipt MUST be updated before release, or the
delivery MUST use a separate, independently recoverable receipt chain. A missing
or mismatched delivery receipt keeps the phase at most `HELD`/`UNKNOWN`.

The following are mandatory:

1. `reconcile_start` MUST NOT release a `HELD` worker without Runtime/Kernel
   authority revalidation.
2. Release MUST bind the original `RequestId` and the original physical instance
   witness. A bare PID is not identity.
3. A reused PID MUST NOT give an old instance release authority.
4. If authority is no longer valid, the old worker MUST NOT be released; the path
   is physical cleanup and M5 recovery.
5. If the release result is uncertain, no second worker may be created.

Receipt crash durability covers more than file bytes:

- the file and its containing directory metadata MUST be durable before release;
- the atomic rename MUST NOT replace an existing different receipt;
- the worker MUST NOT inherit the receipt file or directory handle, nor inherit
  read, write, delete or replace access to it;
- the receipt directory MUST be daemon-owned and explicitly inaccessible to the
  worker on both platforms, verified with negative tests.

The durable record is the validated receipt identity/digest committed by the
RUNNING transaction; the receipt file is recoverable physical evidence. The
relation among receipt file, durable record, and physical instance MUST be
verifiable: same ExecutionId/RequestId, same binding and plan digest, same
instance witness, and a write order that cannot present an unverified file as
applied. If that relation cannot be established after a crash, the Execution
remains physically uncertain and follows M5 recovery.

The policy-only reference profile (§14) also depends on this protection. The
daemon-owned receipt directory MUST have real read/write/delete/replace
isolation tests before the profile is declared; B.4's content-write confinement
is not evidence of confidentiality or of protection for daemon-owned state.

### 9. Receipt location and write discipline

The Windows `prepare_jail` path grants the restricting SID `FILE_MODIFY` on the
jail root and workspace. Any file under `agentype-jail-<request_id>` is therefore
worker-modifiable. The receipt MUST live in a **daemon-owned state directory**
outside the jail root and workspace; it MUST NOT rely on an implicit
"parent directory is not writable" assumption.

Write order:

```text
temporary file → fsync → atomic rename → only then release user code
```

Windows ACLs and Linux confinement must deny worker read/write/delete/replace of
that directory explicitly, with negative tests for each. The access check is a
positive requirement, not an inference from the jail's write restriction.

Two durable objects are deliberately distinct and MUST NOT be conflated:

- the SQLite `applied_security_receipts` row (identity/digest, consumed by the
  RUNNING transaction and authority-bearing);
- the daemon-owned receipt artifact (recoverable physical evidence).

After a crash before the RUNNING transaction, the artifact may be used to
reconstruct and verify the receipt identity, but only under the full relation:
same ExecutionId/RequestId, same binding and plan digest, same instance witness,
and a proven release phase. A missing artifact cannot be synthesized from the row;
an artifact alone cannot produce RUNNING without authority revalidation and the
validated transaction.

The receipt is added to the retained-artifact class tracked by
`D-ARTIFACT-GC`. A B.5 freeze record MUST state this limitation explicitly; the
frozen M6-B.4 report's F2 scope remains historically limited to B.4 artifacts.

### 10. Recipient and instance identity

Receipt identity reuses the B.4 witness and MUST NOT rely on a bare PID:

- Windows: verify birth and the per-Execution restricting SID on the same
  retained process handle;
- Linux: verify the same process instance through the pinned instance handle
  and the instance-token/birth witness.

A receipt for Execution A cannot confirm Execution B, and a missing receipt
cannot be synthesized.

### 11. One fenced RUNNING transaction

After a start or reconcile observation, the runtime commits in one fenced Core
transaction:

```text
Execution observation
+ validated AppliedSecurityReceipt
+ positive RUNNING transition
+ first Lease renewal
```

This persists observations as inputs; OS calls do not run inside the SQLite
transaction. If the receipt cannot be recovered after a crash, the runtime MUST
NOT synthesize one and MUST NOT downgrade to the legacy RUNNING path. Physical
uncertainty remains owned by M5 recovery, preserving the frozen
RUNNING/renewal atomicity without a second supervision authority.

### 12. `AppliedPlan ≡ CommittedPlan` is a digest tuple

The equality is defined over:

```text
(effective_policy_digest, enforcement_profile_digest, plan_semantics_digest)
```

`plan_semantics_digest` covers the **root-relative** compile result. Concrete
root and process-instance identity are bound separately by the receipt, so a
plan can be compared without embedding environment-specific paths. Byte-level
agreement is required; "compatible" is not equality.

### 13. A feature gate is not per-dimension coverage

`security_execution_v1.policy` (defined in
[ADR-0015](0015-m6b5-credentials-feature-gates-scope-freeze.md)) means the
policy/receipt/recovery protocol is implemented, not that every dimension is
enforceable. The `EnforcementSupport` matrix still decides per dimension. Even
with the gate open, a policy using an unsupported dimension makes the candidate
ineligible. The profile digest is part of the evidence subject and is
revalidated inside the acquisition transaction, so gate and profile version
cannot drift apart.

### 14. `SECURITY_V1_POLICY_ONLY_REFERENCE_V1`

B5.0 freezes a minimum positive reference profile so B5.2/B5.4 have a concrete
target and B5.5 has a profile to verify. It is a **positive** claim and a
**minimum implementation target**, not evidence that the current B.4 jail
already provides it. B5.4 MUST publish an OS enforcement-mechanism design note
for every claimed item, and B5.5 MUST verify it with positive and negative tests
before the profile is cited in a freeze record. It is a positive claim in the
further sense that a dimension not listed as supported is unsupported for this
profile, even when it exists in the policy grammar.

Claimed supported:

- per-Execution workspace content-write confinement;
- an explicit worker environment allowlist with no ambient inheritance;
- exact physical instance identity;
- a daemon-owned receipt directory with verified read/write/delete/replace
  isolation;
- receipt ↔ Execution binding and crash recovery, including the hold/release
  phase machine;
- credential-free execution.

Explicit non-claims:

- same-user general read confidentiality;
- arbitrary read-root or visibility isolation;
- network egress restriction (the network dimension remains a policy concept, but
  this profile does not claim it is enforceable);
- tool/command allowlisting;
- OS resource limits;
- attempt isolation (the current jail adapter does not provide it);
- process-tree termination or quiescence;
- general malicious-code isolation;
- arbitrary metadata immutability, including known Linux Landlock limitations.

The profile is platform-common: a capability is claimed only when both Linux and
Windows can prove it for the exact binding. A platform-specific matrix may be
stricter, never looser. B.4's content-write confinement is an input to this
profile, not evidence for any B.5 claim above it.

The receipt-directory claim is specifically **not** implied by the B.4 jail.
Linux content-write confinement is not general read isolation, and a Windows
restricted token does not establish same-user read confidentiality. The B5.4
design note MUST explain and demonstrate why the daemon-owned directory is
unreadable, unwritable, undeletable, and unreplaceable by the worker even when
daemon and worker share a user identity. This is a requirement on the evidence
store itself, not a claim of a general read sandbox.

A `SECURITY_V1` Task whose policy needs a non-claimed dimension is ineligible
under this profile; it MUST NOT be partially enforced. The B5.5 enforcement
matrix MUST classify every spec-10 dimension against this profile as proven,
limited, or unsupported.

## Non-claims

- No B.5 implementation exists at this baseline.
- No claim that every adapter can compile every policy, or that a supported
  feature works for every descendant process.
- The receipt proves applied policy on the proven direct process instance; it
  does not turn process death into quiescence or cover an unproven process tree.
- No new Scheduler authority, no worker-originated Result transport, and no
  in-place migration.

## Consequences

- Candidate resolution never needs the per-RequestId jail directory, so the
  compile step stays before selection without violating B.4 preparation purity.
- A failed winner compile cannot silently promote a weaker candidate.
- A receipt outside the worker-writable jail gives the RUNNING transaction a
  trustworthy input while keeping the same frozen transaction shape.
- Secured start has an explicit phase and a release operation bound to the exact
  RequestId and instance, so crash recovery never blindly launches a second
  worker or releases an instance whose authority has lapsed. The receipt is
  optional in the observation, so a process created before receipt durability is
  `UNKNOWN` instead of a fabricated success.
- Credential delivery is ordered inside `HELD` and before `RELEASED`, so the
  receipt cannot claim a delivery scope that has not been established.
- `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` fixes the minimum supported set and its
  non-claims as a proof obligation, so B.4's write confinement cannot be read as
  a full B.5 claim.
- Adding the receipt artifact class expands the retained-artifact limitation and
  is recorded for the deferred GC design.

## Relationship to other decisions

- [ADR-0013](0013-m6b5-policy-authority-algebra-identity.md) defines the policy
  authority and algebra this ADR compiles.
- [ADR-0011](0011-m6b-acquisition-contract-parity.md) owns the shared
  physical-phase and preparation-fault contract.
- [ADR-0012](0012-m6b4-scope-typed-physical-execution-ack.md) freezes the B.4
  boundary; the receipt extends the ACK interface, not a worker data plane.
- [ADR-0015](0015-m6b5-credentials-feature-gates-scope-freeze.md) owns the
  credential qualification and feature-gate matrix.
- [Spec 14](../specs/v0.2/14-recovery-and-reconciliation.md) owns the
  RUNNING + first-renewal transaction this ADR joins.
