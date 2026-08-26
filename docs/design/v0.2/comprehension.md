# V0.2 design ingestion report

Status: Historical Report
Applies to: V0.2 design bundle landing
Canonical path: docs/design/v0.2/comprehension.md
Not a specification. Does not freeze Rust APIs or storage schema.

This report is the required architecture comprehension output from
`AGENT_INGESTION_GUIDE.md` and the ingestion `plan.txt`. It restates frozen
direction in the ingesting agent's words, records conflicts with the V0.1
tree, and lists questions the bundle marks unresolved. It does not answer
those questions.

The design remains coherent if every available model is equally capable,
cheap, fast, long-context, and general. Model names, price tiers, Python
package layout, and terminal thread identity are not architectural load-bearing
parts.

## A. Ten frozen invariants

Restated from `12-normative-invariants.md`; not a substitute for that file.

1. **Topology stays flat.** Information dependencies may be deep. Core must
   not grow manager / team-lead / subordinate objects. Temporary cooperation
   comes from Generation, type affinity, anchors, dependencies, and Result
   flow — not from a command tree.
2. **Root only integrates and admits.** It keeps one current, revisable
   positive semantic model. It does not run Leases, Attempts, heartbeats,
   Incarnations, or physical sessions.
3. **Worker output is evidence, not authority.** Results enter Root
   integration. Root Result ACK is consumption, not worker completion.
   Producing a Result does not grant semantic final say.
4. **Expand, compress-positive, and compress-negative are information
   operations, not model tiers.** The same underlying capability can instantiate
   Root, a positive maintainer, a negative auditor, or an explorer. The
   distinction is responsibility, visible information, sandbox, and lifecycle.
5. **Only Root admission expands the executable semantic frontier.** Workers
   may emit RawWorkIntent only when Generation policy allows. An intent is not
   an admitted Task.
6. **Generation is a semantic-frontier barrier; Batch is an
   execution-completion barrier.** They do not replace each other. Every
   semantic Task belongs to a Generation, and every Generation has a bounded
   expansion policy.
7. **AgentType and SpawnSource are orthogonal.** AgentType is the
   semantic / capability / security / lifecycle contract. SpawnSource is how a
   compatible logical agent is physically provisioned. Cost, latency, and
   context length are provisioning policy, not type identity.
8. **TRANSFORM changes semantic identity; MOVE and MERGE do not.** Transform
   creates a successor LogicalAgent on the same AgentLineage and retires the
   source. Revival preserves the same LogicalAgent.
9. **A claimed restriction counts only if it is mechanically enforceable.**
   Prompt text is not a sandbox. Effective permission is the intersection of
   AgentType, Generation, Task, and SpawnSource ceilings. A source that cannot
   enforce the sandbox cannot provision the type.
10. **The V0.1 correctness kernel cannot be bypassed by the semantic layer.**
    At-least-once execution, Attempt/Lease fencing, atomic success-and-Result,
    writer quiescence before unsafe replacement, and Scheduler-only
    claim/state remain. Generation, AgentType, and the compiler must not
    complete work outside that kernel.

## B. Eight distinctions

### 1. AgentType vs SpawnSource

AgentType says what a logical agent is for: responsibility, visible state,
tools, sandbox, continuity, lifecycle. SpawnSource says how a compatible
agent is physically hosted. One source may provision many narrower types if
it can enforce their restrictions. One type may be provisioned by several
sources. Naming a type after a model puts provisioning policy in Core.

### 2. LogicalAgent vs Incarnation

LogicalAgent is long-term semantic identity (type, lineage, anchor,
MemoryCapsule, task-facing identity). Incarnation is one physical hosting
period. Loss of a process is not loss of the logical agent. Execution is one
task-scoped turn. Attempt is Scheduler authority over one try to finish a
Task. Those four names are not synonyms.

### 3. Generation vs Batch

Generation answers which work belongs to one semantic frontier (explore,
implement, audit, …) and whether expansion is allowed. Batch answers which
Tasks form one execution/synchronization completion unit. A Generation may
contain several Batches. Using Batch completion to auto-admit the next wave
hands the semantic frontier to the execution layer.

### 4. RawWorkIntent vs CompiledWorkProposal

