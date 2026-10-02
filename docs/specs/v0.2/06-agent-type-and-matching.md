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

Revisions MUST be immutable once published. A committed LogicalAgent, and any
admitted Task agent requirement, MUST pin an exact immutable
`AgentTypeRef(type_id, revision)`. `latest`, ranges, and aliases are pre-commit
selectors only and MUST resolve to an exact revision before any durable binding
is written. Cross-revision compatibility and Transform across revisions remain
DEFERRED (D-TYPE-REV-COMPAT); M6-B v1 performs no automatic cross-revision
upgrade (D-TYPE-REV-PIN resolved to exact-pin).

## Relations (MUST remain four)

Implementations MUST provide distinct predicates:

- `can_execute(AgentType, TaskRequirement)`
- `can_provision(SpawnSource, SourceConfig, AgentType, imported enforcement evidence)`
- `more_specific_for(A, B, TaskRequirement)`
- `is_valid_refinement(Base, Derived)`

They MUST NOT be collapsed into one subtype/inheritance operator.

A broad SpawnSource MAY provision a narrower AgentType if it can enforce it.
A more specific AgentType MAY be preferred for assignment over a broader
compatible one. These directions differ and MUST NOT be inverted.

Concrete encodings of the four relations are FROZEN in M6-B.1 (D-TYPE-REL
resolved) and implemented in `agentype-agent-contract`:

- `can_execute` checks the information function, capability envelope, task
  affinity (an agent with the `Any` affinity accepts any requirement; `Only(S)`
  requires the Task tags to be a subset of `S`), workspace/network policy,
  continuity minimum, sandbox policy, anchor, and budget.
- There is exactly one authoritative sandbox vocabulary. Restrictions are
  expressed as security-class capabilities (evidence-proven) plus workspace,
  network, attempt isolation, and the evidence-enforced `SandboxPolicyRef`; the
  contract has no free-form `tools`/`roots`/`visibility`/permission lists that
  look like security but have no enforcement path. A security/authority
  restriction MUST NOT exist without a proof path, and a derived type MUST NOT
  become narrower without an equal-or-narrower mechanically enforceable policy.
- `can_provision` checks that the config belongs to the exact source revision,
  source/config activity, lifecycle coverage and the continuity minimum against
  the **effective** envelope (a config may narrow, never widen, the source's
  `lifecycle_modes`/`continuity_modes`; the effective set is the config override
  when present, else the source envelope; continuity requires a mode at least as
  strong as required), every required
  capability **at its exact revision** (the source envelope is the provisionable
  ceiling; config/source declarations MUST stay within it), the imported
  `ENFORCED` evidence for security-class capabilities, mechanically enforceable
  workspace/network modes and isolation, the pinned sandbox policy, and the
  policy-bound evidence **subject** (the exact `SpawnSourceRef`, `SourceConfigRef`
  and config digest it was resolved for). A capability's matcher kind and
  security class are defined once by a canonical `CapabilityCatalog`; a
  `DECLARED`/`ENFORCED` claim label is never a proof, and restrictions MUST NOT
  be inferred from a stronger capability.
- `can_provision_task` is the eligible-candidate predicate. The conjunction
  `can_execute(agent, task) && can_provision(agent, source, config, evidence)` is
  **necessary but not sufficient**: the imported environment MUST also be able to
  enforce the Task's effective (stricter) workspace/network and pinned sandbox
  policy, not merely the AgentType ceiling. B.3/B.4 MUST use
  `can_provision_task`, not the bare conjunction.
- `more_specific_for` is defined only once both types are executable, over the
  semantic/authority/scope dimensions (affinity, budget, lifecycle, information
  functions, workspace/network, isolation, sandbox policy, anchor, and capability
  values). It MUST be strict (two types with equivalent authority are
  incomparable) and MUST NOT rank by nominal inheritance depth. `continuity`
  strength is deliberately **not** part of semantic specificity: it is a
  candidate-ranking dimension applied after compatibility (a stronger guarantee
  lets a type execute more continuity-requiring Tasks).
- `is_valid_refinement` enforces monotonic narrowing of affinity, budget,
  information functions, sandbox policy, anchor, security policy, and capability
  values; a derived type MUST NOT widen the lifecycle mode set, weaken required
  isolation, or **weaken a continuity
  guarantee** (`derived.continuity >= base.continuity` — strengthening is
  allowed). Affinity is an allowed-tag ceiling with an explicit top: `Any` is
  general and unconstrained, `Only(A) ≤ Only(B)` iff `A ⊆ B`, and
  `Any ≤ Only(B)` is false. Capability values use the catalog-owned **polarity**,
  which is an independent field: `SecurityClass` decides whether imported
  evidence is required, while `CapabilityPolarity` decides which direction
  narrows. An `Ability` capability narrows by shrinking — a derived type may
  remove or lower it but never add or raise it; a `Restriction` capability
  narrows by growing — a derived type may add or raise it but never drop or
  lower it. Functional/Authority and Sandbox/Continuity are only conventional
  defaults, not bindings. The executable-Task-set-subset
  invariant holds for the ability/scope dimensions; continuity is deliberately a
  guarantee-strengthening dimension outside it. The `lifecycle` set is a
  **required source envelope** (the source must support every listed mode), not
  a set of alternative instantiation choices.

`SourceConfigRef` + revision + `ConfigDigest` are the Core-visible **selection
identity** of an opaque source-private configuration. The configuration body or
locator (model/provider/CLI/config-file semantics) belongs to the source
integration and is not part of AgentType semantics; Core MUST NOT interpret it,
and it need not be stored in `agentype-agent-contract`.

## Refinement monotonicity

A Root-created derived type MUST NOT enlarge authority.

MUST hold:

- DerivedAffinity ≤ BaseAffinity (allowed-tag ceiling)
- DerivedBudget ≤ BaseBudget
- Derived lifecycle mode set ⊆ Base lifecycle mode set
- Derived capability values follow catalog polarity: Ability values must not
  exceed base; Restriction values must not fall below base
- Derived continuity MUST NOT weaken the base guarantee
- Derived workspace/network/attempt-isolation MUST NOT weaken base
- Derived sandbox policy MUST NOT widen
- Derived anchor MUST satisfy base anchor constraints

Anchor MUST satisfy base anchor constraints. There is no free-form
permission/visibility/tools/roots field: those concerns are capabilities
(evidence-proven for security classes) or the sandbox policy reference.

## Information functions

EXPAND, COMPRESS_POSITIVE, and COMPRESS_NEGATIVE are information operations.

They MUST NOT be a required mutually exclusive AgentType taxonomy and MUST
NOT be a retention mode.

An AgentType MAY declare zero, one, or multiple information functions.
A single small-domain LogicalAgent MAY carry both positive and negative
maintenance.

Lifecycle (short-lived explore-and-retire vs long-lived continuity) is
orthogonal.

Concrete set/trait encoding is RESOLVED in M6-A (D-INFO-FN): the closed
three-value `InformationFunction` enum (`EXPAND`, `COMPRESS_POSITIVE`,
`COMPRESS_NEGATIVE`) with state-gated admission. Implementations MUST NOT ship
`enum AgentKind { Positive, Negative, Explorer }` as the type identity.

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
