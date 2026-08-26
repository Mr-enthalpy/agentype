# Revival Transparency, Continuity, and Terminal Boundary

Status: Architecture direction
Canonical path: docs/design/v0.2/07-revival-continuity-and-terminal-boundary.md

## 1. Revival is internal

LogicalAgent is the long-term outward semantic entity.

A physical process/thread/session may disappear because of machine restart, resource pressure, terminal shutdown, disk pressure, adapter failure, or deliberate cold storage.

When the LogicalAgent is needed again, Scheduler may revive it.

Root should normally not observe revival as an orchestration event.

Root should not need to call revive_agent, recreate the agent, or reason about physical generations.

## 2. Semantic continuity guarantee

Revival preserves the LogicalAgent's outward semantic identity:

- logical_agent_id;
- AgentType;
- AgentLineage;
- workstream affinity;
- anchor;
- durable MemoryCapsule;
- checkpoint;
- permissions;
- task-facing identity.

This does not promise bit-for-bit hidden-context equivalence with a continuously running process.

That difference is a continuity-fidelity property, not a Root-level lifecycle event.

## 3. Physical model

LogicalAgent = long-lived semantic identity.

Incarnation = one physical hosting period.

Execution = one Task runtime turn.

A new Incarnation may resume the same native terminal conversation or reconstruct into a new conversation. Either way it may remain the same LogicalAgent.

## 4. Continuity quality ladder

### Level 0: still warm

Same physical runtime remains active.

### Level 1: native exact resume

Runtime persists and resumes the same conversation/session.

### Level 2: runtime/adapter restore

Checkpoint/session state can be restored.

### Level 3: Scheduler reconstruction

Reconstruct from MemoryCapsule, checkpoint, authoritative project/workstream state, AgentType, and anchor.

Level 3 is the mandatory correctness floor.

If Level 3 cannot be satisfied, revival cannot be considered normal transparent recovery and should suspend/escalate.

## 5. Continuity capabilities

Adapter/source capabilities may include:

- persistent_session;
- resume_same_session;
- persistent_runtime_context;
- adapter_checkpoint_restore;
- transcript_replay;
- scheduler_capsule_restore.

These affect fidelity, not Core identity correctness.

## 6. ContinuityBinding

A generic opaque binding may record:

- logical_agent_id;
- spawn_source_id;
- adapter_ref;
- continuity_handle_ref;
- last_verified_at;
- capabilities_used.

Core treats the continuity handle as opaque.

Do not embed Codex thread semantics into Core.

Opaque references should not require storing raw secrets in Core.

## 7. Continuity affinity is not hard execution affinity

For a resident LogicalAgent:

1. prefer original SpawnSource if it preserves stronger native continuity;
2. otherwise use another compatible SpawnSource;
3. reconstruct from Scheduler floor.

A LogicalAgent should not be permanently bound to one terminal/model/source unless AgentType explicitly requires it.

## 8. Terminal integration split

### ExecutionAdapter — correctness required

- start;
- observe;
- interrupt;
- terminate;
- collect outcome;
- reconcile start;
- enforce required sandbox/correctness capabilities.

### TerminalExperienceAdapter — optional UX

- display child agent;
- open conversation;
- show status;
- show workstream;
- load work record;
- render result.

Terminal-native UX must never be required for Core correctness.

## 9. Frozen revival rules

- revival is normally invisible to Root;
- loss of Incarnation does not imply loss of LogicalAgent;
- native session persistence improves continuity but does not replace Scheduler memory floor;
- revival and Transform are distinct:
  - revival preserves semantic identity/type;
  - Transform intentionally changes semantic role and creates a successor identity.
