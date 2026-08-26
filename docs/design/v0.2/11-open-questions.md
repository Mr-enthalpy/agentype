# V0.2 Open Questions

Status: Deliberately unresolved
Canonical path: docs/design/v0.2/11-open-questions.md

The following questions should be resolved during V0.2 design/spec work rather than guessed during implementation.

## Generation policy representation

How small can GenerationPolicy remain without turning into a workflow DSL?

Need to decide:

- exact semantic modes;
- expansion-budget representation;
- whether WorkIntent allowance is boolean or numeric;
- reviewability conditions.

## RawWorkIntent schema strictness

How much structure should ordinary domain workers be required to provide?

Too little burdens the compiler. Too much leaks Scheduler architecture into workers.

## CompiledWorkProposal decomposition

Initial preference is non-expansive:

`1 RawWorkIntent → 0..1 proposal`

Need to decide whether limited one-to-many normalization is ever justified.

## AgentType relation implementation

Need precise representation for:

- can_execute;
- can_provision;
- more_specific_for;
- valid refinement.

Do not collapse these into one subtype relation.

## AgentType revision semantics

Need to decide:

- immutable type revisions;
- compatibility across revisions;
- task references to exact vs compatible revision;
- Transform behavior across revisions.

## MemoryCapsule schema

Need to decide:

- size bounds;
- structured field types;
- evidence/provenance reference format;
- positive/negative specialization;
- update/merge rules.

## Semantic memory promotion protocol

A Result may carry a candidate positive or negative semantic delta.
Workers cannot promote that delta into long-lived MemoryCapsule / canonical
semantic memory merely by labeling it `validated_delta`.

Need to decide who accepts and writes the promotion:

- Root acceptance during Generation review;
- an explicit semantic-integration Task;
- another mechanism that still uses the ordinary correctness kernel.

Until this is specified, implementations must not add a hidden write path.

## Negative semantic entry lifecycle

Need to decide, for a durable negative/exclusion record:

- scope;
- assumptions;
- applicability conditions;
- supersession;
- hot vs cold retention and garbage collection.

Without scope, “Y failed under condition C” degenerates into “Y must never
be used.”

## Information-function representation

Frozen: information function and lifecycle are orthogonal; positive,
negative, and exploratory functions are not required to be mutually
exclusive AgentType categories.

Still need the concrete representation (policy/trait/set, cardinality
limits, how matching uses a set rather than an exclusive kind).

## ContinuityBinding persistence

Need to decide:

- where opaque continuity refs are stored;
- security treatment;
- capability verification;
- expiration/invalidation.

## Root review protocol

Need exact API/result surface for Generation review, proposal admission, defer/reject reasons, and conflict resolution.

## Transform failure semantics

Need to specify source behavior if refinement suspends, cancellation before cutover, partial target preparation, and lineage rollback semantics.

## Type/topology operations

Need exact V0.2 distinction between type refinement, partition capacity changes, MOVE, MERGE, and TRANSFORM.

## Second adapter acceptance

Need the minimal conformance test proving a second adapter can be added without Core semantic changes.

## V0.1 database transition

Need to decide whether Rust V0.2 migrates existing V0.1 SQLite in place, offers explicit import, or starts with a new DB and migration tool.

This should be decided before storage implementation.
