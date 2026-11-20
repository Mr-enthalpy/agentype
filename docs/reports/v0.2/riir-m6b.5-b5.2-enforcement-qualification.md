# RIIR M6-B.5 B5.2 — Enforcement Qualification Audit Record

Status: Historical Report — audit record, not a freeze
Revision: 22 (2026-11-20) — continues the C → D → E order after PR #38's B5.2-C
and PR #39's B5.2-D. Revision 19 implements B5.2-E: the resolver is wired into the
typed acquisition facade with mode-aware enumeration, a complete-round
`SecurityHold` for `SECURITY_V1` Tasks, and a dedicated `security_hold_cooldown`.
With B5.4 disabled a `SECURITY_V1` Task is held `QUEUED` with no Attempt or Lease
instead of cycling `QUEUED → LEASED → lease expiry`. Revisions 21–22 fix the
round-3 audit's code findings: provisional holds for a timed-out probe or a
catalog/roster drift, a one-hour attestation-validity ceiling, and a per-mode
readiness proof. B5.2 is still not frozen and B5.4 (snapshot, secured start,
`ExecutionSecurityEvidence`) remains open. No ADR `Status:` line, spec status or
index entry changes
Applies to: V0.2 / M6-B.5 B5.2 (PR #28 merge `ce96c87`, PR #29 merge `d3ab482`;
PR #30–#37 merged, #37 at `e68a771`; B5.2-C in PR #38; B5.2-D in PR #39; B5.2-E on
`agent/m6b5-b5.2-e-scheduler-wiring`)
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
PR draft stays open. Revision 13 records the third review pass, which found that
the intersected B.4 conjunct still carried the coarse exact-reference check on the
Task's own authored sandbox policy and therefore refused a legal independent Task
fine policy; the reusable conjunct is now split out and the security path uses
it, with the coarse exact-reference claim left intact on the coarse chain.
Revision 14 records the execution-boundary review: a qualified `Claim` could still
be routed into the frozen B.4 Execution commit and to `RUNNING`, so the Kernel now
rejects `SECURITY_V1` at both boundaries, independently of any scheduler caller.
Revision 15 records the semantic-completion review: the same `Claim` could still be
acknowledged with no Execution at all, publishing a Result and releasing the Batch.
`ack_success` now rejects `SECURITY_V1` before any state change, and the ADR
separates transactional re-proof from external physical evidence.
Revision 16 records the continuation-baseline review: it finds no new P1, accepts
the Kernel guards as the fail-closed boundary for this stage, confirms that PR
#30–#36 are merged, and adds the B5.2-E liveness condition. It still accepts this
draft only as a B5.2 continuation, never as a freeze. Revision 17 records PR #37's
merge (`e68a771`) and implements B5.2-C: acquisition re-reads a durable
trusted-importer attestation for the exact profile digest and physical domain,
and rejects an absent or stale row before any qualification or Attempt/Lease.
Revision 18 implements B5.2-D: full candidate identity and a whole-round
`SecurityCandidateRound`, so two configs or two physical bindings under one agent
are distinct and selection is only ever taken from the complete round. Revision 19
implements B5.2-E's scheduler wiring as a mode-aware hold: a `SECURITY_V1` Task is
enumerated over the complete round and then held `QUEUED` before any
Attempt/Lease, instead of cycling through an expiring lease while B5.4 is absent.
Revision 20 records the joint C/D/E audit and fixes its two actionable findings: the
attestation writer now rejects a self-consistent value rebuilt from public parts, so
only the trusted producer can record physical evidence, and a held `SECURITY_V1`
Task no longer consumes the typed dispatch cycle ahead of runnable coarse work.
As with every earlier finding, this record counts no
unmerged fix as closure and does not freeze B5.2. Revision 21 fixes the round-3
audit's code defect: a read-only security probe that exceeded its deadline now
returns a provisional hold instead of aborting the typed cycle, so unrelated coarse
work still dispatches while integrity faults still fail closed. Revision 22 fixes
the remaining three code-level findings of that audit: the attestation claim is
bounded to one hour on construction and on the durable read, the hold's "complete"
round is fenced by a catalog/roster snapshot and reports `CatalogDrift` when it
cannot be coherent, and each readiness probe proves its own Task mode instead of
sharing a one-way `reject_security_v1` flag. It also adds the audit's requested
`probe_limit=1` multi-hold load test. Revision 22 records the audit's §四 scope
choice: the original B5.2 gate stands, so the trusted registry, the installation
epoch and the runtime whole-round budget are still B5.2 acceptance conditions and
B5.2 remains unfrozen. The current stage is
*selected-candidate transactional qualification with trusted-importer provenance,
complete candidate identity, a bounded freshness claim and an explainable security
hold implemented; the trusted registry/epoch, production budget/defer, secured
acquisition/start and `ExecutionSecurityEvidence` remain open*.

The gaps are **latent, not live**. The coarse typed acquisition path still
rejects `SECURITY_V1` at every stage, and although the runtime resolver now
selects a mode-aware security enumeration for a `SECURITY_V1` Task, it uses it
only to build the `SecurityHold`; the authority-bearing security acquisition entry
point is still not driven, so a live scheduler composition still cannot acquire a
`SECURITY_V1` Task. B5.2-E deliberately writes no Attempt, Lease or
`ProvisioningBinding`, and the durable block still has no production writer
because B5.4 owns the imported profile it is keyed by. This record therefore
claims no current privilege escalation and no current production defect. It does
correct two statements that revision 1 and the merged probes made: that the
imported-profile constructor is fenced on the supported surface, and that
qualification settles the ADR-0013 §5 positive-obligation requirement on the
complete effective policy. Revision 20 corrects a third: revision 17's claim that a
value rebuilt through the public `from_parts` could not become accepted evidence,
which was false while the durable writer accepted it.

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
| 13 | `458e86c` (reviewed head; code in this pull request) | Records the third review pass: the `SECURITY_V1` path intersected the frozen B.4 conjunct but that conjunct still carried the `COARSE_V1` exact-reference check on the Task's authored sandbox policy, so a legal independent Task fine policy was refused before the coverage proof. Splits `can_provision_task_source_config_reusable` (ownership/disposition/identity/lifecycle/continuity/envelope/evidence/Task join, no Task policy reference) from `can_provision_task_source_config` (+ the coarse Task reference claim); the security branch calls the reusable conjunct. Adds `b5_enforcement::the_task_policy_reference_is_coarse_only` and `b5_security_acquisition::a_task_fine_policy_without_an_exact_evidence_reference_is_still_selectable`; ADR-0014 §3 names the third replaced relation. No ADR `Status:` line, spec status or index entry changes. |
| 14 | `35e817c` (reviewed head; code in this pull request) | Records the execution-boundary review: P1 — a `Claim` from `acquire_security_v1_task_existing` could still be presented to the frozen B.4 `create_execution_with_snapshot` path and then to `confirm_running_and_renew`, because qualification persists no `ExecutionSecuritySnapshot` or receipt. `requirement::task_is_security_v1` reads the durable mode; `create_execution_in_tx` and `confirm_running_and_renew` now reject `SECURITY_V1` before any Execution insert or `RUNNING` write. Adds `b5_security_acquisition::a_security_v1_claim_cannot_enter_the_b4_execution_or_running_path`, which starts from the acquired `Claim`, asserts zero `executions`/`binding_snapshots`, and proves a stale `STARTING` row is not promoted. ADR-0014 §3 states the Kernel boundary. No ADR `Status:` line, spec status or index entry changes. |
| 15 | `2e6e5c9` (reviewed head; code in this pull request) | Records the semantic-completion review: P1 — a `SECURITY_V1` `Claim` could be acknowledged with `execution_id = None`, writing `SUCCEEDED`/`COMPLETED`, a Result and dependency release without any Execution, snapshot or receipt. `ack_success` now rejects `SECURITY_V1` before any state change; the B5.4 secured acknowledgement path must be defined explicitly. P2 — rewrites ADR-0014 §3 so the transaction re-proof covers durable truth and imported-fact consistency, not external physical truth, and records the four proof layers. Adds `b5_security_acquisition::a_security_v1_claim_cannot_be_acknowledged_as_success`, which also proves a BLOCKED dependent is not released. Note: because a `SECURITY_V1` Task cannot currently reach an Execution, `ack_success` rejects it even when an `execution_id` is supplied, which is deliberate and conservative. No ADR `Status:` line, spec status or index entry changes. |
| 16 | `d37c887` (reviewed head; docs-only) | Records the continuation-baseline review: no new P1; the Kernel execution/RUNNING/acknowledgement guards are accepted as the fail-closed authority boundary. Confirms PR #30–#36 merged, so items 1–4 and 8–10 of the follow-up list are landed rather than pending here. Adds the B5.2-E liveness acceptance condition (a `SECURITY_V1` Task must not cycle `QUEUED → LEASED → lease expiry` while B5.4 is disabled) to ADR-0014 §3 and to the follow-up list. Confirms, rather than rewords, the credential-necessity and B5.4 subject-identity constraints. Docs-only; no code, ADR `Status:` line, spec status or index entry change. |
| 17 | `agent/m6b5-b5.2-c-trusted-importer` (code) | Implements B5.2-C after PR #37 merged at `e68a771`. Adds `EnforcementProfileAttestation` (`ENFORCEMENT_PROFILE_ATTESTATION_SCHEMA_VERSION = 1`, protocol `agentype-enforcement-profile-attest/1`) with a gated producer constructor and an ungated digest-recomputing durable read. Adds schema v11 `enforcement_profile_attestations`: immutable rows keyed by `attestation_digest`, indexed by `profile_digest`, write-once, with a physical-domain-stability trigger. `finish_acquisition`'s `SecurityV1` arm re-reads the freshest row valid at `now` and rejects absence or expiry with `InvalidAuthority` before the reusable B.4 conjunct, tuple validation or qualification. New negatives cover unattested, stale-then-refresh, wider support, forged subjects, foreign domain, and write-once/tamper evidence. ADR-0014 §4 (schema v11) and §5 state the row and gate. No ADR `Status:` line, spec status or index entry change. |
| 18 | `agent/m6b5-b5.2-d-candidate-identity` (code) | Implements B5.2-D after PR #38's B5.2-C branch. Adds `SecurityCandidateIdentity` (`owner` = exact `LogicalAgentId` or pinned `AgentType` revision, exact source/config revision, config digest, resolved physical binding) and replaces `CandidateDomain::from_ids` with `from_identities`. Adds the resolver `SecurityCandidateRound`: it freezes every enumerated candidate with its full identity, deduplicates only exact identities, exposes the complete round cost, rejects a budget below the whole round, and builds the durable `security_acquisition_blocks` defer from the complete domain. `prepare_selection` now ranks only the frozen round, so an agent with two configs or two bindings cannot be collapsed. New tests pin identity components, same-agent distinct configs/bindings, whole-round budget exhaustion, and durable same-condition behavior. ADR-0014 §4 states the identity and round. No schema bump (the durable digest is opaque), no ADR `Status:` line, spec status or index entry change. |
| 19 | `agent/m6b5-b5.2-e-scheduler-wiring` (code) | Implements B5.2-E after PR #39's B5.2-D branch. Adds `CandidateMode::{Coarse, SecurityV1}` and the `resolve_security_source_candidates{,_for_agent}` entry points, which apply `can_provision_task_source_config_reusable` and `Kernel::check_typed_security_readiness` while the coarse path keeps its frozen predicate and the B5.1 `SECURITY_V1` readiness rejection. `acquire_typed_task` reads the durable Task mode first and refuses a `SECURITY_V1` Task with `ProvisioningResolutionError::SecurityHold` after enumerating the complete round; the runtime reports `DispatchOneOutcome::TypedDeferred` and a dedicated `security_hold_cooldown` (default 30 s). The Task stays `QUEUED` with no Attempt/Lease/binding/NACK across cycles and lease expiry. New liveness tests cover a populated and an empty security round. ADR-0014 §3/§4 state the hold, the readiness split and the deferred durable block. No schema bump, no ADR `Status:` line, spec status or index entry change. |
| 20 | `agent/m6b5-b5.2-e-scheduler-wiring` (code) | Records the joint B5.2-C/D/E audit and fixes its two actionable findings. **P1:** a digest-correct attestation built from public parts (`from_parts`) could be written as trusted physical evidence. `EnforcementProfileAttestation` now carries a private `producer_sealed` flag that only the gated `new` constructor sets; the durable read stays unsealed, `record_enforcement_profile_attestation` rejects an unsealed value with `InvalidAuthority` and writes no row, and `PartialEq` compares the attested facts so write-once idempotence still works without making the read path writable. **P2:** a held `SECURITY_V1` Task returned `TypedDeferred` immediately and consumed the cycle's only typed probe slot ahead of a runnable `COARSE_V1` Task. `TypedCandidateAttempt::{Outcome, Deferred, Held}` makes a hold candidate-local: it cools the Task down, keeps scanning, and reports the hold detail only when nothing in the window was started. New tests: `b5_security_acquisition::a_self_consistent_attestation_without_producer_provenance_is_rejected` and `provisioning_resolver::tests::a_held_security_task_does_not_starve_a_runnable_coarse_task`. ADR-0014 §3/§5 state both. No durable-format, ADR `Status:`, spec status or index change. Not a B5.2 freeze: runtime whole-round budget enforcement and the B5.4 secured start remain open. |
| 21 | `agent/m6b5-b5.2-e-scheduler-wiring` (code) | Records the round-3 audit of `a451839` and fixes its single actionable code finding. **P1:** a `SECURITY_V1` hold whose read-only `SourceConfig::attest` exceeded the cycle deadline returned `ProvisioningDeadlineExceeded`, which `defer_or_fail_typed` treated as fatal, so `control::cycle`'s `dispatch_typed_one(...)?` aborted the round and starved an unrelated runnable `COARSE_V1` Task. Attestation redaction now preserves that category with a fixed message; `security_hold` stops scanning the shared-deadline probe and returns a **provisional** hold (`SecurityHold::is_provisional`) whose diagnostic reports a lower bound and "the round is not complete". Integrity, storage and authority faults still propagate and fail closed. New tests: `provisioning_resolver::tests::a_security_hold_with_timed_out_attestation_does_not_block_runnable_coarse_task` and `a_security_hold_integrity_fault_still_fails_closed`. ADR-0014 §3 states the provisional hold. No durable-format, ADR `Status:`, spec status or index change. Still not a B5.2 freeze: importer registration/observation authority, runtime whole-round budget enforcement and the B5.4 secured start remain open. |
| 22 | `agent/m6b5-b5.2-e-scheduler-wiring` (code) | Fixes the three code-level findings of the round-3 audit over `7dca374` and adds its requested load test. **P2-C:** attestations had no validity ceiling, so one observation could claim to outlive the installation it describes; `ENFORCEMENT_PROFILE_ATTESTATION_MAX_VALIDITY_SECS` (3600 s) is enforced in `validated_parts`, i.e. on construction *and* on the durable read. **P2-D/E:** `security_hold` is several consecutive reads, so a concurrent publish could make the reported union never have existed; `HoldProbeSnapshot` (catalog frontier digest, pinned `AgentTypeRef`, sorted matched roster) is compared before and after enumeration, and drift yields `HoldProvisionalCause::CatalogDrift`. **P2-E:** `TypedReadinessMode::{CoarseAcquisition, SecurityV1}` replaces `reject_security_v1: bool`, so each probe proves its own Task mode and rejects the other's (and `LEGACY`). New tests: over-long window rejected at both constructors; `each_readiness_probe_requires_its_own_mode`; catalog-drift drift ⇒ provisional; `held_security_tasks_yield_to_coarse_work_under_a_single_probe_window` (`probe_limit=1`, near-deadline attestations). The `probe_limit=1` test is a bounded-cycle liveness proof, not a latency guarantee. ADR-0014 §3/§4/§5 state the fence, the mode proof and the ceiling. No durable-format, ADR `Status:`, spec status or index change. Scope decision recorded: the original B5.2 gate stands (registry, installation epoch and runtime budget stay B5.2; B5.4 only commits/executes), so B5.2 is still not frozen. |

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

