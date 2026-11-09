# 10 — Sandbox and Security

Status: Normative
Canonical path: docs/specs/v0.2/10-sandbox-and-security.md

## Contract

A claimed AgentType restriction is real only if it is mechanically
enforceable. Prompt instructions MUST NOT count as sandbox enforcement.

SandboxPolicy MUST be able to express at least:

- working directory
- read roots
- write roots
- visible paths
- hidden paths
- allowed tools
- allowed commands
- network policy
- environment policy
- secret scope
- resource limits

Exact schema is IMPLEMENTATION-DEFINED if these categories are enforceable.

## Path

`AgentType → SandboxPolicy → ExecutionRequest → SpawnSource/Adapter enforcement`

If a SpawnSource or Adapter cannot enforce the requested sandbox, it MUST be
ineligible to provision that type.

The effective workspace mode and the effective network policy MUST reach the
physical `EnvironmentStartRequest`; proving a domain *can* enforce a mode is not
enough, the concrete execution must request it. Attempt isolation is
authoritative from the ExecutionTarget registry, never from adapter
self-assertion.

## Intersection

**M6** semantic enforcement: effective execution permission MUST equal

`AgentType policy ∩ Generation policy ∩ Task policy ∩ SpawnSource capability ceiling`

No layer MAY silently widen a stricter upstream policy.
Root-created refinements MUST only narrow ([06](06-agent-type-and-matching.md)).

**M4** continues the V0.1 WorkspaceMode / adapter enforcement contract only.
M4 MUST NOT require AgentType or Generation objects to exist.

Adapter-imported enforceability is an exact-set fact: a domain that can enforce
`Write` does not thereby claim `ReadOnly`, and M6-B.4 MUST NOT infer one
workspace mode from another. An importer that can enforce both MUST import both.
The evidence subject binds the exact `(adapter_kind, adapter_binding_key)` the
enforceability was imported from. This remains the frozen `COARSE_V1` rule.

## M6-B.5 effective security (`SECURITY_V1`)

B5.0 freezes the design authority in
[ADR-0013](../../decisions/0013-m6b5-policy-authority-algebra-identity.md),
[ADR-0014](../../decisions/0014-m6b5-enforcement-compilation-receipt.md) and
[ADR-0015](../../decisions/0015-m6b5-credentials-feature-gates-scope-freeze.md).
This section records the normative consequences; it does not claim the
implementation exists.

In `SECURITY_V1`:

- The effective policy is `P_actual_agent ∩ P_generation ∩ P_task ∩ P_source`.
  `P_actual_agent` is the actually bound AgentType revision, not the Task's
  matching anchor. SourceConfig is not an authored ceiling and there is no
  `P_config`; a physical config effect is admissible only through source
  integration and imported evidence.
- `R_min` is a typed positive obligation product: `RequiredAccess` (formal
  security operation floors), `RequiredCapabilities` (catalog-validated
  capability obligations), `RequiredGuarantees` (isolation/continuity),
  `CoarseExecutionModes` (workspace/network mode), and `PlacementConstraints`
  (frozen M5 predicate). Only `RequiredAccess` uses set inclusion, and only for a
  security-class capability with an explicit normative catalog mapping
  (`CapabilityDefinition.policy_projection`) to a policy access dimension.
  `Bool`, `Set`, `Ordered`, `Quantity` and `Exact`
  capabilities do not thereby define an allowed-operation set; they stay in
  `RequiredCapabilities` and use their existing catalog matchers. Workspace and
  network use their existing dimension predicates, not `⊆`; guarantees and
  placement use `satisfies_requirement`, never `⊆`. Negative constraints such as
  `ReadOnly` or `Disabled` are ceilings, not floors. B.5 v1 makes no path-level
  minimum claim and MUST NOT infer one from natural-language input.
- Coarse and fine fields are never two independent grants. AgentType uses
  `project_agent(P) == SecurityContract`; Generation projects
  `(max_workspace, max_network)` and entails its isolation guarantee; Task uses
  consistency. Define `P_task = TaskSpec coarse policy ∩ optional Task fine
  policy`. A Task coarse/fine conflict MUST be rejected locally, before any
  cross-object meet and before normalization, even if a stricter ceiling would
  hide it; `None` and an explicit authored policy remain distinguishable until
  validation completes. Typed admission evaluates `RequiredAccess` and
  `CoarseExecutionModes` against `meet(P_generation, P_task)`;
  `RequiredCapabilities`, `PlacementConstraints` and candidate-dependent
  guarantees are `Pending` at admission and MUST be discharged during candidate
  matching. Candidate matching and commitment MUST re-evaluate
  `CoarseExecutionModes` and every positive Task
  requirement against the complete `P_eff`, because a later meet can remove every
  writable path even though each ceiling allowed one. `Write` is satisfied only
  when the complete `P_eff` retains an allowed write inside the committed logical
  workspace region; a writable temporary directory or any location the Task cannot
  use as its workspace MUST NOT satisfy it. Provisioning then adds the
  actual AgentType and SpawnSource ceiling.
