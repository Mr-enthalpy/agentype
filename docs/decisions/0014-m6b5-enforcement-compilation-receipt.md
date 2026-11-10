# ADR-0014 — M6-B.5 enforcement compilation, physical binding, and the applied-security receipt chain

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

Receipt binding is not enough on its own: the physical root itself MUST be the
one the committed descriptor authorizes. Define the normative equality

```text
ResolveRoot(committed launch descriptor, RequestId) = receipt.actual_root
```

and require verification against the **materialized filesystem object identity**
through the trusted path-resolution rule, never against string-path equality, a
canonicalized path string, or the adapter's own claim. Two adapters can produce
the same root-relative semantics and the same digests while binding a different
physical directory; a receipt that faithfully describes its own wrong root is
still wrong.

B5.4 enforcement proof MUST verify the chain

```text
committed launch descriptor
    → permitted root binding
    → materialized filesystem identity
    → actual applied rules
    → receipt
```

and B5.5 MUST cover root substitution, symlink/reparse-point aliasing, and
check-then-use (TOCTOU) races as negative cases. B5.0 does not implement path
resolution; it makes the equality a normative MUST.

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
normalization_version
meet_algorithm_version
adapter_kind
adapter_binding_key
enforcement_profile_digest
compiler_protocol_version
```

The effective-policy digest commits to canonical bytes, not by itself to the
normalization and meet algorithm versions that produced them. Both versions are
therefore explicit key components: an algorithm change cannot reuse a compile
result produced under different semantics even when the resulting bytes happen to
match. The derivation identity recorded in the snapshot and the cache key agree on
the same two versions.

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
bootstrap manifest canonical bytes, or a deterministic reconstruction
    recipe over the committed exact launch descriptor
credential access plan (secret-free; canonical request digest,
    proof identity/epoch/issuer/validity)
security protocol version
actual bound AgentTypeRef or digest
Task derivation digest
exact source/binding provenance
```

`BindingSnapshot` currently carries no AgentType; the new snapshot MUST carry
the actual bound AgentType (or its digest) and the Task derivation digest, or a
later reader cannot verify that `P_agent` participated in the derivation. The
receipt and plan subject include the same identity.

The bootstrap manifest digest is part of the committed security contract, not an
independent variable beside it. Because a bootstrap manifest can select the
process environment, required paths, or start exemptions, verifying only policy,
profile and plan digests does not prove that the bootstrap configuration actually
used was the committed one. The trusted physical start path MUST therefore verify

```text
AppliedBootstrapManifest.digest
    == ExecutionSecuritySnapshot.bootstrap_manifest_digest
```

before publishing the sandbox receipt, and `SandboxAppliedReceipt` binds the
applied bootstrap manifest digest. A mismatch, or a manifest that cannot be
measured exactly, fails closed and never reaches `HELD`. The manifest digest is
not assumed to be covered by `plan_semantics_digest`; it is separately enforced
and receipt-bound.

Integrity alone does not prove authorization: a faithfully committed manifest can
still request more than the compiled plan allows. Before the execution security
snapshot is committed, the runtime MUST therefore run

```text
validate_bootstrap(
    committed_effective_policy,
    compiled_enforcement_plan,
    canonical_bootstrap_manifest,
    exact_source_and_binding
)
    → ValidatedBootstrapEvidence
```

`validate_bootstrap` is a pure, deterministic check that proves the manifest does
not widen what the worker can obtain: no environment name, inherited handle,
start exemption, path, credential, or other dimension beyond the compiled plan
and `P_eff`. Only launcher-only privileges that never reach the worker may be
carried as launcher authority, and the manifest MUST prove they are
non-inheritable and dropped before release; a privilege whose residual effect can
reach the worker is not launcher-only. A manifest that fails validation is
rejected before any physical work. `ValidatedBootstrapEvidence` is durable, and
its digest enters the snapshot, so a manifest that was validated cannot be
substituted later.