Revision 13 fixes one defect that a further review found in the revision-10
intersection: the reusable B.4 conjunct still demanded the imported evidence's
exact reference for the Task's own authored sandbox policy, which rejected a legal
independent Task fine policy before the `P_eff` coverage proof. The conjunct is
split; the coarse exact-reference claim is unchanged. Revision 13 still does not
claim a B5.2 freeze, completion, or a B5.4 secured start.

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

## Task fine-policy exact-reference audit (revision 13)

The third review pass found one new policy-matching defect, in the conjunct
revision 10 added:

```text
can_provision_task_source_config
    ├── ownership / disposition / evidence identity / lifecycle / continuity
    ├── functional envelope + security-class evidence
    ├── Task safety, continuity and restriction join
    └── if let Some(policy) = &task.sandbox_policy {
            evidence.enforces_sandbox_policy(policy)   // COARSE_V1 exact reference
        }
```

`SECURITY_V1` called that whole relation. A Task that authors its own fine policy
`P_T` distinct from the Generation's `P_G` is therefore refused before
`qualify_security_candidate` whenever the imported evidence does not list `P_T`'s
exact reference, even when `meet(P_G, P_T, P_A, P_S)` is executable and the
profile covers every obligation of `P_eff`. That is the `COARSE_V1` evidence
relation, not the policy order B5.2 is migrating to, and it rejects exactly the
independent Task fine policy B5.1 permits.

The fixtures hid it: every acquisition fixture admitted a Task with
`sandbox_policy: None`, so the branch never ran.

**Fix.** The relation is split into two public names, so a `SECURITY_V1` caller
cannot accidentally reach the coarse check:

| Function | Contents | Caller |
|---|---|---|
| `can_provision_task_source_config_reusable` | everything except the Task authored-policy reference claim | `SECURITY_V1` acquisition |
| `can_provision_task_source_config` | the reusable conjunct + `evidence.enforces_sandbox_policy(task.sandbox_policy)` | `COARSE_V1` (`can_provision_task`) |

Both delegate to one private function whose flag only decides that check, so the
coarse predicate keeps its exact ordering and error. The Task layer is still
proven on the security path: `TaskSecurityBinding` already meets the Task authored
policy into `P_eff`, and `compile_execution_security` proves obligation coverage
over that policy. ADR-0014 §3 now names this as the third replaced relation
alongside `can_execute`'s Task↔AgentType match and `can_provision`'s evidence
reference claim.

**Tests.**

| Test | What it pins |
|---|---|
| `b5_enforcement::the_task_policy_reference_is_coarse_only` | evidence with no authored-policy claim: the reusable conjunct accepts, the coarse predicate rejects with `SecurityUnenforceable` |
| `b5_security_acquisition::a_task_fine_policy_without_an_exact_evidence_reference_is_still_selectable` | a real admission with a distinct Task fine policy reference; the binding derivation carries that layer; acquisition reaches Leased with one Attempt/Lease |

Both were mutation-checked: pointing the security branch back at the coarse
predicate fails the acquisition test with `Task sandbox policy ... is not enforced
by imported evidence`, and removing the Task-reference flag from the coarse
predicate fails the contract test.

## Execution-commitment boundary audit (revision 14)

The fourth review pass found a new **P1** combination: authority-bearing
acquisition and the old execution entries were not closed against each other.

`acquire_security_v1_task_existing` proves `Eligible_B5.2` and creates a real
Attempt, Lease and `ProvisioningBinding`, but it discards the compiled
`ProvenExecutionSecurity`; no `ExecutionSecuritySnapshot` and no B5.4 receipt are
persisted. The B.4 entries remained open:

```text
acquired Claim
   ├── create_execution_with_snapshot   (freezes only a BindingSnapshot)
   │      └── create_execution_in_tx    ("typed ⇒ snapshot" only; no mode check)
   └── confirm_running_and_renew        (writes RUNNING from ACTIVE lease)
```

The previous "no caller yet" observation is a reachability fact, not an authority
boundary. Once B5.2-E wires the scheduler, a `Claim` from this entry could be
presented to either path without the B5.4 commitment. This was a latent
composition risk, not a live production path — the daemon still does not call the
new acquisition entry — but it must not depend on caller convention.

**Fix (Kernel authority boundary).**

| Boundary | Change |
|---|---|
| `requirement::task_is_security_v1` | the single durable read: `LEGACY` is never security-v1; `TYPED` resolves `task_security_mode`, so a missing requirement row still fails closed |
| `create_execution_in_tx` | rejects a `SECURITY_V1` Task after authority validation and before any insert, so both `create_execution` and `create_execution_with_snapshot` refuse it; a `BindingSnapshot` is not execution authority |
| `confirm_running_and_renew` | rejects the same Task immediately after authority validation, before the `RUNNING`/task/incarnation/lease updates, so a stale `STARTING` Execution cannot be promoted |

The guards read durable Task state, not the caller, so they hold for every
runtime-internal entry and survive scheduler wiring. They stay until B5.4 commits
the security snapshot and receipt chain; B5.2-E must not remove them.

**Acceptance test.**
`b5_security_acquisition::a_security_v1_claim_cannot_enter_the_b4_execution_or_running_path`
starts from the `Claim` produced by `acquire_security_v1_task_existing` and:

1. calls `create_execution_with_snapshot` with a matching physical binding and a
   `BindingSnapshot`; requires `InvalidAuthority` naming `SECURITY_V1`, leaves
   `executions` and `binding_snapshots` empty, and keeps the Task `Leased`;
2. inserts a stale `STARTING` Execution row for that Attempt and calls
   `confirm_running_and_renew`; requires the same rejection and leaves the row
   `STARTING` and the Task `Leased`.

Both guards were mutation-checked:

| Guard disabled | Observed failure |
|---|---|
| the `create_execution_in_tx` mode check | `create_execution_with_snapshot` proceeds to snapshot validation, failing the test on the missing `SECURITY_V1` rejection (`binding snapshot source config digest ...`) |
| the `confirm_running_and_renew` mode check | the call returns `Ok(RunningAuthorityGrant)`; the test's `unwrap_err` fails, proving the path would have written `RUNNING` |

## Semantic-completion boundary audit (revision 15)

The fifth review pass found the same root cause at a third boundary. Revision 14
closed Execution creation and `RUNNING`, but `Kernel::ack_success` still accepted a
`SECURITY_V1` Attempt:

```text
acquire_security_v1_task_existing
    → Claim + Attempt + Lease
    → ack_success(execution_id = None)
    → Attempt = SUCCEEDED, Lease = RELEASED, Task = COMPLETED
    → Result = AVAILABLE, dependencies released, batch recomputed
```

`ack_success` validates the Claim but did not read the Task mode;
`execution_for_attempt(..., None)` returns `Ok(None)`, and `results.execution_id` is
nullable. No physical execution, `ExecutionSecuritySnapshot` or receipt was needed
to publish a successful Result. Like revision 14, this was a latent composition
risk (the daemon does not call the acquisition entry), not a live production
bypass, but "valid Claim" is not "every later transition is authorized".

**Fix.** `ack_success` rejects a `SECURITY_V1` Task immediately after authority
validation and before the quiescence, Execution, incarnation, Attempt, Lease, Task
or Result writes. The B5.4 secured acknowledgement path must be defined explicitly
later; it must not be inferred from a Claim, and the optional `execution_id` is not
proof. Because a `SECURITY_V1` Task cannot currently reach an Execution, the guard
rejects even a supplied `execution_id`; that is deliberate and conservative.

**Acceptance test.**
`b5_security_acquisition::a_security_v1_claim_cannot_be_acknowledged_as_success`
starts from the acquired `Claim`, adds a `BLOCKED` dependent in the same Batch, and
calls `ack_success(..., execution_id = None, ...)`. It requires
`InvalidAuthority` naming `SECURITY_V1`, then asserts the Task is still `Leased`,
Attempt and Lease are still `ACTIVE`, no Result exists, and the dependent is still
`BLOCKED`. Mutation check: disabling the guard returns
`Ok(Some(ResultId(...)))`, and the test's `unwrap_err` fails.

**P2, transaction wording.** ADR-0014 §3 no longer claims that the transaction
re-proves everything except the profile and subject. It now separates durable
truth (re-read in the transaction), imported observation (provenance, identity,
version, freshness — B5.2-C), physical enforcement (trusted importer/OS capability
— B5.2-C) and applied enforcement (B5.4 secured start and receipt). The resolver
selection carries imported facts that the transaction can cross-check but cannot
turn into trusted physical evidence.

**B5.4 recovery note.** `record_physical_outcome` can still record a `RUNNING`
observation against an existing `STARTING` row; the revision-14 guard covers only
`confirm_running_and_renew`. B5.4 must classify a `SECURITY_V1` history without a
security snapshot as recovery-required, not as a valid secured execution. No code
change is made here, because no `SECURITY_V1` Execution can be created.

## Continuation-baseline audit (revision 16)

The sixth review pass examines the revision-15 head `d37c887` and finds **no new
P1** in the paths it re-checks. It accepts the three Kernel guards — Execution
creation, `RUNNING`, acknowledgement — as a fail-closed authority boundary for
this stage, and accepts the draft only as a B5.2 continuation.

**Baseline correction.** PR #30–#36 are merged at the merge-base `535b278` of this
branch. PR #35 bound each source declaration to its `SpawnSource` revision and
completed the durable block's validated read; PR #36 recorded that pass. Items 1–4
and 8–10 of the follow-up list below therefore describe landed work, not pending
work in this draft. The revision-9 acquisition entry and the revision-14/15 Kernel
guards remain the only unmerged parts of this chain.

**What the review confirms.** The lockout is at the Kernel, not at the scheduler
call site: a valid Claim is not authority for a later transition, and each
transition names its own proof. `create_execution_in_tx`,
`confirm_running_and_renew` and `ack_success` each reject `SECURITY_V1` before
their writes, and `a_security_v1_claim_cannot_be_acknowledged_as_success` shows
that no Result, dependency release or state change escapes the refusal. The
review's split between "can this be a trusted base for continued B5.2 work" and
"is B5.2 complete" matches the record: the first is conditionally accepted, the
second is not.

**Three hard boundaries stay.** They are P1 conditions for the follow-ups, not
defects this draft can close:

1. **B5.2-C — trusted importer.** A self-consistent profile digest cannot replace
   provenance, physical-domain ownership and freshness. The acceptance cases must
   prove that a caller cannot widen support, forge subject coverage, reuse stale
   physical-domain evidence, or swap the physical key under one alias and still
   produce trusted evidence.
2. **B5.2-D — complete candidate identity.** `CandidateDomain::from_ids` still
   counts only `LogicalAgentId`, so one agent with two SourceConfigs or two
   physical bindings cannot be ranked correctly. The first commit and test must
   freeze `(AgentType/LogicalAgent, source revision, config revision + digest,
   physical binding)`, and no candidate may be selected from a partial set or a
   silently narrowed budget.
