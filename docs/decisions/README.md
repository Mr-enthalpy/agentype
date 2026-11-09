# Architecture decisions

Status: Index
Canonical path: docs/decisions/README.md

This folder records **why** a durable choice exists (ADRs).

- Architecture invariants stay in [docs/architecture/overview.md](../architecture/overview.md).
- Versioned testable contracts stay in [docs/specs/](../specs/).
- V0.2 design drafts stay in [docs/design/v0.2/](../design/v0.2/).
- Python how-to stays in [docs/development/](../development/).

No ADRs are extracted in the repository-normalization change. The decisions
below already live as Core Invariants; copying them now would duplicate
normative text.

## Extraction TODO (reserved IDs)

| ID (reserved) | Decision already established |
|---|---|
| ADR-0001 | LogicalAgent identity is distinct from physical execution/session identity (invariants 6–14). |
| ADR-0002 | Task execution is at-least-once with fencing, not exactly-once (invariants 15–20). |
| ADR-0003 | SQLite is the V0.1 single-machine authority, not a permanent semantic contract (invariant 65). |
| ADR-0004 | Result transport is durable and separate from Root notification (invariants 25–28, 61). |
| ADR-0005 | Runtime/Frontend adapters do not define Core scheduling semantics (invariants 46–57). |
| ADR-0006 | Physical writer quiescence is required before unsafe replacement (invariants 21–23, §31). |

New ADRs start at `docs/decisions/0007-short-title.md` when a **new** durable
choice is made (expected during V0.2 design, not this layout change).

## Extracted ADRs

| ID | Decision |
|---|---|
| [ADR-0007](0007-m6b-can-provision-task-staging.md) | `can_provision_task` is required at the first authority-bearing typed acquisition (M6-B.4/B.5), not at M6-B.3 non-authoritative semantic preselection. |
| [ADR-0008](0008-m6b-typed-provisioning-acquisition.md) | M6-B.4 owns the first authority-bearing typed acquisition and the internal `ResolvedProvisioningEvidence` producer; target/profile come from partition/anchor and source selects the exact binding. |
| [ADR-0009](0009-m6b-topology-capacity-seam.md) | V0.1 `reconcile_pool` capacity excludes typed (`BOUND`) population; typed population is not V0.1 excess. |
| [ADR-0010](0010-m6b-selection-order-scope.md) | M6-B.4 applies spec 07's hard filters and continuity ranking, and defers availability/cost ranking (no durable model yet); selection is part of the authority path. |
| [ADR-0011](0011-m6b-acquisition-contract-parity.md) | Shared frozen placement/isolation decisions; provisioning reservation stays separate from the M5 Attempt→Incarnation execution association; known hard filters precede attestation. |
| [ADR-0012](0012-m6b4-scope-typed-physical-execution-ack.md) | M6-B.4 ends at typed physical execution and the durable ACK interface; the worker-originated Result data plane (transport, authentication, retry, ingress) is explicitly deferred. |
| [ADR-0013](0013-m6b5-policy-authority-algebra-identity.md) | M6-B.5 policy authority: four security objects, no `P_config`, typed `R_min` (`RequiredAccess` / `RequiredCapabilities` / `RequiredGuarantees` / `CoarseExecutionModes` / `PlacementConstraints`) with a deterministic `Set`-only catalog projection, per-object coarse projection/consistency, Task-local validation before normalization, staged admission versus candidate satisfaction with composite-`P_eff` mode re-check, explicit durable admission mode owned solely by `task_agent_requirements` with a conditional `TaskSecurityBinding` and a Generation mode that follows `policy_mode` (`NONE` derives `COARSE_V1` with no policy row), symmetric cross-mode exclusion for AgentType/Generation/SpawnSource, `P_source = Top` for a source with no authored ceiling, guarantee disjunction separate from the authorization meet, a partially defined `meet` returning `PolicyConflict` and treating an empty allowed set as a valid deny-all, authored catalog vs inline derived policies, and schema v9 identity. |
| [ADR-0014](0014-m6b5-enforcement-compilation-receipt.md) | M6-B.5 enforcement compilation is root-relative and bounded; winner failure never falls back; the Execution security snapshot carries the actual AgentType; secured start has a `HELD`/`RELEASED`/`UNKNOWN`/`TERMINAL` phase with Kernel-authorized release and a fail-closed hold; a policy-neutral durable receipt publication order and a descriptor-bound physical root; an append-only `SandboxAppliedReceipt` → `CredentialDeliveryReceipt` chain, bounded to one receipt per kind per Execution, lives outside the worker-writable jail and joins the RUNNING + first-renewal transaction; `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` fixes the minimum supported profile, with receipt integrity required and read confidentiality explicitly not claimed. |
| [ADR-0015](0015-m6b5-credentials-feature-gates-scope-freeze.md) | M6-B.5 credential eligibility uses a per-acquisition observation epoch with explicit failure classes; v1 production enables only `BrokeredAccess`; three independent imported gates; a dependency-ordered B5.2 → B5.4 feasibility spike → B5.3 credential implementation → B5.4 integration sequence proves the Linux/Windows hold, receipt publication and recovery before broker work; the policy-only, credential-bound and M6-B aggregate freeze scopes are separate. |

ADR-0013/0014/0015 are `Proposed` pending the B5.0 maintainer audit. They become
`Accepted` only when the maintainer authorizes the B5.0 design baseline at an
exact final SHA; this does not freeze M6-B.5.

Template for a new ADR:

- title
- date
- status
- context
- decision
- consequences