The manifest content MUST be recoverable after a restart, either as canonical
bytes persisted in the snapshot or by deterministic reconstruction from the
committed exact launch descriptor and `exact_source_and_binding`. A digest alone
is insufficient: a later reader cannot re-run `validate_bootstrap` or the
`AppliedBootstrapManifest.digest` equality on content it cannot obtain. If neither
recovery path is exact, the Execution fails closed rather than treating an
unverifiable manifest as applied.

A committed Execution's security contract is never changed by a later Task. A
long-lived LogicalAgent may serve different Task policies when continuity and
physical enforceability allow it, but a tightening alone does not mint a new
LogicalAgent; a real source/config/domain/enforceability change does not reuse
the old physical host.

Same-provenance WARM reuse is **not** sufficient proof of cross-Task
security-context reuse. An unchanged `ProvisioningBinding` says the provenance is
the same; it does not prove that resident physical state from the earlier Task
(written paths, caches, mounted secrets, process state, descriptor tables) has
been cleared. When two Tasks differ in write scope, secret scope, or execution
guarantees, reusing one physical host across them requires a fresh per-Execution
isolation proof, or an explicit reset/quiescence witness that covers exactly the
state the earlier Task could have written. Without one of those the reuse fails
closed and the Task does not start on that host.

The B.4 per-`RequestId` jail proves exactly one dimension — content-write
confinement — inside its own adapter, so it is a reference implementation for
that dimension only. A fresh per-Execution write jail is not complete cross-Task
security-context isolation: it does not prove same-user read confidentiality,
that an old credential capability cannot be reused, descendant-process
confinement, or attempt isolation. For a credential-bound WARM reuse the runtime
needs, in addition, an execution-bound verification of the broker capability,
proof that the previous capability cannot be inherited or continued, or a
physically verified reset witness; if none is available the reuse is refused.

### 7. Secured start observation and receipt

For `SECURITY_V1`, a start can become RUNNING only with a valid receipt chain
and a proven secured phase. The chain is optional in the observation because the
physical instance may exist before any receipt is durable:

```text
SecuredStartObservation
    physical StartObservation
    phase: SecuredStartPhase
    receipts: Option<SecuredReceiptChain>

SecuredReceiptChain
    sandbox: SandboxAppliedReceipt
    credential_delivery: Option<CredentialDeliveryReceipt>
```

`SandboxAppliedReceipt` establishes the sandbox-applied state; a credential-bound
Execution adds a `CredentialDeliveryReceipt` that binds the sandbox receipt
digest. Both are append-only and write-once. The sandbox receipt is never updated
or reused to describe a credential delivery; a credential delivery is a separate
receipt of its own kind, and one Execution carries at most one receipt of each
kind (see section 8).

Phase invariants:

- `HELD` requires a valid, persisted `SandboxAppliedReceipt`.
- `RELEASED` requires the sandbox receipt and, for a credential-bound Execution,
  a valid, persisted `CredentialDeliveryReceipt` that binds the sandbox receipt.
- `UNKNOWN` permits the chain to be absent and preserves physical uncertainty; it
  MUST NOT enter RUNNING.
- `TERMINAL` may occur before any receipt exists; a missing receipt MUST NOT be
  used to fabricate success.
- a policy-only Execution MUST have `credential_delivery = None`; a
  credential-bound Execution MUST have it before `RELEASED`.
- if a physical observation exists but the chain cannot be recovered, the runtime
  MUST NOT enter RUNNING and MUST NOT use the legacy path to synthesize
  acceptance.

Both `start` and `reconcile_start` MUST return or recover the same Execution's
receipt chain whenever the phase proves one. The legacy `ExecutionAdapter`
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
the optional receipt chain. An adapter that cannot implement the extension cannot
provide `SECURITY_V1` for that binding. `COARSE_V1` continues to use the
unchanged base trait.