3. **B5.2-E — scheduler wiring and liveness.** Wiring must revalidate the whole
   round before Attempt/Lease and keep `SECURED_QUALIFIED` non-executable. The
   review adds a liveness condition: with B5.4 disabled, production acquisition
   must not repeatedly move a `SECURITY_V1` Task from `QUEUED` to a `LEASED` state
   that can only expire. The scheduler must either hold an explainable waiting
   state or decline production acquisition; an unexecutable qualification must not
   be presented as an ordinary runnable Claim. This is now recorded in ADR-0014 §3
   and as follow-up item 21.

**Two P2 constraints remain recorded, unchanged.** Credential delivery in the pure
component is only a necessary condition: `with_source_config` may settle a request
against a delivering profile, but authority-bearing acquisition still refuses every
non-empty `credential_refs` until B5.3 supplies the broker proof (follow-up item 6
and ADR-0014 §3). And `SecurityEvidenceSubject::digest()` still commits to the
profile, not the chosen subject, so B5.4 must bind
`ExecutionSecurityEvidence = (PlanIdentity, ProfileIdentity, SubjectIdentity)`
into the snapshot and receipt (ADR-0014 §5). Both were already stated in revisions
10 and 11; revision 16 confirms them rather than re-deriving them.

**Verdict.** Conditional acceptance as a B5.2 continuation draft; no B5.2 freeze,
no B5.4 claim, and no conversion of this draft's single-candidate success test into
production orchestration.

## Trusted-importer provenance implementation (revision 17)

PR #37 merged at `e68a771`, so the revision-16 baseline is now `main`. Revision 17
implements the first hard boundary, B5.2-C, on
`agent/m6b5-b5.2-c-trusted-importer`. It does not reopen the earlier findings and
does not touch B5.2-D or B5.2-E.

**The missing fact.** `ImportedEnforcementProfile::import` computes its digest from
the caller's own support matrix, covered subjects and physical key. The digest
therefore proves internal consistency, not that a trusted importer observed that
domain. Revision 17 adds the separate durable fact:

- `EnforcementProfileAttestation` commits to `(profile_digest, importer_id,
  importer_protocol, adapter_kind, adapter_binding_key, issued_at, valid_until)`
  through its own `attestation_digest`.
  `ENFORCEMENT_PROFILE_ATTESTATION_SCHEMA_VERSION` is `1` and the accepted
  protocol is `agentype-enforcement-profile-attest/1`; a row under another
  protocol is rejected, not reinterpreted.