RawWorkIntent is domain language: what remains unknown, why it matters, which
evidence applies. CompiledWorkProposal is architectural language: if admitted,
what type, anchor, sandbox, and capabilities the work would need. Domain
workers need not understand pools, SpawnSource, or Leases. A proposal should
not normally carry a concrete `logical_agent_id`, `lease_id`, or chosen
SpawnSource.

### 5. Compilation vs Root admission

Compilation asks: if this work were admitted, how should it be represented?
Admission asks: should it enter the semantic frontier now? Compilation is an
ordinary typed function (a specialized AgentType, a deterministic validator,
or both). It has no management rank and no frontier authority. Default:
one RawWorkIntent yields at most one proposal. Ambiguity returns
`NEEDS_ROOT_DECISION` or `NEEDS_DECOMPOSITION`. The compiler must not
recursively create more work.

### 6. MOVE / MERGE vs TRANSFORM

MOVE and MERGE change population and topology classification while preserving
LogicalAgent semantic identity (already in V0.1). TRANSFORM changes semantic
responsibility (type, sandbox, affinity, compression policy). It must create a
successor identity rather than mutate type in place. It is a durable workflow
(QUIESCING → REFINING_CONTEXT → CUTOVER), not a special MOVE in one SQLite
transaction.

### 7. Revival vs Transform

Revival: the physical host is gone; the logical agent remains. Root should
normally not see it as an orchestration event. AgentType does not change.
Transform: semantic role changes on purpose. Root may express Transform
intent because that is a semantic decision. Treating revival as “hire a new
worker” leaks physical lifecycle to Root.

### 8. Scheduler continuity floor vs native terminal resume

The correctness floor is MemoryCapsule + Checkpoint + authoritative
project/workstream state (Level 3). Exact same-session resume (Level 1) only
improves fidelity. Revival must still be valid without native resume. Without
the floor, recovery is not normal transparent revival. Terminal child-agent
UI is an ExperienceCapability, never a correctness dependency.

Also keep separate: Correctness vs Continuity vs Experience capabilities;
deep information dependencies vs flat authority topology.

## C. End-to-end example

User problem: determine whether lease expiry can race with writer completion
and break fencing.

1. **User → Root.** Root writes the current positive model: the fencing
   invariant must hold; this race is the highest-value unknown. Semantic
   authority is with Root. Scheduler has no Task authority yet.

2. **Root → exploratory Generation.** Root admits a bounded EXPLORE
   Generation (limited RawWorkIntent allowed; workers cannot create Tasks).
   This is the only transfer of **semantic frontier** authority: from Root’s
   admit/deny into that Generation’s policy ceiling. Still no claim.

3. **Generation → Task.** Root expresses a task requirement (type, anchor,
   read-only sandbox, acceptance). Scheduler creates Task/Batch. **Execution
   authority is born here.** Root does not assign Leases.

4. **Task → exploratory LogicalAgent → Execution.** Scheduler matches a
   short-lived explorer (or provisions one). Claim transactionally creates
   Attempt, Lease, and fencing epoch. Execution authority stays in Scheduler.
   If a physical host is needed, a SpawnSource provisions an Incarnation.
   That is provisioning, not type identity and not semantic final review.

5. **Execution → Result.** The explorer returns a conclusion, evidence
   references, and — if policy allows — a RawWorkIntent: “verify whether
   expiry can race with completion.” Success and the authoritative Result are
   atomic. The worker is done. The Result is evidence, not a patch to Root’s
   model.

6. **RawWorkIntent → compiler → CompiledWorkProposal.** The compiler
   (ordinary function) turns the intent into scheduling requirements: VERIFY
   operation, read-only, fencing-related capabilities, suggested Generation
   policy. **No authority moves.** It cannot claim, open a Generation, or pick
   a physical source. If the work must split, it returns NEEDS_DECOMPOSITION
   instead of hatching more intents.

7. **Generation REVIEWABLE → Root admission.** The wave is drained; Results
   and intents are durable; compilation has finished. Root reviews once:
   integrates compact negative findings, then reject / defer / admit the next
   wave. Semantic frontier authority is again only with Root.

8. **Next Generation → typed long-lived agent.** If the admitted work is
   ongoing audit or positive maintenance, Root may express type refinement or
   population intent. Scheduler resolves TaskRequirement onto an existing
   LogicalAgent or a new identity. Refinements may only narrow authority.

