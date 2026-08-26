# V0.2 design ingestion report

Status: Historical Report
Applies to: V0.2 design bundle at `e71f8ec`
Canonical path: docs/reports/v0.2/design-ingestion-e71f8ec.md
Not a specification. Not part of the canonical design bundle.

This is a fresh reading of `docs/design/v0.2/` at `e71f8ec75c1a4b70952c1240e38e5873556f9265`
per `AGENT_INGESTION_GUIDE.md`. It does not treat
`docs/reports/v0.2/design-ingestion-d1cfc458.md` as source design. That file
remains historical evidence of an earlier, incomplete absorption.

Not a specification. Does not freeze Rust APIs or storage schema.

The architecture remains coherent if every model is equally capable, cheap,
fast, long-context, and general.

## A. Ten frozen invariants

Restated from `12-normative-invariants.md`; not a substitute for that file.

1. **Authority topology is flat.** Information dependencies may be arbitrarily
   deep. Core must not grow manager / team-lead / subordinate objects, and a
   Generation must not become a layer that re-grants spawning rights.
2. **Root keeps one revisable positive model and never yields frontier
   admission.** Admitting a Generation materializes a bounded slice whose
   ceiling Root already fixed. GenerationPolicy constrains work in that slice;
   it does not receive independent frontier-admission authority.
3. **Worker output is evidence.** A Result, including a self-labeled
   `validated_delta`, does not write canonical long-lived semantic memory.
   Root Result ACK is consumption, not worker completion. Promotion protocol
   is still an open question; until specified there is no hidden write path.
4. **EXPAND / COMPRESS-POSITIVE / COMPRESS-NEGATIVE are information
   operations, not exclusive AgentType classes and not retention modes.**
   Lifecycle (short-lived explore-and-retire vs long-lived continuity) is
   orthogonal. A small-domain agent may carry more than one information
   function.
5. **Workers may propose work; they do not expand the executable frontier.**
   RawWorkIntent is allowed only when Generation policy permits. Intent is not
   an admitted Task. Compilation is not admission.
6. **Generation is a semantic-frontier barrier; Batch is an execution
   completion barrier.** Mechanical retry, recovery, reconciliation, and
   revival stay inside the originating Task/Generation. They are not a
   Generation transition.
7. **AgentType and SpawnSource are orthogonal.** Type is
   responsibility / capability / sandbox / lifecycle. SpawnSource is physical
   provisioning. Model cost and latency are provisioning policy, not type
   identity. Root-created refinements may only narrow authority.
8. **TRANSFORM changes semantic identity; MOVE/MERGE and revival do not.**
   Transform creates a successor LogicalAgent on the same AgentLineage.
   Revival preserves the same LogicalAgent and is normally invisible to Root.
9. **Claimed restrictions must be mechanically enforceable.** Prompt text is
   not a sandbox. Effective permission is AgentType ∩ Generation ∩ Task ∩
   SpawnSource ceiling. A source that cannot enforce the sandbox cannot
   provision the type.
10. **The V0.1 correctness kernel is not optional.** Task existence is not
    execution authority. Claim atomically creates Attempt + Lease + fencing.
    At-least-once, writer quiescence, atomic success-and-Result, and
    Scheduler-only claim/state remain. If WorkIntent compilation needs a
    model, that run is ordinary scheduled work with no privileged lifecycle.

## B. Distinction checks

### 1. AgentType vs SpawnSource

AgentType is the semantic/security/lifecycle contract. SpawnSource is how a
compatible LogicalAgent is physically hosted. One source may provision many
narrower types if it can enforce them. Naming a type after a model puts
provisioning policy in Core.

### 2. LogicalAgent vs Incarnation

LogicalAgent is long-term semantic identity. Incarnation is one physical
hosting period. Execution is one task-scoped turn. Attempt is Scheduler
authority over one try. Process loss is not LogicalAgent loss.

### 3. Generation vs Batch

Generation names an already-admitted semantic frontier slice and the
expansion ceiling Root fixed for it. Batch names an execution/synchronization
completion unit. A Generation may contain several Batches. Auto-admitting the
next wave from Batch completion would hand Root’s frontier decision to the
execution layer.