The secured operations inherit the frozen M5 deadline contract in spec 07, and
the versioned extension MUST NOT become a way around it:

- all three operations take a single absolute deadline from the caller; none of
  them derives its own per-stage timeout or fresh wait budget;
- within one physical start, the hold, receipt publication and release share that
  budget; an adapter MUST NOT reset it between stages to extend the start;
- when the deadline expires with physical state unknown, the outcome is `UNKNOWN`
  and M5 recovery, never a synthesized `RELEASED` or RUNNING;
- `reconcile_secured_start` may later run as an independent recovery call with a
  new bounded deadline, but it MUST NOT change the original `RequestId` or create
  a second physical instance;
- Runtime re-qualifies the same deadline after the last evidence-producing
  operation; evidence obtained after the endpoint is `DeadlineExceeded` and never
  a successful observation.

`SandboxAppliedReceipt` binds at least:

```text
ExecutionId
RequestId
exact adapter binding
effective policy digest
enforcement plan digest
bootstrap manifest digest (applied manifest == committed snapshot)
validated bootstrap evidence digest
actual root identity
proven physical instance
enforcement profile/protocol
```

`CredentialDeliveryReceipt` binds at least:

```text
SandboxAppliedReceipt digest
ExecutionId
RequestId
exact adapter binding
secret-free credential access plan digest
proven recipient identity / pinned instance
delivered scope digest
delivery mode, broker epoch, issuer version
```

The credential-delivery receipt is a separate proof, not a field that mutates the
sandbox receipt. Its existence is required only when the Execution is
credential-bound.

The receipt chain is a physical fact, not Task success and not a new authority
source.

### 8. Secured start phase machine, hold/release, and recovery

The sandbox receipt is only useful if it is durable before untrusted worker code
can run. Physical start MUST provide a trusted hold/release barrier:

```text
create controlled instance
    → establish instance witness
    → apply and verify restrictions
    → publish SandboxAppliedReceipt (durable contents, no-replace
      publication, durable namespace update, verified identity)
    → HELD
```

The worker MUST NOT execute before the sandbox receipt is durable. A source whose
process model cannot hold the worker at a pre-exec boundary must provide an
equivalent controlled-start mechanism, or the candidate is unsupported. The hold
MUST remain closed on daemon crash or control-channel disconnect: it MUST NOT be
implemented by a wait whose handle or pipe closes on parent death and thereby
lets the worker continue. If the daemon cannot prove it still controls the
instance, the worker stays stopped and the instance follows M5 recovery.

A persisted receipt does not by itself mean the worker was released. Secured start
therefore has an explicit phase:

```text
SecuredStartPhase:
    HELD       // controlled instance, restrictions applied, sandbox receipt durable, not executing
    RELEASED   // worker authorized to execute
    UNKNOWN    // instance exists, release state cannot be proven
    TERMINAL   // instance ended before or at release; cannot advance
```

The phase constrains RUNNING, not merely whether a receipt is present:

| Phase | RUNNING |
|---|---|
| `HELD` | MUST NOT confirm |
| `RELEASED` | MAY confirm, only with positive physical observation |
| `UNKNOWN` | MUST NOT confirm |
| `TERMINAL` | MUST NOT confirm |

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
the exact `RequestId` and instance before the release call. The proof is not
caller-authored and has an explicit shape:

```text
ReleaseAuthorityProof
    ExecutionId
    RequestId
    Attempt/Lease fencing identity
    exact physical instance witness
    bounded validity
    issuing authority identity
    ReleaseOperationId
```

Only the trusted Kernel path may issue it; a caller cannot supply or extend it.
`ReleaseOperationId` identifies exactly one release authorization and is never
reused. The Kernel records it with the exact fencing identity and the release
intent before invoking the adapter, and the durable intent distinguishes
`not consumed`, `release in progress`, and `released`. First valid use consumes the
operation; a consumed operation is never presented as a new authorization.

