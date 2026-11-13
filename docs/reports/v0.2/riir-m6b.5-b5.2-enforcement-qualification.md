# RIIR M6-B.5 B5.2 — Enforcement Qualification Audit Record

Status: Historical Report — audit record, not a freeze
Applies to: V0.2 / M6-B.5 B5.2 (PR #28 merge `ce96c87`, PR #29 merge `d3ab482`)
Date: 2026-11-14
Canonical path: `docs/reports/v0.2/riir-m6b.5-b5.2-enforcement-qualification.md`
Freeze state: **NOT FROZEN — no freeze gate has run**
Not a specification. This file indexes merged code against ADR-0013 §5 and
ADR-0014 §2–§5, §13–§14. It grants no correctness and closes no milestone.

Verdict: **B5.2 IS IMPLEMENTED AND MERGED, AND IS NOT DECLARED COMPLETE OR
FROZEN.** The two slices landed the typed composite Task authority, the
policy-ordered `SECURITY_V1` candidate relation, the pure root-relative
enforcement compiler, and the schema-v10 durable acquisition-block diagnostic.
B5.2 remains non-reachable from production acquisition by design: the default
build cannot mint an imported enforcement profile, no production path calls the
compiler, and the durable block still has no production caller. The open items
in [Known gaps](#known-gaps-for-the-reviewer) are recorded for the reviewer, not
resolved here.

## Why this record exists

B5.2 was delivered as two merged pull requests without a milestone closure
report. That is enough to read the diff, but not enough to audit it: a reviewer
needs a single page that says which normative sentence each function implements,
which test fails if that sentence is broken, and which fences were checked. This
record is that page. It is deliberately additive: no code, ADR status, spec
status, or index entry changes in the same change.

This record does **not** claim:

- that B5.2 satisfies a freeze gate;
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

## Claim → code → test → commit

Each row is one normative sentence with the smallest code surface that
implements it and the test that would fail if it were removed.

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

### ADR-0014 §2–§5 — compilation, order, budget, evidence

| Claim | Code | Test | Commit |
|---|---|---|---|
| Compilation is pure and root-relative: logical selectors, no directory, no I/O | `enforcement.rs` `compile_execution_security` | `b5_enforcement::compilation_returns_a_root_relative_plan_and_a_partial_subject` | `cbab05f` |
| The imported profile must be the one the source declared; otherwise subject mismatch | `compile_execution_security` first check | `b5_enforcement::a_different_imported_profile_is_a_subject_mismatch` | `cbab05f` |
| An inactive or malformed binding is never compiled | `compile_execution_security` binding checks | `b5_enforcement::an_inactive_binding_is_never_compiled` | `cbab05f` |
| Qualification re-proves the requirement digest, AgentType revision, agent policy digest, and `can_execute_security_v1` before compiling | `qualify_security_candidate` (lines 846–892) | `b5_enforcement::qualification_re_proves_the_requirement_and_agent_identity_before_compiling` | `cbab05f` |
| Qualification re-checks the Task's coarse modes on the four-term `P_eff` | `qualify_security_candidate` → `authority.validate_coarse_modes` | `b5_enforcement::qualification_rechecks_the_task_coarse_modes_on_the_complete_policy` | `cbab05f` |
| The cache key has exactly the seven ADR components and rejects an empty one | `CompileCacheKey::new` | `b5_enforcement::the_cache_key_rejects_an_incomplete_component` | `cbab05f` |
| The budget is whole-round only: there is no partial-success path | `AcquisitionBudget::require_whole_round` | `b5_enforcement::the_budget_has_no_partial_success_path` | `cbab05f` |
| The candidate-domain digest is order-independent over distinct ids | `CandidateDomain::from_ids` | `b5_enforcement::the_candidate_domain_digest_is_a_stable_identity_set` | `cbab05f` |
| The durable diagnostic binds Task, dispatch generation, domain digest/count, binding/profile digest, cost, budget, limit, status, timestamps | `schema.rs` `security_acquisition_blocks`; `security_acquisition.rs`; `kernel.rs` wrappers | `b5_acquisition_block::one_condition_is_one_durable_diagnostic`, `a_block_names_only_its_own_task_generation_and_mode` | `cbab05f`, `5f644b4` |
| The evidence subject binds the §5 tuple and is partial until `with_source_config` | `SecurityEvidenceSubject::new` / `with_source_config` / `digest` | `b5_enforcement::the_subject_completes_only_with_its_own_source_config`, `compilation_returns_a_root_relative_plan_and_a_partial_subject` | `cbab05f` |

### ADR-0014 §4 durable encoding (schema v10)

| Claim | Code | Test | Commit |
|---|---|---|---|
| One `(candidate_domain_digest, binding_profile_digest)` pair is one row; re-recording is a no-op even after release | `security_acquisition.rs` `record_security_acquisition_block`; trigger `..._no_same_condition_reopen` | `b5_acquisition_block::a_released_condition_is_never_reopened` | `5f644b4` |
| A different condition replaces the diagnostic fields and starts `BLOCKED` again | `record_security_acquisition_block` update path | `b5_acquisition_block::one_condition_is_one_durable_diagnostic` | `5f644b4` |
| `RELEASED` requires both release fields; `BLOCKED` forbids them | `schema.rs` table `CHECK`; `SecurityAcquisitionBlock::from_durable_parts` | `b5_enforcement::a_durable_block_row_cannot_have_inconsistent_release_fields`, `b5_acquisition_block::a_release_timestamp_must_be_a_real_instant` | `5f644b4` |
| Release is final: release identity and time cannot change | trigger `..._release_is_final` | `b5_enforcement::a_block_is_released_only_by_operator_action_or_configuration_change` | `5f644b4` |
| Rows are released, never deleted; a raw writer cannot delete or reopen | triggers `..._immutable_delete`, `..._no_same_condition_reopen` | `b5_acquisition_block::the_durable_row_cannot_be_deleted_or_reopened_by_a_raw_writer` | `5f644b4` |
| The parent must be a `SECURITY_V1` requirement and the generation must be the durable dispatch generation | trigger `..._require_security_parent`; `record_security_acquisition_block` generation check | `b5_acquisition_block::a_block_names_only_its_own_task_generation_and_mode` | `5f644b4` |

### ADR-0014 §13–§14 — obligations and the policy-only reference profile

| Claim | Code | Test | Commit |
|---|---|---|---|
| Obligations are semantic: `Top`/unrestricted adds none; a concrete restriction adds one | `RequiredEnforcementFeatures::derive` | `b5_enforcement::obligations_follow_imposed_restrictions_not_field_presence` | `cbab05f` |
| A missing dimension makes the candidate ineligible; it is not a Task failure and not partial enforcement | `compile_execution_security` → `SecurityUnenforceable` | `b5_enforcement::a_missing_dimension_makes_the_candidate_ineligible_not_the_task_failed` | `cbab05f` |
| The policy-only reference profile supports exactly workspace-write confinement and environment allowlist | `EnforcementSupport::policy_only_reference` | `b5_enforcement::the_reference_profile_claims_only_workspace_write_and_environment` | `cbab05f` |
| The enforcement subject is explicit and only the proven domain is inside the claim | `EnforcementSubject::is_inside_proven_domain` | covered by `the_reference_profile_claims_only_workspace_write_and_environment` | `cbab05f` |

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
| M4/M5/B.1–B.4 behavior | not regressed; full suite green |
| Untracked files | not staged, not committed, not deleted |

The schema change is fresh-only: a v9 database is rejected at open under the
existing D-DB-MIGRATE rule, and no historical block is synthesized.

## Deliberate non-reachability

B5.2 is a compiler and a diagnostic, not a live acquisition path. The following
properties are intended and were checked, not accidental:

1. **The default build cannot mint an imported enforcement profile.**
   `ImportedEnforcementProfile::import` and `from_parts` are
   `#[cfg(any(test, feature = "provisioning-producer"))]`. With the feature off,
   no value of the type exists, so `compile_execution_security` is unreachable.
   Public-API probe 31 asserts that the constructor is absent from the supported
   surface.
2. **Production typed acquisition still rejects `SECURITY_V1`.**
   `reject_security_v1_coarse_acquisition` gates `provisioning.rs` at two call
   sites. No production path calls `compile_execution_security` or
   `qualify_security_candidate`; the only callers are `qualify_security_candidate`
   itself, tests, and a `no_run` doctest.
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

## Known gaps for the reviewer

These are not hidden defects; they are the boundary of the slice. Each is
recorded so the reviewer can decide whether it is acceptable for B5.2.

1. **The pure compiler does not cross-check the adapter policy.**
   `compile_execution_security` proves that the binding is active and well-formed
   and that the profile matches the authority, but it does not compare
   `binding.policy_ref` with `SpawnSource.adapter_policy`. `EffectiveTaskAuthority`
   records only `SpawnSourceRef` (id + revision), and
   `SpawnSourceSecurityDeclaration` carries only the source policy and enforcement
   profile, so this information is not in the compiler's inputs. A caller could
   therefore compile against a legal but different adapter binding for the same
   source revision. The binding's `policy_ref` *is* in the evidence subject, so
   the ADR-0014 §5 authority transaction can re-read the source and reject the
   mismatch. B5.2 treats this as a documented caller precondition; a future slice
   that has the full source record should make it an in-function check. It is not
   a P0/P1 here because no production caller exists.
2. **The durable block is not wired.** `record_security_acquisition_block` has
   no production caller, so today a budget defer leaves no durable trace unless
   a test writes one. The table, invariants, and release semantics are real; the
   control-plane trigger is not.
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

## Verification

Run at the PR head with a shared target directory:

```powershell
$env:CARGO_TARGET_DIR="C:\Users\hanni\AppData\Local\Temp\opencode\agentype-target"
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --exclude agentype-public-api-boundary
cargo test -p agentype-public-api-boundary
```

Measured at this audit head (a docs-only change on top of `d3ab482`):

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary` | 57 suites, 1056 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary` | 38 passed, 0 failed |

The workspace suite includes the following new test modules, all green:

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
- probe 31 — `ImportedEnforcementProfile::import` / `from_parts` are absent from
  the supported (non-`provisioning-producer`) build.

## Evidence index

| Artifact | Location |
|---|---|
| Composite authority and policy order | `crates/agentype-agent-contract/src/authority.rs` |
| Policy-order relations and qualified candidate | `crates/agentype-agent-contract/src/matching.rs`, `predicates.rs` |
| Enforcement compiler, budget, block value object | `crates/agentype-agent-contract/src/enforcement.rs` |
| Durable block read/write/release | `crates/agentype-storage-sqlite/src/security_acquisition.rs` |
| Schema v10 table and triggers | `crates/agentype-storage-sqlite/src/schema.rs` (lines 1293–1359) |
| Kernel wrappers | `crates/agentype-storage-sqlite/src/kernel.rs` (lines 1056–1081) |
| ADR — composite authority | `docs/decisions/0013-m6b5-policy-authority-algebra-identity.md` §5 |
| ADR — compilation and receipt chain | `docs/decisions/0014-m6b5-enforcement-compilation-receipt.md` §2–§5, §13–§14 |
| Slice A merge | `ce96c87` (PR #28) |
| Slice B merge | `d3ab482` (PR #29) |
