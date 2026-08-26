# 06 — AgentType and Matching

Status: Normative
Canonical path: docs/specs/v0.2/06-agent-type-and-matching.md

## AgentType

AgentType MUST represent organizational, informational, security, lifecycle,
and continuity semantics.

AgentType MUST NOT be defined by model name, provider, terminal, price tier,
prompt alias, or PoolPartition identity.

Suggested descriptor categories (representation IMPLEMENTATION-DEFINED):
type_id, revision, affinity, capabilities, sandbox policy ref, lifecycle,
continuity, anchor constraint, spawn requirements, information-function set
and memory policy, transform policy, optional `based_on_type_id`.

Revisions, if retained, MUST be immutable once published. Cross-revision
compatibility, whether Tasks pin exact vs compatible revision, and Transform
across revisions are DEFERRED (D-TYPE-REV).

## Relations (MUST remain four)

Implementations MUST provide distinct predicates:

- `can_execute(AgentType, TaskRequirement)`
- `can_provision(SpawnSource, AgentType)`
- `more_specific_for(A, B, TaskRequirement)`
- `is_valid_refinement(Base, Derived)`

They MUST NOT be collapsed into one subtype/inheritance operator.

A broad SpawnSource MAY provision a narrower AgentType if it can enforce it.
A more specific AgentType MAY be preferred for assignment over a broader
compatible one. These directions differ and MUST NOT be inverted.

Concrete encodings of the four relations are DEFERRED (D-TYPE-REL).

## Refinement monotonicity

A Root-created derived type MUST NOT enlarge authority.

MUST hold:

- DerivedPermission ⊆ BasePermission
- DerivedVisibility ⊆ BaseVisibility
- DerivedTools ⊆ BaseTools
- DerivedRoots ⊆ BaseRoots
- DerivedBudget ≤ BaseBudget

Lifecycle MUST NOT widen beyond base. Affinity MAY narrow; it MUST NOT
arbitrarily broaden authority. Anchor MUST satisfy base anchor constraints.

## Information functions

EXPAND, COMPRESS_POSITIVE, and COMPRESS_NEGATIVE are information operations.

They MUST NOT be a required mutually exclusive AgentType taxonomy and MUST
NOT be a retention mode.

An AgentType MAY declare zero, one, or multiple information functions.
A single small-domain LogicalAgent MAY carry both positive and negative
maintenance.

Lifecycle (short-lived explore-and-retire vs long-lived continuity) is
orthogonal.

Concrete set/trait encoding is DEFERRED (D-INFO-FN). Implementations MUST NOT
ship `enum AgentKind { Positive, Negative, Explorer }` as the type identity.

Positive and negative semantic memory MUST be scoped and evidence-backed.
Negative entries MUST retain applicability conditions. Implementations MUST
NOT promote “Y failed under C” into “Y must never be used” without scope.

## Matching preference (semantic order)

1. exact / most-specific compatible resident agent
2. compatible narrower anchored type
3. compatible broader/general type
4. cold/revivable compatible logical agent
5. provision a new logical agent from an eligible SpawnSource

Ranking SHOULD consider compatibility, affinity specificity, capability
surplus, anchor distance, continuity value, and warm/revival cost.
Ranking MUST NOT be by nominal inheritance depth.

V0.1 partition matching ([11](11-pool-topology.md)) remains for kernel
conformance until typed matching is implemented in M6.