- `new` is gated with `provisioning-producer` (the producer's write path);
  `from_parts` is ungated so the storage library can rebuild a row without the
  producer feature. `from_parts` recomputes the digest and rejects a stored value
  that does not describe its parts, so a tampered row fails closed. The digest
  function itself is private, so `from_parts` is a read/reconstruction path, not a
  way for an ordinary caller to mint a new attestation identity. **Revision 20
  corrects the rest of this bullet:** the digest inputs are public
  (`canonical_json_body_digest` is exported, and the schema tag, protocol and field
  names are documented), so a caller limited to `from_parts` could compute a
  self-consistent value; and revision 17's writer accepted it. Acquisition reading
  the durable row rather than the caller's value was therefore not sufficient
  authority, because the caller could write that row. Revision 20 seals only the
  gated producer constructor and rejects an unsealed value at the writer.
- Freshness is half-open (`issued_at <= now < valid_until`). A non-finite clock is
  never fresh.

**Durable encoding.** Schema v11 adds `enforcement_profile_attestations`, keyed by
`attestation_digest` and indexed by `(profile_digest, issued_at DESC)`. Rows are
write-once: UPDATE and DELETE are rejected, and one profile digest cannot move its
`(adapter_kind, adapter_binding_key)` between rows. A refresh is a new immutable
row, never an edit. `record_enforcement_profile_attestation` is idempotent for an
identical digest and rejects a digest that describes different parts.

**The gate.** `finish_acquisition`'s `AcquisitionMode::SecurityV1` arm now starts
with the transactional read
`current_enforcement_profile_attestation(tx, profile_digest, now)`. Absence and
expiry both reject with `InvalidAuthority` before the reusable B.4 conjunct,
`ValidatedSourceBinding::validate`, `qualify_security_candidate`, config
settlement or any Attempt/Lease. The caller cannot supply the attestation. The
selected profile must also name the same physical domain and pass `validate_at`;
because the profile digest already commits to support and subjects, a wider
matrix or forged subject set is a different digest and therefore unattested.

**Acceptance cases pinned by revision 16.** The new negatives in
`b5_security_acquisition` map directly:

| Revision-16 case | Test |
|---|---|
| no forged provenance | `a_profile_without_a_trusted_importer_attestation_is_rejected_before_attempt_or_lease` |
| stale physical-domain evidence is not reusable | `a_stale_trusted_importer_attestation_is_rejected_until_a_fresh_row_exists` |
| a caller cannot widen support | `a_wider_support_profile_cannot_borrow_a_narrower_attestation` |
| forged subject coverage | `a_forged_subject_coverage_cannot_borrow_the_attested_profile` |
| one alias cannot swap the physical key | `an_attestation_for_another_physical_domain_cannot_authorize_this_binding` |
| immutability and tamper evidence | `a_trusted_importer_attestation_is_write_once_and_tamper_evident` |

Contract-level tests in `b5_enforcement` pin the digest stability and recompute,
blank identities and foreign protocol, non-finite/empty windows, exact
profile/domain binding, and half-open freshness.

**What remains open.** B5.2-C gates *selected-candidate* acquisition but does not
enumerate a whole round, rank distinct configs/bindings under one agent, or make
an exhausted budget durable; that is B5.2-D. It also does not wire the security
entry point into the resolver or scheduler, so production acquisition still cannot
select a `SECURITY_V1` Task and the B5.2-E liveness condition is unchanged. The
attestation is the importer's external observation; the actual enforcement-subject
attestation and the secured start remain B5.4 (ADR-0014 §5).

## Complete candidate identity implementation (revision 18)

Revision 18 implements B5.2-D, which revision 17 left open: the
candidate is no longer an agent id. `CandidateDomain::from_ids` is removed. Its
replacement, `CandidateDomain::from_identities`, consumes
`SecurityCandidateIdentity`:

```text
owner              exact LogicalAgentId, or the pinned AgentType id + revision
source revision    exact SpawnSource id + revision
config revision    exact SourceConfig id + revision
config digest      opaque body/locator identity, not only the readable config id
physical binding   resolved (adapter_kind, adapter_binding_key), never the alias
```

Every component is in `SecurityCandidateIdentity::digest()`, and the domain is the
ordered set of those digests. Two config revisions or two physical bindings under
one agent therefore count as two candidates and produce a different domain digest;
reordering the same identities does not.

**The round.** `SecurityCandidateRound` is the resolver's frozen unit of identity,
cost and defer. It keeps every enumerated candidate with its full identity,
deduplicates only exact identities, refuses a new-agent round whose candidates do
not share the pinned `AgentType` revision, and exposes the complete cost. Its
`require_whole_round` delegates to `AcquisitionBudget`; there is no API that
selects a winner from a subset or spends a narrowed budget on the first few
successes. `prepare_selection` now builds the round and ranks only
`round.candidates()`, so the coarse path cannot collapse distinct configs either.

**The durable defer.** `SecurityCandidateRound::budget_defer_block` builds the
existing `security_acquisition_blocks` row from the complete domain and cost. It
adds no schema: the domain digest is opaque, so the durable row already binds the
full identity set. The block's same-condition test now distinguishes two configs
or two bindings under one agent. A new row is still required only when the
complete identity set changes.

**Tests pinned by revision 18.**

| Case | Test |
|---|---|
| identity commits to owner kind/revision, source and config revision, config digest and physical key | `b5_enforcement::the_candidate_domain_digest_is_a_stable_full_identity_set` |
| one agent with two configs is two candidates | the same test |
| one config with two physical bindings is two candidates | the same test |
| the whole-round cost is the full identity count, and one short on either bound defers | `b5_enforcement::the_whole_round_budget_is_measured_in_full_candidate_identities` |
| a durable block's same-condition identity changes with a config or binding | `b5_enforcement::a_block_is_released_only_by_operator_action_or_configuration_change`; `b5_acquisition_block::one_agent_with_a_different_config_or_binding_is_a_new_condition` |
| the resolver round deduplicates exact identities, rejects a foreign pinned type and defers the whole round | `provisioning_resolver::tests::a_security_round_counts_full_candidate_identities_not_agents` |

**What remains open.** The round is defined and used by the resolver's selection
path, but the security entry point is still not wired into the Scheduler: the
production resolver continues to reject `SECURITY_V1`, and no control-plane caller
writes the durable defer automatically. That is B5.2-E, together with the liveness
condition (with B5.4 disabled, a `SECURITY_V1` Task must not cycle
`QUEUED → LEASED → lease expiry`). B5.4 still owns the subject-bound
`ExecutionSecurityEvidence` and the secured start.

## Scheduler wiring and liveness implementation (revision 19)

Revision 19 implements B5.2-E, which revision 18 left open. It makes the durable
Task mode — not the caller — choose the acquisition path, and it keeps a
`SECURITY_V1` Task out of the ordinary lease cycle while B5.4 is disabled.

**Mode-aware enumeration.** `CandidateMode::{Coarse, SecurityV1}` selects the
predicate and the readiness gate, not just a filter. Both modes still read the
same durable catalog and produce the same `SourceProvisioningCandidate` shape, but
they cannot substitute for one another:

- `CandidateMode::Coarse` calls `check_typed_acquisition_readiness` (which still
  rejects `SECURITY_V1`) and evaluates `can_provision_task`, including the frozen
  coarse exact-reference relations.
- `CandidateMode::SecurityV1` calls `Kernel::check_typed_security_readiness`
  (same Task/Batch/partition/backoff proof, without the coarse-only mode
  rejection) and evaluates `can_provision_task_source_config_reusable`. It adds no
  coarse exact-reference claim and deliberately omits `can_execute`, whose
  `sandbox_policy_within` demands reference equality and would wrongly refuse a
  legal narrower security Task policy. Qualification over `P_eff` and the imported
  profile stays with B5.4.

`resolve_source_candidates` / `resolve_source_candidates_for_agent` keep their
coarse signatures and behaviour; the security variants are
`resolve_security_source_candidates` / `resolve_security_source_candidates_for_agent`.

**The hold.** `acquire_typed_task` reads `Kernel::task_security_mode` first. A
`LEGACY` or `COARSE_V1` Task keeps the existing path unchanged. A `SECURITY_V1`
Task is enumerated and then refused with
`ProvisioningResolutionError::SecurityHold` before any Attempt, Lease,
`ProvisioningBinding` or Task NACK. `security_hold` enumerates every existing
owner the Task matches plus the pinned new-agent route, builds each
`SecurityCandidateRound`, and unions the identities into one `CandidateDomain`, so
the hold reports the complete round's count and opaque digest rather than whichever
owner enumerated first.

`Dispatcher::defer_or_fail_typed` reports the refusal as
`DispatchOneOutcome::TypedDeferred` and applies `security_hold_cooldown` (default
30 s, `TypedDispatchConfig::with_security_hold_cooldown`). That arm precedes the
candidate-local branch, so the hold can never silently become a 1 s coarse
deferral, and the dedicated bound keeps the hold a bounded waiting state instead of
a busy re-enumeration. The Task stays `QUEUED`; the control loop's lease-expiry
sweep has nothing to expire.

**Why no durable block.** The durable `security_acquisition_blocks` row is keyed
by `binding_profile_digest(binding, profile)`. B5.4 owns the imported enforcement
profile that function requires, so B5.2-E cannot write the row without inventing
it. The hold therefore carries only the complete domain facts; B5.4 can attach the
profile and convert the same condition into the durable block. B5.2-E is
**hold-only**: it does not claim B5.2 freeze and does not add any B5.4 capability.

**Tests pinned by revision 19.**

| Case | Test |
|---|---|
| a `SECURITY_V1` Task is held with a complete-round detail, no Attempt, and stays `QUEUED` across a lease-expiry sweep and a second cycle | `provisioning_resolver::tests::a_security_v1_task_is_held_without_an_attempt_across_cycles` |
| an empty security round is still a hold, not a coarse fallback or a Task failure (the coarse source is present but disabled security source is not substituted) | `provisioning_resolver::tests::a_security_v1_task_without_a_security_candidate_is_held_not_failed` |
| the coarse path still refuses a `SECURITY_V1` AgentType and the security path enumerates the same fixture | existing `provisioning_resolver::tests::a_security_v1_agent_type_has_no_coarse_candidate` plus the two tests above |

**Mutation check.** Classifying `SecurityHold` as candidate-local (or deleting its
`defer_or_fail_typed` arm) makes the hold fall through to `Ok(None)` and the 1 s
`candidate_cooldown`; `dispatch_typed_one` then reports `NoWork`, so the first
test fails at its `TypedDeferred` pattern and the dedicated
`security_hold_cooldown` bound is not exercised at all. Removing the
`SECURITY_V1` interception in `acquire_typed_task` makes the Task reach the coarse
path, where `check_typed_acquisition_readiness` returns `InvalidAuthority`; both
tests then fail because no hold is reported. This is exactly the regression the
liveness condition guards against: the failure mode is not "an extra Attempt" but
"the Task silently becomes unacquirable instead of explicitly held".

## Joint C/D/E audit fixes (revision 20)

The joint audit of the stacked B5.2-C/D/E heads (`3e044e2`, `69ac067`,
`cb63079`) accepted the D identity model and the E non-executing hold, and
conditionally accepted C's evidence storage. It named two code-level defects that
must close before the aggregate is re-audited. Revision 20 fixes exactly those two
and adds no new ontology.

### P1 — a self-consistent attestation is not trusted provenance

`EnforcementProfileAttestation::new` was behind `provisioning-producer`, but
`from_parts` was public and `record_enforcement_profile_attestation` was ungated.
The schema tag, protocol string, field names and canonical digest function are all
public, so a caller that could reach the internal writer could pick any
`importer_id`, profile digest, physical domain and window, compute a self-consistent
digest, reconstruct the value through `from_parts`, and persist it. Acquisition's
`validate_at` could then only check that the row agreed with itself, the profile and
the clock — never that an importer had observed anything.

The fix puts the distinction inside the value instead of relying on constructor
visibility:

- `producer_sealed` is private and defaults to `false`.
- Only `new`, still `#[cfg(any(test, feature = "provisioning-producer"))]`, seals.
- `from_parts` — the durable read path, which must stay available to
  `agentype-storage-sqlite` without the feature — always returns an unsealed value.
- `record_enforcement_profile_attestation` rejects an unsealed value with
  `Error::InvalidAuthority` before any insert, so no row exists afterwards.
- `PartialEq` is now manual and compares the attested facts (plus the digest that
  commits to them), never the seal. A durable
  read therefore still compares equal to the producer's value for write-once
  idempotence; the seal is a write capability, not part of the evidence identity.

This is a mechanical writer fence, not a claim that the importer registry is
complete. Which importers exist, and that they really observed the domain, remains
the trusted provisioning composition root's responsibility and B5.4's subject
attestation. The audit's scope note still holds: `agentype-storage-sqlite` is
`publish = false` and direct `Kernel` use is not a supported production API, so this
was not a demonstrated external Root-API bypass.

| Case | Test |
|---|---|
| a digest-correct attestation built from public parts, with no producer authority, is rejected and leaves no row; the producer's row is then readable, unsealed on read, and equal on facts | `b5_security_acquisition::a_self_consistent_attestation_without_producer_provenance_is_rejected` |

### P2 — a held security Task must not consume the typed cycle

`dispatch_typed_one` enumerates an ordered, `probe_limit`-bounded window and
returned on the first terminal outcome. `SecurityHold` produced
`Ok(Some(TypedDeferred))`, so a held `SECURITY_V1` Task ended the cycle before any
candidate behind it was examined. Ordinary deferrals returned `Ok(None)` and kept
scanning, so the hold was the only attempt outcome that consumed a probe slot. A run
of held security Tasks ahead of runnable coarse work delayed that work once per Task
per cooldown; the cooldown bounded the repeat rate but not the ordering effect.

Revision 20 replaces the `Result<Option<...>>` attempt shape with a private
three-way result:

```text
TypedCandidateAttempt::Outcome(DispatchOneOutcome)  terminal for this cycle
TypedCandidateAttempt::Deferred                     local defer, keep scanning
TypedCandidateAttempt::Held(String)                 hold detail, keep scanning
```

`dispatch_typed_one` now sets `deferred = true` for both non-terminal variants and
remembers the held detail. After the loop it reports `TypedDeferred` with the hold
detail if anything was held, the generic deferral if only ordinary candidates
deferred, and `NoWork` otherwise. The hold still applies
`security_hold_cooldown`, still creates no Attempt, Lease, `ProvisioningBinding` or
NACK, and still leaves the Task `QUEUED`; only the scan continues.

| Case | Test |
|---|---|
| an older held `SECURITY_V1` Task and a newer runnable `COARSE_V1` Task in the same probe: the coarse Task is started, and the security Task stays `QUEUED` with zero Attempts | `provisioning_resolver::tests::a_held_security_task_does_not_starve_a_runnable_coarse_task` |

**Mutation check.** Making the `Held` arm return `TypedDeferred` immediately (the
pre-fix control flow) makes that test fail with
`TypedDeferred { detail: "SECURITY_V1 acquisition is held until the B5.4 secured start exists: 1 complete candidate(s) ..." }`
and `attempt_count_for_task(&coarse_task) == 0`. The scan-on-hold behaviour was
restored and the test went green again.

### What revision 20 does not settle

- **Runtime whole-round budget discipline.** `SecurityCandidateRound` and
  `require_whole_round` remain pure-component proofs. `prepare_selection` still
  selects from the round the caller supplied, `security_hold` computes the complete
  domain but applies no budget gate, and no code path writes a durable
  `security_acquisition_blocks` defer yet. That row needs the B5.4 imported profile
  for `binding_profile_digest`. The acceptance condition is unchanged and stays with
  the slice that enables real secured acquisition: a real round must come from a
  complete trusted enumeration, the budget must bind before selection, no candidate
  may be selected while its qualification is unknown, and a true over-budget outcome
  must leave a re-verifiable durable defer.
- **B5.2 aggregate closure.** Revision 20 closes two named defects. It does not make
  B5.2 a frozen stage, does not provide a production secured acquisition path, and
  does not claim B5.4 capability. A re-audit of the final stacked SHA is still
  required.

## Round-3 audit fix (revision 21)

The round-3 audit over `a451839` accepted the provenance fence and the
scan-on-hold fairness rule and named three findings: trusted-importer
registration/observation authority (P1, a B5.2-C stage-acceptance condition), the
runtime whole-round budget (P2, a B5.2-D stage-acceptance condition), and one code
defect in the `SECURITY_V1` hold path (P1). Revision 21 fixes only the code defect
and records the other two as explicit, unclosed acceptance conditions.

### P1 — a timed-out source probe must not abort the typed cycle

`security_hold` enumerates the candidate round through the same
`SourceConfigIntegration::attest` seam as real acquisition. `attest` is read-only
and bounded by the cycle's absolute deadline, but a real integration
(`agentype-source-workspace-jail`) returns
`ProvisioningResolutionError::ProvisioningDeadlineExceeded` when the probe budget
is spent. Redaction classified that as `IntegrationFailure { kind: Unavailable,
candidate_ineligible: false, candidate_local: false }`, `defer_or_fail_typed`
routed it to the fatal `_ =>` arm, and `dispatch_typed_one` returned
`Err(DispatchError::TypedPreparation)`. Because `ControlLoopService::cycle` uses
`dispatch_typed_one(...)?`, one slow security probe ended the round instead of
letting a runnable `COARSE_V1` Task behind it dispatch.

The defect is real but narrow: the Task cannot execute under B5.4 whatever the
probe returns, so a probe timeout is a diagnostic problem, not an acquisition
failure. Revision 21 therefore separates "the external probe did not finish" from
"the evidence is wrong":

- Attestation redaction preserves `ProvisioningDeadlineExceeded` and rewrites its
  payload to a fixed message, so no source-private diagnostic escapes and the
  existing redaction tests still pass.
- `security_hold` catches that category, stops scanning (the deadline is shared by
  the whole probe, so every later attestation would fail for the same reason), and
  returns `SecurityHold` with `provisional = Some(HoldProvisionalCause::ProbeDeadline)`
  (revision 22 replaced the boolean with a typed cause).
- `SecurityHold::is_provisional()` exposes the distinction. The complete-hold text
  is unchanged; a provisional hold says "at least N candidate(s) in provisional
  domain D (... the round is not complete)".
- Every other error still propagates. Corruption, integrity, storage and authority
  faults are not softened into a hold, because they are evidence faults that must
  fail closed.

A provisional hold is therefore still a hold: it creates no Attempt, Lease,
`ProvisioningBinding` or NACK, and the Task stays `QUEUED`. What changes is that
`cycle` no longer aborts for it. It is also deliberately **not** a frozen round —
`candidates()` is a lower bound — so B5.4 must re-enumerate before converting it
into the §4 durable block.

| Case | Test |
|---|---|
| an older `SECURITY_V1` Task whose attestation exceeds the deadline, plus a newer runnable `COARSE_V1` Task: acquisition returns a provisional hold, the coarse Task is dispatched, the security Task stays `QUEUED` with zero Attempts, and the round does not return `Err` | `provisioning_resolver::tests::a_security_hold_with_timed_out_attestation_does_not_block_runnable_coarse_task` |
| an integrity fault during the same hold probe is not converted into a hold: it propagates, stays redacted, and leaves the Task `QUEUED` with zero Attempts | `provisioning_resolver::tests::a_security_hold_integrity_fault_still_fails_closed` |

**Mutation check.** Deleting both `ProvisioningDeadlineExceeded` arms from
`security_hold` (so the generic `Err(err) => return Err(err)` catches them) makes
the timeout test fail at the dispatch step with
`the provisioning integration exceeded the absolute deadline: the source integration did not finish attestation before its deadline`.
The arms were restored and the test went green again.

### What revision 21 does not settle

- **Trusted importer registration and observation authority (finding 1).** The
  `producer_sealed` flag is a writer fence, not proof of who observed what.
  `importer_id` is still a caller-supplied string, and no trusted composition root
  registers importers or authorizes one to observe a specific `AdapterBinding`. The
  required reverse test — an internal caller that knows every profile field and
  digest still cannot publish an acquirable attestation without the importer's
  authority — is not written. This stays an explicit B5.2-C acceptance condition.
- **Runtime whole-round budget (finding 3).** `SecurityCandidateRound`,
  `require_whole_round` and `budget_defer_block` remain callable data-model proofs.
  `security_hold` computes a domain, not a budgeted round, and no production path
  writes a durable `security_acquisition_blocks` defer. The durable row needs the
  B5.4 imported profile for `binding_profile_digest`, so this stays an explicit
  B5.2-D acceptance condition (follow-up row 22).
- **B5.2 aggregate closure.** Revision 21 closes one code defect. It does not make
  B5.2 a frozen stage, does not provide a production secured acquisition path, and
  does not claim B5.4 capability.

## Round-3 audit fixes (revision 22)

The round-3 audit over `7dca374` accepted the hold direction and named three
code-level findings plus one requested load test:

- **P2-C:** an attestation had no independent validity ceiling, so one claimed
  observation could outlive the physical installation it describes.
- **P2-D/E:** `security_hold`'s "complete domain" was assembled from several
  consecutive reads with no common observation snapshot.
- **P2-E:** `check_typed_security_readiness` was `typed_readiness(id, false)`, which
  meant only "do not reject `SECURITY_V1`" and therefore also accepted a
  `COARSE_V1` Task.
- The requested test: `probe_limit=1` with multiple `SECURITY_V1` Tasks whose
  attestation consumes nearly the whole budget must still let coarse work run.

Revision 22 fixes all four. The two stage-acceptance conditions the audit repeated
(trusted importer registry, production whole-round budget) remain open and are
recorded below.

### P2-C — an attestation window is now bounded, not only checked against `now`

`EnforcementProfileAttestation` required `issued_at < valid_until` and
`issued_at <= now < valid_until`. Both are point/structure checks: nothing bounded
`valid_until - issued_at`, so a producer could name a window years long and the
row would stay "fresh" after the adapter was reinstalled, the execution domain
moved, or the trusted importer changed. Revision 22 adds
`ENFORCEMENT_PROFILE_ATTESTATION_MAX_VALIDITY_SECS = 3600.0` and enforces it inside
`validated_parts`, the single path shared by the gated producer constructor `new`
and the public durable read `from_parts`. An over-long window is `InvalidNumber` at
both boundaries, so an old or hostile row fails closed instead of being trusted.
Exactly one hour remains legal; one second more does not.

The ceiling is deliberately **not** an installation-epoch proof. Re-installing the
physical adapter must invalidate older rows even inside the window, and that fact
cannot be derived from an attestation value: it needs a trusted registry binding
an importer to the exact `AdapterBinding` plus an explicit installed epoch. Without
that registry an epoch would be one more caller-supplied string, so it is recorded
as part of the same P1-C acceptance condition (row 16) rather than added as an
unproven field.

### P2-D/E — the hold's claimed round is fenced by an observation snapshot

`security_hold` enumerated each matched existing owner, then the pinned new-agent
route, as separate reads and unioned the identities. A concurrent publish could
therefore produce a domain that never existed at one instant:

```text
T1 existing path observes source A
T2 A deactivates, source B activates
T3 new-agent path observes source B
reported complete domain = {A, B}
```

That union is still only an operator diagnostic today (no Attempt/Lease), but it
must not be read as a frozen round when B5.4 converts it into a durable block.
Revision 22 adds `HoldProbeSnapshot`: the active catalog frontier digest, the
Task's exact pinned `AgentTypeRef`, and the sorted matched-agent roster
(`LogicalAgentId`, bound `AgentTypeRef`). The probe takes it before and after
enumeration. The roster is included because `catalog_frontier_digest` covers
active sources, configs, policies, security declarations and policy digests but
**not** agent bindings, so a changed binding would otherwise be invisible.
Equality is a coherence fence for "this union was observed across a stable catalog
and roster", not a transaction and not a proof of physical truth.

`SecurityHold.provisional` becomes `Option<HoldProvisionalCause>` with
`ProbeDeadline` and `CatalogDrift`; `provisional_cause()` exposes the distinction
and the `Display` text varies by cause. A drifted hold keeps its enumerated count
as a lower bound, does not spend a second snapshot read when the deadline was
already hit, and remains non-fatal: no Attempt, Lease, `ProvisioningBinding` or
NACK, so unrelated coarse work still dispatches.

### P2-E — each readiness probe proves its own Task mode

`reject_security_v1: bool` could express only one of the two directions: `false`
meant "do not reject `SECURITY_V1`", so `check_typed_security_readiness` returned
success for a ready `COARSE_V1` Task. The production entry
`acquire_typed_task` reads the durable mode first, so this was an interface hazard
for future (B5.4) callers rather than a live bypass. Revision 22 replaces the flag
with `TypedReadinessMode::{CoarseAcquisition, SecurityV1}`; `typed_readiness`
mechanically rejects the other path's Task, and the security arm also rejects a
`LEGACY` Task (no security contract). Both public probes are thin named wrappers, so
a caller cannot select a mode without naming it.

### Requested load test — bounded liveness under `probe_limit=1`

`held_security_tasks_yield_to_coarse_work_under_a_single_probe_window` queues three
`SECURITY_V1` Tasks (each attestation burns up to a 30 ms budget and then misses
it), then a runnable `COARSE_V1` Task, with `probe_limit = 1` and a 5 s hold
cooldown. Each cycle probes at most one held Task; the earlier Tasks stay cooling
while the window advances, so the coarse Task is reached in a bounded number of
cycles (asserted `<= 6`, actually four). Every held Task stays `QUEUED` with zero
Attempts throughout. This is a bounded-cycle proof, not a wall-clock latency
guarantee: the attestation budget itself is the latency bound.

### Scope decision (audit §四)

The audit asked for a formal choice between (1) keeping the original B5.2 gate —
trusted importer, installation epoch and production qualification/budget inside
B5.2, with B5.4 only committing and executing — and (2) redefining B5.2 as
non-executing substrate plus scheduler hold. This revision takes **(1)**: the
B5.2 completion condition keeps the trusted importer registry, the physical
installation epoch, and a production whole-round budget that binds before
selection. Consequence: B5.2 is **not frozen** by revisions 19–22. The hold,
provisional diagnostics and identity kernel are a continuing substrate, not a
stage closure, and the durable budget-defer row stays open until the B5.4 imported
profile exists.

### What revision 22 does not settle

- **Trusted importer registration, observation authority and installation epoch
  (P1-C).** `producer_sealed` fences the writer, and the one-hour ceiling bounds
  the claim, but `importer_id` is still caller-supplied and no composition root
  binds an importer to the exact `AdapterBinding` it observed. The required
  reverse test remains unwritten. This stays a B5.2-C stage-acceptance condition.
- **Production whole-round budget and durable defer (P1-D).** `SecurityCandidateRound`,
  `require_whole_round` and `budget_defer_block` remain pure data-model proofs;
  no production path writes `security_acquisition_blocks`, which needs the B5.4
  imported profile for `binding_profile_digest`. A provisional hold (deadline or
  drift) is explicitly a lower bound and cannot be converted without re-enumeration.
- **B5.2 aggregate closure.** Revision 22 closes three code defects and adds the
  requested liveness case. It does not freeze B5.2, does not deliver a production
  secured acquisition, and does not claim B5.4 capability.

| Case | Test |
|---|---|
| an over-long attestation window is rejected by the producer constructor and by the durable read; exactly one hour is accepted | `b5_enforcement::an_attestation_rejects_a_validity_window_longer_than_the_freshness_ceiling` |
| the security probe accepts only `SECURITY_V1`, rejects `COARSE_V1` and `LEGACY`; the coarse probe rejects `SECURITY_V1`; no probe creates authority | `b5_coarse_path_gate::each_readiness_probe_requires_its_own_mode` |
| a catalog publish during hold enumeration makes the hold provisional with `CatalogDrift`, not a complete round, while leaving one enumerated candidate | `provisioning_resolver::tests::a_security_hold_reports_provisional_when_the_catalog_drifts_mid_round` |
| `probe_limit=1`, three near-deadline `SECURITY_V1` holds, one coarse Task: coarse runs within a bounded number of cycles, holds stay `QUEUED` with zero Attempts | `provisioning_resolver::tests::held_security_tasks_yield_to_coarse_work_under_a_single_probe_window` |

**Mutation check.** Disabling the post-enumeration snapshot comparison
(`else if false && hold_probe_snapshot(...) != before`) makes the drift test fail at
`assert!(hold.is_provisional())`; the comparison was restored and the test went
green again.

## B5.1 handoff acceptance, re-checked

| Acceptance condition | Revision 2 |
|---|---|
| `match_existing_agents_for_task` selects the relation from the Task security mode | met |
| `more_specific_for_with_policies` uses the real policy order | met |
| A differing sandbox policy reference does not lose a legal existing-agent candidate | met, differential test |
| `can_provision` / acquisition uses the same complete qualification for `SECURITY_V1` | **met on the security chain from revision 9** (`d4d8dc3`), and completed in revision 10 (`0a0ed56`): `acquire_security_v1_task_existing` now intersects the frozen B.4 eligibility conjunct before it qualifies, and only then creates an Attempt/Lease. The coarse chain stays deliberately disjoint and still refuses `SECURITY_V1`. From revision 19 the runtime resolver does call the security **enumeration** entry point, but only to build an explainable hold; it does not call the authority-bearing `acquire_security_v1_task_existing`, so no Attempt/Lease is created for a `SECURITY_V1` Task |

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
| **(rev 13)** The reusable B.4 conjunct excludes the Task authored-policy exact-reference claim: `SECURITY_V1` proves the Task layer through `P_eff` coverage, `COARSE_V1` keeps the reference claim | `predicates.rs` `can_provision_task_source_config_reusable` / `can_provision_task_source_config`; `provisioning.rs` `finish_acquisition` (`AcquisitionMode::SecurityV1`) | `b5_enforcement::the_task_policy_reference_is_coarse_only`, `b5_security_acquisition::a_task_fine_policy_without_an_exact_evidence_reference_is_still_selectable` | this pull request (rev 13) |
| **(rev 14)** Qualification is not execution authority: a `SECURITY_V1` `Claim` cannot commit an Execution or enter `RUNNING` through the frozen B.4 paths before B5.4 | `requirement.rs` `task_is_security_v1`; `kernel.rs` `create_execution_in_tx`, `confirm_running_and_renew` | `b5_security_acquisition::a_security_v1_claim_cannot_enter_the_b4_execution_or_running_path` | this pull request (rev 14) |
| **(rev 15)** Qualification is not semantic completion authority: a `SECURITY_V1` `Claim` cannot be acknowledged into `COMPLETED`/Result/dependency release before B5.4 | `kernel.rs` `ack_success` (guard after authority validation) | `b5_security_acquisition::a_security_v1_claim_cannot_be_acknowledged_as_success` | this pull request (rev 15) |
| **(rev 17)** A profile digest is not provenance: `SECURITY_V1` acquisition requires a fresh durable trusted-importer attestation for the exact profile digest, importer protocol and resolved physical domain, re-read transactionally before any qualification, config settlement or Attempt/Lease | `enforcement.rs` `EnforcementProfileAttestation`; `profile_attestation.rs` `current_enforcement_profile_attestation`; `provisioning.rs` `finish_acquisition` (`AcquisitionMode::SecurityV1`) | `b5_enforcement::an_attestation_digest_is_stable_and_recomputable`, `an_attestation_binds_the_exact_profile_and_physical_domain`, `an_attestation_is_fresh_only_inside_its_half_open_window`; `b5_security_acquisition::a_profile_without_a_trusted_importer_attestation_is_rejected_before_attempt_or_lease`, `a_stale_trusted_importer_attestation_is_rejected_until_a_fresh_row_exists`, `a_wider_support_profile_cannot_borrow_a_narrower_attestation`, `a_forged_subject_coverage_cannot_borrow_the_attested_profile`, `an_attestation_for_another_physical_domain_cannot_authorize_this_binding`, `a_trusted_importer_attestation_is_write_once_and_tamper_evident` | this pull request (rev 17) |
| **(rev 22)** An attestation's claimed validity window is at most one hour: an over-long window is `InvalidNumber` on producer construction and on the durable read, while exactly one hour remains legal | `enforcement.rs` `ENFORCEMENT_PROFILE_ATTESTATION_MAX_VALIDITY_SECS`, `validated_parts` | `b5_enforcement::an_attestation_rejects_a_validity_window_longer_than_the_freshness_ceiling` | this pull request (rev 22) |
| **(rev 22)** Each typed readiness probe proves its own Task mode: the security probe refuses a `COARSE_V1` or `LEGACY` Task, and the coarse probe still refuses `SECURITY_V1`; neither grants authority | `kernel.rs` `TypedReadinessMode`, `typed_readiness`, `check_typed_{acquisition,security}_readiness` | `b5_coarse_path_gate::each_readiness_probe_requires_its_own_mode` | this pull request (rev 22) |

### ADR-0014 §4 durable encoding (schema v11)

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
| **(rev 17)** A trusted-importer attestation is immutable, write-once, keyed by its own digest and indexed by the exact profile digest; one profile digest cannot move its physical domain | `schema.rs` `enforcement_profile_attestations` + immutability/domain-stability triggers; `profile_attestation.rs` `record_enforcement_profile_attestation` | `b5_security_acquisition::a_trusted_importer_attestation_is_write_once_and_tamper_evident` | this pull request (rev 17) |
| **(rev 17)** A durable attestation is rebuilt through the validated constructor, so a row whose stored digest does not describe its parts fails closed | `enforcement.rs` `EnforcementProfileAttestation::from_parts`; `profile_attestation.rs` `raw_attestation_row` | `b5_enforcement::an_attestation_digest_is_stable_and_recomputable`, `b5_security_acquisition::a_trusted_importer_attestation_is_write_once_and_tamper_evident` | this pull request (rev 17) |
| **(rev 20)** A digest-correct attestation rebuilt from public parts is not producer authority: the writer rejects it with `InvalidAuthority` and writes no row, while the producer's row stays readable and equal on facts | `enforcement.rs` `EnforcementProfileAttestation::{producer_sealed,is_producer_sealed}`, manual `PartialEq`; `profile_attestation.rs` `record_enforcement_profile_attestation` seal check | `b5_security_acquisition::a_self_consistent_attestation_without_producer_provenance_is_rejected` | this pull request (rev 20) |
| **(rev 22)** A hold claims a complete round only if the catalog frontier, pinned AgentType and matched roster were identical before and after enumeration; drift yields a provisional lower bound (`CatalogDrift`) | `provisioning_resolver.rs` `HoldProbeSnapshot`, `hold_probe_snapshot`, `security_hold`, `SecurityHold::provisional_cause` | `provisioning_resolver::tests::a_security_hold_reports_provisional_when_the_catalog_drifts_mid_round` | this pull request (rev 22) |
| **(rev 22)** With `probe_limit=1`, several `SECURITY_V1` holds whose attestation nearly exhausts its budget do not starve a runnable coarse Task; the coarse Task runs within a bounded number of cycles and every hold stays `QUEUED` with zero Attempts | `provisioning_resolver.rs` `security_hold`; `lib.rs` `dispatch_typed_one`, `defer_or_fail_typed` hold arm, `TypedDispatchState::cooling_ids` | `provisioning_resolver::tests::held_security_tasks_yield_to_coarse_work_under_a_single_probe_window` | this pull request (rev 22) |

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

Revisions 12–16 add a fence check of their own, against
`fb3641f..d37c887` plus the revision-13, revision-14 and revision-15 code:

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §3 body text only) |
| Durable format / schema version | unchanged up to `d37c887`; revision 13's split is a predicate/alias change and revisions 14–15 add kernel checks, so no schema or stored-format change; revision 16 is docs-only |
| Compile cache key list | unchanged; the physical-key component from revision 11 stays |
| Coarse acquisition behavior | byte-for-byte the same relation: `can_provision_task_source_config` keeps the Task exact-reference claim and `b5_enforcement`'s coarse assertions still fail without it; the revision-14/15 guards read the Task mode and leave `COARSE_V1`/`LEGACY` Execution and acknowledgement paths unchanged |
| Untracked files | not staged, not committed, not deleted |

