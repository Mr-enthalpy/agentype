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
- `R_min` contains only positive Task obligations drawn from existing structured
  fields (`required_workspace`, `required_network`, `required_capabilities`,
  `required_attempt_isolation`, continuity/placement). Negative constraints such
  as `ReadOnly` or `Disabled` are ceilings, not floors. B.5 v1 makes no
  path-level minimum claim and MUST NOT infer one from natural-language input.
- Coarse and fine fields are never two independent grants. AgentType uses
  `project_agent(P) == SecurityContract`; Generation projects
  `(max_workspace, max_network)` and entails its isolation guarantee; Task uses
  consistency (`Write ⇒ P_task allows write`, `ReadOnly ⇒ P_eff has no write`,
  network by existing rank/polarity, isolation guarantee carried in `P_eff`).
  Typed admission MUST reject when `R_min(T) ⊄ meet(P_generation, P_task)`;
  provisioning then adds the actual AgentType and SpawnSource ceiling.
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
  bound AgentType, and an applied-security receipt MUST reach physical start.
  The receipt binds policy, profile, plan, root and proven instance, and joins
  the existing RUNNING + first-renewal transaction. It is stored outside the
  worker-writable jail root/workspace.
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
