# ADR-0010 — M6-B.4 deterministic source selection scope

Status: Accepted (M6-B.4)
Date: 2026-10-06
Canonical path: `docs/decisions/0010-m6b-selection-order-scope.md`

## Context

Spec 07 freezes a selection order that MUST consider correctness, sandbox
enforceability, AgentType compatibility, **continuity value, availability, and
cost/resource policy**. M6-B.4 introduces the first authority-bearing typed
acquisition and must choose deterministically among eligible `SourceConfig`
candidates. The repository currently has no durable availability or cost model
for a `SpawnSource`/`SourceConfig` candidate.

A milestone implementation must not silently rewrite a frozen normative MUST.
This ADR scopes what B.4 actually implements and what remains deferred.

## Decision

1. In M6-B.4, the hard filters (correctness, sandbox enforceability, AgentType
   compatibility via `can_provision_task`) are applied exactly as spec 07
   requires. Among the survivors, the next frozen dimension is **continuity
   value**: the strongest continuity candidate wins.

2. **Availability and cost/resource ranking are not implemented and are not
   replaced by an invented rule.** The durable model has no availability or cost
   fact for a candidate, so when more than one candidate is tied on continuity
   the selection is **unresolved and fails closed** (`SelectionAmbiguous`, mapped
   to `RESOURCE_UNAVAILABLE`). A stable-identity tie-break is deliberately NOT
   used: deterministic ordering is not the frozen selection semantics, and a
   B.4 milestone MUST NOT silently rewrite spec 07's MUST.

3. **Selection is part of the authority path.** The authority-bearing runtime
   facade re-enumerates the eligible candidate set, selects the winner, and
   builds the committed selection from that freshly resolved winner; no
   caller-supplied candidate or enforcement fact enters the transaction.

4. When a source availability/cost model is defined, the selection order MUST be
   extended to include it (or the exact selection policy fixed).

## Consequences

- Spec 07's selection order remains normative and unchanged. B.4 implements its
  correctness/sandbox/compatibility hard filters and the continuity dimension,
  and refuses rather than fabricates when the remaining dimensions are
  unavailable.
- The physical provisioning choice is derived by the Scheduler, not a free
  parameter of the authority caller.
- Any future change to the selection order or winner rule is a new ADR.