9. **SpawnSource → Execution → Result → Root integration.** Provisioning
   picks a source that can enforce the sandbox. The adapter runs Execution.
   Another authoritative Result is created atomically. Root integrates
   reference-first into the single current positive model; the obsolete model
   becomes a negative record. Root ACK consumes the Result Queue and does not
   complete the worker.

What does not happen: the explorer does not recursively mint executable
Tasks; the compiler does not become a manager; thread IDs are not
LogicalAgent identity; Scheduler does not run a hidden LLM to “tidy memory”
outside a Task.

## D. Failure modes from violating the bundle

1. **Recursive frontier explosion.** Worker Results become Tasks directly, or
   the compiler turns one intent into a tree of executable work. Exploration
   becomes autocatalytic. Mitigation: non-expansive compilation; AUDIT
   Generations forbid intents; mechanical expansion budgets.

2. **Hierarchy creep.** Team / reports_to / compiler-as-manager objects are
   added “for manageability.” Information flow is mistaken for command.
   Mitigation: persist caused_by / depends_on / audits / supersedes; let
   temporary teams emerge from affinity and Generation.

3. **Model-routing semantics leaking into Core.** AgentType is named by
   expensive vs cheap models, or vendor SpawnSource fields enter the claim
   path. When models homogenize, the architecture collapses. Mitigation:
   types encode responsibility and enforceable constraints; cost and latency
   stay in provisioning policy.

## E. Explicitly unresolved questions

Do not invent answers here. Source: `11-open-questions.md`.

- How small GenerationPolicy can stay without becoming a workflow DSL
  (semantic modes, expansion-budget shape, boolean vs numeric WorkIntent
  allowance, reviewability conditions).
- How much structure ordinary domain workers must put in RawWorkIntent.
- Whether limited 1-to-many compilation is ever justified (current
  preference: no).
- Precise representation of `can_execute`, `can_provision`,
  `more_specific_for`, and `is_valid_refinement`.
- AgentType revision semantics (immutable revisions, cross-revision
  compatibility, exact vs compatible task pins, Transform across revisions).
- MemoryCapsule size bounds, field types, evidence/provenance format,
  positive/negative specialization, merge rules.
- ContinuityBinding storage, security, verification, invalidation.
- Exact Root review protocol (API/result surface, admit/defer/reject,
  conflict resolution).
- Transform failure: refinement suspend, cancel before cutover, partial
  target, lineage rollback.
- Exact V0.2 distinction among type refinement, partition capacity, MOVE,
  MERGE, and TRANSFORM.
- Minimal second-adapter conformance test.
- Whether Rust V0.2 migrates V0.1 SQLite in place, offers explicit import,
  or starts a new database plus a tool. Decide this before storage
  implementation.

## Conflicts with the current repository

Classified; not silently resolved.

| Conflict | Class | Handling |
|---|---|---|
| `docs/architecture/overview.md` is still V0.1 direction (Codex as first integration; partitions bind an execution target). | V0.1 architecture document | This landing does not rewrite it. V0.2 frozen direction lives in this directory. |
| V0.1 `PoolPartition` carries target, profile, capacity, and classification together. | V0.1 executable contract (`docs/specs/v0.1.md`) | Leave the V0.1 spec untouched. V0.2 wants partitions as desired population of an AgentType at an anchor — later spec work, not this landing. |
| V0.1 has no Generation, AgentType, SpawnSource, Transform, or WorkIntent. | Kernel vs semantic layer | Expected. Reproduce the kernel (RIIR M4) before adding the semantic layer (M6). |
| Root README and package still describe Python 0.1.3. | Current product line | Keep. Python is the correctness oracle until staged RIIR gates say otherwise. Deleting it now skips gates. |
| `10-rust-rewrite-boundary.md` sketches crate names. | Unfrozen representation | Keep as suggestion. This landing creates no `Cargo.toml`. |
| The zip and ingestion `plan.txt` at repo root. | Task input | Unpacked markdown is tracked here. The zip and `plan.txt` are not committed. |

Aligned already, and V0.2 requires keeping: LogicalAgent ≠ physical session;
at-least-once plus fencing; Result transport separate from Root notification;
writer quiescence; adapters do not define Core; Root is notification-driven
and does not poll Scheduler.

## What this landing is not

- Not M3+ RIIR.
- Not a V0.2 executable specification.
- Not ADR extraction.
- Not an answer to the open questions.
- Not a rewrite of the V0.1 correctness kernel.
