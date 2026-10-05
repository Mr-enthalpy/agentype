# ADR-0007 — `can_provision_task` is required at B.4 authority-bearing acquisition, not B.3 preselection

Status: Accepted (M6-B.3 amendment)
Date: 2026-10-06
Canonical path: `docs/decisions/0007-m6b-can-provision-task-staging.md`

## Context

Frozen spec 06 (M6-B.1) stated that `can_provision_task` is a **mandatory
eligibility conjunct** and, more specifically, that "B.3/B.4 MUST include
`can_provision_task` as a mandatory conjunct".

M6-B.2 froze the durable exact-revision Agent Contract catalog. M6-B.3 then
introduced typed semantic admission and pure existing-agent preselection without
selecting any `SpawnSource`, `SourceConfig`, or `ResolvedProvisioningEvidence`.
At B.3 there is no source, config, adapter binding, or imported enforcement
evidence to evaluate, so `can_provision_task` cannot be honestly computed. The
original B.3 draft amended spec 06 in place to move the obligation to B.4/B.5.

That in-place edit is the wrong mechanism: a milestone PR must not silently
rewrite a frozen normative MUST. This ADR is the explicit amendment the M6-B.3 PR
references.

## Decision

1. M6-B.3 `match_existing_agents` is **non-authoritative semantic candidate
   preselection**. Its eligibility is the frozen M5 placement rules plus
   `can_execute` plus the explicit cross-revision exclusion
   (`D-TYPE-REV-COMPAT`). `more_specific_for` is a preference relation only and
   never decides eligibility. A B.3 result proves no physical eligibility and
   grants no Task/Attempt/Lease/Execution authority.

2. The first **authority-bearing typed acquisition/provisioning path** is
   introduced in M6-B.4. It MUST include `can_provision_task`, an active
   `AdapterBindingPolicy`, exact binding resolution, required physical safety,
   enforcement evidence, and credential availability as applicable, before any
   Claim/Attempt/Lease authority is created.

3. This supersedes the frozen phrase "B.3/B.4 MUST include `can_provision_task`
   as a mandatory conjunct" in spec 06. The obligation now attaches to the first
   authority-bearing acquisition path (B.4/B.5), not to B.3 preselection.

4. A `LogicalAgentTypeBinding` and a B.3 candidate are semantic identity only.
   They are never physical eligibility evidence for an existing Incarnation, and
   a B.3 candidate MUST NOT be converted into execution eligibility without the
   B.4 physical qualification. This is the B.4 freeze gate.

## Consequences

- spec 06 keeps the original four-relation contract; only the *stage* at which
  `can_provision_task` becomes mandatory is clarified by this ADR.
- B.3 remains narrow (no `SpawnSource`, credentials, adapter binding, or physical
  launch) and is honestly unable to claim physical eligibility.
- B.4 MUST consume `can_provision_task` and the remaining physical conjuncts as
  its freeze gate; a `B3Candidate` is NOT an `EligibleExecutionCandidate`.
- Any future change to this staging is a new ADR, not an edit inside a milestone
  PR.