Revision 17 adds a fence check of its own, against `e68a771..HEAD` on
`agent/m6b5-b5.2-c-trusted-importer`:

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §4 heading and §5 body text only) |
| Durable format / schema version | **intentionally** `10 → 11`; adds only the `enforcement_profile_attestations` table, its profile index and its immutability/domain-stability triggers; no existing table, column or row changes, and the fresh-only open rule is unchanged |
| Compile cache key list | unchanged; the physical-key component from revision 11 stays |
| Coarse acquisition behavior | unchanged: the attestation gate is inside `AcquisitionMode::SecurityV1`, and `b5_coarse_path_gate` / `b4_provisioning_binding` pass |
| Untracked files | not staged, not committed, not deleted |

Revision 18 adds a fence check of its own, against
`e68a771..HEAD` on `agent/m6b5-b5.2-d-candidate-identity` (stacked on the
revision-17 branch):

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §4 body text only) |
| Durable format / schema version | unchanged at 11: `candidate_domain_digest`, `candidate_count` and the budget/cost columns already exist, and the richer digest stays an opaque value, so no table or migration change |
| Compile cache key list | unchanged; the physical-key component from revision 11 stays |
| Coarse acquisition behavior | unchanged: the round is built only on the security enumeration/selection path, and `prepare_selection` still selects the same ranked winner when no two identities collide |
| Durable-block identity | the same `security_acquisition_blocks` row and triggers; only the set of identities represented by the opaque domain digest is richer |
| Untracked files | not staged, not committed, not deleted |

