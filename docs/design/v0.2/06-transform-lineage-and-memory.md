# Transform, AgentLineage, and MemoryCapsule

Status: Architecture direction
Canonical path: docs/design/v0.2/06-transform-lineage-and-memory.md

## 1. Transform is semantic change

MOVE and MERGE do not change the semantic type of a LogicalAgent.

TRANSFORM may change AgentType, sandbox, affinity, anchor, memory compression policy, and long-term responsibility.

Therefore Transform is not a special MOVE.

## 2. Do not mutate LogicalAgent semantic identity in place

Preferred model:

LogicalAgent A, type X, memory M1

→ Transform

LogicalAgent B, type Y, memory M2, `B supersedes A`, same AgentLineage

A becomes RETIRED.

AgentLineage preserves continuity across semantic successors.

## 3. AgentTransform is a persistent workflow

Transform cannot be one SQLite transaction if semantic memory refinement requires asynchronous work.

Suggested saga:

`REQUESTED → QUIESCING → REFINING_CONTEXT → TARGET_READY → CUTTING_OVER → COMPLETED`

Exceptional states:

- SUSPENDED;
- CANCELLED.

## 4. Transform flow

1. Request Transform from source LogicalAgent to target AgentType specification.
2. Quiesce source:
   - block new claims;
   - active assignment finishes at a safe assignment boundary;
   - preserve writer safety.
3. Freeze transform input:
   - source type revision;
   - MemoryCapsule revision;
   - checkpoint;
   - workstream anchor;
   - project references.
4. Run context refinement as an ordinary scheduled Task.
5. Apply normal Attempt / Lease / Result / retry / suspension / escalation semantics.
6. Validate target type.
7. Create successor LogicalAgent in the same AgentLineage.
8. Attach refined MemoryCapsule/checkpoint.
9. Cut over topology.
10. Retire source LogicalAgent.

Scheduler does not secretly invoke an untracked memory-management LLM.

## 5. MemoryCapsule

MemoryCapsule is Scheduler-owned durable structured continuity.

Suggested fields:

- capsule_id;
- logical_agent_id;
- lineage_id;
- version;
- type_revision;
- workstream_id;
- anchor_ref;
- invariants;
- decisions;
- current_design;
- rejected_alternatives;
- open_questions;
- known_failures;
- current_checkpoint;
- next_likely_steps;
- provenance;
- created_at.

It should be externally readable under authorization.

## 6. Three memory layers

### Runtime-local hot context

Opaque, high-fidelity, non-authoritative.

### Scheduler durable MemoryCapsule

Bounded, structured, versioned, portable recovery floor.

### Authoritative project/workstream state

Repository/database/artifacts that define actual current state.

Runtime transcript is not MemoryCapsule.

## 7. Memory synthesis policy

Scheduler stores structured capsules but does not silently synthesize them.

Updates may come from normal checkpoint promotion, explicit maintenance Tasks, or Transform refinement Tasks.

Any LLM-based compression is ordinary scheduled work with provenance.

## 8. Positive and negative memory specialization

Positive-semantic maintainer capsules emphasize current valid structure, accepted decisions, and implementation state.

Negative-semantic auditor capsules emphasize rejected alternatives, failure conditions, counterexamples, and invalid assumptions.

Both use the same durable mechanism but different compression policy.

## 9. Type garbage collection

Transform retiring an old LogicalAgent does not imply deleting its AgentType globally.

AgentTypes should be immutable/versioned.

Type garbage collection or tombstoning is a separate operation and only valid when no agents, partitions, tasks, transforms, or history reference the type.
