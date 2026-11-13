# RIIR M6-B.5 B5.2 — Enforcement Qualification Audit Record

Status: Historical Report — audit record, not a freeze
Revision: 3 (2026-11-14) — records the P2-3 decision: the durable-block
idempotence claim is narrowed, not implemented as a history table
Applies to: V0.2 / M6-B.5 B5.2 (PR #28 merge `ce96c87`, PR #29 merge `d3ab482`)
Date: 2026-11-14
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
must not be opened against it as-is.

The gaps are **latent, not live**. Typed acquisition still rejects
`SECURITY_V1` at `provisioning.rs:734–741` (called at `1069` and `1342`), no
production path calls `compile_execution_security` or
`qualify_security_candidate`, and the durable block has no production writer.
This record therefore claims no current privilege escalation and no current
production defect. It does correct two statements that revision 1 and the merged
probes made: that the imported-profile constructor is fenced on the supported
surface, and that qualification settles the ADR-0013 §5 positive-obligation
requirement on the complete effective policy.

## Revision history

| Revision | Head | Change |
|---|---|---|
| 1 | `d14137f` | First audit record over `c0497eb..d3ab482`; docs-only. |
| 2 | `a8117fc` | Records the independent re-audit: two revision-1 claims corrected, five findings added, B5.1 handoff table added. Still docs-only; no code, ADR, spec or index change. |
| 3 | this commit | Records the P2-3 decision: ADR-0014 §4 and this record state that durable-block idempotence is scoped to the currently stored condition, instead of adding a per-condition history table. Docs-only; no code, schema, ADR `Status:` line, spec or index change. |

Revision 2 does not resolve any finding. Each one is recorded with the code it
touches, the test that does not cover it, and the acceptance test that would
close it, so the follow-up code pull requests can be reviewed against the same
statements.

Revision 3 resolves exactly one finding, and it resolves it by narrowing the
claim rather than changing the encoding: P2-3. P1-1 and P1-2 have proposed fixes
in PR #31, which was open and unmerged when this revision was written; this
record does not count an unmerged fix as closure.

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
  [Re-audit findings](#re-audit-findings-revision-2));
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

## Re-audit findings (revision 2)

Revision 2 re-read the merged code rather than the revision-1 prose. Every
finding below was reproduced against `d3ab482`; none is inferred from a comment
or a test name. Severity is stated for the qualification chain, not for the
current build: while acquisition rejects `SECURITY_V1`, all of them are latent.

| ID | Severity | Finding | Blocks |
|---|---|---|---|
| P1-1 | P1 | The four-term `P_eff` is never re-checked against the Task's positive `RequiredAccess` floor | Closing the qualification chain; opening B5.4 |
| P1-2 | P1 | The "default build cannot mint an imported profile" claim is false, and public-API probe 31 cannot prove it | Using the record as a B5.4 gate input |
| P2-1 | P2 | The compiler never checks that the profile was imported for this binding or that it covers the executing subject | Trustworthy `SECURITY_V1` evidence |
| P2-2 | P2 | A credential *authorization* is treated as a credential *delivery* obligation | Correctness of candidate rejection (over-rejection) |
| P2-3 | P2 | Durable-block idempotence holds only for the single stored condition, not across history | The ADR-0014 §4 statement the report quotes (**closed in revision 3 by narrowing the ADR**) |
| P2-4 | P2 | Candidate preselection is implemented, but acquisition qualification is not wired (the fourth B5.1 handoff item) | Declaring B5.2 complete |

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

### Positive correction from the re-audit

The B5.1 audit found that Task→Generation security-mode agreement was checked in
one direction only. `verify_generation_mode_agreement`
(`requirement.rs:295–309`) now compares the stored Task mode against the
Generation's mode and rejects any difference, so the Task reads cannot present a
`SECURITY_V1` Generation as a dispatchable coarse Task. That B5.1 item is closed.

## B5.1 handoff acceptance, re-checked

| Acceptance condition | Revision 2 |
|---|---|
| `match_existing_agents_for_task` selects the relation from the Task security mode | met |
| `more_specific_for_with_policies` uses the real policy order | met |
| A differing sandbox policy reference does not lose a legal existing-agent candidate | met, differential test |
| `can_provision` / acquisition uses the same complete qualification for `SECURITY_V1` | **not met** (P2-4) |

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
| An inactive or malformed binding is never compiled | `compile_execution_security` binding checks | `b5_enforcement::an_inactive_binding_is_never_compiled` | `cbab05f` |
| Qualification re-proves the requirement digest, AgentType revision, agent policy digest, and `can_execute_security_v1` before compiling | `qualify_security_candidate` (lines 846–892) | `b5_enforcement::qualification_re_proves_the_requirement_and_agent_identity_before_compiling` | `cbab05f` |
| Qualification re-checks the Task's coarse modes on the four-term `P_eff` | `qualify_security_candidate` → `authority.validate_coarse_modes` | `b5_enforcement::qualification_rechecks_the_task_coarse_modes_on_the_complete_policy` | `cbab05f` |
| **(rev 2)** The profile was imported for this exact binding and covers the executing subject | **absent** — `covers_subject` and `is_inside_proven_domain` have no caller | **absent** | — (P2-1) |
| The cache key has exactly the seven ADR components and rejects an empty one | `CompileCacheKey::new` | `b5_enforcement::the_cache_key_rejects_an_incomplete_component` | `cbab05f` |
| The budget is whole-round only: there is no partial-success path | `AcquisitionBudget::require_whole_round` | `b5_enforcement::the_budget_has_no_partial_success_path` | `cbab05f` |
| The candidate-domain digest is order-independent over distinct ids | `CandidateDomain::from_ids` | `b5_enforcement::the_candidate_domain_digest_is_a_stable_identity_set` | `cbab05f` |
| The durable diagnostic binds Task, dispatch generation, domain digest/count, binding/profile digest, cost, budget, limit, status, timestamps | `schema.rs` `security_acquisition_blocks`; `security_acquisition.rs`; `kernel.rs` wrappers | `b5_acquisition_block::one_condition_is_one_durable_diagnostic`, `a_block_names_only_its_own_task_generation_and_mode` | `cbab05f`, `5f644b4` |
| The evidence subject binds the §5 tuple and is partial until `with_source_config` | `SecurityEvidenceSubject::new` / `with_source_config` / `digest` | `b5_enforcement::the_subject_completes_only_with_its_own_source_config`, `compilation_returns_a_root_relative_plan_and_a_partial_subject` | `cbab05f` |

### ADR-0014 §4 durable encoding (schema v10)

| Claim | Code | Test | Commit |
|---|---|---|---|
| A Task holds **one current** diagnostic; an indistinguishable condition is a no-op, a different condition replaces the row and restarts `BLOCKED` **(rev 2: scope corrected)** | `security_acquisition.rs` `record_security_acquisition_block`; `schema.rs` `task_id PRIMARY KEY` | `b5_acquisition_block::one_condition_is_one_durable_diagnostic` | `5f644b4` |
| Re-recording the **currently stored** condition is a no-op even after release; an earlier released condition that is no longer stored can be reopened **(rev 2; ADR-0014 §4 states this scope in rev 3)** | trigger `..._no_same_condition_reopen` compares only `OLD`/`NEW` | `b5_acquisition_block::a_released_condition_is_never_reopened` covers the stored case only | `5f644b4` (P2-3) |
| `RELEASED` requires both release fields; `BLOCKED` forbids them | `schema.rs` table `CHECK`; `SecurityAcquisitionBlock::from_durable_parts` | `b5_enforcement::a_durable_block_row_cannot_have_inconsistent_release_fields`, `b5_acquisition_block::a_release_timestamp_must_be_a_real_instant` | `5f644b4` |
| Release is final: release identity and time cannot change | trigger `..._release_is_final` | `b5_enforcement::a_block_is_released_only_by_operator_action_or_configuration_change` | `5f644b4` |
| Rows are released, never deleted; a raw writer cannot delete or reopen the stored condition | triggers `..._immutable_delete`, `..._no_same_condition_reopen` | `b5_acquisition_block::the_durable_row_cannot_be_deleted_or_reopened_by_a_raw_writer` | `5f644b4` |
| The parent must be a `SECURITY_V1` requirement and the generation must be the durable dispatch generation | trigger `..._require_security_parent`; `record_security_acquisition_block` generation check | `b5_acquisition_block::a_block_names_only_its_own_task_generation_and_mode` | `5f644b4` |

### ADR-0014 §13–§14 — obligations and the policy-only reference profile

| Claim | Code | Test | Commit |
|---|---|---|---|
| Obligations are semantic: `Top`/unrestricted adds none; a concrete restriction adds one | `RequiredEnforcementFeatures::derive` | `b5_enforcement::obligations_follow_imposed_restrictions_not_field_presence` | `cbab05f` |
| A missing dimension makes the candidate ineligible; it is not a Task failure and not partial enforcement | `compile_execution_security` → `SecurityUnenforceable` | `b5_enforcement::a_missing_dimension_makes_the_candidate_ineligible_not_the_task_failed` | `cbab05f` |
| The policy-only reference profile supports exactly workspace-write confinement and environment allowlist | `EnforcementSupport::policy_only_reference` | `b5_enforcement::the_reference_profile_claims_only_workspace_write_and_environment` | `cbab05f` |
| The enforcement subject is explicit and only the proven domain is inside the claim | `EnforcementSubject::is_inside_proven_domain` | covered by `the_reference_profile_claims_only_workspace_write_and_environment` | `cbab05f` |
| **(rev 2)** `CredentialDelivery` is derived from the credential the selected config **requests**, not from the authorization ceiling | **absent** — `derive` keys on `policy.credentials()` | **absent** | — (P2-2) |

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

## Deliberate non-reachability

B5.2 is a compiler and a diagnostic, not a live acquisition path. The following
properties are intended and were checked, not accidental. Item 1 is **corrected
in revision 2**: the original wording overstated the fence.

1. **A contract-only host cannot mint an imported enforcement profile; a host
   that embeds the runtime can.** `ImportedEnforcementProfile::import` and
   `from_parts` are `#[cfg(any(test, feature = "provisioning-producer"))]`
   (`enforcement.rs:307, 339`). The boundary fixture depends on the runtime,
   which enables the feature, so the constructor **is** visible in the build
   that runs the probes (see P1-2). The gate that actually matters is the next
   item plus ADR-0014 §5 re-proof, not constructor absence.
2. **Production typed acquisition still rejects `SECURITY_V1`.**
   `reject_security_v1_coarse_acquisition` gates `provisioning.rs` at two call
   sites. No production path calls `compile_execution_security` or
   `qualify_security_candidate`; the only callers of the compiler are
   `qualify_security_candidate`, tests, and a `no_run` doctest.
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

Items 2–5 are unchanged by the re-audit. Item 1 no longer supports any security
argument, and this record no longer uses it as one.

## Known gaps for the reviewer

These are not hidden defects; they are the boundary of the slice. The first two
are narrowed by [Re-audit findings](#re-audit-findings-revision-2) rather than
resolved.

1. **The pure compiler does not cross-check the adapter policy.**
   `compile_execution_security` proves that the binding is active and well-formed
   and that the profile matches the authority, but it does not compare
   `binding.policy_ref` with `SpawnSource.adapter_policy`. `EffectiveTaskAuthority`
   records only `SpawnSourceRef` (id + revision), and
   `SpawnSourceSecurityDeclaration` carries only the source policy and enforcement
   profile, so this information is not in the compiler's inputs. The binding's
   `policy_ref` *is* in the evidence subject, so the ADR-0014 §5 authority
   transaction can re-read the source and reject the mismatch. Revision 2 groups
   this with P2-1: the same input extension that binds the profile to its
   `(adapter_kind, binding_key)` should carry the expected adapter policy.
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
6. **(rev 2)** **Qualification is not on the acquisition path.** P2-4. The
   predicate exists, is tested, and has no production caller; the frozen coarse
   path still rejects `SECURITY_V1`. This is the conservative state, not the
   completed one.
7. **(rev 2)** **The supported-surface claim for imported evidence needs a
   replacement evidence story.** P1-2. Until the runtime import and the
   authority transaction are the named proof, no `#[cfg]` visibility argument
   should appear in a milestone gate.

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
| Repository-wide content search for `check_required_access`, `covers_subject`, `is_inside_proven_domain`, `qualify_security_candidate` callers | as stated in P1-1, P2-1, P2-4 |
| Repository-wide content search for the `credential` derivation and `PolicyAccess::from_policy` | as stated in P2-2 |

Revision 3 is also docs-only and re-runs none of these commands. It changes one
ADR sentence and this record; no code, schema or test changed.

The revision-1 suite result remains a record of that run. It is not re-attested
by revision 2 or 3, and the re-auditor did not independently re-run it either.
Once PR #31 lands, the workspace command above also excludes a second fixture,
`agentype-public-api-contract-only`, for the same reason as
`agentype-public-api-boundary`.

The workspace suite includes the following new test modules, all green at
revision 1:

| Module | Tests |
|---|---|
| `crates/agentype-agent-contract/tests/b5_authority.rs` | 8 |
| `crates/agentype-agent-contract/tests/b5_matching_order.rs` | 5 |
| `crates/agentype-agent-contract/tests/b5_enforcement.rs` | 14 |
| `crates/agentype-storage-sqlite/tests/b5_candidate_order.rs` | 3 |
| `crates/agentype-storage-sqlite/tests/b5_acquisition_block.rs` | 5 |

Public-API probes added in `tests/public-api/src/lib.rs`:

- `b5_composite_authority_surface` — the composite-authority, qualified-candidate,
  and policy-order surface, as a `no_run` doctest.
- `b5_compiled_security_surface` — the pure compiler surface, as a `no_run`
  doctest.
- probe 31 — **does not prove** what revision 1 claimed (P1-2).

## Required follow-up, in order

| Order | Work | Acceptance |
|---|---|---|
| 1 | Re-prove `RequiredAccess` on `P_eff` in `qualify_security_candidate` (P1-1) | tool, endpoint and file-operation negatives reject as `SecurityUnenforceable`; positive compiles |
| 2 | Replace probe 31 and correct its comment (P1-2) | contract-only fixture rejects; runtime+contract fixture records visibility |
| 3 | Bind the profile to its adapter binding and execution subject (P2-1) | cross-binding and unproven-subject negatives |
| 4 | Separate credential authorization from delivery request (P2-2) | empty `credential_refs` adds no obligation |
| 5 | ~~Decide durable-block history semantics (P2-3)~~ **decided in revision 3: narrow ADR-0014 §4** | the stored released condition is never reopened; `A → release A → B → A` is specified to start a new diagnostic, not to retry the released one |
| Gate | Wire the qualification chain into acquisition (P2-4) | a qualified candidate is selectable; an unqualified one is rejected before Attempt/Lease; the coarse gate still rejects `SECURITY_V1` |

Items 1–4 are code changes and belong in follow-up pull requests, not in this
docs-only record. Item 5 was a design decision and is settled in revision 3 by
narrowing the ADR rather than by adding history. This record is what those pull
requests are reviewed against.

## Evidence index

| Artifact | Location |
|---|---|
| Composite authority and policy order | `crates/agentype-agent-contract/src/authority.rs` |
| Policy-order relations and qualified candidate | `crates/agentype-agent-contract/src/matching.rs`, `predicates.rs` |
| Enforcement compiler, budget, block value object | `crates/agentype-agent-contract/src/enforcement.rs` |
| Access floor, projection and check | `crates/agentype-agent-contract/src/sandbox.rs` (`PolicyAccess`, `check_required_access`) |
| Admission-time floor proof | `crates/agentype-storage-sqlite/src/requirement.rs` (lines 1341–1356, 1485–1509) |
| Durable block read/write/release | `crates/agentype-storage-sqlite/src/security_acquisition.rs` |
| Schema v10 table and triggers | `crates/agentype-storage-sqlite/src/schema.rs` (lines 1293–1359) |
| Kernel wrappers | `crates/agentype-storage-sqlite/src/kernel.rs` (lines 1056–1081) |
| Coarse acquisition gate | `crates/agentype-storage-sqlite/src/provisioning.rs` (lines 734–741, 1069, 1342) |
| Supported-surface feature wiring | `crates/agentype-runtime/Cargo.toml`, `tests/public-api/Cargo.toml` |
| Public-API probes | `tests/public-api/src/lib.rs` (probes 18 and 31) |
| ADR — composite authority | `docs/decisions/0013-m6b5-policy-authority-algebra-identity.md` §5 |
| ADR — compilation and receipt chain | `docs/decisions/0014-m6b5-enforcement-compilation-receipt.md` §2–§5, §13–§14 |
| Slice A merge | `ce96c87` (PR #28) |
| Slice B merge | `d3ab482` (PR #29) |
