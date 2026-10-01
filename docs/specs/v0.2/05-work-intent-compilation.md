# 05 — RawWorkIntent and Compilation

Status: Normative
Canonical path: docs/specs/v0.2/05-work-intent-compilation.md

## Boundary

```text
RawWorkIntent → compilation → CompiledWorkProposal → Root admission
  → admitted semantic operation
       TASK              → Task
       TRANSFORM         → AgentTransform (refinement work still uses the
                           ordinary Task/Attempt/Lease/Result kernel)
       TYPE_REFINEMENT   → type-refinement intent
       TOPOLOGY_CHANGE   → topology intent
       NEEDS_ROOT_DECISION → stays with Root
```

The operation vocabulary above is the generic **V0.2** vocabulary. **M6-A
implements only the `TASK` path**: every proposal admissible in M6-A
materializes as an ordinary M5 Task. `TRANSFORM`, `TYPE_REFINEMENT`,
`TOPOLOGY_CHANGE`, and their admission are **not** implemented by M6-A and MUST
NOT be assumed to be. So in M6-A "not every admitted proposal becomes a Task"
does not apply: admission always creates a Task.

Compilation answers: if admitted, how would the architecture represent the
work?
Admission answers: should it enter the frontier now?

Compilation MUST NOT imply admission.
Any step that needs model/agent execution MUST still enter the ordinary
Task/Attempt/Lease/Result kernel.

## RawWorkIntent

Domain-semantic, architecture-light. Typical content: objective, rationale,
question or change, expected outcome, evidence refs, dependency refs,
observed constraints, affected domain, blocking indicator.

It is an ingress value, not a durable aggregate (see
[01](01-domain-model.md)). In M6-A a worker-originated intent is durable only
through the source `Result` that carried it, and Root-originated intent is
command input; the compiler turns it into a durable `CompiledWorkProposal`,
which is the first Scheduler-owned review object.

A worker/harness intent is carried inside the ordinary Result payload under the
reserved envelope `_agentype.raw_work_intents`, keyed by `raw_intent_key`:

```json
{"_agentype": {"raw_work_intents": {"<key>": {
    "objective": "...",
    "information_function": "EXPAND",
    "rationale": null,
    "semantic_input_set": {"result_ids": [], "artifact_refs": [], "seed_refs": []},
    "suggested_task_spec": null
}}}}
```

Compilation selects `(ResultId, raw_intent_key)` and reconstructs the intent from
the immutable Result. A caller MUST NOT supply the intent separately, so a
proposal's `source_ref` cannot claim provenance the Result does not carry. An
absent or malformed envelope fails closed and produces no semantic commitment.

Ordinary workers MUST NOT need AgentType registries, Generation mechanics,
Pool topology, SpawnSource, Transform, or Lease/Attempt/Incarnation knowledge.

M6-A freezes the concrete ingress `RawWorkIntent` value defined by the Semantic
Frontier Kernel. Richer domain-specific intent schemas are post-M6-A extensions
and MUST NOT reopen `D-INTENT-SCHEMA`, which is RESOLVED in M6-A
([17](17-deferred-open-questions.md)).

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
NEEDS_ROOT_DECISION as future vocabulary. In M6-A only the TASK shape is
admissible; see the Boundary scope above. These remain proposals until Root
admission.

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

These outcome names are compiler **vocabulary**, not a persistent state machine
an M6-A implementation must materialize.

The concrete M6-A compiler surface is narrower: a deterministic compile either
produces exactly one durable `CompiledWorkProposal` carrying the intent's
content, or fails deterministically without creating any semantic commitment.
Mapping onto the vocabulary:

| Outcome | Meaning |
|---|---|
| COMPILED | one durable proposal produced |
| REDUNDANCY_CANDIDATE | stable-identity replay returns the existing durable proposal; it stays Root-visible/auditable through that proposal |
| NEEDS_ROOT_DECISION | proposal produced without a normalized TaskSpec; Root decides at admission |
| NEEDS_DECOMPOSITION | cannot compile without split; MUST NOT recurse; no commitment produced |
| INVALID | malformed or disallowed by schema/policy; no commitment produced |

Root-visibility for a rejected or ambiguous intent is preserved through the
durable proposal when one exists. A compile that produces no commitment leaves
no Scheduler object, because the intent itself is not a durable Scheduler record
([01](01-domain-model.md)).

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

`Generation drains → Results durable → intents read (from source Results / Root input) → compilation pass → durable proposals → Root-reviewable → Root reject/defer/admit`.

If compilation requires agent/model execution, that work is ordinary
Task/Attempt/Lease/Result. **Which Generation (if any) that compiler Task
belongs to** is DEFERRED (D-COMPILATION-CLOSURE). It MUST NOT be used to
recursively expand the frontier. RIIR MUST NOT pick a default.
