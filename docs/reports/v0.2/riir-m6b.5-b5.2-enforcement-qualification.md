# RIIR M6-B.5 B5.2 — Enforcement Qualification Audit Record

Status: Historical Report — audit record, not a freeze
Revision: 12 (2026-11-18) — records the conditional-acceptance review of the
revision-11 draft head `181c9e0`. The review accepts the static identity and
capability fixes and the B5.2-C/D/E decomposition, but not a B5.2 freeze. It sets
two hard conditions for the follow-ups — trusted importer provenance (B5.2-C) and
no physical start before the B5.4 snapshot/secured start (B5.2-E) — and requires
the unified candidate identity before budget, cache and durable-block wiring
(B5.2-D). No code change; the record still carries P2-1's profile binding and
subject proof (7), P2-2's credential request/authorization split (8), and the
P2-4 acquisition wiring (9), each with its own ADR amendment, gate run and
mutation table. No ADR `Status:` line, spec status or index entry changes
Applies to: V0.2 / M6-B.5 B5.2 (PR #28 merge `ce96c87`, PR #29 merge `d3ab482`;
closing fixes proposed in draft PR #37)
Date: 2026-11-18
Canonical path: `docs/reports/v0.2/riir-m6b.5-b5.2-enforcement-qualification.md`
Freeze state: **NOT FROZEN — no freeze gate has run**
Not a specification. This file indexes merged code against ADR-0013 §5 and
ADR-0014 §2–§5, §13–§14. It grants no correctness and closes no milestone.

Verdict: **B5.2's INFRASTRUCTURE IS IMPLEMENTED AND MERGED; B5.2 COMPLETION IS
NOT CONFIRMED AND NOTHING IS FROZEN.** The two slices landed the typed composite
Task authority, the policy-ordered `SECURITY_V1` candidate relation, the pure
root-relative enforcement compiler, and the schema-v10 durable acquisition-block
diagnostic. An independent re-audit (revision 2) found two P1 gaps in the
qualification chain and four P2 gaps in how its proofs are scoped, so the
security qualification chain is **not closed** and B5.3/B5.4 executable paths
must not be opened against it as-is. Later passes added three more findings —
P1-3 and P2-5 in revision 4, P1-4 in revision 5 — and none of them is closed by
this record. Revision 6 adds findings A and B, whose fixes are in a code pull
request stacked on the P1-3/P1-4 pull request. Revisions 7, 8 and 9 carry the
closing fixes P2-1, P2-2 and P2-4 in draft PR #37, revision 10 fixes the two
P1 defects that the continuation audit then found in that same draft (an omitted
B.4 eligibility conjunct and a credential delivery capability treated as an
authorization), and revision 11 fixes the two identity/capability defects the
next audit found: the profile bound the source `binding_ref` alias instead of the
resolved physical `adapter_binding_key`, and the fixed reference profile could
widen its support matrix. Revision 12 records the conditional-acceptance review
of that draft: the implemented parts and the B5.2-C/D/E decomposition are accepted
for continuation, with two hard conditions carried into the follow-ups, and the
PR draft stays open. As with every earlier finding, this record counts no
unmerged fix as closure and does not freeze B5.2. The current stage is
*selected-candidate transactional qualification implemented; complete
`SECURITY_V1` acquisition orchestration remains open*.

The gaps are **latent, not live**. The coarse typed acquisition path still
rejects `SECURITY_V1` at every stage, and the runtime resolver and daemon do not
invoke the new security acquisition entry point, so a live scheduler composition
still cannot acquire a `SECURITY_V1` Task. The durable block still has no
production writer. This record therefore claims no current privilege escalation
and no current production defect. It does correct two statements that revision 1
and the merged probes made: that the imported-profile constructor is fenced on
the supported surface, and that qualification settles the ADR-0013 §5
positive-obligation requirement on the complete effective policy.

## Revision history

| Revision | Head | Change |
|---|---|---|
| 1 | `d14137f` | First audit record over `c0497eb..d3ab482`; docs-only. |
| 2 | `6a3318d` | Records the independent re-audit: two revision-1 claims corrected, six findings added (two P1, four P2), B5.1 handoff table added. Still docs-only; no code, ADR, spec or index change. |
| 3 | `aa2f3b8` | Records the P2-3 decision: ADR-0014 §4 and this record state that durable-block idempotence is scoped to the currently stored condition, instead of adding a per-condition history table. Docs-only; no code, schema, ADR `Status:` line, spec or index change. |
| 4 | `983bc16` | Records the second independent re-audit: P1-3 (the attempt-isolation guarantee does not reach the plan accessor or its digest) and P2-5 (a durable block does not re-prove that its cost exceeded its budget). Also corrects revision 2's "five findings" to six. Docs-only; no code, ADR `Status:` line, spec or index change. |
| 5 | `097f35d` | Records the third independent re-audit: P1-4 (the compile cache key omits the guarantee identity, so plans with different obligations share one memoization key). Amends the ADR-0014 §4 frozen key list in the P2-3 ADR pull request and fixes the key in the P1-3 code pull request. Docs-only here; no code, ADR `Status:` line, spec or index change in this pull request. |
| 6 | `9b8d830` (code) | Records the fourth independent re-audit: A (a source declaration is not bound to its `SpawnSource` revision) and B (the durable block's validated read does not re-bind the generation, relate the candidate count to the cost, or validate the existing row on a same-condition record). Also records the write-once Task security mode premise and the candidate-domain identity model as a B5.3 design item, and adds the ADR-0014 §4 count-coherence sentence. Fixes are in the code pull request on `9b8d830`; docs-only here, no ADR `Status:` line, spec or index change. |
| 7 | `decb13a` (code) | Implements P2-1: an imported profile names the `(adapter_kind, binding_ref)` it was imported for, the compiler accepts only a validated source/binding/profile tuple, and a compiled plan is distinct from a proven subject. `ENFORCEMENT_PROFILE_SCHEMA_VERSION` is `/2`; ADR-0014 §2, §3, §5 and §14 state the tuple, the subject proof and the import-time subject bound. Revision 11 supersedes the identity: the profile now names the resolved physical `adapter_binding_key`, and the schema tag is `/3`. No ADR `Status:` line, spec status or index entry changes. |
| 8 | `28d6592` (code) | Implements P2-2: credential delivery is a request-scoped obligation settled when the exact `SourceConfig` is attested, not a policy-derived one. `RequiredEnforcementFeatures::derive` no longer inserts `CREDENTIAL_DELIVERY` from the authorization ceiling; `CompiledExecutionSecurity::with_source_config` rejects a non-empty request the profile cannot deliver. `ENFORCEMENT_COMPILER_PROTOCOL` is `/3`; ADR-0014 §3 and §4 state the request/authorization split. No ADR `Status:` line, spec status or index entry changes. |
| 9 | `d4d8dc3` (code) | Implements P2-4: `acquire_security_v1_task_existing` is the first non-test caller of `qualify_security_candidate`. One fenced transaction re-loads the Task binding, requirement, actual AgentType revision and authored policy, source, declaration and `P_source`, builds the composite authority, validates the caller's imported profile against the durable binding, qualifies, settles the attested config's credential request, and only then creates the Attempt/Lease and freezes the `ProvisioningBinding`. The two acquisition chains are disjoint. ADR-0014 §3 states the acquisition order and the disjointness. No ADR `Status:` line, spec status or index entry changes. |
| 10 | `0a0ed56` (code) | Records the continuation audit over `26a44f9`: P1-1 (the `SECURITY_V1` branch replaced the frozen B.4 provisioning eligibility instead of intersecting it) and P1-2 (delivery capability was read as a credential authorization). Extracts `can_provision_task_source_config` as the reusable B.4 conjunct, intersects it on the security path, and makes every non-empty `credential_refs` fail closed on both chains. Also records the P2 subject-identity and trusted-importer handoff constraints. ADR-0014 §3/§5 and ADR-0015 §5/§9 state the intersection and the fail-closed rule. No ADR `Status:` line, spec status or index entry changes. |
| 11 | `39e85c5` (code) | Records the identity/capability re-audit over `015220a`: P1 (the profile was bound to `AdapterBindingPolicy.binding_ref`, an alias, rather than the resolved physical `adapter_binding_key`) and P2 (the frozen reference profile could claim dimensions outside its fixed support matrix). `ValidatedSourceBinding::validate(source, binding, adapter_binding_key, profile)` now proves the profile names the same physical domain as the selection; the profile digest, compile cache key and evidence subject commit to that key. `ImportedEnforcementProfile::import` rejects `Support_imported ⊄ Support_reference` for the exact `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` identity. `ENFORCEMENT_PROFILE_SCHEMA_VERSION` is `/3`; ADR-0014 §2/§5/§14 state both rules. No ADR `Status:` line, spec status or index entry changes. |
| 12 | `181c9e0` (reviewed head; docs-only) | Records the conditional-acceptance review of the revision-11 draft: the implemented B.4 intersection, physical key/alias split, reference ceiling, credential fail-closed boundary and selected-candidate transaction are accepted for continuation, and B5.2-C/D/E remain separate follow-up pull requests. Sets the two hard conditions (trusted importer provenance; no physical start before the B5.4 snapshot/secured start), requires the unified candidate identity as the first B5.2-D commit, and pins five follow-up acceptance cases. Docs-only; no ADR `Status:` line, spec status or index change. |

Revision 2 does not resolve any finding. Each one is recorded with the code it
touches, the test that does not cover it, and the acceptance test that would
close it, so the follow-up code pull requests can be reviewed against the same
statements.

Revision 3 resolves exactly one finding, and it resolves it by narrowing the
claim rather than changing the encoding: P2-3. P1-1 and P1-2 have proposed fixes
in PR #31, which was open and unmerged when this revision was written; this
record does not count an unmerged fix as closure.

Revision 4 records two findings the revision-2 pass missed and resolves none.
Both were checked by static reading against the same merged code, `d3ab482`;
this revision executed no new test. It also corrects revision 2's finding count:
the table below has always listed two P1 and four P2 findings, not five.

Revision 5 records one finding the earlier passes missed: the compile cache key.
It resolves nothing; P1-4's fix is in a code pull request and its normative
statement is an ADR amendment in the ADR pull request, neither of which this
record counts as closure before merge. The finding is confirmed by the same kind
of evidence as P1-3 — a temporary assertion in the isolation matrix that
`baseline.cache_key() == task_only.cache_key()` while the plan digests differ —
and that assertion is now part of the fixed matrix.

Revision 6 records two findings from the fourth pass. Both were confirmed by
reading the code at `6c8fd9c` and are fixed in a code pull request whose head is
`9b8d830`; this record counts neither as closed before that pull request merges.
It also records one check the audit asked for that the code does not need (the
Task security mode is write-once, so a read-time re-check adds no guarantee) and
one question the audit raised that is a design item, not a defect (the candidate
identity model for a round over unmaterialised candidates).

Revision 7 carries the first fix of the closing pull request: P2-1. It is a code
change with a normative ADR amendment, so it is not docs-only. Three
consequences are deliberate and recorded below: the imported profile now names
its adapter binding, the profile schema tag moves to `/2` because the profile
digest commits to that identity, and a successful compile is no longer readable
as an applied-security claim for an unnamed subject. P2-2 and P2-4 remain open in
the same pull request, so this record still does not declare B5.2 complete.

Revision 8 carries the second fix: P2-2. It is also a code change with a
normative ADR amendment. The credential dimension is split into its
authorization (the effective policy's grant map) and its request (a non-empty
`SourceConfig.credential_refs`), and only the request creates a delivery
obligation. Because that changes which obligations the compiler derives, the
compiler protocol tag moves `/2 -> /3`; deliberately, no durable format, cache
key list or profile digest changes.

Revision 9 carries the third closing fix: P2-4. It is a code change with a
normative ADR amendment. The qualification chain now has an authority-bearing
caller inside the acquisition transaction, so a `SECURITY_V1` Task can no longer
be read as "preselected but unacquirable". This is deliberately **not** a claim
that the runtime drives it: the resolver and daemon still acquire through the
coarse path, so the security chain is reachable from the Kernel's
`runtime-internal` surface and from tests, not from a live scheduler
composition. Nor does it claim a secured start: the qualified value is
discarded at the end of the transaction (B5.4 owns the durable plan/snapshot).
As of revision 9, P2-1, P2-2 and P2-4 were all implemented in draft PR #37; this
record still does not declare B5.2 complete or frozen, and the freeze gate
remains the user's independent re-audit.

Revision 10 carries the two P1 fixes the continuation audit found in that same
draft. It is a code change with normative ADR amendments. The direction of P2-1,
P2-2 and P2-4 is accepted; the defects are compositional: the new security branch
had replaced the frozen B.4 source/config eligibility instead of intersecting it
(P1-1), and it could grant Attempt/Lease to a credential-bearing config whose
request no positive grant or broker qualification covered (P1-2). The audit's
verdict on the stage is recorded verbatim in substance: **selected-candidate
transactional qualification implemented; complete `SECURITY_V1` acquisition
orchestration remains open.** Revision 10 does not claim B5.2 orchestration,
B5.2 freeze or B5.4 secured execution.

Revision 11 carries the two defects the next audit found in the same draft. It is
a code change with normative ADR amendments. The audit accepted P1-1/P1-2 and the
provisional soundness of P2-1, but showed that B.4 has two different identities:
`AdapterBindingPolicy.binding_ref` is a stable alias, while the resolved
`AdapterBindingKey` is what the Execution freezes. The validated tuple compared
the profile with the alias, so a profile imported for physical domain K1 could be
compiled against a selection resolved to K2 through the same alias. It also
accepted the profile's support matrix without a ceiling, so the fixed
policy-only reference identity could claim a dimension it does not declare. Both
are corrected at the import boundary and in the validated tuple. The audit's
verdict still governs: **selected-candidate transactional qualification
implemented; complete `SECURITY_V1` acquisition orchestration remains open.**
Revision 11 does not mark P2-1 fully closed, does not claim complete B5.2
orchestration, and does not claim a B5.4 secured start or receipt.

Revision 12 records the conditional-acceptance review of `181c9e0`. The review
accepts #37 as a continuing B5.2 implementation draft, not as a freeze: the B.4
intersection, the physical key/alias separation, the reference-profile ceiling,
the credential fail-closed boundary and the single-candidate transaction are
accepted, and B5.2-C/D/E are explicitly left to separate pull requests rather than
required in this one. The review adds two hard conditions and two ordering rules,
recorded below. No implementation is added to #37 by this revision.

## Why this record exists

B5.2 was delivered as two merged pull requests without a milestone closure
report. That is enough to read the diff, but not enough to audit it: a reviewer
needs a single page that says which normative sentence each function implements,
which test fails if that sentence is broken, and which fences were checked. This
record is that page. Apart from the revision-3 wording correction to ADR-0014 §4
recorded below, it is deliberately additive: no code, ADR `Status:` line, spec
status, or index entry changes in the same change.

This record does **not** claim:

- that B5.2 satisfies a freeze gate;
- that the security qualification chain is complete (revision 2: it is not, see
  [Re-audit findings](#re-audit-findings-revisions-2-4-5-and-6));
- that ADR-0014 §6–§12 (physical binding, receipts, hold/release, recovery) is
  implemented — it is not, and it belongs to B5.4/B5.5;
- that `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` is enforced by any OS mechanism;
- that `docs/README.md` should list B5.2 as frozen.

## Scope baseline

Audited range: `c0497eb..d3ab482` (eight commits: two merges and six slice
commits).

```text
21 files changed, 5654 insertions(+), 115 deletions(-)
```

| Slice | PR | Merge | Commits |
|---|---|---|---|
| A — relations + composite authority | #28 | `ce96c87` | `a331a6a`, `34fedee`, `90ad263` |
| B — enforcement compiler + schema v10 | #29 | `d3ab482` | `cbab05f`, `5f644b4`, `c1f7db8` |

What B5.2 owns, in the ADR vocabulary:

```text
ExistingAgentCandidate                          (unchanged)
QualifiedExistingAgentCandidate<'a>             (new, borrows a validated SecurityRevision)
TaskSecurityCeiling                             (new, candidate-independent)
EffectiveTaskAuthority                          (new, four-term fold + Task coarse ceiling)
compile_execution_security                      (new, pure, root-relative)
qualify_security_candidate                      (new, acquisition-side predicate)
CompileCacheKey / AcquisitionBudget             (new, key and whole-round gate only)
security_acquisition_blocks                     (new, schema v10, one diagnostic per condition)
```

What B5.2 explicitly does not own: `prepare`, physical root binding, secured
start, the applied-security receipt, hold/release, and recovery. Those are
ADR-0014 §6–§12 and remain unimplemented.

## Re-audit findings (revisions 2, 4, 5 and 6)

Revision 2 re-read the merged code rather than the revision-1 prose and
reproduced every finding against `d3ab482`. Revision 4 adds P1-3 and P2-5,
checked by static reading against the same code. Revision 5 adds P1-4, the one
gap those passes missed: a guarantee is plan identity but was not cache-key
identity. None is inferred from a comment or a test name. Severity is stated for
the qualification chain, not for the current build: while acquisition rejects
`SECURITY_V1`, all of them are latent.

| ID | Severity | Finding | Blocks |
|---|---|---|---|
| P1-1 | P1 | The four-term `P_eff` is never re-checked against the Task's positive `RequiredAccess` floor | Closing the qualification chain; opening B5.4 |
| P1-2 | P1 | The "default build cannot mint an imported profile" claim is false, and public-API probe 31 cannot prove it | Using the record as a B5.4 gate input |
| P1-3 | P1 | The attempt-isolation guarantee becomes an obligation but never becomes the plan's execution-request value, and it is not explicit in the plan digest | B5.4's committed execution request; reading a compiled plan as an applied-security claim |
| P1-4 | P1 | The compile cache key omits the guarantee identity, so two plans with different obligations share one memoization key | Any reuse of a compile result; the ADR-0014 §4 memoization claim |
| P2-1 | P2 | The compiler never checks that the profile was imported for this binding or that it covers the executing subject | Trustworthy `SECURITY_V1` evidence (**fix proposed in revision 7, not closure before merge**) |
| P2-2 | P2 | A credential *authorization* is treated as a credential *delivery* obligation | Correctness of candidate rejection (over-rejection) (**fix proposed in revision 8, not closure before merge**) |
| P2-3 | P2 | Durable-block idempotence holds only for the single stored condition, not across history | The ADR-0014 §4 statement the report quotes (**closed in revision 3 by narrowing the ADR**) |
| P2-4 | P2 | Candidate preselection is implemented, but acquisition qualification is not wired (the fourth B5.1 handoff item) | Declaring B5.2 complete |
| P2-5 | P2 | A durable acquisition block does not re-prove that its recorded cost actually exceeded its recorded budget | Trustworthiness of the operator-actionable budget diagnostic |

### P1-1 — the positive access floor is not re-proved on `P_eff`

**Normative sentence.** ADR-0013 §5, Candidate row (line 430):

> `RequiredCapabilities` against the actual AgentType claims through their catalog
> matchers, … whole-policy `FeatureObligations` coverage, **and every positive
> Task requirement (`RequiredAccess`, `RequiredCapabilities`,
> `CoarseExecutionModes`) re-evaluated against the complete `P_eff`**

The same section repeats it: "A later Candidate or Execution stage re-applies the
same view to the complete `P_eff` because a later meet can only narrow it." The
Candidate stage therefore owes an explicit `RequiredAccess` re-check on `P_eff`.

**What admission checks.** `requirement.rs:1485–1505`: the durable admission
transaction computes the floor from the requirement and the exact catalog and
checks it against `task_coarse_authority(meet(P_generation, P_task), coarse)`.

**What the candidate stage checks.** `authority.rs:209, 250–336` builds

```text
P_eff = TaskAuthority(meet(P_binding, P_agent, P_source))
```

`enforcement.rs:846–892` (`qualify_security_candidate`) then re-proves the
requirement digest, the AgentType revision, `is_security_v1`, the agent-policy
digest and `can_execute_security_v1`, re-checks the Task's coarse modes, and
compiles. `can_execute_security_v1` (`predicates.rs:242–360` → `can_execute_inner`)
checks the requirement's capability matchers against the AgentType contract and
requires `is_within(effective, agent_policy)`; it does not project
`required_capabilities` into `RequiredAccess` and does not call
`check_required_access`. `compile_execution_security` (`enforcement.rs:758–837`)
derives enforcement-feature obligations and checks support coverage; it has no
access floor either.

**Why it is a real defect, not a missing convenience.** The added AgentType and
SpawnSource terms can only narrow, and narrowing can delete a capability the
Task's floor requires. `authority.rs:422–426` already states the identical
argument for the coarse modes — "admission checked the fine meet alone. A later
intersection with the AgentType or SpawnSource ceiling can erase the only
writable or reachable region, so every candidate stage MUST call this on the
composed policy" — but only `validate_coarse_modes` was added. The positive
floor receives no candidate-stage check at all.

Counterexample:

```text
Task requirement:      tools = {"git"}            -> RequiredAccess(tools: git)
binding + agent:       allow tool "git"
source ceiling:        deny  tool "git"
P_eff.tools:           git absent
result:                all existing checks pass; qualification succeeds
```

**Required fix.** In `qualify_security_candidate`, rebuild the floor from
`requirement.hard.required_capabilities` through the exact catalog (the
`project_capability_access` + `RequiredAccess::from_operations` path already
used by `required_access_for` at `requirement.rs:1341–1356`), assert the
recomputed `required_access_content_digest` equals
`authority.required_access_digest()` (`authority.rs:388`), then run
`check_required_access(&required, &PolicyAccess::from_policy(authority.policy()))`.
A deficit is candidate-local `SecurityUnenforceable`, not a Task failure. Both
the requirement and the catalog are already parameters of the function, so the
fix is local.

**Acceptance test.** Three negatives — the source ceiling removes a required
tool, a required network endpoint, and a required file operation — each must
reject with `SecurityUnenforceable`; one positive must still compile.

**Status.** Open. Blocks P2-4 and any B5.4 executable path.

**Revision 4 note.** The second re-audit's stricter acceptance — re-project the
requirement's claims through the exact catalog and prove the digest before
checking the floor — is already implemented in the unmerged PR #31:
`required_access_from_claims` rebuilds `RequiredAccess`,
`required_access_content_digest` is compared with
`authority.required_access_digest()`, and only then does `check_required_access`
run on `P_eff`. This record does not count the finding as closed until that pull
request merges.

### P1-2 — the imported-profile constructor claim is false, and probe 31 is vacuous

**What revision 1 said.** "The default build cannot mint an imported enforcement
profile. … Public-API probe 31 asserts that the constructor is absent from the
supported surface."

**What the code shows.** `crates/agentype-runtime/Cargo.toml` enables
`provisioning-producer` on `agentype-agent-contract`; the boundary fixture
depends on both `agentype-runtime` and `agentype-agent-contract`. Under
resolver 2, features of normal dependencies are unified within one build, so the
exact build that runs the probes has the constructor compiled in:

```text
cargo tree -p agentype-public-api-boundary -e features -i agentype-agent-contract
└── agentype-agent-contract feature "provisioning-producer"
    └── agentype-runtime
```

Probe 31 is `let _ = ImportedEnforcementProfile::import;`. Both `import` and
`from_parts` take `impl IntoIterator<...>`, so taking the function item without
a concrete argument type is rejected with E0283 independently of whether the
function exists. A five-line rustc program reproduces it:

```rust
struct S;
impl S { fn f(x: impl IntoIterator<Item = u8>) { let _ = x; } }
fn main() { let _ = S::f; }   // error[E0283]: type annotations needed
```

The `compile_fail` probe therefore passes for a reason unrelated to the claim,
and the adjacent probe 18 in the same file already records the correct position:
"under Cargo feature unification a host that also depends on the runtime can see
`from_imported_binding` (`provisioning-producer`). The load-bearing boundary is
the acquisition control surface plus in-transaction re-proof, not constructor
secrecy."

**Correct statement for this record.** No *contract-only* host on the default
feature set can name the constructor. Any host that also embeds the runtime can,
because the runtime is the producer. The `#[cfg]` gate is a supported-surface
convenience, not an unforgeable capability, and must not be cited as evidence
for the B5.4 gate. Note also that probe 31's own doc comment repeats the false
premise; fixing it is code work, not part of this docs-only change.

**Required fix.** Replace probe 31 with a fully typed constructor call in a
*contract-only* fixture (it must fail with the item absent, not with an
inference error), and add a runtime+contract fixture that records the
constructor as visible. Then state the real boundary: no production API accepts
a caller-submitted profile, and the authority transaction re-proves imported
facts.

**Status.** Open; the record is corrected here, the probe and its comment in the
follow-up code change.

### P1-3 — the attempt-isolation guarantee never reaches the plan accessor

**Normative sentence.** ADR-0013 §3 and spec `10-sandbox-and-security.md`: the
execution guarantee is a disjunction, not an authorization intersection —

```text
I_required = I_AgentType ∨ I_Requirement ∨ I_SourceRequirement
```

— and it "must then reach `ExecutionTarget.attempt_isolation = true`, be proven
enforceable by the exact adapter, and be requested by the committed execution."
An `EnforcementSupport` capability that merely supports isolation must not create
a requirement, and no policy meet may clear one.

**What is implemented.** `EffectiveTaskAuthority` folds the Task requirement and
the bound AgentType into `requires_attempt_isolation`
(`authority.rs:337–342`, accessor `418–420`).
`EffectiveExecutionGuarantees::from_authority` copies that value
(`enforcement.rs:169–173`). `RequiredEnforcementFeatures::derive` inserts
`AttemptIsolation` when either the policy bit or the guarantee says so
(`enforcement.rs:220–222`), and `compile_execution_security` passes
`EffectiveExecutionGuarantees::from_authority(authority)` into that relation
(`enforcement.rs:780–783`). The support check therefore does see the guarantee.

**What is missing.** `PolicyEnforcementPlan` stores only the effective policy,
obligations, profile reference and digest (`enforcement.rs:489–494`).
`PolicyEnforcementPlan::attempt_isolation()` returns
`self.effective_policy.attempt_isolation()` (`enforcement.rs:531–533`), not the
guarantee. The plan digest (`enforcement.rs:797–804`) records the effective-policy
digest and the obligation set, but not the guarantee bit explicitly.

Counterexample:

```text
Generation policy:   attempt_isolation = false
AgentType policy:    attempt_isolation = false
Task requirement:    required_attempt_isolation = true
SpawnSource policy:  attempt_isolation = false

authority.requires_attempt_isolation() = true
obligations contain AttemptIsolation
plan.attempt_isolation() = false
```

`compile_execution_security` accepts this authority if the imported profile
supports `AttemptIsolation`: the obligation is satisfied, and the returned plan
contradicts it. `qualify_security_candidate` currently rejects the same input
earlier, because `can_execute_security_v1` checks
`(agent.requires_attempt_isolation || task.required_attempt_isolation) &&
!safety.attempt_isolation()` (`predicates.rs:564–565`). That makes the full
qualification chain safe today, but it does not repair the compiler's own
contract: a direct `compile_execution_security` caller — which B5.4 will be — can
read `plan.attempt_isolation() == false` from a plan whose obligations require
isolation.

**Required fix.** Carry `EffectiveExecutionGuarantees` on
`PolicyEnforcementPlan`, make `attempt_isolation()` read the carried guarantee,
add the guarantee bit to the plan digest, and expose it as plan identity. The
composed guarantee is the disjunction of the Task requirement, the AgentType
contract and the effective policy bit, which already folds every authored layer
(Generation, Task and SpawnSource): a Task-only or AgentType-only requirement
then survives a wider policy, and a policy-authored bit is never silently
dropped. Add a Task-only / AgentType-only / neither matrix asserting that the
plan accessor, the obligation set and the digest agree, including a direct
compile where every authored policy bit is false and only the Task requirement
asks for isolation, plus a source-only case where the policy bit is the only
carrier. A plan whose policy bit is true without a matching guarantee must be
unreachable under that composition; a defensive invariant rejects it if a future
change breaks the disjunction.

**Status.** Open in the record. Proposed in PR #34, stacked on PR #31; this
record does not count the finding as closed until that pull request merges.
Blocks B5.4's committed execution request, not the current acquisition gate.

### P1-4 — the compile cache key does not carry the guarantee identity

**Normative sentence.** ADR-0014 §4 lists the compile cache key components and
requires that a compile proof "is never reused across a changed profile or
binding", and that "no cache key is allowed to make a stale disposition look
current". `ENFORCEMENT_COMPILER_PROTOCOL`'s own contract adds the same rule for
compiler semantics: a semantic change cannot reuse an old result even when the
inputs are byte-identical.

**What is implemented.** `CompileCacheKey` commits to the effective-policy
digest, both derivation versions, the adapter kind, the adapter binding key, the
enforcement-profile digest and the compiler protocol version
(`enforcement.rs:406–420`, constructed at `805–813`). After the P1-3 fix, the
*plan* digest also carries the composed guarantee
(`enforcement.rs:832` in PR #34). So plan identity and key identity diverged:
the plan is guarantee-distinguished, the memoization key is not.

**What is missing.** No key component carries the guarantee, the obligation set,
or any digest of either. The Task-only counterexample from P1-3 is also a
counterexample here, because it changes no keyed input:

```text
baseline  : policy bits false, no Task/AgentType requirement
task-only : policy bits false, Task requires isolation

effective_policy_digest     equal      (the policy bytes are identical)
normalization_version       equal
meet_algorithm_version      equal
adapter_kind / binding_key  equal
enforcement_profile_digest  equal
compiler_protocol_version   equal
obligations                 differ     (ATTEMPT_ISOLATION present only in task-only)
plan_digest                 differ
cache_key                   equal      <- the gap
```

Confirmed by a temporary assertion inside the isolation matrix —
`assert_eq!(baseline.cache_key(), task.cache_key())` passed while
`assert_ne!(baseline.plan().plan_digest(), task.plan().plan_digest())` also
passed — and removed again; the fixed matrix now asserts the opposite and that
the policy bytes are unchanged.

The consequence is not a wrong plan, it is a wrong *lookup*: any memoization
layer keyed exactly as ADR-0014 §4 specifies would return a result compiled
without `ATTEMPT_ISOLATION` for a plan whose obligations require it, and nothing
in the compile path would detect the substitution. The finding is latent only
because no production path memoizes today and `qualify_security_candidate` still
has no caller.

There is a second-order form of the same gap: the seven-part list was frozen
before the guarantee existed, so a future guarantee added to the obligation
relation would inherit the identical defect unless the key commits to the
guarantee *document* rather than to today's single boolean.

**Required fix.** Make the guarantee identity an explicit key component (a
canonical, schema-versioned digest of `EffectiveExecutionGuarantees`), amend the
ADR-0014 §4 list, and bump `ENFORCEMENT_COMPILER_PROTOCOL` to
`agentype-enforcement-compile/2`: a `/1` result carries neither the guarantee
nor its digest and MUST NOT be reused. The acceptance test is the P1-3 matrix
extended with `assert_ne!(baseline.cache_key(), task.cache_key())` plus the
assertion that the effective policies are still equal, so a regression cannot be
"satisfied" by changing policy bytes instead.

**Status.** Open in the record. Proposed in PR #34, stacked on PR #31; the
normative key-list amendment is in the P2-3 ADR pull request (#32). This record
counts neither as closure before both merge. Blocks the ADR-0014 §4 memoization
claim and any B5.2 acquisition cache, not the current acquisition gate.

### P2-1 — the profile is never bound to its adapter binding or its subject

Two distinct omissions share one cause: `ImportedEnforcementProfile` is a bag of
facts with no relation to the physical domain it was imported for.

- **Binding identity.** The type stores `reference`, `protocol_version`,
  `support`, `covered_subjects` and `profile_digest`
  (`enforcement.rs:296–302`); `import` (`304–334`) validates only a non-zero
  protocol and a non-empty subject set. The compiler writes the passed
  `(adapter_kind, binding_ref)` into the cache key and evidence subject, but
  nothing proves the support matrix was imported for that pair, so a profile
  imported for binding A can be compiled against binding B.
- **Executing subject.** `compile_execution_security` never calls
  `profile.covers_subject(...)`, and `EnforcementSubject::is_inside_proven_domain`
  has no caller anywhere in the workspace. A profile that names only
  `UnprovenExternalProcess` therefore passes every current check for a
  controlled primary process.

**Required fix.** Carry the import-time `(adapter_kind, adapter_binding_key)` and
the required `EnforcementSubject` as qualification inputs; reject when the
subject is not covered or the binding identity differs; and keep "compiled"
(`CompiledExecutionSecurity`) distinct from "subject proven" so a caller cannot
read a successful compile as an applied-security claim. The existing gap about
`binding.policy_ref` versus `SpawnSource.adapter_policy` (see
[Known gaps](#known-gaps-for-the-reviewer)) is the same family and should be
closed by the same change.

**Acceptance test.** Cross-binding swap rejected; only-`UnprovenExternalProcess`
profile rejected for `ControlledPrimaryProcess`; matching profile accepted.

**Landed shape (revision 7, code `decb13a`).** The fix keeps the three facts
separate but related:

- `ImportedEnforcementProfile` carries `(adapter_kind, binding_ref)` and commits
  to them in its profile digest, so "imported for binding A" is a property of the
  imported fact, not a claim made at check time.
- `ValidatedSourceBinding::validate(source, binding, profile)` is the only
  constructor of the compiler's physical input. It rejects an inactive source, an
  inactive binding, a binding whose `policy_ref` is not the source's
  `adapter_policy`, and a profile imported for another binding.
- `compile_execution_security(authority, &ValidatedSourceBinding)` additionally
  rejects a tuple whose source revision is not the authority's, then compiles.
  `CompiledExecutionSecurity` carries the covered subject set but has no
  `subject()` accessor: `prove_subject(subject)` is the only way to obtain
  `ProvenExecutionSecurity`, so a successful compile cannot be read as an
  applied-security claim.
- `qualify_security_candidate` takes the tuple and the subject and returns the
  proven value; `with_source_config` completes the evidence subject on it.
- `import` rejects a policy-only reference profile that names a subject outside
  the proven domain. That gives `is_inside_proven_domain` its caller and states
  ADR-0014 §14 at the import boundary instead of leaving the escape hatch to a
  wider subject set on the same protocol.

**Version consequence.** `ENFORCEMENT_PROFILE_SCHEMA_VERSION` is `/2`. The plan
schema, the eight-part compile cache key and `ENFORCEMENT_COMPILER_PROTOCOL`
are unchanged **by P2-1**: the plan's inputs are the same three facts, now
related. (Revision 8 changes the protocol tag for an unrelated reason: the
credential obligation relation changed.) The
profile digest is an input to the evidence subject, the plan digest, the cache
key and the durable `binding_profile_digest`, so all of those change value.
Nothing durable stores a profile digest as a typed identity today (the block
table keeps it as opaque text), so no migration exists to perform; a stored
block written by an older build would simply be a different condition. Tests are
named in the claim table below. This is a proposed fix in an unmerged pull
request, not a closure.

**Revision 11 correction — the identity is the resolved key, not the alias.** The
revision-7 shape above compared the profile with
`AdapterBindingPolicy.binding_ref`. That string is only B.4's stable operator
alias: the resolver resolves it through `resolve_binding_ref` to an exact
`AdapterBindingKey`, and the Execution and its `BindingSnapshot` freeze the key
(ADR-0008). A profile imported for physical domain `k1` could therefore be
compiled against a selection whose policy still carries the alias `primary` but
which resolved to `k2`. The correction keeps the alias as the source-policy input
and adds the resolved key as the shared identity:

- `ImportedEnforcementProfile` records and digests `(adapter_kind,
  adapter_binding_key)`. `profile_digest` uses the physical key.
- `ValidatedSourceBinding::validate(source, binding, adapter_binding_key,
  profile)` takes the resolver's resolved key, rejects an empty key, and requires
  the profile to name that same key. It cannot itself re-run resolution; the
  authority transaction separately proves
  `evidence.adapter_binding_key == selection.adapter_binding_key` and passes that
  durable selection key in.
- `compile_execution_security` writes `domain.adapter_binding_key()` into the
  `CompileCacheKey` and the `SecurityEvidenceSubject`, so neither commits to the
  alias.
- `ENFORCEMENT_PROFILE_SCHEMA_VERSION` moves `/2 -> /3`. The plan schema and
  compiler protocol are unchanged by this correction.
- Tests: `an_alias_resolving_to_the_profiles_physical_domain_is_accepted`
  (`primary -> k2`, asserts the cache key and subject carry `k2`), the negative
  `a_profile_from_another_physical_domain_under_the_same_alias_is_rejected`, and
  the storage-level `the_binding_alias_resolves_to_the_selections_physical_key`,
  which proves the acquisition transaction passes the selection key rather than
  the policy alias.

This closes the *static* alias/key gap the audit named. It does **not** implement
the trusted importer identity the same audit separates for B5.2-C/B5.4: `import`
still accepts the caller's claims once they are key-bound and inside the fixed
reference ceiling. P2-1 is implemented but not fully closed, and the B5.4 handoff
below still applies.

### P2-2 — an authorized credential is treated as a requested delivery

`RequiredEnforcementFeatures::derive` (`enforcement.rs:223–225`) inserts
`CredentialDelivery` whenever `policy.credentials()` is non-empty. That set is
the AgentType ceiling's *authorization*, not the selected SourceConfig's request:
`CredentialRef` lives in SourceConfig (ADR-0013 §5), and the compiler has no
SourceConfig until `with_source_config` (`enforcement.rs:604–613`), which runs
after compilation.

The same file is internally inconsistent about grant presence:
`PolicyAccess::from_policy` (`sandbox.rs:2252–2260`) drops grants whose facet is
empty and records that such a grant "authorizes nothing and MUST NOT satisfy a
credential obligation", while `derive` treats the mere presence of any grant as
requiring delivery.

**Consequence and direction.** This over-rejects: a policy-only profile is
refused even when the selected config requests no credential. It cannot
under-enforce. The fix belongs where the config request becomes exact: split the
authorized set from the requested set before deriving `CredentialDelivery`
(B5.3 config resolution), or accept an optional request in the compiler input
now. Acceptance tests: authorized but empty `credential_refs` derives no
delivery obligation; authorized and requested but unsupported is
`SecurityUnenforceable`.

**Landed shape (revision 8, code `28d6592`).** Because ADR-0014 §3 places the
compile before `SourceConfig` attest, the request is genuinely not a compile-time
fact, so the second option is implemented as a completion-time check rather than
a new compiler parameter:

- `RequiredEnforcementFeatures::derive` no longer inserts `CREDENTIAL_DELIVERY`
  at all. The policy's credential map stays the authorization ceiling; an
  omitted grant authorizes nothing, which is a grant question. The derivation no
  longer contradicts `PolicyAccess::from_policy`.
- `CompiledExecutionSecurity` keeps the profile's delivery capability, and
  `with_source_config` — the step that first sees the exact attested config —
  rejects a non-empty `credential_refs` when the profile cannot deliver. The
  result is candidate-local `SecurityUnenforceable`, not a Task failure.
- An empty request adds no obligation and leaves the policy-only profile
  eligible; a request under a delivering profile completes; a request under the
  reference profile is ineligible.

Revision 9 is where that completion-time check first runs on an authority path:
`acquire_security_v1_task_existing` binds the attested config before it creates
the Attempt/Lease, so a request the profile cannot deliver is refused while the
Task is still `Queued`.

Revision 10 corrects the authority-path reading of that check: it is necessary but
not sufficient, because a profile **claim** can satisfy it. Every non-empty
`credential_refs` is now refused at the acquisition entry, before the compile
settlement, until B5.3 supplies the canonical `CredentialRequest`, the
`CredentialEligibilityEvidence` and broker qualification. The two pure-component
tests above still pass, so "the profile can deliver" and "this request is
authorized" remain distinct facts. See the continuation re-audit's P1-2.

**Version consequence.** `ENFORCEMENT_COMPILER_PROTOCOL` moves `/2 -> /3`: the
obligations derived from a credential-bearing policy changed, and the protocol
tag is the guard that prevents a `/2` result being reused. The plan schema, the
eight-part cache key, `ENFORCEMENT_PROFILE_SCHEMA_VERSION` and every durable
encoding are unchanged. The canonical `CredentialRequest` (refs + use + scope +
digest) remains B5.3; this fix only consumes the request-presence fact the exact
config already owns. Tests are named in the claim table. This is a proposed fix
in an unmerged pull request, not a closure.

### P2-3 — durable-block idempotence is per-row, not per-history

`security_acquisition_blocks` is keyed by `task_id TEXT PRIMARY KEY`
(`schema.rs:1293–1294`), so a Task holds exactly one diagnostic.
`record_security_acquisition_block` replaces that row's fields and clears
`released_at`/`release_reason` when the condition differs
(`security_acquisition.rs:92–116`). The `no_same_condition_reopen` trigger only
compares `OLD` and `NEW` of the one stored row.

Therefore this sequence is accepted:

```text
condition A  -> BLOCKED
release A    -> RELEASED
condition B  -> BLOCKED (A's row replaced)
condition A  -> BLOCKED (B's row replaced; the RELEASED A is gone)
```

Revision 1's wording — "re-recording is a no-op even after release" — is true
only for the condition currently stored. Two resolutions were acceptable: keep
per-condition immutable history plus a current pointer, or narrow the ADR-0014 §4
sentence and this record to "the currently stored condition is never reopened".

**Resolved by narrowing (revision 3).** ADR-0014 §4 now states the rule in the
narrow form: a Task holds one diagnostic whose identity is the currently stored
condition, a stored released condition cannot resurrect itself, and the encoding
promises no cross-history idempotence. The `A → release A → B → A` sequence above
is therefore **specified behaviour, not a defect**: re-observing A after B
replaced it starts a new `BLOCKED` diagnostic and is not a retry of the released
A. No history table, schema bump or code change was added: the narrowed rule is
now specified, and the diagnostic still has no production writer.

**Status.** Closed by this revision, as a specification narrowing. The existing
`b5_acquisition_block::a_released_condition_is_never_reopened` test already pins
the stored-row rule, so no new regression is owed by this decision. If the
diagnostic ever gains a production writer, the `A → release A → B → A` sequence
should be pinned by a test so the narrowed semantics cannot drift back.

### P2-4 — preselection is done, acquisition qualification is not

`can_provision` / `can_provision_task` (`predicates.rs:381–649`) are unchanged
and still compare the Task's fine policy against the AgentType's by exact
reference, which is the `COARSE_V1` relation. `qualify_security_candidate` is a
new, independent, pure predicate with no caller outside tests and the `no_run`
doctest. Production acquisition rejects `SECURITY_V1` at
`provisioning.rs:734–741` (called at `1069` and `1342`).

This is conservative and correct for the current build, but it means the fourth
B5.1 handoff item — "`can_provision` / acquisition uses the same complete
qualification for `SECURITY_V1`" — is **not** met. Candidate preselection being
correct must not be read as acquisition qualification being complete.

**Landed shape (revision 9, code `d4d8dc3`).** Two acquisition entry points now
exist, and they are disjoint:

- `acquire_typed_task_existing` / `acquire_typed_task_new_agent` keep the frozen
  `COARSE_V1` relation (`can_execute` + `can_provision_task`) and the B5.1 gates.
  A `SECURITY_V1` Task, AgentType or source is still refused there, and a
  credential-bearing config still fails closed.
- `acquire_security_v1_task_existing` takes a
  `SecurityProvisioningSelection { provisioning, profile, subject }`. It refuses
  a `COARSE_V1` Task, AgentType or source, then re-loads every durable input the
  composite authority needs (Task security binding, requirement, the actual
  AgentType revision with its mode and authored policy read in one validated
  pass, source revision, its security declaration and authored `P_source`).
  Before deriving authority it proves the frozen B.4 provisioning conjunct
  `can_provision_task_source_config` (revision 10, P1-1), then derives
  `EffectiveTaskAuthority`, validates the caller's imported profile with
  `ValidatedSourceBinding::validate(source, durable_binding, profile)`, calls
  `qualify_security_candidate`, and completes the evidence subject with
  `with_source_config`. Only then does it run the shared Attempt/Lease and
  `ProvisioningBinding` tail.
- The credential request is already fail-closed at the entry (revision 10,
  P1-2): no credential-bearing config reaches the Attempt/Lease tail until B5.3.
  `with_source_config` therefore only binds the exact config and re-checks the
  profile's delivery capability as a defensive necessary condition.
- The only caller-supplied facts with no durable row are the imported profile and
  the requested subject. The profile is re-checked for subject, version and
  validity; the binding, source, config and evidence are re-read from the catalog
  rather than trusted from the caller. A qualification failure is
  `InvalidAuthority`, writes nothing, and leaves the Task `Queued`.
- The compiled proof is discarded (`_proven`). Persisting a security plan or
  snapshot, and observing a secured start, remain B5.4; this transaction grants
  only the mechanical Attempt/Lease the coarse path also grants.

**Status (revision 10).** *Selected-candidate transactional qualification
implemented; complete `SECURITY_V1` acquisition orchestration remains open.* One
qualified candidate is not a deterministic global winner: there is no complete
`SECURITY_V1` candidate enumeration, no whole-round compile budget or defer
wiring, no broker observation epoch, no `SECURITY_V1` new-agent acquisition and
no resolver→scheduler production wiring. `catalog_frontier_digest` fences the
source/config frontier; it does not prove the full `SECURITY_V1` candidate set.
The work is implemented in draft PR #37 and is **not closed** until that pull
request merges and the user's independent re-audit accepts it. The Gate row below
records the acceptance tests; the mutation table in
[Verification](#verification) records that each new guard is load-bearing.

### P2-5 — a durable block does not re-prove its over-budget truth

**Normative sentence.** ADR-0014 §4 makes the block the durable diagnostic of a
*whole round* that cannot fit the fixed acquisition budget; the only producer of
the diagnostic is `AcquisitionBudget::require_whole_round`.

**What is sound.** `SecurityAcquisitionBlock::from_budget_defer`
(`enforcement.rs:1075–1113`) consumes a `BudgetExhaustion`. Its fields are private
and there is no public constructor, so the in-memory producer path can only carry
a real exhaustion produced by `require_whole_round` (`enforcement.rs:964–982`).

**What is missing.** `SecurityAcquisitionBlock::from_durable_parts`
(`enforcement.rs:1115–1160`) validates only non-empty key digests and the
release-field shape. It does not recompute whether `cost` exceeds `budget`, and it
does not require the stored `limit` to be exactly the exceeded bound.
`record_security_acquisition_block` (`security_acquisition.rs:37–118`) writes the
object's fields without re-checking either. This record is therefore accepted:

```text
cost_candidates = 0, cost_compiles = 0
budget_max_candidates = 10, budget_max_compiles = 10
budget_limit = CANDIDATE_COUNT
status = BLOCKED
```

**Why it is a real defect.** It cannot create an Attempt or a Lease, but it makes
the "operator-actionable budget diagnostic" untrue: the validated read reports a
legal block for a round that never exceeded any budget. A corrupted or
hand-written row is indistinguishable from a real defer, and the write boundary
does not reject a forged value object.

**Required fix.** Share one validation function between `from_durable_parts` and
`record_security_acquisition_block`: recompute the exhaustion through
`require_whole_round`, require it to fail, and require the failure's limit to
equal the stored limit. Reject the row as `InvariantViolation` otherwise. Add a
SQL corruption test: insert a legal blocked row, lower its recorded cost below the
budget with a raw `UPDATE`, and assert the validated read returns an invariant
failure rather than a block.

**Status.** Open in the record. Proposed in PR #34, stacked on PR #31, which also
validates the two timestamps; this record does not count the finding as closed
until that pull request merges. This is a truth gap in the diagnostic, not an
execution-authority bypass.

### A — a source declaration is not bound to its `SpawnSource` revision

**Normative sentence.** ADR-0013 §5: `P_eff` is the meet of the four authority
terms, and the source term is the declared ceiling of one exact source revision.
The declaration is the durable form of that term.

**What is missing.** `EffectiveTaskAuthority::new` (`authority.rs:250–344`) takes
`source: &SpawnSource`, `source_declaration: &SpawnSourceSecurityDeclaration` and
`source_policy: Option<&NormalizedSandboxPolicy>` as independent parameters. The
declaration carries only `source_policy` and `enforcement_profile`
(`sandbox.rs:2524–2527`) and no `SpawnSourceRef`, so the constructor can prove
that the declaration's policy digest matches the supplied policy bytes but not
that the declaration belongs to the supplied source. This composes:

```text
source A      P_source = restrictive
source B      P_source = Top

new(source = A, declaration = B, source_policy = None)
```

The `(None, None)` arm matches, so A's authored ceiling never enters the meet.
The mismatch is worse than the ceiling alone: `enforcement_profile` is also read
from the declaration, so B's declared profile replaces A's while
`spawn_source_ref` still records A.

**Reachability.** No production acquisition calls the constructor, and the
storage reader keys the declaration on `(source id, revision)`
(`catalog.rs:2006–2023`), so the mismatch is not reachable through the validated
read. This is a provenance gap in a public constructor, not a live escalation.

**Required fix.** Bind the declaration to its revision: carry the
`SpawnSourceRef` in the declaration, check it in `EffectiveTaskAuthority::new`
before the policy/digest match, and check it at `insert_spawn_source_declaration`
so a foreign declaration cannot occupy the `(source id, revision)` row either.
The declaration has no canonical JSON encoding and no digest, so no format,
schema or version constant changes.

**Status.** Open in this record; fixed in the code pull request whose head is
`9b8d830`. Tests: `b5_authority::the_source_layer_must_match_its_own_declaration`
(foreign-revision case) and
`b5_policy_catalog::a_source_declaration_cannot_be_published_under_another_revision`.
The `ValidatedSourceSecurityRevision` composite read that the audit suggested is
not introduced; the invariant is enforced at both boundaries, and the composite
read remains the natural vehicle for P2-1.

### B — the durable block's validated read is incomplete

**Normative sentence.** ADR-0014 §4 makes the block the durable diagnostic of a
complete round whose candidate set or compile cost exceeds the fixed budget,
bound to the blocked Task and to its durable dispatch generation.

**What is missing.** Three gaps at the two sides of one row:

1. `get_security_acquisition_block` (`security_acquisition.rs:127–149`) passed
   the SQL row to `RawBlock::into_block` and never compared the recorded
   `generation_id` with the Task's `GenerationTaskBinding`. The write path does
   compare it, so a raw `UPDATE` of that column is invisible to the validated
   read: the row rebuilds into a legal-looking block that names a Generation the
   Task is not bound to.
2. `validate_durable_truth` never related `candidate_count` to `cost_candidates`.
   `CandidateDomain` holds the digest and the count as independent fields, so a
   row can describe a domain of N while recording M < N attempted candidates and
   still read back as legal.
3. The same-condition arm of `record_security_acquisition_block` compared the two
   key digests and returned success. A row corrupted outside those two columns is
   therefore reported as a current diagnostic and is never healed, even though
   the same write carries a sound value object.

**Why the read-time mode re-check is not one of the gaps.** See
[the write-once premise](#task-security-mode-is-write-once-premise-not-a-finding).

**Required fix.** Require `cost.candidates() == domain.candidate_count()` in
`validate_durable_truth` — a whole-round defer attempts the complete set, so a
partial candidate cost is not a budget block; re-bind the generation in the read
path; and rebuild the existing row through the durable constructor before a
same-condition record returns.

**Status.** Open in this record; fixed in the code pull request whose head is
`9b8d830`. Tests:
`b5_enforcement::a_durable_block_must_record_the_complete_domain_as_its_candidate_cost`,
`b5_acquisition_block::a_validated_read_rejects_a_block_that_names_another_generation`,
`a_validated_read_rejects_a_block_whose_candidate_cost_is_incomplete` and
`a_same_condition_write_revalidates_the_existing_row`.

### Task security mode is write-once (premise, not a finding)

The audit's third sub-claim — that the block read should also re-check that the
Task's current durable mode is `SECURITY_V1` — does not hold, and no check was
added. `task_agent_requirements.security_contract_mode` is written by exactly one
`INSERT` (`requirement.rs:115`) and there is no `UPDATE` of that table anywhere in
storage; `frontier.rs:1156` rejects a mode change on a committed Task. The mode
is therefore immutable per Task, and the insert-time parent rule already admits
only a `SECURITY_V1` Task. A read-time re-check would restate the trigger and add
no guarantee, so it is recorded here as a premise instead.

### Candidate-domain identity is a B5.3 design item

`CandidateDomain::from_ids` takes only `LogicalAgentId` (`enforcement.rs:994`).
A complete round over unmaterialised `SpawnSource × SourceConfig` candidates
cannot be expressed with that identity. This is a design question, not a defect
in the current diagnostic. Revision 9's acquisition wiring does not change it:
`acquire_security_v1_task_existing` still requires an existing, READY,
type-bound `LogicalAgent`, and there is no security counterpart to
`acquire_typed_task_new_agent`. The domain therefore only ever holds existing
agents. The unified candidate identity model must be settled before a round can
materialise a security candidate; this record does not invent it.

### Positive correction from the re-audit

The B5.1 audit found that Task→Generation security-mode agreement was checked in
one direction only. `verify_generation_mode_agreement`
(`requirement.rs:295–309`) now compares the stored Task mode against the
Generation's mode and rejects any difference, so the Task reads cannot present a
`SECURITY_V1` Generation as a dispatchable coarse Task. That B5.1 item is closed.

## Continuation re-audit (revision 10)

The continuation audit accepted the design direction of P2-1, P2-2 and P2-4 and
every "what the draft did right" item, but found two P1 defects in the new
authority path and rejected both direct merge and any B5.2 freeze claim. Both are
fixed in `0a0ed56`; neither is counted as closed before the pull request merges
and the user re-audits it.

### P1-1 — the security branch replaced B.4 eligibility instead of intersecting it

`finish_acquisition`'s `AcquisitionMode::SecurityV1` arm proved the new policy
algebra, the validated binding/profile tuple, coverage, the compile and the
subject, but it did **not** run `can_provision_task` — the frozen B.4 predicate
that also checks the `SourceConfig` ACTIVE disposition, source/config ownership,
the evidence subject/config digest, AgentType lifecycle, continuity, the
functional envelope, security-class capability evidence and the Task restriction
join. A `SECURITY_V1` Task with a `DISABLED` config, evidence resolved for
another config digest, a missing functional capability, or an unsatisfiable
continuity could therefore reach the shared Attempt/Lease tail.

The defect is compositional, not a policy-algebra error: the new layer must
**tighten** the frozen control plane, never omit its necessary conditions.

**Landed shape.** `can_provision_task_source_config` is the extracted reusable
B.4 conjunct. `can_provision_task` is now `can_execute` plus that conjunct, so the
coarse path is byte-for-byte the same relation. The security path calls the
conjunct **before** deriving the composite authority, then adds policy order, the
positive access floor, coverage, compile and subject proof. Exactly two
coarse-only relations are replaced, both by stronger proofs: the exact
Task↔AgentType policy reference (`can_execute` → `can_execute_security_v1`) and
the "evidence claims this exact authored policy reference" claim
(`can_provision` → the obligation-coverage proof of
`compile_execution_security`). Every other conjunct is shared.

Acceptance tests (all in `b5_security_acquisition`): evidence for another config
digest, a missing functional capability, unsatisfiable AgentType continuity and
lifecycle, and a `DISABLED` config with a **refreshed frontier** (so the
stale-frontier fence is not what rejects it). The contract-level
`the_provisioning_conjunct_is_shared_by_both_paths` pins the same matrix, plus
the deliberate difference: it accepts evidence that does not list the authored
policy reference (the security compile replaces that claim), while the coarse
predicate still refuses it. Each guard in that matrix was mutation-checked at
`0a0ed56`; the table is in [Verification](#verification).

### P1-2 — delivery capability was read as a credential authorization

`CompiledExecutionSecurity::with_source_config` refused a request only when the
profile did **not** claim `CredentialDelivery`. That correctly separated the
authorization ceiling from the delivery request (P2-2), but it also meant:

```text
P_eff.credentials = {}
SourceConfig.credential_refs = ["vault://codex-prod"]
profile.support includes CredentialDelivery
→ with_source_config passes
→ SECURITY_V1 acquisition grants Attempt/Lease
```

No canonical `CredentialRequest`, `CredentialEligibilityEvidence`, scope/audience/
use check or broker qualification exists yet. B5.3 being unimplemented does not
make broker eligibility satisfied, and ADR-0015 §5/§9 make credential
authorization positive and deny-by-default.

**Landed shape.** The pure component keeps the delivery-capability check as a
necessary condition for B5.3, so `a_requested_credential_needs_a_profile_that_can_deliver`
and `a_requested_credential_completes_when_the_profile_delivers` still pass. The
authority-bearing entry is fail-closed for **any** non-empty `credential_refs`,
independent of the profile claim, and rejects as `InvalidAuthority` before the
Attempt/Lease tail. The dangerous direction is now the pinned negative:
`a_credential_bearing_config_is_fail_closed_even_when_the_profile_can_deliver`.
Mutation-checking the gate back to "coarse-only" makes exactly that test fail, and
the candidate acquires.

### P2 handoff constraints for B5.4

Two boundary facts are recorded as hard B5.4 conditions, not defects in B5.2:

- **Subject identity is not in the evidence digest.**
  `SecurityEvidenceSubject::digest()` commits to the profile identity, not to the
  chosen `EnforcementSubject`, so two subject proofs over one profile share a
  digest. That is sufficient for candidate qualification (the acquisition
  re-checks the caller's subject and persists no proof), but B5.4 must define
  `ExecutionSecurityEvidence = (PlanIdentity, ProfileIdentity, SubjectIdentity)`
  or a versioned `subject_proof_digest`, and must not treat the existing digest as
  full physical subject proof.
- **`SelfConsistentProfile ≠ TrustedPhysicalEvidence`.** `import` accepts the
  caller's support matrix and covered subjects. The code proves consistency with
  the declared `(adapter_kind, adapter_binding_key)` and, for the policy-only
  protocol, the proven subject domain and the fixed support ceiling; it does not
  prove that a real trusted integration enforces those restrictions. Trusted importer identity, exact domain ownership,
  profile freshness and actual subject attestation are the B5.2→B5.4 handoff
  conditions. Constructor visibility must not be reintroduced as the security
  boundary (cf. #31).

## Identity and capability re-audit (revision 11)

The next audit re-read `015220a` and accepted the P1-1/P1-2 fixes, the B.4
intersection, the two disjoint acquisition modes, the root-relative compile and
the selected-candidate transaction. It found two new defects in the identity and
capability proofs, both fixed in `39e85c5`; neither is counted as B5.2 closure
before merge and re-audit.

### P1 — the profile was bound to the alias, not the physical adapter key

B.4 deliberately distinguishes two identities:

```text
AdapterBindingPolicy.binding_ref = "primary"      stable operator alias
        ↓ resolve_binding_ref(kind, alias)
ImportedAdapterBinding.adapter_binding_key = "k2" exact physical domain
        ↓
ResolvedProvisioningSelection.adapter_binding_key = "k2"
```

The resolver and the authority transaction already carried the physical key, and
`finish_acquisition` proved `evidence.adapter_binding_key ==
selection.adapter_binding_key`. But revision 7's
`ValidatedSourceBinding::validate` compared `profile.binding_ref()` with
`binding.binding_ref`. Nothing related those two comparisons to each other, so a
profile imported for physical domain K1 could pass validation for a selection
resolved to K2 through the same alias. The existing tests mostly used
`binding_ref == adapter_binding_key == "k1"`, so they could not see the split.

**Landed shape.** `ImportedEnforcementProfile` records the resolved
`(adapter_kind, adapter_binding_key)` and commits to it in `profile_digest`.
`ValidatedSourceBinding::validate(source, binding, adapter_binding_key, profile)`
requires the resolver's resolved key, rejects an empty key, and requires the
profile to name it. `ValidatedSourceBinding` stores the key; the compiler writes
it into the cache key and the evidence subject. `ENFORCEMENT_PROFILE_SCHEMA_VERSION`
is `/3`. The alias stays where B.4 puts it: in the source's
`AdapterBindingPolicy`, resolved by the resolver, never in the compiled physical
identity.

**Acceptance tests.** In `b5_enforcement`:
`an_alias_resolving_to_the_profiles_physical_domain_is_accepted` uses
`binding_ref = "primary"` and physical key `"k2"`, and asserts the cache key and
evidence subject both carry `"k2"`;
`a_profile_from_another_physical_domain_under_the_same_alias_is_rejected` swaps
in a profile imported for `"k1"` and requires `EvidenceSubjectMismatch`. In
`b5_security_acquisition`,
`the_binding_alias_resolves_to_the_selections_physical_key` runs the real
authority transaction with `primary -> k2` and asserts the durable
`ProvisioningBinding` freezes `"k2"`. Mutation checks at `39e85c5`: comparing
only the profile's adapter kind, passing the policy alias at the call site, or
writing the alias into either the cache key or the subject each fails the
expected test.

**Status.** This closes the *binding-identity comparison* defect. It is not a
claim that P2-1 is fully closed: trusted importer identity, profile freshness and
actual subject attestation are still B5.2-C/B5.4 handoffs, and the rest of the
audit's B5.2-C/D/E decomposition below is not implemented.

### P2 — the fixed reference profile could widen its support matrix

`ImportedEnforcementProfile::import` accepted any caller `EnforcementSupport` for
the exact `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` identity. It already rejected a
subject outside the proven domain, but not a capability outside the fixed claim.
A profile could therefore declare `NetworkEgressRestriction` under the reference
identity, and the compiler would then treat a network obligation as covered even
though ADR-0014 §14 explicitly does not claim network egress restriction.

**Landed shape.** For the exact reference identity (all of protocol, profile name
and version), `import` enforces

```text
Support_imported ⊆ Support_reference_profile
```

and rejects an over-claim as `InvalidSandboxPolicy` before any compiler sees it.
A stricter subset remains valid. A profile that needs an additional dimension
must use a new versioned identity; reusing the frozen one with a wider matrix is
not an upgrade path. `the_frozen_reference_profile_cannot_widen_its_support_matrix`
pins both directions (network over-claim rejected, workspace-only subset
accepted); disabling the ceiling fails that test.

### Remaining B5.2 boundaries the audit named

The audit's three-slice decomposition is recorded here as the follow-up shape.
Revision 11 implements only the static alias/physical-key and reference-ceiling
parts of B5.2-C; everything else in the table is not work this revision
performed:

| Slice | Work | Gate |
|---|---|---|
| B5.2-C | Alias/physical-key binding, the fixed reference-profile ceiling and trusted importer provenance | no cross-physical-domain reuse; no widening of the reference profile |
| B5.2-D | Complete candidate identity, full enumeration, compile budget, cache, automatic durable block and defer | an unknown candidate or exhausted budget cannot partially select |
| B5.2-E | Resolver→Scheduler production wiring, full candidate revalidation, existing/new-agent handling | every qualification proof closes before Attempt/Lease; no non-security start path |

Two boundaries are explicitly **not** moved into B5.4:

- **Trusted import.** B5.2 must distinguish *a profile that is self-consistent
  and key-bound* from *evidence from a trusted exact physical binding*. Revision
  11 improves the former; it does not implement the trusted importer identity,
  evidence version or freshness proof. That proof belongs to B5.2-C.
- **Applied receipt.** Whether the OS restrictions were actually applied to an
  Execution instance, and the recoverable receipt for that, remains B5.4.

`CandidateDomain::from_ids` still counts `LogicalAgentId` only. The audit records
complete candidate identity as a **B5.2 remaining-design item**, not a B5.3
deferral: a candidate is `LogicalAgent + exact Source/Config + physical binding`
for existing agents, or `pinned AgentType + exact Source/Config + physical
binding` for new ones. Full enumeration and a whole-round budget must use that
unit before B5.2-E can be claimed. B5.3 may add the broker observation epoch and
credential eligibility, but does not redefine B5.2's general candidate identity.

Finally, wiring B5.2-E into the ordinary physical start path requires its own
feature gate and must not happen before B5.4's secured start: the current
`acquire_security_v1_task_existing` creates a real Attempt/Lease but persists no
`ExecutionSecuritySnapshot`, and `SecurityEvidenceSubject::digest()` still does
not include the chosen `EnforcementSubject`.

## Conditional-acceptance review (revision 12)

An external review of `181c9e0` accepted PR #37 as a continuing B5.2
implementation draft, not as a B5.2 freeze. It confirmed the static alias/physical
key split, the import-time reference ceiling, the B.4 conjunct, the credential
fail-closed boundary and the single-candidate transaction, and agreed that
B5.2-C/D/E belong in separate follow-up pull requests rather than in this one.
The review found no new code defect in this draft. It added two hard conditions and
two ordering rules, none of which is implemented here.

### Hard condition 1 — trusted importer provenance is B5.2-C

`import` still derives its facts from the caller:

- `support`, `covered_subjects`, `adapter_kind` and `adapter_binding_key` are
  caller-supplied;
- the profile digest is a content-integrity identity, not a statement about who
  produced the profile or whether that producer can enforce the claims;
- the frozen reference identity now has a ceiling, but a differently named
  profile can still over-claim relative to its real adapter.

B5.2-C's completion definition is therefore:

- the profile comes from a trusted local importer, not from the Scheduler or the
  candidate caller;
- the importer protocol version and the exact physical binding identity are
  verifiable;
- support and subject claims are produced by the importer's real capability, not
  declared by the caller;
- profile freshness has explicit invalidation conditions;
- renaming a profile cannot be used to escape the reference-profile claim.

B5.2-C proves *what the physical domain can implement*. Whether the restrictions
were applied to one Execution remains B5.4. A self-consistent profile digest MUST
NOT be consumed by B5.4 as physical qualification evidence without this proof.

### Hard condition 2 — B5.2-E must not open a physical start before B5.4

`acquire_security_v1_task_existing` re-proves the durable inputs in one
transaction and then creates an Attempt/Lease/`ProvisioningBinding`, but it
discards the compiled plan. It is an authority-transaction prototype, not a
secured start.

The follow-up MUST separate two states:

| State | Meaning | Executable |
|---|---|---|
| `SECURED_QUALIFIED` | B5.2 qualification proof completed | **no** |
| `SECURED_EXECUTION_COMMITTED` | B5.4 snapshot and secured start protocol present | yes |

No ordinary M5 physical start path may run on `SECURED_QUALIFIED`. The review
accepts explicit types and a feature gate instead of a new durable enum, but an
Attempt/Lease MUST NOT be read as proof that secured execution is reachable.

### B5.2-D — freeze candidate identity first

The unified candidate identity is the first B5.2-D commit and the first acceptance
test: `(LogicalAgent, exact SourceConfig, physical binding)` for existing agents,
and `(pinned AgentType, exact SourceConfig, physical binding)` for new ones, with
the exact source revision and config digest included. `CandidateDomain::from_ids`
still counts only `LogicalAgentId`, so two configs or two physical bindings under
one agent can be merged today. Budget, cache and durable-block wiring MUST NOT
land before that identity is fixed.

Credential-free enumeration may land in B5.2-D, and B5.3 may add the broker
observation epoch and credential eligibility, but B5.3 MUST use the same complete
candidate resolution; an asynchronous or failed broker check MUST NOT degrade to
partial selection.

### B5.4 — the subject identity is part of the execution commitment

`ProvenExecutionSecurity` now requires the profile to cover the chosen
`EnforcementSubject`, but `SecurityEvidenceSubject::digest()` still commits to the
profile only, so two subjects over one profile share a digest. The review makes
the handoff non-optional:

```text
ExecutionSecurityEvidence = (PlanIdentity, ProfileIdentity, SubjectIdentity)
```

The subject identity MUST enter the persisted snapshot and the receipt integrity
chain, not merely a transaction-local argument. This is a B5.4 requirement and
does not block this draft.

### Follow-up acceptance cases the review asks to pin

| Case | Required result |
|---|---|
| One alias resolves to different physical keys | the old profile cannot be reused; qualification fails |
| A profile claims more than its real importer can do | no trusted evidence is produced |
| One LogicalAgent with two source/config/binding candidates | candidate count and domain digest distinguish both |
| One candidate's eligibility is unknown, the others qualify | no partial selection |
| `SECURITY_V1` qualification succeeds with B5.4 disabled | no entry into the old physical start path |

A resolution-epoch observation and the final authority-transaction revalidation
are separate: the latter MUST NOT pretend to read external importer or broker
mutable facts atomically inside SQLite.

### Disposition

PR #37 stays **draft**. Its implemented parts and the C/D/E decomposition are
accepted for continuation; the freeze is not. No B5.2-C/D/E implementation is
added to this draft.

## B5.1 handoff acceptance, re-checked

| Acceptance condition | Revision 2 |
|---|---|
| `match_existing_agents_for_task` selects the relation from the Task security mode | met |
| `more_specific_for_with_policies` uses the real policy order | met |
| A differing sandbox policy reference does not lose a legal existing-agent candidate | met, differential test |
| `can_provision` / acquisition uses the same complete qualification for `SECURITY_V1` | **met on the security chain from revision 9** (`d4d8dc3`), and completed in revision 10 (`0a0ed56`): `acquire_security_v1_task_existing` now intersects the frozen B.4 eligibility conjunct before it qualifies, and only then creates an Attempt/Lease. The coarse chain stays deliberately disjoint and still refuses `SECURITY_V1`, and the runtime resolver does not call the security entry point |

## Claim → code → test → commit

Each row is one normative sentence with the smallest code surface that
implements it and the test that would fail if it were removed. Rows marked
**(rev 2)** were added or corrected by the re-audit.

### ADR-0013 §5 — policy authority algebra and candidate relations

| Claim | Code | Test | Commit |
|---|---|---|---|
| Only a validated `SECURITY_V1` revision can carry Task security authority; a `COARSE_V1` revision is rejected, not degraded | `authority.rs` `SecurityRevision::new` / `is_security_v1` | `b5_authority::a_coarse_agent_revision_can_never_carry_security_v1_authority` | `a331a6a` |
| A security revision requires both a declared policy reference and the loaded authored policy; `COARSE_V1` ignores any supplied policy | `authority.rs` `SecurityRevision::new` | `b5_authority::a_security_revision_requires_its_authored_policy` | `a331a6a` |
| An unimplemented derivation/normalization/meet version is not authority | `authority.rs` `validate_binding_policy` → `SecurityDerivation::ensure_supported` | `b5_authority::an_unsupported_derivation_version_is_not_authority` | `a331a6a` |
| `P_eff = TaskAuthority(meet(P_binding, P_agent, P_source))`, with the Task's coarse modes applied last | `authority.rs` `EffectiveTaskAuthority::new` (lines 250–344) | `b5_authority::the_composite_folds_binding_agent_source_and_task_coarse_modes`, `a_write_task_keeps_only_the_intersection_of_all_three_layers` | `a331a6a` |
| A present source policy must be exactly the bytes its declaration names; presence must agree | `authority.rs` `EffectiveTaskAuthority::new` source-layer match | `b5_authority::the_source_layer_must_match_its_own_declaration` | `a331a6a` |
| **(rev 6)** A source declaration belongs to exactly one source revision; the constructor and the writer both reject a foreign declaration | `sandbox.rs` `SpawnSourceSecurityDeclaration`; `authority.rs` `EffectiveTaskAuthority::new`; `catalog.rs` `insert_spawn_source_declaration` | `b5_authority::the_source_layer_must_match_its_own_declaration`, `b5_policy_catalog::a_source_declaration_cannot_be_published_under_another_revision` | `9b8d830` |
| Coarse modes are re-checked on the complete `P_eff`, not reused from admission | `authority.rs` `validate_coarse_modes` | `b5_authority::the_composite_rechecks_the_task_coarse_modes_after_the_meet` | `a331a6a` |
| `TaskSecurityCeiling` is the candidate-independent `TaskAuthority(meet(P_generation, P_task))` with identity recorded | `authority.rs` `TaskSecurityCeiling::from_binding` | `b5_authority::the_task_ceiling_applies_the_coarse_modes_and_records_identity` | `a331a6a` |
| A legal candidate whose policy *reference* differs is ranked by the algebra, not by reference equality | `matching.rs` policy-order rank, `predicates.rs` policy-aware specificity | `b5_matching_order::policy_order_keeps_a_legal_candidate_whose_reference_differs`, `policy_aware_specificity_uses_the_algebra_not_the_reference` | `a331a6a` |
| Narrower policy bounds rank first; a non-bound is excluded | `predicates.rs` policy-order relation; `requirement.rs` candidate assembly | `b5_matching_order::policy_order_ranks_narrower_first_and_excludes_non_bounds` | `a331a6a` |
| A qualified security candidate must project to its own coarse contract | `matching.rs` `QualifiedExistingAgentCandidate` | `b5_matching_order::a_security_qualified_candidate_must_project_to_its_coarse_contract` | `a331a6a` |
| The candidate assembly uses policy order for `SECURITY_V1` and the frozen reference relation for `COARSE_V1` | `requirement.rs` candidate fold (`match_existing_agents_security_v1`); `catalog.rs` lookup | `b5_candidate_order::security_task_ranks_by_policy_order_and_excludes_coarse_candidates`, `coarse_task_keeps_the_frozen_reference_relation` | `a331a6a` |
| **(rev 2)** Every positive Task access obligation is re-proved on the four-term `P_eff` | **absent** — `qualify_security_candidate` never calls `check_required_access` | **absent** | — (P1-1) |

### ADR-0014 §2–§5 — compilation, order, budget, evidence

| Claim | Code | Test | Commit |
|---|---|---|---|
| Compilation is pure and root-relative: logical selectors, no directory, no I/O | `enforcement.rs` `compile_execution_security` | `b5_enforcement::compilation_returns_a_root_relative_plan_and_a_partial_subject` | `cbab05f` |
| The imported profile must be the one the source declared; otherwise subject mismatch | `compile_execution_security` first check | `b5_enforcement::a_different_imported_profile_is_a_subject_mismatch` | `cbab05f` |
| An inactive or malformed binding is never compiled | `ValidatedSourceBinding::validate` (revision 7 moved this from the compiler) | `b5_enforcement::an_inactive_binding_is_never_validated`, `a_binding_must_implement_the_sources_own_adapter_policy` | `cbab05f`, `decb13a` |
| Qualification re-proves the requirement digest, AgentType revision, agent policy digest, and `can_execute_security_v1` before compiling | `qualify_security_candidate` (lines 846–892) | `b5_enforcement::qualification_re_proves_the_requirement_and_agent_identity_before_compiling` | `cbab05f` |
| Qualification re-checks the Task's coarse modes on the four-term `P_eff` | `qualify_security_candidate` → `authority.validate_coarse_modes` | `b5_enforcement::qualification_rechecks_the_task_coarse_modes_on_the_complete_policy` | `cbab05f` |
| **(rev 2; implemented in revision 7, corrected in revision 11)** The profile was imported for this exact **physical** adapter domain and covers the executing subject | `ImportedEnforcementProfile` import-time `(adapter_kind, adapter_binding_key)` identity; `ValidatedSourceBinding::validate(source, binding, adapter_binding_key, profile)`; `CompiledExecutionSecurity::prove_subject`; `qualify_security_candidate` | `b5_enforcement::a_profile_imported_for_another_binding_is_a_subject_mismatch`, `an_alias_resolving_to_the_profiles_physical_domain_is_accepted`, `a_profile_from_another_physical_domain_under_the_same_alias_is_rejected`, `the_validated_tuple_must_describe_the_authoritys_own_source`, `a_profile_that_does_not_claim_the_executing_subject_is_not_proven`, `qualification_proves_the_subject_and_completes_its_own_config`; `b5_security_acquisition::the_binding_alias_resolves_to_the_selections_physical_key` | `decb13a`, `39e85c5` |
| **(rev 7, corrected rev 11)** A source that is not active is never validated, and a profile must name the exact physical adapter binding key it was imported for | `ValidatedSourceBinding::validate`; `ImportedEnforcementProfile::import` | `b5_enforcement::a_source_that_is_not_active_is_never_validated`, `a_profile_must_name_its_exact_physical_adapter_binding` | `decb13a`, `39e85c5` |
| **(rev 11)** The resolved physical key, not the `binding_ref` alias, is the compiled identity: the cache key and the evidence subject commit to it | `ValidatedSourceBinding::adapter_binding_key`; `compile_execution_security` → `CompileCacheKey::new` / `SecurityEvidenceSubject::new` | `b5_enforcement::an_alias_resolving_to_the_profiles_physical_domain_is_accepted`, `a_profile_from_another_physical_domain_under_the_same_alias_is_rejected` | `39e85c5` |
| **(rev 4)** The plan carries the composed attempt-isolation guarantee; its accessor and digest agree with the obligation set | **absent** — `PolicyEnforcementPlan::attempt_isolation` reads the effective policy; the digest omits the guarantee | **absent** | — (P1-3) |
| The cache key commits to the ADR-0014 §4 components and rejects an empty one **(rev 5: the listed set was incomplete — see P1-4)** | `CompileCacheKey::new` | `b5_enforcement::the_cache_key_rejects_an_incomplete_component` | `cbab05f` |
| **(rev 5)** The cache key distinguishes two compile inputs whose guarantees, and therefore obligations, differ | **absent** — the key has no guarantee component; only the plan digest carries it | **absent** | — (P1-4) |
| The budget is whole-round only: there is no partial-success path | `AcquisitionBudget::require_whole_round` | `b5_enforcement::the_budget_has_no_partial_success_path` | `cbab05f` |
| The candidate-domain digest is order-independent over distinct ids | `CandidateDomain::from_ids` | `b5_enforcement::the_candidate_domain_digest_is_a_stable_identity_set` | `cbab05f` |
| The durable diagnostic binds Task, dispatch generation, domain digest/count, binding/profile digest, cost, budget, limit, status, timestamps | `schema.rs` `security_acquisition_blocks`; `security_acquisition.rs`; `kernel.rs` wrappers | `b5_acquisition_block::one_condition_is_one_durable_diagnostic`, `a_block_names_only_its_own_task_generation_and_mode` | `cbab05f`, `5f644b4` |
| The evidence subject binds the §5 tuple and is partial until `with_source_config` | `SecurityEvidenceSubject::new` / `with_source_config` / `digest` | `b5_enforcement::the_subject_completes_only_with_its_own_source_config`, `compilation_returns_a_root_relative_plan_and_a_partial_subject` | `cbab05f` |
| **(rev 9)** Typed acquisition has two disjoint chains: the coarse one refuses a `SECURITY_V1` Task, AgentType or source, and the security one refuses a `COARSE_V1` one | `provisioning.rs` `AcquisitionMode`, `reject_security_v1_coarse_acquisition`, `require_security_v1_acquisition` | `b5_coarse_path_gate::a_coarse_task_cannot_execute_a_security_v1_agent_type`, `a_coarse_task_cannot_select_a_security_v1_source_even_without_a_ceiling`, `b5_security_acquisition::the_two_acquisition_chains_are_disjoint`, `a_coarse_agent_type_is_not_a_security_candidate`, `a_coarse_source_cannot_provision_a_security_task` | `d4d8dc3` |
| **(rev 9)** The authority transaction re-loads the binding, requirement, actual AgentType revision and policy, source, declaration and `P_source`, and qualifies before any Attempt/Lease; the only caller-supplied facts are the imported profile and the requested subject | `provisioning.rs` `acquire_security_v1_task_existing` / `finish_acquisition` (`AcquisitionMode::SecurityV1`) | `b5_security_acquisition::a_qualified_candidate_is_selectable`, `an_uncovered_profile_is_rejected_before_attempt_or_lease`, `an_uncovered_subject_is_rejected_before_attempt_or_lease`, `a_profile_imported_for_another_physical_domain_is_rejected_before_attempt_or_lease`, `the_durable_declaration_wins_over_the_callers_profile`, `the_callers_subject_is_the_one_proved`, `an_impossible_source_ceiling_is_candidate_local_ineligibility`, `the_binding_alias_resolves_to_the_selections_physical_key` | `d4d8dc3`, `39e85c5` |
| **(rev 9)** A credential request the selected profile cannot deliver is ineligible before Attempt/Lease, settled at the attested config | `provisioning.rs` `finish_acquisition` (`ProvenExecutionSecurity::with_source_config`) | `b5_security_acquisition::a_requested_credential_the_profile_cannot_deliver_is_rejected_before_attempt_or_lease` | `d4d8dc3` |
| **(rev 10)** `SECURITY_V1` acquisition intersects the frozen B.4 source/config eligibility (ACTIVE dispositions, ownership, evidence subject/config digest, lifecycle, continuity, functional envelope, security-class evidence, Task safety/continuity/restriction join); only the coarse policy-reference relations are replaced by policy order and the compile | `predicates.rs` `can_provision_task_source_config`; `provisioning.rs` `finish_acquisition` (`AcquisitionMode::SecurityV1`) | `b5_enforcement::the_provisioning_conjunct_is_shared_by_both_paths`, `b5_security_acquisition::evidence_resolved_for_another_config_cannot_authorize_this_config`, `an_agent_type_capability_the_source_cannot_provide_is_rejected`, `a_config_that_cannot_realize_the_agent_type_continuity_is_rejected`, `a_config_that_cannot_realize_the_agent_type_lifecycle_is_rejected`, `a_disabled_source_config_is_rejected_even_with_a_refreshed_frontier` | `0a0ed56` |
| **(rev 10)** A profile's delivery capability is never a credential authorization: any non-empty `credential_refs` is fail-closed at the authority-bearing entry until B5.3 | `provisioning.rs` `finish_acquisition` pre-authority gate; `enforcement.rs` `with_source_config` remains the pure necessary condition | `b5_security_acquisition::a_credential_bearing_config_is_fail_closed_even_when_the_profile_can_deliver`, `a_requested_credential_the_profile_cannot_deliver_is_rejected_before_attempt_or_lease`; `b5_enforcement::a_requested_credential_completes_when_the_profile_delivers` (pure component) | `0a0ed56` |

### ADR-0014 §4 durable encoding (schema v10)

| Claim | Code | Test | Commit |
|---|---|---|---|
| A Task holds **one current** diagnostic; an indistinguishable condition is a no-op, a different condition replaces the row and restarts `BLOCKED` **(rev 2: scope corrected)** | `security_acquisition.rs` `record_security_acquisition_block`; `schema.rs` `task_id PRIMARY KEY` | `b5_acquisition_block::one_condition_is_one_durable_diagnostic` | `5f644b4` |
| Re-recording the **currently stored** condition is a no-op even after release; an earlier released condition that is no longer stored can be reopened **(rev 2; ADR-0014 §4 states this scope in rev 3)** | trigger `..._no_same_condition_reopen` compares only `OLD`/`NEW` | `b5_acquisition_block::a_released_condition_is_never_reopened` covers the stored case only | `5f644b4` (P2-3) |
| `RELEASED` requires both release fields; `BLOCKED` forbids them | `schema.rs` table `CHECK`; `SecurityAcquisitionBlock::from_durable_parts` | `b5_enforcement::a_durable_block_row_cannot_have_inconsistent_release_fields`, `b5_acquisition_block::a_release_timestamp_must_be_a_real_instant` | `5f644b4` |
| Release is final: release identity and time cannot change | trigger `..._release_is_final` | `b5_enforcement::a_block_is_released_only_by_operator_action_or_configuration_change` | `5f644b4` |
| Rows are released, never deleted; a raw writer cannot delete or reopen the stored condition | triggers `..._immutable_delete`, `..._no_same_condition_reopen` | `b5_acquisition_block::the_durable_row_cannot_be_deleted_or_reopened_by_a_raw_writer` | `5f644b4` |
| The parent must be a `SECURITY_V1` requirement and the generation must be the durable dispatch generation | trigger `..._require_security_parent`; `record_security_acquisition_block` generation check | `b5_acquisition_block::a_block_names_only_its_own_task_generation_and_mode` | `5f644b4` |
| **(rev 6)** The validated read re-binds the recorded generation to the Task's durable dispatch generation | `security_acquisition.rs` `get_security_acquisition_block` | `b5_acquisition_block::a_validated_read_rejects_a_block_that_names_another_generation` | `9b8d830` |
| **(rev 6)** The recorded candidate cost equals the recorded domain size; a partial candidate cost is not a whole-round defer | `enforcement.rs` `validate_durable_truth` | `b5_enforcement::a_durable_block_must_record_the_complete_domain_as_its_candidate_cost`, `b5_acquisition_block::a_validated_read_rejects_a_block_whose_candidate_cost_is_incomplete` | `9b8d830` |
| **(rev 6)** A same-condition record revalidates the stored row instead of returning early on two digests | `security_acquisition.rs` `record_security_acquisition_block` | `b5_acquisition_block::a_same_condition_write_revalidates_the_existing_row` | `9b8d830` |
| **(rev 4)** The recorded cost actually exceeds the recorded budget, and `budget_limit` is exactly the exceeded bound | **absent** — `from_durable_parts` and `record_security_acquisition_block` re-check neither | **absent** | — (P2-5) |

### ADR-0014 §13–§14 — obligations and the policy-only reference profile

| Claim | Code | Test | Commit |
|---|---|---|---|
| Obligations are semantic: `Top`/unrestricted adds none; a concrete restriction adds one | `RequiredEnforcementFeatures::derive` | `b5_enforcement::obligations_follow_imposed_restrictions_not_field_presence` | `cbab05f` |
| A missing dimension makes the candidate ineligible; it is not a Task failure and not partial enforcement | `compile_execution_security` → `SecurityUnenforceable` | `b5_enforcement::a_missing_dimension_makes_the_candidate_ineligible_not_the_task_failed` | `cbab05f` |
| The policy-only reference profile supports exactly workspace-write confinement and environment allowlist | `EnforcementSupport::policy_only_reference`; `ImportedEnforcementProfile::import` | `b5_enforcement::the_reference_profile_claims_only_workspace_write_and_environment` | `cbab05f` |
| **(rev 11)** The frozen reference profile identity cannot widen its support matrix; only a stricter subset is importable | `enforcement.rs` `is_policy_only_reference`, `EnforcementSupport::is_subset_of`, `ImportedEnforcementProfile::import` | `b5_enforcement::the_frozen_reference_profile_cannot_widen_its_support_matrix` | `39e85c5` |
| The enforcement subject is explicit and only the proven domain is inside the claim | `EnforcementSubject::is_inside_proven_domain`; `ImportedEnforcementProfile::import` | `b5_enforcement::the_policy_only_reference_protocol_cannot_claim_an_external_subject`, `a_profile_that_does_not_claim_the_executing_subject_is_not_proven` | `cbab05f`, `decb13a` |
| **(rev 2; implemented in revision 8)** `CredentialDelivery` is derived from the credential the selected config **requests**, not from the authorization ceiling | `RequiredEnforcementFeatures::derive` + `CompiledExecutionSecurity::with_source_config` | `b5_enforcement::an_authorized_credential_is_not_a_delivery_request`, `a_requested_credential_needs_a_profile_that_can_deliver`, `a_requested_credential_completes_when_the_profile_delivers` | `28d6592` |

## Fences preserved

Checked by `git diff c0497eb d3ab482`:

| Fence | Result |
|---|---|
| `agentype-core` unchanged | 0 lines changed |
| `canonical.rs` unchanged | 0 lines changed |
| `CANONICAL_FORMAT_VERSION` unchanged | `"agentype-contract/1"` |
| `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions unchanged | `1` / `1` / `1` |
| No ADR `Status:` line changed | none modified |
| No spec status changed | none modified |
| `SCHEMA_VERSION` | `9 → 10` only; no v8→v9 or v9→v10 migration, no backfill |
| COARSE_V1 durable bytes | unchanged; only new `SECURITY_V1` surfaces were added |
| M4/M5/B.1–B.4 behavior | not regressed; full suite green at revision 1 |
| Untracked files | not staged, not committed, not deleted |

The schema change is fresh-only: a v9 database is rejected at open under the
existing D-DB-MIGRATE rule, and no historical block is synthesized.

The closing pull request (revisions 7–9) adds a paragraph and a fence check of
its own, against `fb3641f..d4d8dc3`:

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §2, §3, §4, §5 and §14 body text only) |
| Durable format / schema version | unchanged in revisions 8 and 9; revision 7's profile digest is an opaque value, not a new column |
| Compile cache key list | unchanged by revisions 8 and 9; revision 7's profile digest already entered it |
| Coarse acquisition behavior | byte-for-byte the same relation and gates; `b5_coarse_path_gate` 8/8 and `b4_provisioning_binding` 56/56 pass unchanged |
| Untracked files | not staged, not committed, not deleted |

Revisions 10 and 11 add a fence check of their own, against `fb3641f..39e85c5`:

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §2, §3, §5 and §14 and ADR-0015 §5 and §9 body text only) |
| Durable format / schema version | unchanged; the profile digest and `security_acquisition_blocks.binding_profile_digest` stay opaque values, so no migration |
| Compile cache key list | unchanged in shape; the adapter component carries the resolved physical key instead of the alias |
| Coarse acquisition behavior | unchanged; the security chain remains disjoint and still refuses `COARSE_V1` |
| Untracked files | not staged, not committed, not deleted |

## Deliberate non-reachability

B5.2 provides a compiler, a durable diagnostic and (from revision 9) a security
acquisition primitive; it is not a live scheduler path. The following
properties are intended and were checked, not accidental. Item 1 is **corrected
in revision 2**: the original wording overstated the fence.

1. **A contract-only host cannot mint an imported enforcement profile; a host
   that embeds the runtime can.** `ImportedEnforcementProfile::import` and
   `from_parts` are `#[cfg(any(test, feature = "provisioning-producer"))]`
   (`enforcement.rs:307, 339`). The boundary fixture depends on the runtime,
   which enables the feature, so the constructor **is** visible in the build
   that runs the probes (see P1-2). The gate that actually matters is the next
   item plus ADR-0014 §5 re-proof, not constructor absence.
2. **Typed acquisition has two disjoint chains. The security chain exists and is
   callable, but no runtime composition drives it.** The coarse entry point still
   rejects `SECURITY_V1` at every stage (`reject_security_v1_coarse_acquisition`,
   the AgentType mode branch and the source mode branch in `provisioning.rs`),
   and the security entry point rejects a `COARSE_V1` Task, AgentType or source.
   `acquire_security_v1_task_existing` is the first non-test caller of
   `compile_execution_security` and `qualify_security_candidate`, but it is
   compiled under `runtime-internal` and neither `agentype-runtime`'s resolver nor
   its daemon invokes it: selection and acquisition still run through the coarse
   path. It also grants no secured start; the compiled proof is discarded.
3. **The durable block has no production caller.**
   `Kernel::record_security_acquisition_block` is a thin wrapper; the only
   callers are `b5_acquisition_block.rs` tests. Wiring the control plane to
   record a real budget defer is later work.
4. **There is no compile cache implementation.**
   `CompileCacheKey` is constructed and validated; it is not persisted. The
   ADR defines the key so a later memo store cannot reuse a stale result.
5. **There is no OS enforcement.**
   `PolicyEnforcementPlan` holds logical policy plus obligations. `prepare`,
   physical root binding, secured start, and receipts are absent.

Item 1 no longer supports any security argument, and this record no longer uses
it as one. Item 2 is amended in revision 9; items 3–5 are unchanged by the
re-audit.

## Known gaps for the reviewer

These are not hidden defects; they are the boundary of the slice. The first two
are narrowed by [Re-audit findings](#re-audit-findings-revisions-2-4-5-and-6) rather than
resolved; the first is now fixed in an unmerged code commit and is kept here so
the earlier boundary remains auditable.

1. **The pure compiler does not cross-check the adapter policy.** **Closed in
   revision 7 by the validated source/binding/profile tuple (`decb13a`) and
   corrected in revision 11 (`39e85c5`) to use the resolved physical key.**
   `compile_execution_security` no longer takes the binding and the profile as
   independent values: `ValidatedSourceBinding` proves that
   `binding.policy_ref == source.adapter_policy`, that source and binding are
   active, and that the profile was imported for the same resolved
   `(adapter_kind, adapter_binding_key)` physical domain the selection names. The
   compiler also rejects a tuple whose source revision is not the authority's.
   The historical shape is kept for review: `EffectiveTaskAuthority` records only
   `SpawnSourceRef`, `SpawnSourceSecurityDeclaration` carries no adapter policy,
   and the binding's `policy_ref` alone was in the evidence subject.
2. **The durable block is not wired.** `record_security_acquisition_block` has
   no production caller, so today a budget defer leaves no durable trace unless
   a test writes one. Its stated history property is narrower than revision 1
   claimed (P2-3); revision 3 narrows ADR-0014 §4 to match it.
3. **`budget_limit` records one limit.** If a round exceeds both the candidate
   and compile budgets, `require_whole_round` reports `CANDIDATE_COUNT`. This is
   deterministic and documented here so it is not mistaken for a missing field;
   the cost carries both observed counts.
4. **`SecurityAcquisitionBlock` is `PartialEq` only.** `UnixTime` is `f64`, so
   the type cannot be `Eq`/`Hash`. It is a durable value object, not a map key.
5. **`TaskSecurityCeiling` is not itself compiler input.** It exists for
   candidate-independent preselection; the compiler still requires the
   four-term `EffectiveTaskAuthority`. A reviewer should confirm no caller
   substitutes the ceiling for the composite.
6. **(rev 2, amended rev 9 and 10)** **Selected-candidate qualification is on the
   security acquisition path; complete orchestration is not.** P2-4. The
   predicate now has exactly one non-test caller,
   `acquire_security_v1_task_existing`, which re-proves the whole chain inside the
   authority transaction, including the B.4 conjunct from revision 10. The coarse
   path still refuses `SECURITY_V1`, and the runtime resolver/daemon does not call
   the security entry point. There is no complete candidate enumeration,
   whole-round budget/defer wiring, broker observation epoch, or new-agent
   security acquisition, and `catalog_frontier_digest` fences only the
   source/config frontier. The Gate's acceptance tests pass at `0a0ed56`, but the
   finding stays open until the pull request merges and is re-audited.
7. **(rev 2)** **The supported-surface claim for imported evidence needs a
   replacement evidence story.** P1-2. Until the runtime import and the
   authority transaction are the named proof, no `#[cfg]` visibility argument
   should appear in a milestone gate.
8. **(rev 4)** **The plan's isolation accessor is not the composed guarantee.**
   P1-3. The obligation set is derived from the guarantee, but
   `PolicyEnforcementPlan::attempt_isolation()` reads the effective-policy bit.
   A direct compiler caller can therefore read `false` from a plan that requires
   isolation. The full qualification path currently rejects the contradicting
   input first, so this is latent, not a live bypass.
9. **(rev 4)** **The durable diagnostic does not re-prove its own budget
   truth.** P2-5. `from_budget_defer` is sound, but the durable constructor and
   write boundary accept a `BLOCKED` row whose cost never exceeded its budget.
   The row cannot create Attempt/Lease authority; it is a false operator
   diagnostic and must fail the validated read.
10. **(rev 5)** **The memoization key is not plan identity.** P1-4. A Task-only
    isolation guarantee changes the obligations and the plan digest but no cache
    key component, so a memoized result compiled without `ATTEMPT_ISOLATION`
    would be returned for a plan that requires it. Nothing memoizes yet, so this
    is latent; it is nevertheless the first thing a B5.2 acquisition cache would
    get wrong, and the ADR-0014 §4 list itself has to change.
11. **(rev 6)** **A source declaration had no revision identity.** A. Being
    policy/digest-correct was not the same as belonging to this source. The
    declaration now carries its `SpawnSourceRef`, and both the constructor and the
    storage writer reject a foreign one; the composite read is still the cleaner
    long-term shape and remains the vehicle for P2-1.
12. **(rev 6)** **The durable block trusted its own row.** B. A tampered
    generation, a candidate cost smaller than the recorded domain, and a corrupt
    same-condition row were all accepted. Each now fails closed; none of them
    could create Attempt or Lease authority, so the fix is diagnostic truth, not
    a privilege boundary.
13. **(rev 10, B5.4 handoff)** **The subject proof has no durable identity.**
    `SecurityEvidenceSubject::digest()` commits to the profile, not to the chosen
    `EnforcementSubject`, so two subjects over one profile share a digest. Not a
    B5.2 defect (the acquisition re-checks the caller's subject and persists no
    proof), but B5.4 must bind `(PlanIdentity, ProfileIdentity, SubjectIdentity)`
    or a versioned `subject_proof_digest`.
14. **(rev 10, B5.4 handoff)** **A self-consistent profile is not trusted physical
    evidence.** `import` still accepts the caller's support matrix and covered
    subjects. Revision 11 key-binds that matrix and forbids widening the frozen
    reference identity, but trusted importer identity, exact domain ownership,
    freshness and actual subject attestation remain B5.2-C/B5.4 conditions;
    constructor visibility is not the boundary.
15. **(rev 11)** **P2-1 is implemented but not fully closed.** The physical
    key/alias gap is fixed at the validated tuple, the cache key, the evidence
    subject and the transaction, but the audit's P2-1 acceptance also requires a
    trusted importer provenance proof. That proof is not implemented; the source
    of `support`/`covered_subjects` is still the caller. Do not read revision 11
    as "P2-1 closed" or as evidence reusable by B5.4 without that handoff.
16. **(rev 11)** **The reference-profile ceiling is enforced at import, not
    compiler-wide.** Only the exact `SECURITY_V1_POLICY_ONLY_REFERENCE_V1`
    identity is checked against the frozen matrix. A differently named or
    versioned profile is a different identity and must carry its own authored
    target; it is not silently widened by this rule. Additional dimensions for the
    reference protocol require a new versioned profile, not a wider matrix on the
    frozen one.
17. **(rev 12)** **A successful qualification is not execution authority.**
    `acquire_security_v1_task_existing` creates an Attempt/Lease and a
    `ProvisioningBinding`, but it discards the compiled plan and persists no
    `ExecutionSecuritySnapshot`. Wiring this entry point to an ordinary M5
    physical start before B5.4 would let `SECURED_QUALIFIED` masquerade as an
    executable secured start. B5.2-E must keep the qualified state explicitly
    non-executable — by type and feature gate, not only by convention — and only
    B5.4's snapshot plus secured start protocol may produce the executable
    `SECURED_EXECUTION_COMMITTED` state. The presence of an Attempt/Lease is not
    proof that secured execution is reachable.
18. **(rev 12)** **The candidate identity is not yet unified.** `CandidateDomain`
    is built from `LogicalAgentId` alone, so two exact `SourceConfig`s or two
    physical bindings under one logical agent can be counted as one candidate.
    That would corrupt `AcquisitionCost.candidates`, the compile count, the
    candidate-domain digest, the durable block's same-condition test, and the
    no-partial-selection rule. B5.2-D must freeze
    `(LogicalAgent/pinned AgentType, exact Source/Config, physical binding)` —
    including source revision and config digest — before budget, cache or
    durable-block wiring lands.

## Verification

Run at the PR head of revision 1 with a shared target directory:

```powershell
$env:CARGO_TARGET_DIR="C:\Users\hanni\AppData\Local\Temp\opencode\agentype-target"
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --exclude agentype-public-api-boundary
cargo test -p agentype-public-api-boundary
```

Measured at revision 1's head (a docs-only change on top of `d3ab482`):

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary` | 57 suites, 1056 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary` | 38 passed, 0 failed |

Revision 2 is docs-only and re-runs none of these commands. It adds only the
read-only checks that produced the findings:

| Check | Result |
|---|---|
| `cargo tree -p agentype-public-api-boundary -e features -i agentype-agent-contract` | `provisioning-producer` enabled through `agentype-runtime` |
| `rustc` on a five-line generic-fn-item program | `error[E0283]: type annotations needed` |
| Repository-wide content search for `check_required_access`, `covers_subject`, `is_inside_proven_domain`, `qualify_security_candidate` callers | as stated in P1-1, P2-1, P2-4. Revision 7 moves `covers_subject` into `CompiledExecutionSecurity::prove_subject` and `is_inside_proven_domain` into `ImportedEnforcementProfile::import`; revision 9 gives `qualify_security_candidate` its first non-test caller, `acquire_security_v1_task_existing` |
| Repository-wide content search for the `credential` derivation and `PolicyAccess::from_policy` | as stated in P2-2; revision 8 removes the derivation entirely and settles the request at config completion |

Revision 3 is also docs-only and re-runs none of these commands. It changes one
ADR sentence and this record; no code, schema or test changed.

Revision 4 is docs-only and re-runs none of these commands either. Its two
findings come from the same static reading:

| Check | Result |
|---|---|
| Source search for readers of `attempt_isolation` and `EffectiveExecutionGuarantees` | the guarantee reaches the obligation derivation but not `PolicyEnforcementPlan`; the plan digest omits it |
| Source search for validation in `from_durable_parts` and `record_security_acquisition_block` | neither re-checks cost against budget or the recorded limit |

Revision 5 is docs-only as well. Its finding is the only one in this record that
was first confirmed by executing a temporary assertion rather than by reading:

| Check | Result |
|---|---|
| `assert_eq!(baseline.cache_key(), task_only.cache_key())` inserted into the isolation matrix (temporary, then removed) | passes while the two plan digests differ |
| Source search for every `CompileCacheKey` reads and writes | the guarantee appears in the plan digest, never in the key |

The revision-1 suite result remains a record of that run. It is not re-attested
by revision 2, 3, 4 or 5, and the re-auditor did not independently re-run it
either. Once PR #31 lands, the workspace command above also excludes a second
fixture, `agentype-public-api-contract-only`, for the same reason as
`agentype-public-api-boundary`.

Revision 6 is the first revision since 1 that ships code, in the companion pull
request whose head is `9b8d830`. These commands were run at that head; the
docs-only head of this revision re-runs none of them:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 57 suites, 1072 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only` | 3 passed, 0 failed |

Each new guard was also mutation-checked: the guard was disabled, the affected
tests were run, and each one failed for the expected reason.

| Guard disabled | Failing tests |
|---|---|
| `cost.candidates() == candidate_count()` | the contract count test and the storage count test |
| read-path generation re-bind | `a_validated_read_rejects_a_block_that_names_another_generation` |
| same-condition rebuild (digest-only no-op restored) | `a_same_condition_write_revalidates_the_existing_row` |
| constructor provenance check | `the_source_layer_must_match_its_own_declaration` |
| writer provenance check | `a_source_declaration_cannot_be_published_under_another_revision` |

Revision 7 also ships code, in this pull request, and re-runs the full gate at
`decb13a`:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 57 suites, 1080 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Each new P2-1 guard was mutation-checked the same way: the guard was disabled,
the affected test was run, and it failed for the expected reason.

| Guard disabled | Failing test |
|---|---|
| profile `(adapter_kind, binding_ref)` equality (revision-7 shape; revision 11 compares the resolved physical key) | `a_profile_imported_for_another_binding_is_a_subject_mismatch` |
| `binding.policy_ref == source.adapter_policy` | `a_binding_must_implement_the_sources_own_adapter_policy` |
| source-activity check | `a_source_that_is_not_active_is_never_validated` |
| tuple source identity vs the authority | `the_validated_tuple_must_describe_the_authoritys_own_source` |
| `prove_subject` coverage check | `a_profile_that_does_not_claim_the_executing_subject_is_not_proven` |
| policy-only reference subject bound | `the_policy_only_reference_protocol_cannot_claim_an_external_subject` |
| non-blank profile binding identity | `a_profile_must_name_its_adapter_binding` (renamed `a_profile_must_name_its_exact_physical_adapter_binding` in revision 11) |

Revisions 8 and 9 also ship code in the same draft pull request, which re-runs the
full gate at `d4d8dc3` (P2-1, P2-2 and P2-4 together):

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1094 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

The P2-2 guards were mutation-checked at `28d6592`: restoring the
policy-derived `CREDENTIAL_DELIVERY` branch fails
`an_authorized_credential_is_not_a_delivery_request`, and disabling the
`with_source_config` delivery gate fails
`a_requested_credential_needs_a_profile_that_can_deliver`.

Each new P2-4 guard was mutation-checked the same way at `d4d8dc3`:

| Guard disabled | Failing test |
|---|---|
| security-mode dispatch gate (`require_security_v1_acquisition`) | `the_two_acquisition_chains_are_disjoint` |
| AgentType `SECURITY_V1` mode check in the security branch | `a_coarse_agent_type_is_not_a_security_candidate` |
| source `SECURITY_V1` mode check in the security branch | `a_coarse_source_cannot_provision_a_security_task` |
| declared `P_source` loading (`source_policy.as_ref()`) | `an_impossible_source_ceiling_is_candidate_local_ineligibility` |
| credential-request settlement (`with_source_config`) | `a_requested_credential_the_profile_cannot_deliver_is_rejected_before_attempt_or_lease` |
| caller-supplied subject (`security.subject`) | `the_callers_subject_is_the_one_proved` |
| profile binding identity (`ValidatedSourceBinding::validate`) | `a_profile_imported_for_another_physical_domain_is_rejected_before_attempt_or_lease` (renamed in revision 11) |
| both coarse AgentType/source gates | `b5_coarse_path_gate::a_coarse_task_cannot_execute_a_security_v1_agent_type`, `a_coarse_task_cannot_select_a_security_v1_source_even_without_a_ceiling` |

The last two rows are deliberately cross-crate: they show that P2-1's binding
check and the pre-existing B5.1 coarse gates are still load-bearing on the new
production caller, not only in their original tests.

Revision 10 ships code in the same draft pull request and re-runs the full gate at
`0a0ed56`:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1101 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Each revision-10 guard was mutation-checked at `0a0ed56` the same way: the guard
was disabled, the affected tests were run, and each failed for the expected
reason.

| Guard disabled | Failing tests |
|---|---|
| the security branch's `can_provision_task_source_config` call | the five P1-1 acquisition negatives: `evidence_resolved_for_another_config_cannot_authorize_this_config`, `an_agent_type_capability_the_source_cannot_provide_is_rejected`, `a_config_that_cannot_realize_the_agent_type_continuity_is_rejected`, `a_config_that_cannot_realize_the_agent_type_lifecycle_is_rejected`, `a_disabled_source_config_is_rejected_even_with_a_refreshed_frontier` |
| credential pre-authority gate (restored to `matches!(mode, Coarse)`) | `a_credential_bearing_config_is_fail_closed_even_when_the_profile_can_deliver` — the candidate acquires |
| config/source ACTIVE disposition | `the_provisioning_conjunct_is_shared_by_both_paths`, `a_disabled_source_config_is_rejected_even_with_a_refreshed_frontier` |
| evidence source/config/digest identity | `the_provisioning_conjunct_is_shared_by_both_paths`, `evidence_resolved_for_another_config_cannot_authorize_this_config` |
| AgentType continuity conjunct | `the_provisioning_conjunct_is_shared_by_both_paths`, `a_config_that_cannot_realize_the_agent_type_continuity_is_rejected` |
| AgentType lifecycle conjunct | `the_provisioning_conjunct_is_shared_by_both_paths`, `a_config_that_cannot_realize_the_agent_type_lifecycle_is_rejected` |
| functional-envelope miss (entry skipped) | `the_provisioning_conjunct_is_shared_by_both_paths`, `an_agent_type_capability_the_source_cannot_provide_is_rejected` |

Revision 11 ships code in the same draft pull request and re-runs the full gate at
`39e85c5`:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1105 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Each revision-11 guard was mutation-checked at `39e85c5` the same way: the guard
was disabled, the affected tests were run, and each failed for the expected
reason.

| Guard disabled | Failing tests |
|---|---|
| profile `adapter_binding_key` equality (adapter-kind check alone) | `b5_enforcement::a_profile_imported_for_another_binding_is_a_subject_mismatch`, `a_profile_from_another_physical_domain_under_the_same_alias_is_rejected` |
| reference-profile support ceiling | `b5_enforcement::the_frozen_reference_profile_cannot_widen_its_support_matrix` |
| acquisition passes the policy alias instead of `selection.adapter_binding_key` | `b5_security_acquisition::the_binding_alias_resolves_to_the_selections_physical_key` — rejected as `EvidenceSubjectMismatch` |
| compile cache key uses the alias instead of the physical key | `b5_enforcement::an_alias_resolving_to_the_profiles_physical_domain_is_accepted` |
| evidence subject uses the alias instead of the physical key | `b5_enforcement::an_alias_resolving_to_the_profiles_physical_domain_is_accepted` |

Revision 12 is docs-only and re-runs none of the gates. It adds no code, schema or
test; it records the conditional-acceptance review of `181c9e0` and the two hard
conditions carried into B5.2-C and B5.2-E. The revision-11 suite result above
remains the last executed code gate and is not re-attested here.

The workspace suite includes the following new test modules, all green at
revision 1:

| Module | Tests |
|---|---|
| `crates/agentype-agent-contract/tests/b5_authority.rs` | 8 |
| `crates/agentype-agent-contract/tests/b5_matching_order.rs` | 5 |
| `crates/agentype-agent-contract/tests/b5_enforcement.rs` | 14 |
| `crates/agentype-storage-sqlite/tests/b5_candidate_order.rs` | 3 |
| `crates/agentype-storage-sqlite/tests/b5_acquisition_block.rs` | 5 |

At revision 6's code head those modules hold `b5_authority` 8,
`b5_enforcement` 24 and `b5_acquisition_block` 10 tests; `b5_policy_catalog`
holds 25.

At revision 9's code head `d4d8dc3`, the B5 modules hold `b5_authority` 8,
`b5_matching_order` 5, `b5_enforcement` 35, `b5_candidate_order` 3,
`b5_acquisition_block` 10, `b5_policy_catalog` 25, `b5_admission_binding` 28,
`b5_coarse_path_gate` 8 and the new `b5_security_acquisition` 11.

At revision 10's code head `0a0ed56`, the same modules hold `b5_enforcement` 36
(one new shared-conjunct test) and `b5_security_acquisition` 17 (six new P1
negatives), with the other counts unchanged.

At revision 11's code head `39e85c5`, `b5_enforcement` holds 39 (the alias/key
positive, its cross-domain negative, and the reference-ceiling test) and
`b5_security_acquisition` holds 18 (the real-transaction `primary -> k2` test);
the other counts are unchanged.

Public-API probes added in `tests/public-api/src/lib.rs`:

- `b5_composite_authority_surface` — the composite-authority, qualified-candidate,
  and policy-order surface, as a `no_run` doctest.
- `b5_compiled_security_surface` — the pure compiler surface, as a `no_run`
  doctest.
- probe 31 — **does not prove** what revision 1 claimed (P1-2).

## Required follow-up, in order

| Order | Work | Acceptance |
|---|---|---|
| 1 | Re-prove `RequiredAccess` on `P_eff` in `qualify_security_candidate` (P1-1) | tool, endpoint and file-operation negatives reject as `SecurityUnenforceable`; positive compiles. Proposed in PR #31, including the digest re-projection |
| 2 | Replace probe 31 and correct its comment (P1-2) | contract-only fixture rejects; runtime+contract fixture records visibility. Proposed in PR #31 |
| 3 | Carry the attempt-isolation guarantee into the plan accessor and digest (P1-3) | Task-only, AgentType-only and neither matrix: plan accessor, obligations and digest agree; a direct compile with a false policy bit and a true guarantee returns isolation. Proposed in PR #34 |
| 4 | Carry the guarantee identity into the compile cache key and amend ADR-0014 §4 (P1-4) | the same matrix asserts equal effective policies and different cache keys; an empty guarantee digest is rejected; the compiler protocol records the semantic change. Proposed in PR #34, ADR list amended in PR #32 |
| 5 | Bind the profile to its adapter binding and execution subject (P2-1) | cross-binding and unproven-subject negatives. Implemented in revision 7 (`decb13a`) and corrected in revision 11 (`39e85c5`) to bind the resolved physical key; tests named in the claim table |
| 6 | Separate credential authorization from delivery request (P2-2) | empty `credential_refs` adds no obligation, a requested credential under a non-delivering profile is `SecurityUnenforceable`, and a requested credential under a delivering profile **completes at the pure component only**. Implemented in revision 8 (`28d6592`); the authority path is further narrowed by revision 10 (P1-2) |
| 7 | ~~Decide durable-block history semantics (P2-3)~~ **decided in revision 3: narrow ADR-0014 §4** | the stored released condition is never reopened; `A → release A → B → A` is specified to start a new diagnostic, not to retry the released one |
| 8 | Re-prove the durable block's over-budget truth at construction and at the write boundary (P2-5) | a non-exhausted or limit-mismatched row is rejected; a raw-SQL corruption fails the validated read. Proposed in PR #34 |
| 9 | Bind each source declaration to its source revision (A) | the constructor and the writer both reject a declaration from another revision, and the validated read still fills the ref from `(source id, revision)`. Proposed in PR #35 |
| 10 | Complete the durable block's validated read and count coherence (B) | a tampered generation, a candidate cost smaller than the domain, and a corrupt same-condition row each fail closed on read or re-record. Proposed in PR #35 |
| 11 | Intersect the frozen B.4 source/config eligibility on the `SECURITY_V1` path (P1-1, rev 10) | disabled config, foreign evidence digest, missing functional capability, and unsatisfiable continuity/lifecycle each reject with no Attempt/Lease; the coarse path is unchanged. Implemented in revision 10 (`0a0ed56`) |
| 12 | Keep credential-bearing acquisition fail-closed until B5.3 (P1-2, rev 10) | a delivering profile plus non-empty `credential_refs` is `InvalidAuthority` before Attempt/Lease; the pure-component delivery check and its positive test remain. Implemented in revision 10 (`0a0ed56`) |
| 13 | Bind the imported profile to the resolved physical `adapter_binding_key`, not the B.4 alias (P1, rev 11) | `primary -> k2` positive with cache key and subject carrying `k2`; a profile imported for `k1` under the same alias is rejected; the acquisition transactions pass the selection key. Implemented in revision 11 (`39e85c5`) |
| 14 | Enforce `Support_imported ⊆ Support_reference_profile` for the fixed reference identity (P2, rev 11) | a reference-identity profile claiming `NetworkEgressRestriction` fails at import; a stricter subset remains valid. Implemented in revision 11 (`39e85c5`) |
| 15 | Define the complete candidate identity and enumerate every candidate under a whole-round budget (B5.2-D) | **first commit and first test freeze the unified identity**: `LogicalAgent/pinned AgentType + exact Source/Config + physical binding`, including source revision and config digest; two configs or bindings under one agent are distinct; an unknown candidate or exhausted budget cannot partially select. Not implemented |
| 16 | Prove trusted importer provenance, profile freshness and exact physical-domain ownership (B5.2-C) | the profile is produced by a trusted local importer whose protocol version and physical-domain ownership are verifiable; capability/subject claims cannot be self-declared; freshness has invalidation conditions; renaming cannot escape the reference claim. Not implemented |
| 17 | Wire resolver→Scheduler, revalidate the full candidate set, and handle existing/new agents (B5.2-E) | all qualification proofs close before Attempt/Lease; `SECURED_QUALIFIED` remains non-executable and no ordinary physical start path runs on it; production execution requires the B5.4 snapshot/secured start; feature-gated. Not implemented |
| Gate | Wire the qualification chain into acquisition (P2-4) | a qualified candidate is selectable; an unqualified one is rejected before Attempt/Lease; the coarse gate still rejects `SECURITY_V1`. Implemented in revision 9 (`d4d8dc3`) as `acquire_security_v1_task_existing`; tests named in the claim table. This is **selected-candidate transactional qualification**; complete `SECURITY_V1` orchestration remains a later row |

Items 1–6, 8, 9 and 10 are code changes delivered outside the revision-1 record.
Item 7 was a design decision and is settled in revision 3 by narrowing the ADR
rather than by adding history; item 4 also needs its normative sentence in the
ADR, which is why that part sits in the ADR pull request. Items 11 and 12 are
implemented in revision 10 (`0a0ed56`), items 13 and 14 in revision 11
(`39e85c5`), in the same unmerged draft pull request as the Gate. Items 15–17 are
the audit's B5.2-C/D/E decomposition and are **not implemented** by this revision;
revision 12 accepts that decomposition and carries the two hard conditions into
the follow-ups without adding code.

The Gate is the B5.2 completion condition. Revisions 9, 10 and 11 implement
*selected-candidate transactional qualification* in an unmerged draft pull
request; complete `SECURITY_V1` acquisition orchestration is still open. As with
every finding in this record, the row is evidence for the re-audit, not a
closure. Revision 11 specifically does not claim P2-1 fully closed, because the
trusted importer proof (item 16) remains open.

## Evidence index

| Artifact | Location |
|---|---|
| Composite authority and policy order | `crates/agentype-agent-contract/src/authority.rs` |
| Policy-order relations and qualified candidate | `crates/agentype-agent-contract/src/matching.rs`, `predicates.rs` |
| Enforcement compiler, budget, block value object | `crates/agentype-agent-contract/src/enforcement.rs` |
| Validated source/binding/profile tuple and subject proof | `crates/agentype-agent-contract/src/enforcement.rs` (`ImportedEnforcementProfile` import identity and reference ceiling, `ValidatedSourceBinding`, `CompiledExecutionSecurity::prove_subject`, `ProvenExecutionSecurity`) |
| Attempt-isolation guarantee and plan identity | `crates/agentype-agent-contract/src/enforcement.rs` (`EffectiveExecutionGuarantees`, `PolicyEnforcementPlan`) |
| Credential authorization versus delivery request | `crates/agentype-agent-contract/src/enforcement.rs` (`RequiredEnforcementFeatures::derive`, `CompiledExecutionSecurity::with_source_config`), `crates/agentype-agent-contract/src/sandbox.rs` (`PolicyAccess::from_policy`) |
| Compile memoization key and guarantee identity | `crates/agentype-agent-contract/src/enforcement.rs` (`CompileCacheKey`, `effective_execution_guarantees_digest`) |
| Durable block truth validation | `crates/agentype-agent-contract/src/enforcement.rs` (`from_durable_parts`), `crates/agentype-storage-sqlite/src/security_acquisition.rs` |
| Access floor, projection and check | `crates/agentype-agent-contract/src/sandbox.rs` (`PolicyAccess`, `check_required_access`) |
| Admission-time floor proof | `crates/agentype-storage-sqlite/src/requirement.rs` (lines 1341–1356, 1485–1509) |
| Durable block read/write/release | `crates/agentype-storage-sqlite/src/security_acquisition.rs` |
| Source-declaration provenance | `crates/agentype-agent-contract/src/sandbox.rs` (`SpawnSourceSecurityDeclaration`), `crates/agentype-agent-contract/src/authority.rs` (`EffectiveTaskAuthority::new`), `crates/agentype-storage-sqlite/src/catalog.rs` (`insert_spawn_source_declaration`) |
| Schema v10 table and triggers | `crates/agentype-storage-sqlite/src/schema.rs` (lines 1293–1359) |
| Kernel wrappers | `crates/agentype-storage-sqlite/src/kernel.rs` (lines 1056–1081) |
| Coarse acquisition gate | `crates/agentype-storage-sqlite/src/provisioning.rs` (`reject_security_v1_coarse_acquisition` and the `AcquisitionMode::Coarse` branches) |
| Security acquisition entry point | `crates/agentype-storage-sqlite/src/provisioning.rs` (`acquire_security_v1_task_existing`, the `finish_acquisition` security branch), `kernel.rs` (`Kernel::acquire_security_v1_task_existing`) |
| Security acquisition conformance | `crates/agentype-storage-sqlite/tests/b5_security_acquisition.rs` |
| Supported-surface feature wiring | `crates/agentype-runtime/Cargo.toml`, `tests/public-api/Cargo.toml` |
| Public-API probes | `tests/public-api/src/lib.rs` (probes 18 and 31) |
| ADR — composite authority | `docs/decisions/0013-m6b5-policy-authority-algebra-identity.md` §5 |
| ADR — compilation and receipt chain | `docs/decisions/0014-m6b5-enforcement-compilation-receipt.md` §2–§5, §13–§14 |
| Slice A merge | `ce96c87` (PR #28) |
| Slice B merge | `d3ab482` (PR #29) |