Revision 19 adds a fence check of its own, against `HEAD` on
`agent/m6b5-b5.2-e-scheduler-wiring` (stacked on the revision-18 branch):

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §3/§4 body text only) |
| Durable format / schema version | unchanged at 11: `SecurityHold` is runtime-local and writes nothing, so no table, column or migration change |
| Compile cache key list | unchanged; the physical-key component from revision 11 stays |
| Coarse acquisition behavior | unchanged: the coarse `CandidateMode` calls the same `check_typed_acquisition_readiness` and `can_provision_task`, and the `SECURITY_V1` interception is before the existing-first/new-agent loop |
| Credential fail-closed rule | unchanged: the security hold refuses before credential resolution, and the authority path's non-empty-`credential_refs` refusal is untouched |
| Kernel execution / `RUNNING` / acknowledgement guards | unchanged; B5.2-E creates no earned path to them and removes no guard |
| Untracked files | not staged, not committed, not deleted |

Revision 20 adds a fence check of its own, against `HEAD` on
`agent/m6b5-b5.2-e-scheduler-wiring`:

| Fence | Result |
|---|---|
| `agentype-core`, `canonical.rs`, `CANONICAL_FORMAT_VERSION`, `SANDBOX_POLICY_SCHEMA_VERSION` / normalization / meet versions | unchanged |
| ADR `Status:` lines, spec statuses, index entries | unchanged (ADR-0014 §3/§5 body text only) |
| Durable format / schema version | unchanged at 11: the seal is a process-local field that is never encoded, and the tests assert the same v11 tables, so no migration or column change |
| Attestation row identity | unchanged: `attestation_digest`, `profile_digest`, importer, protocol, physical domain and window are the same; `PartialEq` ignores only the seal |
| Coarse acquisition behavior | unchanged: the fairness change is inside typed dispatch, and the coarse relation, readiness gate and execution/acknowledgement guards are untouched |
| `agentype-storage-sqlite` library build | still compiles **without** `provisioning-producer`; only `new` is gated, `from_parts`/`is_producer_sealed`/the writer are ungated |
| Untracked files | not staged, not committed, not deleted |

## Deliberate non-reachability

B5.2 provides a compiler, a durable diagnostic, a security acquisition primitive
and (from revision 19) a scheduler hold; it is not a live secured execution path.
The following properties are intended and were checked, not accidental. Item 1 is
**corrected in revision 2**: the original wording overstated the fence.

1. **A contract-only host cannot mint an imported enforcement profile; a host
   that embeds the runtime can.** `ImportedEnforcementProfile::import` and
   `from_parts` are `#[cfg(any(test, feature = "provisioning-producer"))]`
   (`enforcement.rs:307, 339`). The boundary fixture depends on the runtime,
   which enables the feature, so the constructor **is** visible in the build
   that runs the probes (see P1-2). The gate that actually matters is the next
   item plus ADR-0014 §5 re-proof, not constructor absence.
2. **Typed acquisition has two disjoint chains. The security chain exists and is
   callable, but no runtime composition drives it to authority.** The coarse entry
   point still rejects `SECURITY_V1` at every stage
   (`reject_security_v1_coarse_acquisition`, the AgentType mode branch and the
   source mode branch in `provisioning.rs`), and the security entry point rejects a
   `COARSE_V1` Task, AgentType or source. `acquire_security_v1_task_existing` is
   the first non-test caller of `compile_execution_security` and
   `qualify_security_candidate`, but no runtime composition invokes it. Since
   revision 19 `agentype-runtime`'s resolver does select a mode-aware security
   enumeration for a `SECURITY_V1` Task, but only to compute the explainable
   `SecurityHold`; `acquire_typed_task` refuses before Attempt/Lease, so the
   authority-bearing security acquisition is still not driven and no selective
   qualification reaches a Claim. It also grants no secured start; the compiled
   proof is discarded. Since revision 14 the Kernel reinforces this:
   `create_execution_in_tx` and `confirm_running_and_renew` reject every
   `SECURITY_V1` Task, so even a `Claim` produced by that entry cannot reach an
   Execution or `RUNNING` through the old B.4 paths. Since revision 15 `ack_success`
   rejects the same Task, so the Claim cannot publish a Result or release its Batch
   either.
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
6. **The attestation producer seal does not close the importer registry.**
   Since revision 20 only the gated producer constructor seals a value, and the
   durable writer rejects an unsealed one, so a caller limited to `from_parts`
   cannot record physical evidence. Which importers exist, whether they are
   registered by the trusted composition root, and whether they really observed
   the domain remain outside this fence; B5.4's subject attestation is still the
   execution-side proof. Row-level raw SQL against an
   `agentype-storage-sqlite` database is outside the supported surface, and this
   fence is not claimed to stop it.

Item 1 no longer supports any security argument, and this record no longer uses
it as one. Item 2 is amended in revision 9 and item 6 is added in revision 20;
items 3–5 are unchanged by the re-audit.

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
14. **(rev 10, B5.4 handoff; amended rev 17)** **A self-consistent profile is
    not trusted physical evidence.** `import` still accepts the caller's support
    matrix and covered subjects. Revision 11 key-binds that matrix and forbids
    widening the frozen reference identity. Revision 17 adds the trusted-importer
    identity, exact domain ownership and freshness as a durable attestation gated
    at acquisition, but the importer's observation is still external and the
    actual subject attestation remains a B5.4 condition; constructor visibility is
    not the boundary.
15. **(rev 11; amended rev 17)** **P2-1 is implemented but not fully closed.** The
    physical key/alias gap is fixed at the validated tuple, the cache key, the
    evidence subject and the transaction. Revision 17 supplies the previously
    missing trusted-importer provenance proof as a durable, fresh,
    domain-bound attestation, so a caller can no longer self-declare support or
    subjects into accepted acquisition evidence. The source of a *new* attestation
    is still the producer, and B5.4 must bind the execution subject; do not read
    revision 17 as a B5.4 evidence handoff.
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
    durable-block wiring lands. **Implemented in revision 18:** the agent-only
    constructor is gone, every identity component feeds the candidate digest, the
    resolver freezes the complete round before ranking, and the durable block is
    built from that same domain.
19. **(rev 13)** **A coarse exact-reference claim had leaked into the reusable
    B.4 conjunct.** `can_provision_task_source_config` still rejected a Task
    whose authored `sandbox_policy` reference was absent from the imported
    evidence. On `SECURITY_V1` that refused a legal independent Task fine policy
    before the `P_eff` coverage proof, because the Task layer is already met into
    `TaskSecurityBinding`. The relation is now split: the reusable conjunct is
    exported separately for the security path, and the exact-reference claim
    stays on the coarse predicate. Every acquisition fixture used
    `sandbox_policy: None`, which is why the earlier suites did not see it; the
    new acquisition test uses a real distinct Task reference. This is a
    candidate-eligibility correctness fix, not a privilege boundary: the leaked
    check over-rejected, it never granted authority.
20. **(rev 14)** **Qualification was not execution authority.** A `Claim` from
    `acquire_security_v1_task_existing` could be presented to the frozen B.4
    `create_execution_with_snapshot` path and then to `confirm_running_and_renew`,
    even though the acquisition persists no `ExecutionSecuritySnapshot` and no
    B5.4 receipt. The daemon does not call the entry today, so this was not a live
    production bypass, but it depended on caller convention. The Kernel now
    rejects every `SECURITY_V1` Task at `create_execution_in_tx` and at
    `confirm_running_and_renew` before any insert or `RUNNING` write. The guards
    stay until B5.4 lands; B5.2-E must not remove them, and this record does not
    treat the fix as B5.4 or as B5.2 completion.
21. **(rev 15)** **Qualification was not semantic completion authority either.**
    `ack_success` could take the same `Claim` with `execution_id = None`, write
    `SUCCEEDED`/`COMPLETED`, publish a Result and release dependencies without any
    Execution, snapshot, physical terminal state or receipt. The guard now rejects
    `SECURITY_V1` before every such write. This is the third face of one root
    cause: a valid Claim is not authority for a later transition, and each
    transition must name its own proof. B5.4 must define the secured
    acknowledgement explicitly; it cannot be inferred from the Claim.
    `record_physical_outcome` on a `SECURITY_V1` `STARTING` history remains a
    B5.4 recovery-required case, not a success path.
22. **(rev 16)** **A non-executable qualification must not look like a runnable
    Claim.** This is not a defect in the current draft, which has no production
    scheduler caller, but it is an acceptance condition for B5.2-E. With B5.4
    disabled, production acquisition must not cycle a `SECURITY_V1` Task from
    `QUEUED` to `LEASED` and then let the lease expire. It must either keep an
    explainable waiting state or refuse production acquisition. Kernel refusal is
    enough for authority safety; it is not enough for scheduler liveness.

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
conditions carried into B5.2-C and B5.2-E. Revision 13 re-runs the gate (below),
so the revision-11 result is no longer the last executed code gate.

Revision 13 ships code in the same draft pull request and re-runs the full gate:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1107 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Each revision-13 guard was mutation-checked: the guard was disabled, the affected
test was run, and it failed for the expected reason.

| Guard disabled | Failing test |
|---|---|
| the security branch calls the coarse `can_provision_task_source_config` instead of the reusable conjunct | `b5_security_acquisition::a_task_fine_policy_without_an_exact_evidence_reference_is_still_selectable` — `InvalidAuthority("... Task sandbox policy task-fine-policy@1 is not enforced by imported evidence")` |
| the coarse predicate's Task-reference flag forced off | `b5_enforcement::the_task_policy_reference_is_coarse_only` — the coarse predicate returns `Ok` |

Revision 14 ships code in the same draft pull request and re-runs the full gate:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1108 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Each revision-14 guard was mutation-checked:

| Guard disabled | Observed failure |
|---|---|
| `create_execution_in_tx` mode check | `b5_security_acquisition::a_security_v1_claim_cannot_enter_the_b4_execution_or_running_path` — the call reaches snapshot validation and fails on the missing `SECURITY_V1` rejection (`binding snapshot source config digest disagrees with the validated revision`) |
| `confirm_running_and_renew` mode check | the same test — the call returns `Ok(RunningAuthorityGrant)`, demonstrating that `RUNNING` would have been written |

Revision 15 ships code in the same draft pull request and re-runs the full gate:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1109 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Revision-15 mutation check:

| Guard disabled | Observed failure |
|---|---|
| `ack_success` mode check | `b5_security_acquisition::a_security_v1_claim_cannot_be_acknowledged_as_success` — the call returns `Ok(Some(ResultId(...)))`, proving the path would have published a Result and released the Batch |

Revision 16 is docs-only (this record and ADR-0014 §3) and re-runs no code gate.
The revision-15 gate above remains the last executed local code gate at `69ad80a`;
CI on the docs head `d37c887` re-ran the full workspace and the two doctest targets
and passed.

Revision 17 ships B5.2-C code after PR #37 merged and re-runs the full gate at the
`agent/m6b5-b5.2-c-trusted-importer` head:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | pass |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

The schema-version assertion moved with the bump: `catalog_contract` now expects
11, and `b4_provisioning_binding` reads `SCHEMA_VERSION` instead of a literal.

Revision 18 ships B5.2-D code on `agent/m6b5-b5.2-d-candidate-identity` (stacked
on the revision-17 branch) and re-runs the full gate:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | 58 suites, 1123 passed, 0 failed |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

The only public-surface adjustment is the `b5_compiled_security_surface` doctest,
which now names `CandidateDomain::from_identities` instead of the removed
agent-only constructor.

Revision-18 mutation check: collapsing the identity back to its owner (mapping
each candidate to `owner.id_str()` in `CandidateDomain::from_identities`) makes
`the_candidate_domain_digest_is_a_stable_full_identity_set`,
`the_whole_round_budget_is_measured_in_full_candidate_identities` and
`a_security_round_counts_full_candidate_identities_not_agents` fail on the
two-configs/two-bindings cases; the identity was restored and the suites went
green again.

Revision 19 ships B5.2-E code on `agent/m6b5-b5.2-e-scheduler-wiring` (stacked on
the revision-18 branch) and re-runs the full gate:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | pass, 0 failed (runtime lib 303, including the two new liveness tests) |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

Revision-19 mutation check: classifying `SecurityHold` as candidate-local (or
deleting its `defer_or_fail_typed` arm) makes
`a_security_v1_task_is_held_without_an_attempt_across_cycles` fail its
`TypedDeferred` pattern (`NoWork` instead), and removing the `SECURITY_V1`
interception in `acquire_typed_task` makes both liveness tests fail with the
coarse `InvalidAuthority` instead of a hold. The hold was restored and the tests
went green again.

