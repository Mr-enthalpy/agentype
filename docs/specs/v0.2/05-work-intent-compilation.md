# 05 — RawWorkIntent and Compilation

Status: Normative
Canonical path: docs/specs/v0.2/05-work-intent-compilation.md

## Boundary

```text
RawWorkIntent → compilation → CompiledWorkProposal → Root admission → Task
```

Compilation answers: if admitted, how would the architecture represent the
work?
Admission answers: should it enter the frontier now?

Compilation MUST NOT imply admission.

## RawWorkIntent

Domain-semantic, architecture-light. Typical content: objective, rationale,
question or change, expected outcome, evidence refs, dependency refs,
observed constraints, affected domain, blocking indicator.

Ordinary workers MUST NOT need AgentType registries, Generation mechanics,
Pool topology, SpawnSource, Transform, or Lease/Attempt/Incarnation knowledge.

How much structure is required is DEFERRED (D-INTENT-SCHEMA). Too little
burdens the compiler; too much leaks Scheduler architecture.

## CompiledWorkProposal

Architecture-aware, execution-unbound.

It MUST NOT normally bind `logical_agent_id`, `incarnation_id`, `attempt_id`,
`lease_id`, or a concrete SpawnSource, unless a future normative rule
explicitly requires it.

It MAY include: normalized objective, semantic operation, TaskRequirement,
affinity/anchor/capability/sandbox needs, continuity preference, dependencies,
acceptance criteria, suggested Generation policy, candidate type constraints,
decision requirements, compiler evidence.

Proposal kinds MAY include TASK, TYPE_REFINEMENT, TRANSFORM, TOPOLOGY_CHANGE,
NEEDS_ROOT_DECISION. These remain proposals until Root admission.

## Compiler

Conceptually `compile_work_intent(intent, architecture_view) → outcome`.

The compiler MUST NOT possess worker-management authority, frontier-admission
authority, or hierarchical status.

It has no privileged lifecycle. If compilation requires agent/model
execution, that execution MUST be ordinary scheduled work governed by
Task/Attempt/Lease/Result.

Compiler view MAY include AgentType registry, type revisions, Generation
policy, capability/sandbox vocabularies, anchor taxonomy. It MUST NOT need
active Leases, heartbeat, current Incarnation IDs, or physical processes.

## Outcomes

Normative names:

| Outcome | Meaning |
|---|---|
| COMPILED | one proposal produced |
| REDUNDANCY_CANDIDATE | possible duplicate; intent remains Root-visible |
| NEEDS_ROOT_DECISION | architectural/semantic ambiguity |
| NEEDS_DECOMPOSITION | cannot compile without split; MUST NOT recurse |
| INVALID | malformed or disallowed by schema/policy |

`REJECTED_AS_REDUNDANT` is **not** a V0.2 disposition that may drop an
intent from Root's frontier view. Compiler MAY detect redundancy;
disposition remains Root's. A `REDUNDANCY_CANDIDATE` MUST stay auditable
(intent id + compiler evidence + optional pointer to an existing
proposal/intent/evidence_ref).

Automatic drop of an intent is forbidden unless a later spec defines a
**deterministic exact-duplicate** predicate (same originating Result and
normalized objective at minimum) **and** Root override/audit. Until then,
implementations MUST NOT auto-drop.

Architectural ambiguity MUST return to Root. The compiler MUST NOT guess
and MUST NOT negatively admit (silently close) the frontier.

## Cardinality

POLICY-DEFINED (V0.2 initial): `1 RawWorkIntent → 0..1 CompiledWorkProposal`.

Whether limited one-to-many normalization is ever justified is DEFERRED
(D-INTENT-FANOUT). Default MUST remain non-expansive.

End-of-generation flow:

`Generation drains → Results durable → intents collected → compilation pass → proposals → REVIEWABLE → Root reject/defer/admit`.