### 4. RawWorkIntent vs CompiledWorkProposal

RawWorkIntent is domain language (unknown, why it matters, evidence).
CompiledWorkProposal is architectural language (type, anchor, sandbox,
capabilities *if* admitted). Proposals should not normally carry concrete
`logical_agent_id`, `lease_id`, or a chosen SpawnSource.

### 5. Compilation vs admission

Compilation asks how the work would be represented. Admission asks whether it
enters the frontier now. The compiler has no management rank, no frontier
authority, and no privileged lifecycle. Default: one intent yields at most
one proposal; otherwise `NEEDS_ROOT_DECISION` / `NEEDS_DECOMPOSITION`.

### 6. MOVE / MERGE vs TRANSFORM

MOVE/MERGE change population/topology classification and preserve
LogicalAgent identity. TRANSFORM changes semantic responsibility and must
create a successor identity. It is a durable workflow, not a special MOVE.

### 7. Revival vs Transform

Revival: physical host gone, same LogicalAgent, Root should not orchestrate
it. Transform: intentional semantic role change; Root may express that
intent. Treating revival as “hire a new worker” leaks physical lifecycle.

### 8. Scheduler continuity floor vs native terminal resume

Mandatory floor: MemoryCapsule + Checkpoint + authoritative
project/workstream state. Exact session resume only improves fidelity.
Terminal child-agent UI is an ExperienceCapability, never a correctness
dependency.

Also keep separate, as the guide requires: organizational role vs model
choice; positive vs negative vs exploratory *functions* vs exclusive classes;
correctness vs continuity vs experience capabilities; deep information
dependencies vs flat command topology; Generation transition vs mechanical
retry/recovery/revival; `AGENT_INGESTION_GUIDE` (how an implementer
understands Agentype) vs `ROOT_OPERATING_DOCTRINE` (how a running Root uses
it).

## C. End-to-end example

User problem: determine whether lease expiry can race with writer completion
and break fencing.

1. **User → Root.** Root states the current positive model and the highest
   value unknown. Semantic frontier authority stays with Root. No Task exists.
   No execution authority exists. Root follows `ROOT_OPERATING_DOCTRINE.md`:
   it will not manage leases, revival, or polling.

2. **Root → exploratory Generation.** Root admits a bounded EXPLORE
   Generation. That admission materializes a slice whose scope and expansion
   ceiling Root already fixed. GenerationPolicy constrains admitted work; it
   does not receive frontier-admission authority. Still no claim.

3. **Generation → Task.** Root expresses a task requirement. Scheduler
   durably materializes schedulable work. **No execution authority yet.** A
   Task is not Attempt, Lease, or fencing.

4. **Task → claim → Execution.** Scheduler atomically creates Attempt +
   Lease + fencing epoch. **Execution authority is established here.**
   Matching a short-lived explorer and provisioning an Incarnation via
   SpawnSource are provisioning, not type identity and not semantic final
   review.

5. **Execution → Result.** The explorer returns evidence and, if policy
   allows, a RawWorkIntent. Success and the authoritative Result are atomic.
   A `validated_delta` in the Result is still evidence, not MemoryCapsule
   contents.

6. **RawWorkIntent → compilation.** Compilation answers representation, not
   admission. If it needs a model, that invocation is ordinary
   Task/Attempt/Lease/Result work. The compiler cannot claim, admit a
   Generation, or pick a SpawnSource. Ambiguity returns to Root.

7. **Generation REVIEWABLE → Root admission.** Root reviews evidence,
   keeps negative findings scoped, and reject/defer/admits the next slice.
   Frontier-admission authority never left Root. Mechanical retry of the
   explorer, if any, would not have opened a new Generation.

8. **Next Generation → typed LogicalAgent.** If admitted work is ongoing
   maintenance, Root may express type refinement (narrowing only). Scheduler
   resolves TaskRequirement onto an existing LogicalAgent or a new identity.
   Information functions on that type are a set, not `enum {Positive,
   Negative, Explorer}`.

