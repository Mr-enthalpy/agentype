# 09 — Memory, Continuity, and Revival

Status: Normative
Canonical path: docs/specs/v0.2/09-memory-continuity-revival.md

## Three layers

1. Runtime-local hot context: opaque, high-fidelity, non-authoritative.
2. Scheduler MemoryCapsule: bounded, structured, versioned, portable floor.
3. Authoritative project/workstream state (repo/db/artifacts).

Runtime transcript MUST NOT be MemoryCapsule.

## MemoryCapsule

Scheduler-owned durable structured continuity.

MUST: belong to a LogicalAgent and AgentLineage; be versioned; be bounded;
carry provenance; be externally readable under authorization.

Relation to Checkpoint: checkpoint promotion remains fenced by current
Attempt (V0.1). Capsule version promotion as a distinct semantic operation
MUST NOT be a hidden LLM write.

Scheduler MUST NOT silently synthesize capsules via untracked LLM execution.
Any LLM compression MUST be ordinary scheduled work with provenance.

Worker-produced semantic delta (including `validated_delta`) MUST NOT
automatically mutate canonical MemoryCapsule state.

Who accepts promotion (Root review vs explicit integration Task vs another
kernel-governed mechanism) is DEFERRED (D-MEM-PROMOTE).
Field types, size bounds, merge rules, positive/negative specialization
encoding: DEFERRED (D-MEM-SCHEMA).
Negative entry scope, assumptions, applicability, supersession, hot/cold GC:
DEFERRED (D-NEG-GC) while the MUST of retaining applicability still holds.

## Revival

Revival is internal Scheduler behavior.

MUST:

- preserve LogicalAgent semantic identity (id, AgentType, lineage, affinity,
  anchor, durable capsule, checkpoint, permissions, task-facing identity);
- NOT require Root orchestration for normal revival;
- NOT treat Incarnation/session loss as LogicalAgent death;
- NOT treat native session persistence as correctness-critical.

Revival MUST NOT create a new Generation.
Revival MUST NOT be Transform.

Bit-for-bit hidden-context equivalence with a continuously running process is
NOT promised. That is continuity fidelity, not identity.

## Continuity levels

| Level | Meaning | Correctness |
|---|---|---|
| 0 | still warm | best |
| 1 | exact native resume | fidelity |
| 2 | runtime/checkpoint restore | fidelity |
| 3 | Scheduler reconstruction | **mandatory floor** |

Level 3 MUST reconstruct from MemoryCapsule, Checkpoint, authoritative
project/workstream state, AgentType, and anchor.

If Level 3 cannot be satisfied, revival MUST NOT be treated as normal
transparent recovery. Scheduler MUST suspend and escalate. It MUST NOT
silently create a semantically new agent.

## Continuity affinity

For a resident LogicalAgent, implementations SHOULD prefer the original
SpawnSource if it preserves stronger native continuity, else another
compatible source, else Level 3.

A LogicalAgent MUST NOT be permanently bound to one terminal/model/source
unless AgentType explicitly requires it.

ContinuityBinding is opaque. Core MUST NOT store raw secrets as plaintext
policy. Persistence details DEFERRED (D-CONTINUITY-BIND).

## V0.1 continuity capsules

V0.1 bounded JSON continuity keys remain kernel behavior for M4.
V0.2 MemoryCapsule MAY evolve representation after D-MEM-SCHEMA; it MUST NOT
weaken fencing of promotion on the kernel path.
