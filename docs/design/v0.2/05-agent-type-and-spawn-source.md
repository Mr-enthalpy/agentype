# AgentType, SpawnSource, and Typed Scheduling

Status: Architecture direction
Canonical path: docs/design/v0.2/05-agent-type-and-spawn-source.md

## 1. AgentType

AgentType is a semantic/capability/affinity/security/lifecycle contract.

It is not a model name, prompt alias, PoolPartition, SpawnSource, or OO subclass.

Suggested descriptor:

- type_id;
- revision;
- affinity_spec;
- capability_spec;
- sandbox_policy_ref;
- lifecycle_policy;
- continuity_policy;
- anchor_constraint;
- spawn_requirements;
- semantic function / memory policy;
- transform policy;
- optional based_on_type_id for provenance/config reuse.

## 2. No subtype-tree semantics

Traditional parent/child inheritance terminology is misleading because relation direction differs by concern.

For provisioning, a broad SpawnSource may be able to provision a narrower AgentType.

For task assignment, a narrower/more specific AgentType may be preferred over a broader compatible one.

For configuration, a refined type may reuse a broader type's configuration.

Therefore Core should use explicit relations:

- can_provision(SpawnSource, AgentType);
- can_execute(AgentType, TaskRequirement);
- more_specific_for(TypeA, TypeB, TaskRequirement);
- is_valid_refinement(BaseType, DerivedType).

These are compatibility/partial-order relations, not OO inheritance.

## 3. Refinement security monotonicity

Root-created derived/refined types may narrow authority but must not silently enlarge it.

Examples:

`DerivedPermission ⊆ BasePermission`

`DerivedVisibility ⊆ BaseVisibility`

`DerivedTools ⊆ BaseTools`

`DerivedRoots ⊆ BaseRoots`

`DerivedBudget <= BaseBudget`

Lifecycle may not widen beyond base policy.

Affinity may narrow/refine but not arbitrarily broaden authority.

Anchor must satisfy base anchor constraints.

## 4. PoolPartition

PoolPartition becomes primarily a desired logical population object.

Suggested fields:

- partition_id;
- agent_type_ref;
- optional anchor_ref;
- desired_capacity;
- scheduling_weight;
- active;
- topology_revision.

It answers:

> How many LogicalAgents of this AgentType at this anchor do we want?

Do not let PoolPartition absorb type semantics, model configuration, and capacity semantics simultaneously.

## 5. SpawnSource

SpawnSource is a physical provisioning source.

It sits above the current V0.1 notion of ExecutionTarget × ExecutionProfile.

Suggested fields:

- source_id;
- adapter_ref;
- target_selector;
- profile_selector;
- provisionable_capability_envelope;
- enforceable_sandbox_features;
- lifecycle_modes;
- supported_continuity_modes;
- source_tags;
- availability.

Core does not interpret vendor semantics.

## 6. Source selection

Selection flow:

`AgentType requirements → eligible SpawnSources → selection policy → Adapter`

Selection policy may use correctness constraints, sandbox enforceability, capability match, continuity preservation score, availability, cost, latency, context length, and tool support.

These are provisioning concerns, not AgentType semantics.

## 7. TaskRequirement

V0.2 should evolve task matching from direct partition selection toward typed requirements such as:

- type requirement;
- affinity;
- anchor;
- capability requirements;
- sandbox requirements.

Scheduler then resolves:

`TaskRequirement → matching LogicalAgent / PoolPartition → SpawnSource if provisioning is needed`

## 8. Matching preference

Conceptual order:

1. exact / most-specific compatible resident agent;
2. compatible narrower anchored type;
3. compatible broader/general type;
4. cold/revivable compatible logical agent;
5. provision a new logical agent from an eligible SpawnSource.

Ranking should consider compatibility, affinity specificity, capability surplus, anchor distance, continuity value, and warm/revival cost.

Do not rank by nominal type depth.
