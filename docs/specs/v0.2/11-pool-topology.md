# 11 — Pool Topology

Status: Normative
Canonical path: docs/specs/v0.2/11-pool-topology.md
V0.1 preservation: UNCHANGED MOVE/MERGE/RETIRE/writer-cutover kernel
Source: [docs/specs/v0.1.md](../v0.1.md) §5

## Kernel rules (MUST for M4)

Matching order, pending vs current membership, MOVE_CAPACITY, idle
cross-target fence of reusable Incarnation, cutover safety independent of
expiry-vs-topology order, MERGE capacity add, pending rebase, RETIRE
rejection while nonterminal Tasks or inbound desired members remain, open
`WRITER_QUIESCENCE_UNKNOWN` blocking RETIRE, upsert immutability of existing
partition retention/target/profile/tags — all UNCHANGED from V0.1.

MOVE and MERGE MUST preserve LogicalAgent semantic identity.
They MUST NOT be implemented as Transform.

## V0.2 evolution (semantic layer)

Design intends PoolPartition as desired population of an AgentType at an
anchor, without absorbing type semantics, model configuration, and capacity
in one object.

Until D-TOPOLOGY is resolved:

- M4 MUST implement V0.1 partition semantics.
- M6 MUST NOT silently treat ExecutionTarget as AgentType.
- Exact distinction among type refinement, partition capacity change, MOVE,
  MERGE, and TRANSFORM remainder is DEFERRED (D-TOPOLOGY). Frozen now:
  MOVE/MERGE preserve identity; TRANSFORM does not mutate identity in place.
