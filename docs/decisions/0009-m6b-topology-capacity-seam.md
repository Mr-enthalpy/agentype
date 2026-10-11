# ADR-0009 — V0.1 PoolPartition capacity excludes typed population

Status: Accepted (M6-B.4)
Date: 2026-10-06
Canonical path: `docs/decisions/0009-m6b-topology-capacity-seam.md`

## Context

M6-B.3 quarantined type-bound `LogicalAgent`s from the legacy claim/consumer
path, but V0.1 `reconcile_pool` still counted every live-state agent in a
partition toward `desired_capacity`, including `BOUND` agents, and could select a
bound agent as excess and retire or drain it. The B.3 freeze report recorded this
as the `D-TOPOLOGY` seam to be made explicit in M6-B.4.

Spec 11 states that V0.1 `PoolPartition` is desired *population* by target/profile
and MUST NOT be silently conflated with typed semantic population or `AgentType`
identity; `D-TOPOLOGY` remains the deferred question of the full type/capacity/
MOVE/MERGE/TRANSFORM remainder.

## Decision

1. M6-B.4 makes the capacity seam explicit: `reconcile_pool` counts only
   `UNBOUND` (untyped, legacy) LogicalAgents toward V0.1 `desired_capacity`, and
   MUST NOT select a `BOUND` agent as excess. A typed agent is governed by typed
   provisioning, not by V0.1 capacity.

2. Deficit births continue to produce `UNBOUND` agents only; typed agents are
   never birthed by capacity reconciliation.

3. This is a deliberate, documented behavior change to the frozen M4/V0.1 kernel,
   scoped to the typed-population seam. It MUST NOT alter untyped capacity,
   MOVE/MERGE, or retirement fencing for `UNBOUND` agents.

4. Full typed-population targets, and the type-refinement / capacity / MOVE /
   MERGE / TRANSFORM split, remain deferred (`D-TOPOLOGY`) and are not implemented
   here.

## Consequences

- A partition may hold more physical agents than `desired_capacity` when typed
  agents are present; that is expected and is not excess.
- Operator capacity math applies to the legacy/untyped population only; typed
  population is managed by publishing catalog revisions and binding types.
- Any future typed-capacity model is a new ADR and a new spec 11 evolution, not a
  reinterpretation of this seam.