9. **SpawnSource → Execution → Result → Root integration.** After a new
   claim, the adapter runs Execution. Root integrates reference-first into
   the single current positive model. Root ACK consumes the Result Queue.
   Who later promotes accepted deltas into long-lived MemoryCapsule remains
   an open question; Root does not invent a hidden write path.

What does not happen: recursive Task minting; compiler-as-manager;
Generation-as-sub-Root; thread id as LogicalAgent; retry as a new
Generation; worker self-promotion of memory; Scheduler LLM outside a Task.

## D. Failure modes

1. **Recursive frontier explosion.** Worker Results become Tasks, or the
   compiler turns one intent into a tree of executable work. Mitigation:
   non-expansive compilation; AUDIT Generations forbid intents; mechanical
   expansion budgets.

2. **Hierarchy creep.** Team / reports_to objects, or treating Generation
   admission as a transfer of frontier authority so that “Root grants a
   Generation, the Generation grants workers bounded spawning.” Mitigation:
   persist caused_by / depends_on / audits / supersedes; keep admission
   exclusively with Root.

3. **Model-routing semantics leaking into Core.** AgentType named by
   expensive vs cheap models, or vendor SpawnSource fields on the claim
   path. When models homogenize, the architecture collapses. Mitigation:
   types encode responsibility and enforceable constraints; cost stays in
   provisioning policy.

Related modeling errors this bundle now forbids explicitly: encoding
information functions as a mutually exclusive AgentKind enum; opening a new
Generation for retry/revival/reconciliation; giving the compiler a
privileged lifecycle.

## E. Explicitly unresolved questions

Do not invent answers. Source: `11-open-questions.md` at `e71f8ec`.

- GenerationPolicy size vs workflow DSL (modes, expansion-budget shape,
  boolean vs numeric WorkIntent allowance, reviewability).
- How much structure ordinary workers must put in RawWorkIntent.
- Whether limited 1-to-many compilation is ever justified (current
  preference: no).
- Precise `can_execute` / `can_provision` / `more_specific_for` /
  `is_valid_refinement` representation.
- AgentType revision semantics.
- MemoryCapsule size, fields, provenance, positive/negative specialization,
  merge rules.
- **Semantic memory promotion protocol:** who accepts a Result delta into
  long-lived MemoryCapsule (Root review vs explicit integration Task vs
  another kernel-governed mechanism).
- **Negative semantic entry lifecycle:** scope, assumptions, applicability,
  supersession, hot/cold retention and GC.
- **Information-function representation:** exclusivity is frozen as false;
  concrete set/trait encoding and matching rules are not.
- ContinuityBinding storage, security, verification, invalidation.
- Exact Root review protocol.
- Transform failure and lineage rollback.
- Exact V0.2 distinction among type refinement, partition capacity, MOVE,
  MERGE, TRANSFORM.
- Minimal second-adapter conformance test.
- Whether Rust V0.2 migrates V0.1 SQLite in place, imports, or starts a new
  database. Decide before storage implementation.

## Conflicts with the current repository

Unchanged classification; not silently resolved.

| Conflict | Class | Handling |
|---|---|---|
| `docs/architecture/overview.md` is still V0.1 direction. | V0.1 architecture document | Leave it. V0.2 frozen direction lives in `docs/design/v0.2/`. |
| V0.1 `PoolPartition` binds target, profile, capacity, and classification. | V0.1 executable contract | Leave `docs/specs/v0.1.md`. |
| V0.1 has no Generation / AgentType / SpawnSource / Transform / WorkIntent. | Kernel vs semantic layer | Expected. Reproduce the kernel before adding the semantic layer. |
| Root README and package still describe Python 0.1.3. | Current product line | Keep as correctness oracle until RIIR gates say otherwise. |
| `10-rust-rewrite-boundary.md` sketches crate names. | Unfrozen representation | Suggestion only. No `Cargo.toml`. |

Aligned already: LogicalAgent ≠ physical session; at-least-once plus
fencing; Result transport separate from Root notification; writer
quiescence; adapters do not define Core; Root is notification-driven.
