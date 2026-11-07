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
enforceability was imported from.

## V0.1 mapping (kernel)

V0.1 `WorkspaceMode` remains: `read_only` or `write`.
Adapter wire formats are IMPLEMENTATION-DEFINED. Scheduler vocabulary MUST
NOT be accidentally treated as vendor wire enums (V0.1 Codex SandboxMode
lesson). A write Task MUST NOT escalate to unrestricted full access as a
default mapping.