The Kernel MUST verify the current Lease/fencing authority immediately before the
release call, not merely at proof issuance. The adapter verifies the exact
instance, the proof's current validity, and the operation identity, and performs
the mechanical release; it does not interpret Scheduler semantics. The release
result is then part of the evidence consumed by the RUNNING transaction; it is not
a second security authority.

Replay rejection and retry idempotency are different cases, keyed by the operation
identity. Once the release result is durable and the phase is proven `RELEASED`, a
repeated call for the same Execution, `RequestId`, exact instance and
`ReleaseOperationId` MUST return that recorded result idempotently: it performs no
second OS release, consumes no new authority, and cannot downgrade the phase. It is
a read of an achieved outcome, not a replay of the authorization that produced it.
If the operation was consumed but the release result is unknown, the instance stays
`UNKNOWN`/`HELD` and follows M5 recovery; the same operation is not re-issued. If
the instance is still `HELD` and the operation was consumed or failed, or its
validity/fencing identity is no longer current, the call MUST obtain a fresh proof
with a new `ReleaseOperationId` under the current authority, or leave the instance
`HELD` and follow physical cleanup plus M5 recovery. A repeated call never returns
`RELEASED` merely because some earlier call attempted release, and an old proof is
never reinterpreted as a new authorization.

Two claims must stay separate. A proof known to be invalid — expired, replayed,
mismatched instance, or bound to a superseded Lease/fencing identity — MUST NOT
initiate a release. But once a currently valid proof passes the Kernel check, a
Lease revocation can still race the OS call; that window cannot be eliminated
across SQLite and the OS. M5 fencing and recovery guarantee that such a release
cannot yield a new valid semantic result (RUNNING and worker output remain fenced),
rather than claiming the physical call never happens. B.5 does not claim an atomic
release, and the design fails closed rather than widening authority when the race
is observed.

When the policy requires credential delivery, the chain is ordered:

```text
SandboxAppliedReceipt durable
    → HELD
    → [credential-bound only] credential authorization/delivery
    → CredentialDeliveryReceipt durable, binding the sandbox receipt digest
    → Kernel authority revalidation
    → RELEASED
```

The initial `HELD` receipt cannot prove a delivery that has not happened yet, so
the sandbox receipt MUST NOT claim the credential delivery scope. The credential
receipt records the scope actually established. B.5 v1 fixes the credential
cardinality:

```text
one Execution
    → one immutable CredentialAccessPlan
    → at most one CredentialDeliveryReceipt
```

A request for a different credential scope under the same Execution MUST be
rejected before any broker call or physical delivery. It is not a second delivery
in the same chain and is never appended as a second row. Establishing a different
or wider scope requires a new Execution with its own qualification, immutable
plan, security snapshot, and receipt chain. A new Execution is not a new
Incarnation: it may reuse the same WARM host under section 6, including that
section's isolation or reset/quiescence witness. Multi-delivery inside one
Execution is outside B.5 v1; it would need a versioned delivery identity and its
own durable protocol in a later ADR, and B5.1 MUST NOT invent a delivery sequence
on its own. A missing or mismatched credential receipt keeps the phase at most
`HELD`/`UNKNOWN`. A policy-only Execution has `credential_delivery = None` and
does not need a credential receipt.

The single receipt covers the whole plan, not one reference at a time. When the
plan requests several credential references, the receipt MUST prove that the
entire requested set was delivered exactly, with the committed
use/scope/audience/delivery mode. Partial delivery is not a successful receipt, and
an Execution with a partially delivered plan MUST NOT reach `RELEASED`: if some
references are established and another fails,
the phase stays at most `HELD`/`UNKNOWN`, the runtime reconciles the broker
delivery and closes the partial capabilities, and when closure cannot be proven it
records `UNKNOWN` and follows M5 recovery rather than treating the delivered
subset as harmless. The next attempt is a new Execution with a new plan and chain.

