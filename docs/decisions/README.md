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
| [ADR-0013](0013-m6b5-policy-authority-algebra-identity.md) | M6-B.5 policy authority: four security objects, no `P_config`, typed `R_min` (`RequiredAccess` / `RequiredCapabilities` / `RequiredGuarantees` / `CoarseExecutionModes` / `PlacementConstraints`) with a deterministic `Set`-only catalog projection, per-object coarse projection/consistency, Task-local validation before normalization, staged admission versus candidate satisfaction with every `Pending` discharged by Candidate and never at commitment, `can_provision` limited to AgentType/source-level eligibility while `can_provision_task` and commitment prove the full Task-specific `P_eff`, composite-`P_eff` mode re-check, explicit durable admission mode owned solely by `task_agent_requirements` with a conditional `TaskSecurityBinding` and a Generation mode that follows `policy_mode` (`NONE` derives `COARSE_V1` with no policy row), symmetric cross-mode exclusion for AgentType/Generation/SpawnSource (a single-mode `SECURITY_V1` source stays full-path even when `P_source = Top`, because `Top` adds no restriction but does not change the declared protocol mode), `P_source = Top` for a source with no authored ceiling, guarantee disjunction separate from the authorization meet, a projecting `Set` must be `Ability` with an `Authority`/`Sandbox` class so a `Restriction` never becomes a positive floor, a partially defined `meet` returning `PolicyConflict` and treating an empty allowed set as a valid deny-all, authored catalog vs inline derived policies, credential authorization as a stable namespaced identity rather than a bare source-local string, and schema v9 identity. |
| [ADR-0014](0014-m6b5-enforcement-compilation-receipt.md) | M6-B.5 enforcement compilation is root-relative and bounded, with a compile cache keyed on the effective-policy digest plus both `normalization_version` and `meet_algorithm_version`; winner failure never falls back, and repeated provable whole-set over-budget defer surfaces a durable operator-actionable diagnostic and blocked-for-operator state rather than indefinite indistinguishable retries, without auto-enlarging the budget, shrinking the candidate set or selecting among compiled candidates; the Execution security snapshot carries the actual AgentType and the applied bootstrap manifest is bound to it and to the sandbox receipt, with a pre-commit `validate_bootstrap` proving the manifest does not widen worker authority and remains recoverable without digest-only reconstruction, not left beside the plan-equality tuple, and whose typed structure cannot carry raw credential values or usable bearer capabilities; secured start has a `HELD`/`RELEASED`/`UNKNOWN`/`TERMINAL` phase with Kernel-authorized release carrying a bounded, single-use `ReleaseAuthorityProof` (operation id plus current fencing identity, rechecked immediately before release), a durable Kernel-owned `ReleaseOperationIntent` (`prepared`/`dispatched`/`released`/`unknown`/`aborted`) with creation and dispatch in fenced transactions, no second operation while a prior intent is `dispatched`/`unknown`, and an idempotent retry after durable `RELEASED` distinct from an unauthorized stale-proof release attempt, a launch window bounded by the earlier of the absolute deadline and remaining Lease validity with a conservative secured-start admission budget required before creating an instance (not a mathematical OS upper bound; overrun still fails closed), stage-aligned failure handling (pre-acquisition defer without an Attempt, pre-Execution reservation settlement without a physical instance, post-Execution M5 recovery rather than a no-Attempt defer) and no pre-RUNNING renewal, historical settlement of an expired-Lease observation under narrowly scoped recovery authorization without new release authority while unsettled intents stay `unknown` as instance-scoped recovery debt, and an explicit separation between refusing a known-invalid authorization and the irreducible Lease-revocation/OS-release race owned by M5 fencing, restart-time classification of those phases, and a fail-closed hold, and the secured adapter operations inherit the M5 absolute-deadline contract without per-stage budget resets; a policy-neutral durable receipt publication order and a descriptor-bound physical root; an append-only `SandboxAppliedReceipt` → `CredentialDeliveryReceipt` chain, bounded to one receipt per kind per Execution and proving exact delivery of the whole requested credential set, lives outside the worker-writable jail and joins the RUNNING + first-renewal transaction; `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` fixes the minimum supported profile, with `RequiredEnforcementFeatures` deriving obligations only from concrete restrictions or guarantees so a `Top`/unrestricted value adds none while still passing the complete compile/receipt/execution protocol, receipt integrity required and read confidentiality explicitly not claimed; credential-free means only no Agentype-managed delivery or committed credential plan, not that the worker can reach no host secret. |
| [ADR-0015](0015-m6b5-credentials-feature-gates-scope-freeze.md) | M6-B.5 credential eligibility uses a per-acquisition observation epoch with explicit failure classes; credential authorization is positive and deny-by-default over namespaced identities (a source-local ref cannot gain a grant by string equality across authorities), requiring an explicit versioned AgentType grant, including delivery mode, rather than an omitted-dimension `Top`, with the request resolved during candidate qualification from the exact `SourceConfig` revision by a pure, read-only, secret-free, deterministic and deadline-bounded resolver into a canonical secret-free `CredentialRequest` whose canonical reference set must equal `SourceConfig.credential_refs` and validated before any probe, and broker availability never substitutes for it; v1 production enables only `BrokeredAccess`; three independent imported gates; a dependency-ordered sequence starts the B5.4 physical feasibility spike alongside B5.1 and in parallel with B5.2 semantic qualification, gates B5.3 end-to-end acceptance and the policy-only freeze on the spike proving the Linux/Windows hold, receipt publication, restart-time phase classification and recovery, and orders the B5.4 policy-only handoff before B5.3 real broker integration and the B5.4 credential-bound handoff after it, and a spike gap in a mandatory trust condition cannot be converted into a policy-only freeze by dropping it; B5.3 has component-ready and end-to-end accepted levels, the latter requiring one real external runtime through a real `SpawnSource` completing end-to-end brokered use on the policy-only handoff rather than mock-only broker and receipt tests, and proves broker secret isolation independent of the workspace jail (raw store unreachable by the worker, capability unusable outside its scope/audience, broker-enforced expiry/revocation/version fencing); the policy-only and credential-bound slice freezes are separate and both are mandatory prerequisites of an explicitly enumerated M6-B aggregate freeze, each at `FROZEN` rather than merely `FREEZE READY`, with a same-SHA authorization recording each slice status before the aggregate; one `CredentialAccessPlan` must be fulfilled entirely by one trusted broker delivery domain (references spanning independent broker authorities are unsupported in v1, not implicitly fanned out); a replacement Execution after unproven partial-delivery closure needs independent broker epoch/scope fencing, not physical worker recovery alone; the secret boundary keeps opaque publisher configuration outside Core secret detection. |

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