Revision-17 mutation check: making the attestation rejection optional at the
`SecurityV1` arm makes exactly the new provenance negatives fail
(`a_profile_without_a_trusted_importer_attestation_is_rejected_before_attempt_or_lease`,
`a_stale_trusted_importer_attestation_is_rejected_until_a_fresh_row_exists`,
`a_wider_support_profile_cannot_borrow_a_narrower_attestation`,
`a_forged_subject_coverage_cannot_borrow_the_attested_profile`,
`an_attestation_for_another_physical_domain_cannot_authorize_this_binding`); the
gate was restored and the suite went green again.

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
the other counts are unchanged. At revision 13, `b5_enforcement` holds 40 (the
coarse-only Task policy reference split) and `b5_security_acquisition` holds 19
(the independent Task fine policy acquisition). At revision 14,
`b5_security_acquisition` holds 20 (the Kernel execution-boundary acceptance test);
`b5_enforcement` stays 40. At revision 15 it holds 21 (the semantic-completion
acceptance test). At revision 17's code head, `b5_enforcement` holds 45 (five
attestation contract tests) and `b5_security_acquisition` holds 27 (the six
provenance negatives and the retained positive fixtures); the other counts are
unchanged. At revision 18's code head, `b5_enforcement` holds 46 (the full
identity/domain test and the whole-round budget test replace the old domain
test) and `b5_acquisition_block` holds 11 (the full-identity same-condition
test); `agentype-runtime`'s lib suite holds 301, including
`provisioning_resolver::tests::a_security_round_counts_full_candidate_identities_not_agents`.
The workspace total is 1123 passed in 58 suites. At revision 19's code head,
`agentype-runtime`'s lib suite holds 303 (the two B5.2-E liveness tests:
`a_security_v1_task_is_held_without_an_attempt_across_cycles` and
`a_security_v1_task_without_a_security_candidate_is_held_not_failed`); the other
B5 module counts are unchanged because B5.2-E adds runtime-unit tests only.

Revision 20 re-runs the same gate on `agent/m6b5-b5.2-e-scheduler-wiring` with the
audit fixes:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | pass, 0 failed; 1127 passed in 58 suites |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