The following are mandatory:

1. `reconcile_start` MUST NOT release a `HELD` worker without Runtime/Kernel
   authority revalidation.
2. Release MUST bind the original `RequestId` and the original physical instance
   witness. A bare PID is not identity.
3. A reused PID MUST NOT give an old instance release authority.
4. If authority is no longer valid, the old worker MUST NOT be released; the path
   is physical cleanup and M5 recovery.
5. If the release result is uncertain, no second worker may be created.
6. The hold MUST fail closed on daemon crash or control-channel loss; a worker
   that can self-release is not a valid held instance.
7. Receipts are write-once and append-only; no receipt replaces a different
   existing receipt, and the sandbox receipt is never edited to describe a later
   credential delivery.

Receipt crash durability covers more than file bytes:

- the file and its containing directory metadata MUST be durable before release;
- the atomic rename MUST NOT replace an existing different receipt;
- the worker MUST NOT inherit the receipt file or directory handle, nor inherit
  write, delete or replace access to it;
- the receipt directory MUST be daemon-owned and MUST deny worker write, delete
  and replace on both platforms, verified with negative tests. Read
  confidentiality is not part of this requirement (see §14).

The durable record is the validated receipt identity/digest committed by the
RUNNING transaction; the receipt file is recoverable physical evidence. The
relation among receipt file, durable record, and physical instance MUST be
verifiable: same ExecutionId/RequestId, same binding and plan digest, same
instance witness, and a write order that cannot present an unverified file as
applied. If that relation cannot be established after a crash, the Execution
remains physically uncertain and follows M5 recovery.

The policy-only reference profile (§14) depends on the integrity of this
protection, not on read confidentiality. The daemon-owned receipt directory MUST
have real integrity, non-replacement, write/delete/replace-denial and crash
recovery tests before the profile is declared. B.4's content-write confinement is
not evidence of any of these, and it is not evidence of confidentiality.

### 9. Receipt location and write discipline

The Windows `prepare_jail` path grants the restricting SID `FILE_MODIFY` on the
jail root and workspace. Any file under `agentype-jail-<request_id>` is therefore
worker-modifiable. The receipt MUST live in a **daemon-owned state directory**
outside the jail root and workspace; it MUST NOT rely on an implicit
"parent directory is not writable" assumption.

Write order (platform-neutral; the OS mechanism that proves each step belongs to
the B5.4 design note):

```text
write temporary receipt
    → durable file contents
    → atomic no-replace publication
    → durable directory/namespace update
    → verify published identity
    → release user code
```

Syncing file contents alone does not prove the published directory entry is
durable, so the namespace step is an explicit requirement rather than an assumed
consequence of the rename. Publication MUST NOT replace an existing different
receipt, and the published identity MUST be re-read and matched before release;
POSIX `fsync`/`rename` is not by itself a cross-platform proof.

Windows ACLs and Linux confinement must deny worker write, delete and replace of
that directory explicitly, with negative tests for each. The access check is a
positive requirement, not an inference from the jail's write restriction. Read
access is not required to be denied by this profile; the security model MUST NOT
depend on the worker being unable to read a non-secret receipt (see §14).

Two durable objects are deliberately distinct and MUST NOT be conflated:

- the SQLite receipt rows (`SandboxAppliedReceipt` / `CredentialDeliveryReceipt`
  identities and digests), consumed by the RUNNING transaction and
  authority-bearing;
- the daemon-owned receipt artifacts (recoverable physical evidence).

