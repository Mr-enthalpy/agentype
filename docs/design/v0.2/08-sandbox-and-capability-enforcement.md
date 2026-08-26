# Sandbox and Capability Enforcement

Status: Architecture direction
Canonical path: docs/design/v0.2/08-sandbox-and-capability-enforcement.md

## 1. AgentType is a security contract

A derived/specialized AgentType should be able to mechanically narrow:

- working directory;
- read roots;
- write roots;
- visible paths;
- hidden paths;
- tools;
- commands;
- network access;
- environment;
- secret scope;
- resource limits.

These restrictions are not prompt reminders.

## 2. SandboxPolicy

Suggested minimum fields:

- working_directory;
- read_roots;
- write_roots;
- visible_paths;
- hidden_paths;
- allowed_tools;
- allowed_commands;
- network_policy;
- environment_policy;
- secret_scope;
- resource_limits.

## 3. Enforcement path

`AgentType → SandboxPolicy → ExecutionRequest → SpawnSource / Adapter enforcement`

If a SpawnSource/Adapter cannot enforce the requested sandbox, it cannot provision that AgentType.

## 4. Policy intersection

Execution restrictions arise from multiple layers:

`AgentType policy ∩ Generation policy ∩ Task policy ∩ source capability ceiling`

No layer may silently widen a stricter upstream policy.

## 5. Security monotonicity

Root-generated refinements may narrow authority but not enlarge it.

A semantic specialization should mechanically become smaller in authority where required.

This allows a broad provisioning source to instantiate many narrower roles safely.

## 6. Capability classes

### CorrectnessCapabilities

- execution;
- observation;
- termination;
- reconciliation;
- required sandbox enforcement.

### ContinuityCapabilities

- session persistence;
- exact resume;
- checkpoint restore;
- transcript replay.

### ExperienceCapabilities

- UI visibility;
- native child-agent display;
- navigation;
- workstream rendering.

Correctness capabilities are hard requirements.

Continuity affects fidelity.

Experience is ergonomics.