At revision 20 the runtime lib suite holds 304
(`a_held_security_task_does_not_starve_a_runnable_coarse_task` is the new one) and
`b5_security_acquisition` holds 28
(`a_self_consistent_attestation_without_producer_provenance_is_rejected` is the new
one). Revision-20 mutation checks are recorded in the
[Joint C/D/E audit fixes](#joint-cde-audit-fixes-revision-20) section: reverting the
held arm to an immediate `TypedDeferred` fails the fairness test, and the writer
fence was verified by the negative test asserting `InvalidAuthority` plus an absent
row.

Revision 21 re-runs the same gate on the same branch with the timeout fix:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | pass, 0 failed; 1129 passed in 58 suites |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

At revision 21 the runtime lib suite holds 306
(`a_security_hold_with_timed_out_attestation_does_not_block_runnable_coarse_task`
and `a_security_hold_integrity_fault_still_fails_closed` are the new ones); the
other module counts are unchanged at 28 for `b5_security_acquisition`. The
revision-21 mutation check is recorded in the
[Round-3 audit fix](#round-3-audit-fix-revision-21) section: removing both
`ProvisioningDeadlineExceeded` arms from `security_hold` fails the timeout test at
the dispatch step with the redacted deadline message.

Revision 22 re-runs the same gate on the same branch with the three audit fixes:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | pass, 0 failed; 1133 passed in 58 suites |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

At revision 22 the runtime lib suite holds 308
(`a_security_hold_reports_provisional_when_the_catalog_drifts_mid_round` and
`held_security_tasks_yield_to_coarse_work_under_a_single_probe_window` are the new
ones), `b5_enforcement` holds 47
(`an_attestation_rejects_a_validity_window_longer_than_the_freshness_ceiling`), and
`b5_coarse_path_gate` holds 9 (`each_readiness_probe_requires_its_own_mode`). The
revision-22 mutation check is recorded in the
[Round-3 audit fixes](#round-3-audit-fixes-revision-22) section: disabling the
post-enumeration snapshot comparison fails the drift test at
`assert!(hold.is_provisional())`.

Revision 23 ships the P1-C registry/installation epoch and the P1-D
single-observation round and budget gate on a new branch from `main` after PR
#38–#40 merged, and re-runs the full gate:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass, no warnings |
| `cargo test --workspace --all-features --exclude agentype-public-api-boundary --exclude agentype-public-api-contract-only` | pass, 0 failed; 1142 passed in 58 suites |
| `cargo test -p agentype-public-api-boundary --doc` | 38 passed, 0 failed |
| `cargo test -p agentype-public-api-contract-only --doc` | 3 passed, 0 failed |

At revision 23 the runtime lib suite holds 310
(`a_complete_security_round_is_built_from_one_observation` and
`a_complete_security_round_checks_the_budget_before_selecting_a_candidate` are the
new ones), `b5_security_acquisition` holds 35
(`an_attestation_must_name_a_registered_installation_epoch`,
`an_attestation_must_match_its_epochs_importer_and_physical_domain`,
`a_retired_installation_epoch_invalidates_every_older_attestation`,
`knowing_every_field_and_the_old_epoch_cannot_publish_an_acquirable_attestation`,
`an_installation_epoch_is_immutable_and_cannot_move_backwards`,
`a_protocol_upgrade_retires_the_previous_epoch_for_the_same_domain`, and
`a_stale_expected_enforcement_plan_digest_is_rejected_before_any_authority`), and
`b5_enforcement` holds 47 unchanged.

Revision-23 mutation checks, both reverted before the gate run above:

- dropping the `expected_plan_digest` comparison makes
  `a_stale_expected_enforcement_plan_digest_is_rejected_before_any_authority`
  acquire successfully instead of rejecting;
- dropping the post-enumeration observation-digest comparison fails
  `a_security_hold_reports_provisional_when_the_catalog_drifts_mid_round` at
  `assert!(hold.is_provisional())`.

Revision 23 therefore closes item 16's registry and epoch condition and item 22's
enumeration and pure-gate half, but it still does **not** freeze B5.2, does not
deliver the production secured acquisition path, does not write a production
durable budget defer, and does not claim B5.4 capability.

Public-API probes added in `tests/public-api/src/lib.rs`:

- `b5_composite_authority_surface` — the composite-authority, qualified-candidate,
  and policy-order surface, as a `no_run` doctest.
- `b5_compiled_security_surface` — the pure compiler surface, as a `no_run`
  doctest.
- probe 31 — **does not prove** what revision 1 claimed (P1-2).

## Required follow-up, in order

| Order | Work | Acceptance |
|---|---|---|
| 1 | Re-prove `RequiredAccess` on `P_eff` in `qualify_security_candidate` (P1-1) | tool, endpoint and file-operation negatives reject as `SecurityUnenforceable`; positive compiles. Merged in PR #31, including the digest re-projection |
| 2 | Replace probe 31 and correct its comment (P1-2) | contract-only fixture rejects; runtime+contract fixture records visibility. Merged in PR #31 |
| 3 | Carry the attempt-isolation guarantee into the plan accessor and digest (P1-3) | Task-only, AgentType-only and neither matrix: plan accessor, obligations and digest agree; a direct compile with a false policy bit and a true guarantee returns isolation. Merged in PR #34 |
| 4 | Carry the guarantee identity into the compile cache key and amend ADR-0014 §4 (P1-4) | the same matrix asserts equal effective policies and different cache keys; an empty guarantee digest is rejected; the compiler protocol records the semantic change. Merged in PR #34; ADR list merged in PR #32 |
| 5 | Bind the profile to its adapter binding and execution subject (P2-1) | cross-binding and unproven-subject negatives. Implemented in revision 7 (`decb13a`) and corrected in revision 11 (`39e85c5`) to bind the resolved physical key; tests named in the claim table |
| 6 | Separate credential authorization from delivery request (P2-2) | empty `credential_refs` adds no obligation, a requested credential under a non-delivering profile is `SecurityUnenforceable`, and a requested credential under a delivering profile **completes at the pure component only**. Implemented in revision 8 (`28d6592`); the authority path is further narrowed by revision 10 (P1-2) |
| 7 | ~~Decide durable-block history semantics (P2-3)~~ **decided in revision 3: narrow ADR-0014 §4** | the stored released condition is never reopened; `A → release A → B → A` is specified to start a new diagnostic, not to retry the released one |
| 8 | Re-prove the durable block's over-budget truth at construction and at the write boundary (P2-5) | a non-exhausted or limit-mismatched row is rejected; a raw-SQL corruption fails the validated read. Merged in PR #34 |
| 9 | Bind each source declaration to its source revision (A) | the constructor and the writer both reject a declaration from another revision, and the validated read still fills the ref from `(source id, revision)`. Merged in PR #35 |
| 10 | Complete the durable block's validated read and count coherence (B) | a tampered generation, a candidate cost smaller than the domain, and a corrupt same-condition row each fail closed on read or re-record. Merged in PR #35 |
| 11 | Intersect the frozen B.4 source/config eligibility on the `SECURITY_V1` path (P1-1, rev 10) | disabled config, foreign evidence digest, missing functional capability, and unsatisfiable continuity/lifecycle each reject with no Attempt/Lease; the coarse path is unchanged. Implemented in revision 10 (`0a0ed56`) |
| 12 | Keep credential-bearing acquisition fail-closed until B5.3 (P1-2, rev 10) | a delivering profile plus non-empty `credential_refs` is `InvalidAuthority` before Attempt/Lease; the pure-component delivery check and its positive test remain. Implemented in revision 10 (`0a0ed56`) |
| 13 | Bind the imported profile to the resolved physical `adapter_binding_key`, not the B.4 alias (P1, rev 11) | `primary -> k2` positive with cache key and subject carrying `k2`; a profile imported for `k1` under the same alias is rejected; the acquisition transactions pass the selection key. Implemented in revision 11 (`39e85c5`) |
| 14 | Enforce `Support_imported ⊆ Support_reference_profile` for the fixed reference identity (P2, rev 11) | a reference-identity profile claiming `NetworkEgressRestriction` fails at import; a stricter subset remains valid. Implemented in revision 11 (`39e85c5`) |
| 15 | Define the complete candidate identity and enumerate every candidate under a whole-round budget (B5.2-D) | **first commit and first test freeze the unified identity**: `LogicalAgent/pinned AgentType + exact Source/Config + physical binding`, including source revision and config digest; two configs or bindings under one agent are distinct; an unknown candidate or exhausted budget cannot partially select. **Implemented in revision 18** as `SecurityCandidateIdentity` / `CandidateDomain::from_identities` and the resolver's frozen `SecurityCandidateRound`; a budget below the complete round is a whole-round defer and the durable block binds the same domain. The acceptance table in the revision-18 section names each case |
| 16 | Prove trusted importer provenance, profile freshness and exact physical-domain ownership (B5.2-C) | the profile is produced by a trusted local importer whose protocol version and physical-domain ownership are verifiable; capability/subject claims cannot be self-declared; freshness has invalidation conditions; renaming cannot escape the reference claim. **Implemented in revision 17** as a durable `enforcement_profile_attestations` row for the exact profile digest, importer protocol and resolved physical domain, re-read transactionally and gated before qualification/Attempt/Lease; the negatives named in the revision-17 acceptance table cover each case. **Revision 20 adds the writer fence**: only the gated producer constructor seals a value, a digest-correct value rebuilt from public parts is rejected with `InvalidAuthority` and leaves no row, and the durable read stays unsealed. **Revision 22 bounds the claim**: an attestation window longer than `ENFORCEMENT_PROFILE_ATTESTATION_MAX_VALIDITY_SECS` (3600 s) is `InvalidNumber` on construction and on the durable read, so freshness cannot be widened into an unexpiring claim. **Revision 23 closes the registry and epoch**: schema v12 adds `enforcement_importer_installations` (one ACTIVE epoch per importer and exact physical domain; immutable identity; retirement final and rows undeletable), the attestation carries `installation_id` and its schema is `2`, the writer requires a matching ACTIVE epoch and `issued_at >= installed_at`, and the acquisition read joins the epoch so a retired installation invalidates every older row even inside the one-hour window. The reverse test now exists: an internal caller knowing every public field and the exact digest, and able to rebuild the old epoch, cannot publish a row acquisition accepts. **Still outside B5.2:** the production provider of imported profiles, and any claim of production secured enforcement |
| 17 | Wire resolver→Scheduler, revalidate the full candidate set, and handle existing/new agents (B5.2-E) | all qualification proofs close before Attempt/Lease; `SECURED_QUALIFIED` remains non-executable and no ordinary physical start path runs on it; production execution requires the B5.4 snapshot/secured start; feature-gated; with B5.4 disabled the scheduler must not cycle a `SECURITY_V1` Task `QUEUED → LEASED → lease expiry` or present it as runnable. **Implemented in revision 19** for the hold/decline half: the durable Task mode selects a mode-aware resolver, the complete security round is enumerated, and `acquire_typed_task` refuses before any Attempt/Lease/binding/NACK. **Revision 22 makes the readiness half a mode proof**: `TypedReadinessMode::{CoarseAcquisition, SecurityV1}` replaces the one-way `reject_security_v1` flag, so the security probe refuses a `COARSE_V1`/`LEGACY` Task and cannot qualify the wrong contract for a future caller. The full secured acquisition/start remains B5.4, so this row is **not** a B5.2 freeze |
| 18 | Keep the Task authored-policy exact-reference claim off the reusable B.4 conjunct (rev 13) | a `SECURITY_V1` Task with an independent fine policy, and evidence naming no authored reference, acquires; the coarse predicate still refuses the same input; the binding derivation carries the Task layer. Implemented in revision 13 (this pull request) |
| 19 | Keep a qualified `SECURITY_V1` `Claim` out of the frozen B.4 Execution and `RUNNING` paths (rev 14) | starting from the acquired `Claim`, `create_execution_with_snapshot` fails before any `executions`/`binding_snapshots` row; a stale `STARTING` row is not promoted to `RUNNING`; the Task stays `Leased`. Implemented in revision 14 (this pull request); removed only by the B5.4 snapshot/secured start |
| 20 | Keep a qualified `SECURITY_V1` `Claim` out of the semantic completion path (rev 15) | starting from the acquired `Claim`, `ack_success` with no `execution_id` fails; Task/Attempt/Lease keep their state, no Result is inserted and a blocked dependent is not released. Implemented in revision 15 (this pull request); replaced only by an explicit B5.4 secured acknowledgement path |
| 21 | Keep a non-executable `SECURITY_V1` qualification from cycling as an ordinary lease (B5.2-E liveness, rev 16) | with B5.4 disabled, production acquisition either holds an explainable waiting state or refuses acquisition; a `SECURITY_V1` Task is not moved `QUEUED → LEASED` only to expire, and the qualification is not presented as a runnable Claim. **Implemented in revision 19**: `SecurityHold` + `security_hold_cooldown`; the Task stays `QUEUED` with zero Attempts across a lease-expiry sweep and a second cycle, for both a populated and an empty security round. **Revision 20 adds the fairness half**: a hold is candidate-local, the typed scan continues, and a held `SECURITY_V1` Task ahead of a runnable `COARSE_V1` Task no longer consumes the cycle. **Revision 21 adds the timeout half**: a read-only probe that exceeds the shared deadline returns a provisional hold instead of a fatal resolution error, so the typed cycle is not aborted and unrelated coarse work still dispatches; integrity faults still fail closed. **Revision 22 adds the coherence and window-bound halves**: catalog/roster drift during enumeration makes the hold provisional (`CatalogDrift`) instead of "complete", and `held_security_tasks_yield_to_coarse_work_under_a_single_probe_window` proves the `probe_limit=1` multi-hold case with near-deadline attestations. The durable block still awaits the B5.4 profile |
| 22 | Make the whole-round budget an actual runtime discipline, not only a pure component (rev 20 audit) | a real `SECURITY_V1` round must come from a complete trusted enumeration; the budget must bind **before** selection; no candidate may be selected while its qualification is unknown; a true over-budget outcome must leave a re-verifiable durable `security_acquisition_blocks` defer. **Revision 23 closes the enumeration and gate half**: the hold no longer unions consecutive reads; `Kernel::security_candidate_observation` loads every durable input in one transaction, `CompleteSecurityCandidateRound` is built from that one observation, `select_first` refuses a deadline-truncated round and checks the complete cost before returning any candidate, and the post-enumeration digest re-read is the drift fence. The pure gate is therefore real, but it is **not** a production durable defer. **Open, gated on B5.4**: the durable row is keyed by `binding_profile_digest(binding, profile)`, so it cannot be written before the imported profile exists |
| Gate | Wire the qualification chain into acquisition (P2-4) | a qualified candidate is selectable; an unqualified one is rejected before Attempt/Lease; the coarse gate still rejects `SECURITY_V1`. Implemented in revision 9 (`d4d8dc3`) as `acquire_security_v1_task_existing`; tests named in the claim table. This is **selected-candidate transactional qualification**; complete `SECURITY_V1` orchestration remains a later row |

Items 1–6, 8, 9 and 10 are code changes delivered outside the revision-1 record
and have since merged in PR #30–#36; they are not pending on this draft.
Item 7 was a design decision and is settled in revision 3 by narrowing the ADR
rather than by adding history; item 4 also needs its normative sentence in the
ADR, which is why that part sits in the ADR pull request. Items 11 and 12 are
implemented in revision 10 (`0a0ed56`), items 13 and 14 in revision 11
(`39e85c5`), in the same unmerged draft pull request as the Gate. Items 15–17 are
the audit's B5.2-C/D/E decomposition: item 15 (B5.2-D) is implemented in
revision 18, item 16 (B5.2-C) in revision 17, and item 17 (B5.2-E) is implemented
in revision 19 as the mode-aware hold. Revision 19 does **not** claim items 17 and
21 as full completion of secured acquisition: the B5.4 snapshot/secured start and
`ExecutionSecurityEvidence` remain the capability boundary, and the durable
budget-defer row still needs B5.4's imported profile.
Revision 12 accepted the decomposition and carried the two hard conditions into
the follow-ups without adding code. Item 18 is the revision-13 fix for the
coarse Task exact-reference leak in the reusable conjunct; item 19 is the
revision-14 Kernel boundary that keeps a qualified `SECURITY_V1` `Claim` out of
the B.4 Execution and `RUNNING` paths; item 20 is the revision-15 boundary that
keeps it out of semantic completion; item 21 is the revision-16 liveness condition
for B5.2-E. Revision 20 does not add a new item for the two audit findings; it
completes items 16 and 21 with the provenance writer fence and the dispatch
fairness rule, and adds item 22 as the acceptance condition that runtime
whole-round budget enforcement stays open until B5.4 supplies the imported
profile. Revision 20 therefore closes two named defects but does **not** freeze
B5.2, does not deliver a production secured acquisition path, and does not claim
B5.4 capability; the final stacked SHA still needs its own re-audit. Revision 21
completes item 21's liveness argument for the timed-out-probe case and does not add
a new item: the provisional hold is not a whole-round result, so it does not change
item 22, and it does not supply importer registration authority, so it does not
close item 16. Revision 21 still does not freeze B5.2, does not deliver the
production secured acquisition path, and does not claim B5.4 capability.
Revision 22 completes item 16's freshness bound and item 21's coherence and
`probe_limit=1` liveness cases, and makes item 17's readiness probe a mode proof;
it does not add a new item because the trusted registry, the physical installation
epoch and the production whole-round budget remain the named B5.2 acceptance
conditions under the audit's §四 option 1. Revision 22 therefore does **not** freeze
B5.2, does not deliver the production secured acquisition path, and does not claim
B5.4 capability. Revision 23 implements the trusted registry and installation
epoch (schema v12) and the single-observation round with the budget-before-selection
gate, so those two named conditions move from "open design" to implemented
substrate; the production whole-round durable defer and the production secured
acquisition path still require B5.4's imported profile, so revision 23 does **not**
freeze B5.2 and does not claim B5.4 capability.

The Gate is the B5.2 completion condition. Revisions 9, 10 and 11 implement
*selected-candidate transactional qualification* in an unmerged draft pull
request; complete `SECURITY_V1` acquisition orchestration is still open. As with
every finding in this record, the row is evidence for the re-audit, not a
closure. Revision 11 specifically does not claim P2-1 fully closed, because the
trusted importer proof (item 16) remains open. Revision 13 fixes a
candidate-eligibility over-rejection in the same Gate and does not close it
either. Revision 14 closes the composition gap between that Gate and the old
execution entries at the Kernel boundary; it still grants no B5.4 snapshot,
receipt or secured start, and complete `SECURITY_V1` orchestration remains open.
Revision 15 closes the semantic-completion face of the same gap (`ack_success`)
and restricts the ADR's transactional claim to durable and imported-fact
consistency; it likewise grants no B5.4 authority. Revision 16 records the
continuation-baseline review, which finds no new P1, confirms the merged
PR #30–#36 baseline, and adds the B5.2-E liveness condition; it accepts this
draft only as a B5.2 continuation. Revision 17 implements item 16's B5.2-C
provenance gate after PR #37 merged; revision 18 implements item 15's B5.2-D
candidate identity and whole-round defer, leaving item 17 (B5.2-E) open, and
neither revision claims the Gate as closed. Revision 19 implements item 17's
scheduler wiring as the mode-aware **hold** plus item 21's liveness condition: a
`SECURITY_V1` Task is enumerated and refused before Attempt/Lease and stays
`QUEUED`, instead of cycling through an expiring lease. Revision 19 still does not
close the Gate: the secured acquisition/start, the snapshot/
`ExecutionSecurityEvidence` and the durable budget-defer write all remain B5.4,
and no freeze is claimed.

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
| Complete candidate identity and domain | `crates/agentype-agent-contract/src/enforcement.rs` (`SecurityCandidateOwner`, `SecurityCandidateIdentity`, `CandidateDomain::from_identities`, `acquisition_cost`) |
| Whole-round resolver round and defer | `crates/agentype-runtime/src/provisioning_resolver.rs` (`SecurityCandidateRound`, `SourceProvisioningCandidate::security_identity`, `prepare_selection`) |
| B5.2-D conformance | `crates/agentype-agent-contract/tests/b5_enforcement.rs` (`the_candidate_domain_digest_is_a_stable_full_identity_set`, `the_whole_round_budget_is_measured_in_full_candidate_identities`), `crates/agentype-runtime/src/provisioning_resolver.rs` (`a_security_round_counts_full_candidate_identities_not_agents`), `crates/agentype-storage-sqlite/tests/b5_acquisition_block.rs` (`one_agent_with_a_different_config_or_binding_is_a_new_condition`) |
| B5.2-E mode-aware enumeration and hold | `crates/agentype-runtime/src/provisioning_resolver.rs` (`CandidateMode`, `resolve_security_source_candidates{,_for_agent}`, `SecurityHold`, `HoldProvisionalCause`, `HoldProbeSnapshot`, `security_hold`, `ProvisioningResolutionError::SecurityHold`, the `acquire_typed_task` interception), `crates/agentype-runtime/src/lib.rs` (`TypedCandidateAttempt`, `TypedDispatchConfig::security_hold_cooldown`, `defer_or_fail_typed` hold arm, the scan-on-hold loop in `dispatch_typed_one`), `crates/agentype-storage-sqlite/src/kernel.rs` (`task_security_mode`, `TypedReadinessMode`, `check_typed_security_readiness`) |
| B5.2-E liveness and fairness conformance | `crates/agentype-runtime/src/provisioning_resolver.rs` (`a_security_v1_task_is_held_without_an_attempt_across_cycles`, `a_security_v1_task_without_a_security_candidate_is_held_not_failed`, `a_held_security_task_does_not_starve_a_runnable_coarse_task`, `a_security_hold_with_timed_out_attestation_does_not_block_runnable_coarse_task`, `a_security_hold_integrity_fault_still_fails_closed`, `a_security_hold_reports_provisional_when_the_catalog_drifts_mid_round`, `held_security_tasks_yield_to_coarse_work_under_a_single_probe_window`) |
| Source-declaration provenance | `crates/agentype-agent-contract/src/sandbox.rs` (`SpawnSourceSecurityDeclaration`), `crates/agentype-agent-contract/src/authority.rs` (`EffectiveTaskAuthority::new`), `crates/agentype-storage-sqlite/src/catalog.rs` (`insert_spawn_source_declaration`) |
| Schema v10 block table and triggers | `crates/agentype-storage-sqlite/src/schema.rs` (`security_acquisition_blocks`) |
| Schema v11 attestation table, index and triggers | `crates/agentype-storage-sqlite/src/schema.rs` (`enforcement_profile_attestations`) |
| Trusted-importer attestation value object | `crates/agentype-agent-contract/src/enforcement.rs` (`EnforcementProfileAttestation`, `ENFORCEMENT_PROFILE_ATTESTATION_MAX_VALIDITY_SECS`, `producer_sealed` / `is_producer_sealed`) |
| Durable attestation read/write | `crates/agentype-storage-sqlite/src/profile_attestation.rs` (validated read; the writer's producer-seal check) |
| Kernel wrappers | `crates/agentype-storage-sqlite/src/kernel.rs` (`record_enforcement_profile_attestation`, `get_enforcement_profile_attestation`, the block wrappers) |
| Coarse acquisition gate | `crates/agentype-storage-sqlite/src/provisioning.rs` (`reject_security_v1_coarse_acquisition` and the `AcquisitionMode::Coarse` branches) |
| Security acquisition entry point | `crates/agentype-storage-sqlite/src/provisioning.rs` (`acquire_security_v1_task_existing`, the `finish_acquisition` security branch), `kernel.rs` (`Kernel::acquire_security_v1_task_existing`) |
| Security acquisition conformance | `crates/agentype-storage-sqlite/tests/b5_security_acquisition.rs` |
| Reusable B.4 provisioning conjunct vs the coarse Task policy reference | `crates/agentype-agent-contract/src/predicates.rs` (`can_provision_task_source_config_reusable`, `can_provision_task_source_config`), `crates/agentype-agent-contract/tests/b5_enforcement.rs` (`the_task_policy_reference_is_coarse_only`) |
| Kernel execution-commitment boundary | `crates/agentype-storage-sqlite/src/requirement.rs` (`task_is_security_v1`), `crates/agentype-storage-sqlite/src/kernel.rs` (`create_execution_in_tx`, `confirm_running_and_renew`, `ack_success`), `crates/agentype-storage-sqlite/tests/b5_security_acquisition.rs` (`a_security_v1_claim_cannot_enter_the_b4_execution_or_running_path`, `a_security_v1_claim_cannot_be_acknowledged_as_success`) |
| Supported-surface feature wiring | `crates/agentype-runtime/Cargo.toml`, `tests/public-api/Cargo.toml` |
| Public-API probes | `tests/public-api/src/lib.rs` (probes 18 and 31) |
| ADR — composite authority | `docs/decisions/0013-m6b5-policy-authority-algebra-identity.md` §5 |
| ADR — compilation and receipt chain | `docs/decisions/0014-m6b5-enforcement-compilation-receipt.md` §2–§5, §13–§14 |
| Slice A merge | `ce96c87` (PR #28) |
| Slice B merge | `d3ab482` (PR #29) |
