# Agent Instruction: Absorb the V0.2 Design Bundle

Canonical path: docs/design/v0.2/AGENT_INGESTION_GUIDE.md

You are about to work on the Agentype V0.2 architecture or Rust rewrite.

Your first task is not to modify code.

Your first task is to absorb this design bundle and construct a faithful internal model of its architecture.

## 1. Reading order

Read every document in this order:

1. README.md
2. 00-design-charter.md
3. 01-system-thesis-and-information-functions.md
4. 02-root-operating-model.md
5. ROOT_OPERATING_DOCTRINE.md
6. 03-flat-organization-and-generations.md
7. 04-work-intent-compilation.md
8. 05-agent-type-and-spawn-source.md
9. 06-transform-lineage-and-memory.md
10. 07-revival-continuity-and-terminal-boundary.md
11. 08-sandbox-and-capability-enforcement.md
12. 09-v01-correctness-kernel-boundary.md
13. 10-rust-rewrite-boundary.md
14. 11-open-questions.md
15. 12-normative-invariants.md

Do not stop after the overview.

Do not treat a previous agent's comprehension report as canonical input.
Those reports live under `docs/reports/` as historical evidence. Absorb this
bundle, then write a new report under `docs/reports/v0.2/`. Never add that
report to `MANIFEST.json`.

## 2. Required interpretation

Build your architecture model around these distinctions:

- organizational role vs model choice;
- AgentType vs SpawnSource;
- LogicalAgent vs Incarnation vs Execution vs Attempt;
- positive semantics vs negative semantics vs exploration;
- Generation vs Batch;
- RawWorkIntent vs CompiledWorkProposal vs admitted Task;
- compilation vs Root admission;
- MOVE/MERGE vs TRANSFORM;
- revival vs Transform;
- Scheduler memory floor vs native terminal continuity;
- correctness capabilities vs continuity capabilities vs experience capabilities;
- flat information topology vs forbidden command hierarchy;
- information function vs mutually exclusive AgentType class;
- Generation transition vs mechanical retry/recovery/revival.

If you collapse any pair above into one concept, your interpretation is probably wrong.

## 3. Do not import common multi-agent assumptions

Do not assume:

- Root is Root because it uses the most expensive model;
- workers are workers because they use cheaper models;
- AgentType is an OO class hierarchy;
- every long-lived agent is a positive-semantic maintainer;
- every worker may recursively spawn/subdivide work;
- a compiler agent is a manager;
- a Generation is an organizational layer;
- terminal thread/session identity is LogicalAgent identity;
- transcript retention is durable memory;
- a native terminal subagent UI is required for correctness;
- more agents is inherently better.

## 4. Core mental model

Root owns the canonical semantic model and frontier admission.

Workers perform typed information functions.

Scheduler owns mechanical execution correctness and lifecycle.

Messages and type affinity create temporary work organization.

No persistent multi-level command tree is required.

The architecture structures cognition even if all models are equally capable.

## 5. Before proposing implementation

Produce a short architecture comprehension report under `docs/reports/v0.2/`,
not inside this bundle. A previous report is evidence of one reading, not
source design.

The report must contain:

### A. Ten frozen invariants in your own words

Do not copy them verbatim.

### B. Eight distinction checks

Explain the difference between:

1. AgentType and SpawnSource
2. LogicalAgent and Incarnation
3. Generation and Batch
4. RawWorkIntent and CompiledWorkProposal
5. compilation and admission
6. MOVE/MERGE and TRANSFORM
7. revival and Transform
8. Scheduler continuity floor and native terminal resume

### C. One end-to-end example

Trace:

user problem
→ Root
→ exploratory Generation
→ Result
→ RawWorkIntent
→ compilation
→ Root admission
→ next Generation
→ typed LogicalAgent
→ SpawnSource
→ Execution
→ Result integration

Show where authority changes and where it does not.

### D. Three failure modes caused by violating the design

At minimum cover:

- recursive frontier explosion;
- hierarchy creep;
- model-routing semantics leaking into Core.

### E. Open questions

Identify which questions are explicitly unresolved rather than inventing answers.

## 6. Implementation discipline

After the comprehension report, if implementation is requested:

- preserve the V0.1 correctness kernel first;
- treat Python V0.1.2 as a behavior oracle, not a code template;
- do not transliterate module structure mechanically;
- do not introduce V0.2 semantic features before the Rust correctness kernel reaches parity;
- keep Core free from vendor-specific model/terminal semantics;
- make security restrictions mechanically enforceable;
- make revival transparent to Root;
- keep WorkIntent compilation non-authoritative;
- keep agent topology flat.

## 7. Conflict handling

If repository code, old docs, or implementation convenience conflicts with this design bundle:

1. identify the conflict explicitly;
2. determine whether the conflicting material is V0.1 historical implementation or V0.2 architecture;
3. do not silently preserve the old shape;
4. do not silently rewrite the frozen direction;
5. escalate unresolved architecture contradictions before coding through them.

## 8. Completion criterion for ingestion

You have absorbed the design only when you can reason about Agentype without relying on specific model names, Codex-specific thread semantics, Python implementation layout, or hierarchical manager/worker metaphors.

The architecture should still make sense under a hypothetical universal-AGI, single-model world.
