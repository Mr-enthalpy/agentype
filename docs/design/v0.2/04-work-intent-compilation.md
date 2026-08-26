# RawWorkIntent and CompiledWorkProposal

Status: Architecture direction
Canonical path: docs/design/v0.2/04-work-intent-compilation.md

## 1. Domain agents should not understand Scheduler architecture

Ordinary agents should describe what remains unknown, what should be verified, what needs to change, why it matters, and what evidence is relevant.

They should not be required to understand AgentType registry, Generation mechanics, Pool topology, SpawnSource, Transform mechanics, or Lease/Attempt/Incarnation state.

Architecture complexity should be absorbed by the architecture.

## 2. RawWorkIntent

RawWorkIntent is domain-semantic and architecture-light.

Suggested shape:

- intent_id;
- originating_result_id;
- objective;
- rationale;
- question_or_change;
- expected_outcome;
- relevant_evidence_refs;
- dependency_refs;
- observed_constraints;
- affected_domain;
- blocking indicator.

Example:

Objective: verify whether lease expiry can race with writer completion.

Rationale: an experiment observed completion after authority expiry.

Expected outcome: determine whether this violates the fencing invariant.

Evidence refs: ...

This is enough for the worker.

## 3. Compilation

RawWorkIntent is compiled into a scheduler-semantic intermediate representation:

`RawWorkIntent → CompiledWorkProposal`

Compilation answers:

> If this work were admitted, how should the architecture represent its requirements?

Compilation does not answer:

> Should we expand the semantic frontier now?

That remains Root admission.

## 4. CompiledWorkProposal

Suggested content:

- proposal_id;
- source_intent_id;
- normalized_objective;
- semantic_operation;
- task_requirement;
- affinity requirements;
- anchor requirements;
- capability requirements;
- sandbox requirements;
- continuity preference;
- dependency refs;
- acceptance criteria;
- suggested Generation policy;
- candidate type constraints;
- decision requirements;
- compiler evidence.

It should not normally contain logical_agent_id, incarnation_id, lease_id, or concrete SpawnSource selection.

Compilation produces scheduling requirements, not a physical execution plan.

## 5. Compiler is a function, not an authority level

Conceptually:

`compile_work_intent(intent, architecture_view) → CompiledWorkProposal`

The implementation may use a specialized long-lived AgentType, deterministic validation, or both.

It has no worker-management authority, no frontier-admission authority, no special Task lifecycle, and no hierarchical status.

## 6. Compiler input view

The compiler may need:

- AgentType registry;
- type revisions;
- Generation policy;
- capability vocabulary;
- sandbox vocabulary;
- anchor/workstream taxonomy;
- semantic operation vocabulary.

It should not need active Leases, heartbeat state, current Incarnation IDs, or physical process details.

## 7. Non-expansive compilation

Default invariant:

> WorkIntent compilation is non-expansive.

Preferred initial V0.2 rule:

`1 RawWorkIntent → 0..1 CompiledWorkProposal`

If decomposition is required, return `NEEDS_DECOMPOSITION` to Root rather than recursively creating more semantic work.

## 8. Compilation outcomes

Suggested outcomes:

- COMPILED;
- REJECTED_AS_REDUNDANT;
- NEEDS_ROOT_DECISION;
- NEEDS_DECOMPOSITION;
- INVALID.

Architectural ambiguity is not an excuse for the compiler to guess.

## 9. Proposal kinds

A compiled proposal may recommend:

- TASK;
- TYPE_REFINEMENT;
- TRANSFORM;
- TOPOLOGY_CHANGE;
- NEEDS_ROOT_DECISION.

These remain proposals until Root admission.

## 10. End-of-generation flow

`Generation N drains → Results durable → RawWorkIntents collected → compilation pass → CompiledWorkProposals → Generation REVIEWABLE → Root review → reject/defer/admit → Generation N+1`