- Execution guarantees are composed by disjunction, not intersection. `P_eff` is
  an authorization intersection; attempt isolation is
  `I_required = AgentType.requires_attempt_isolation OR
  TaskAgentRequirement.required_attempt_isolation OR
  SourceRequirement.requires_attempt_isolation`, and it must then reach
  `ExecutionTarget.attempt_isolation = true`, be proven enforceable by the exact
  adapter, and be requested by the committed execution. An `EnforcementSupport`
  capability that merely *supports* isolation MUST NOT create a requirement, and
  no policy meet may clear one. Any failed step makes the candidate ineligible.
- The admission mode is explicit and durable:
  `COARSE_V1 | SECURITY_V1` is an input, never inferred from
  `sandbox_policy != None`, because `SECURITY_V1 + None` is legal. The mode is
  recorded with `TaskSecurityBinding`, and a `SECURITY_V1` Task requires both its
  Generation and the actual bound AgentType to support `SECURITY_V1`. It MUST
  NOT be downgraded at matching, provisioning or recovery. A `COARSE_V1`
  AgentType cannot execute a `SECURITY_V1` Task, and in B.5 v1 a `SECURITY_V1`
  AgentType cannot execute a `COARSE_V1` Task either: the coarse path cannot
  prove its authored fine restrictions, so its coarse projection is not a
  substitute for the full contract.
- Authored policies live in an immutable catalog with canonical digests.
  Intersection products are canonical inline bytes plus a digest and MUST NOT be
  published as catalog rows. `TaskSecurityBinding` persists the admission-stage
  derivation; `ExecutionSecuritySnapshot` persists the Execution-stage result.
- `meet` is defined per dimension. A combination that cannot be represented or
  proven MUST fail closed; it MUST NOT be made executable by dropping Task
  requirements or by adding an unrestricted bootstrap exception.
- Every candidate needs imported `EnforcementSupport` for the whole composite
  policy, not merely for its parts. Compilation is root-relative and bounded;
  whole-round defer is required when any relevant candidate is unqualified, and
  a failed winner MUST NOT fall back to a weaker candidate. Unsupported
  combinations exclude the candidate; protocol/corruption faults are control
  plane failures.
- The effective policy, an execution security snapshot containing the actual
  bound AgentType, and an append-only receipt chain MUST reach physical start.
  `SandboxAppliedReceipt` binds policy, profile, plan, root and proven instance
  and establishes `HELD`; a credential-bound Execution adds a
  `CredentialDeliveryReceipt` that binds the sandbox receipt digest. The chain
  joins the existing RUNNING + first-renewal transaction and is stored outside the
  worker-writable jail root/workspace. Receipt publication is
  `durable contents → atomic no-replace publication → durable namespace update →
  verify published identity`, and the physical root MUST satisfy
  `ResolveRoot(committed descriptor, RequestId) = receipt.actual_root` by
  materialized filesystem identity, not by path-string equality. Physical start
  holds worker code until the sandbox receipt is durable, and the hold stays
  closed on daemon crash or
  control-channel loss. `SecuredStartPhase` is `HELD`, `RELEASED`, `UNKNOWN`, or
  `TERMINAL`; only a `RELEASED` instance with positive physical observation may
  confirm RUNNING. A `HELD` instance is released only after the Kernel revalidates
  authority for the exact `RequestId` and instance witness; a reused PID never
  grants release, and an uncertain release never creates a second worker.
- The policy-only implementation target is
  `SECURITY_V1_POLICY_ONLY_REFERENCE_V1`
  ([ADR-0014](../../decisions/0014-m6b5-enforcement-compilation-receipt.md) §14).
  It is a minimum target to be empirically proven by B5.4/B5.5, not a claim about
  the current B.4 jail. Its supported set and explicit non-claims are normative: a
  dimension outside the profile is ineligible, not partially enforced, and a
  capability is claimed only when both supported platforms prove it. B5.4 must
  publish an OS enforcement-mechanism design note that proves receipt integrity,
  non-replaceability, identity binding, crash recovery and worker
  write/delete/replace denial. Receipt read confidentiality is explicitly not a
  minimum-profile claim and MUST NOT be assumed from the B.4 jail; the security
  model never depends on the receipt path, digest or content being unknown to the
  worker. B5.5 classifies every dimension against this profile.
- Credential-bearing configs remain fail-closed unless the exact binding
  declares the corresponding imported gate. B.5 v1 production enables only
  `security_execution_v1.policy` and `credential_brokered`;
  `credential_transient_raw` is defined but unsupported. Raw secrets MUST NOT
  enter the catalog, snapshots, launch descriptors, command lines, control
  errors or control logs.
- `COARSE_V1` replay is unchanged. `SECURITY_V1` parents without their security
  child are corruption, and a v8 database MUST be rejected at v9 open rather
  than silently reinterpreted.

## V0.1 mapping (kernel)

V0.1 `WorkspaceMode` remains: `read_only` or `write`.
Adapter wire formats are IMPLEMENTATION-DEFINED. Scheduler vocabulary MUST
NOT be accidentally treated as vendor wire enums (V0.1 Codex SandboxMode
lesson). A write Task MUST NOT escalate to unrestricted full access as a
default mapping.
