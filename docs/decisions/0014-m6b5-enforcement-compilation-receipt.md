# ADR-0014 — M6-B.5 enforcement compilation, physical binding, and the applied-security receipt

Status: Accepted (M6-B.5 / B5.0 normative design; not a milestone freeze)
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

The key is scoped to a verifiable profile version and exact physical domain; a
proof is never reused across a changed profile, source disposition, or binding.

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
credential access plan (secret-free)
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

For `SECURITY_V1`, a missing valid `AppliedSecurityReceipt` forbids the positive
RUNNING transition. A versioned secured start result is required:

```text
SecuredStartObservation
    physical StartObservation
    AppliedSecurityReceipt
```

Both `start` and `reconcile_start` MUST return or recover the same Execution's
trusted receipt. The legacy `ExecutionAdapter` interface remains available for
`COARSE_V1`.

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

### 8. Receipt location and write discipline

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

The receipt is added to the retained-artifact class tracked by
`D-ARTIFACT-GC`. A B.5 freeze record MUST state this limitation explicitly; the
frozen M6-B.4 report's F2 scope remains historically limited to B.4 artifacts.

### 9. Recipient and instance identity

Receipt identity reuses the B.4 witness and MUST NOT rely on a bare PID:

- Windows: verify birth and the per-Execution restricting SID on the same
  retained process handle;
- Linux: verify the same process instance through the pinned instance handle
  and the instance-token/birth witness.

A receipt for Execution A cannot confirm Execution B, and a missing receipt
cannot be synthesized.

### 10. One fenced RUNNING transaction

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

### 11. `AppliedPlan ≡ CommittedPlan` is a digest tuple

The equality is defined over:

```text
(effective_policy_digest, enforcement_profile_digest, plan_semantics_digest)
```

`plan_semantics_digest` covers the **root-relative** compile result. Concrete
root and process-instance identity are bound separately by the receipt, so a
plan can be compared without embedding environment-specific paths. Byte-level
agreement is required; "compatible" is not equality.

### 12. A feature gate is not per-dimension coverage

`security_execution_v1.policy` (defined in
[ADR-0015](0015-m6b5-credentials-feature-gates-scope-freeze.md)) means the
policy/receipt/recovery protocol is implemented, not that every dimension is
enforceable. The `EnforcementSupport` matrix still decides per dimension. Even
with the gate open, a policy using an unsupported dimension makes the candidate
ineligible. The profile digest is part of the evidence subject and is
revalidated inside the acquisition transaction, so gate and profile version
cannot drift apart.

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