The chain is append-only: a `CredentialDeliveryReceipt` row references the
`SandboxAppliedReceipt` digest and never replaces it. After a crash before the
RUNNING transaction, the artifacts may be used to reconstruct and verify the chain,
but only under the full relation: same ExecutionId/RequestId, same binding and
plan digest, same instance witness, a matching sandbox receipt digest, and a
proven release phase. A missing artifact cannot be synthesized from the rows; an
artifact alone cannot produce RUNNING without authority revalidation and the
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
+ validated SandboxAppliedReceipt
+ CredentialDeliveryReceipt when the Execution is credential-bound
+ positive RUNNING transition
+ first Lease renewal
```

This persists observations as inputs; OS calls do not run inside the SQLite
transaction. If the required receipts cannot be recovered after a crash, the
runtime MUST NOT synthesize them and MUST NOT downgrade to the legacy RUNNING
path. Physical uncertainty remains owned by M5 recovery, preserving the frozen
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

The tuple does not include the bootstrap manifest. That is intentional: the
manifest is not inferred from the plan equality but bound by the separate
`AppliedBootstrapManifest.digest == snapshot.bootstrap_manifest_digest` check of
§6 and recorded in the receipt. A reader MUST NOT treat a matching triple as
proof that the committed bootstrap was applied.

This equality is necessary but not sufficient: identical digests do not prove the
physical root is the committed one. RUNNING additionally requires the
`ResolveRoot(committed descriptor, RequestId) = receipt.actual_root` equality of
§2, checked against materialized filesystem object identity. A digest match with a
substituted, symlinked, or reparse-point-aliased root MUST NOT confirm RUNNING.

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
- a daemon-owned receipt store with verified integrity, non-replaceability and
  worker write/delete/replace denial;
- an append-only `SandboxAppliedReceipt` bound to the exact Execution and
  physical instance, with crash recovery and the hold/release phase machine;
- credential-free execution.

The credential-bound scope additionally requires a `CredentialDeliveryReceipt`
that binds the sandbox receipt digest. That second receipt is not part of this
policy-only profile, and a policy-only Execution MUST NOT fabricate one.

Explicit non-claims:

- same-user general read confidentiality;
- receipt read confidentiality: a worker may be able to read non-secret receipt
  content, and the security model MUST NOT depend on the receipt path, digest or
  content being unknown to it;
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

The receipt-store claim is specifically **not** implied by the B.4 jail. Linux
content-write confinement is not evidence of integrity for daemon-owned state, and
a Windows restricted token does not establish either integrity or same-user read
confidentiality. B5.4 MUST publish an OS enforcement-mechanism design note that
proves, for the exact binding:

- the worker cannot write, delete or replace the receipt store;
- receipts are integrity-protected, non-replaceable, identity-bound and
  crash-recoverable;
- concurrent writers or a stale writer cannot make an older receipt look current;
- the receipt's `actual_root` is the root that the committed launch descriptor
  authorizes, proven by materialized filesystem identity rather than path string
  equality (§2).

Read confidentiality is deliberately not a minimum-profile claim. The receipts
contain no secret, and replay is bound to the exact Execution and physical
instance, so worker readability is acceptable if integrity and identity binding
hold. The security model MUST NOT rely on the receipt path, digest or content
being unknown to the worker. If a later scope decides that same-user read
isolation is required, it MUST be recorded as a high-risk implementation
dependency with its own OS mechanism and proof; it MUST NOT be assumed to follow
from a small extension of the current jail.

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
- The receipt is not self-validating: `actual_root` must equal the root resolved
  from the committed descriptor by materialized identity, so a faithful receipt
  for the wrong directory cannot confirm RUNNING.
- Secured start has an explicit phase and a release operation bound to the exact
  RequestId and instance, so crash recovery never blindly launches a second
  worker or releases an instance whose authority has lapsed. The receipt chain is
  optional in the observation, so a process created before receipt durability is
  `UNKNOWN` instead of a fabricated success.
- The receipt chain is append-only and monotonic: the sandbox receipt establishes
  `HELD`, and a credential-bound Execution adds a delivery receipt that binds it.
  No receipt has to pre-prove a future delivery, and no receipt is rewritten.
- Credential delivery is ordered inside `HELD` and before `RELEASED`, so the
  receipt chain records only a scope that was actually established.
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
