# Root Operating Model

Status: Architecture direction
Canonical path: docs/design/v0.2/02-root-operating-model.md

## 1. Root is the single semantic integrator

Root maintains one clean, current, revisable positive semantic model of the user problem.

Root is not a lease manager, incarnation manager, retry controller, worker process supervisor, transcript accumulator, or shared blackboard for all subagents.

Root owns the semantic frontier.

The runtime prompt contract for a running Root is
`ROOT_OPERATING_DOCTRINE.md`. That file is how Root *uses* Agentype. It is
not the architecture/RIIR ingestion guide.

## 2. Root retains

- problem definition;
- current canonical model;
- decomposition;
- dependencies;
- acceptance criteria;
- semantic integration;
- conflict resolution;
- final decisions;
- generation admission;
- topology/type/transform intent when semantically justified.

Root delegates work that benefits from independent exploration, falsification, verification, large-context inspection, experimentation, implementation, testing, or specialized continuity.

## 3. Positive model discipline

Root should retain:

- what is currently believed;
- current design/plan;
- accepted decisions;
- unresolved uncertainty.

Root should not keep every superseded model active.

When evidence invalidates the current model:

`old model → historical/negative semantic record`

`new model → sole current positive model`

Single does not mean immutable.

## 4. Negative semantics

Root retains compact exclusion knowledge such as:

- X is invalid under condition Y;
- assumption A was disproven;
- interface B cannot guarantee C;
- approach D was rejected because evidence E.

Raw failed exploration remains outside Root active context unless needed.

> Preserve conclusions and constraints, not narrative history.

## 5. Evidence policy

Default:

> Reference first, materialize on demand.

Root expands evidence for conflict resolution, low confidence, high-impact decisions, citation, or re-verification.

## 6. Result integration

Worker output is evidence, not authority.

A Result should ideally expose:

- conclusion;
- validated semantic delta;
- unresolved uncertainty;
- negative findings;
- evidence references;
- artifacts;
- RawWorkIntents, if allowed by Generation policy.

Root performs integration rather than replay.

## 7. Root / Scheduler boundary

Root expresses semantic intent:

- task requirement;
- dependency;
- affinity;
- acceptance;
- Generation admission;
- type refinement;
- Transform intent;
- topology intent.

Root does not manage Lease, Attempt retry, heartbeat, Incarnation, physical session, or revival mechanics.

> Express semantic intent; do not micromanage scheduler mechanics.

## 8. Root loop

1. State the current problem model.
2. Identify the highest-value unresolved uncertainty.
3. Decide which uncertainty should be isolated into scheduled work.
4. Admit a bounded Generation.
5. Continue independent reasoning where possible.
6. When the Generation becomes reviewable:
   - integrate positive deltas;
   - integrate useful negative semantics;
   - inspect conflicts;
   - inspect compiled proposals;
   - reject, defer, or admit the next frontier.
7. Replace obsolete semantic state cleanly.
8. Stop when the user's acceptance condition is satisfied.
