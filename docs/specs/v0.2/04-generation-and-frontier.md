# 04 — Generation and Semantic Frontier

Status: Normative
Canonical path: docs/specs/v0.2/04-generation-and-frontier.md
Conformance: **M6 only**. M4 MUST NOT implement Generation.

Generation is a **semantic frontier barrier**, not an organizational level
and not a Batch.

## Ownership

Root MUST retain semantic frontier admission.
In M6-A, worker suggestions (`RawWorkIntent`) are strictly unprivileged and non-expansive by default.
Frontier expansion occurs exclusively through explicit Root admission of `EXPAND` proposals into an `OPEN` Generation.
Automated policy-governed admission (`GenerationPolicy`) is deferred to later extensions.
A Generation MUST NOT receive independent frontier-admission authority.
Workers MUST NOT receive spawning authority from a Generation.

## States

In M6-A, the durable Generation state machine is frozen to three states:

```text
OPEN
  |
  | freeze (atomic: increments revision, expires pending EXPAND proposals, writes outbox)
  v
FROZEN
  |
  | close (atomic: requires is_generation_settled barrier, increments revision, expires remaining proposals, writes outbox)
  v
CLOSED
```

| State | Semantic Meaning | Allowed Admissions |
|---|---|---|
| `OPEN` | Semantic breadth may expand. Root may explore depth or breadth. | `EXPAND`, `COMPRESS_POSITIVE`, `COMPRESS_NEGATIVE` |
| `FROZEN` | Semantic breadth is sealed. Expansion stops; only convergence/compression permitted. | `COMPRESS_POSITIVE`, `COMPRESS_NEGATIVE` (EXPAND is rejected) |
| `CLOSED` | Terminal. Bounded slice completed and settled. | None |

### State Transitions and Atomic Barriers

1. **`create_generation`**: Transitions `(none) -> OPEN`. Materializes the generation record with initial seed payload and revision 0.
2. **`freeze_generation(generation_id, expected_revision)`**: Transitions `OPEN -> FROZEN`.
   - Atomic SQLite transaction:
     - Verifies `expected_revision` matches current revision.
     - Transitions state to `FROZEN` and increments `revision`.
     - Atomically expires all pending `EXPAND` proposals belonging to this generation with reason `GENERATION_FROZEN`.
     - Writes outbox event `GENERATION_FROZEN` in the same transaction.
   - Race condition rule (Race A): If an admission commits before freeze, the resulting task is part of the generation and must settle before closure. If freeze commits before admission, any pending EXPAND admission is rejected.
3. **`close_generation(generation_id, expected_revision)`**: Transitions `FROZEN -> CLOSED`.
   - Requires `is_generation_settled` barrier predicate:
     - `generation.state == FROZEN`.
     - Every task admitted into the generation has reached a terminal M5 disposition (`Completed` or `Cancelled`; note that retry exhaustion leaves tasks in `Suspended` until Root explicit intervention).
   - Atomic SQLite transaction:
     - Verifies `expected_revision` matches current revision.
     - Transitions state to `CLOSED` and increments `revision`.
     - Atomically expires any remaining pending proposals with reason `GENERATION_CLOSED`.
     - Writes outbox event `GENERATION_CLOSED` in the same transaction.

### Settled Barrier Predicate

```text
GenerationSettled(G) :=
    G.state == FROZEN
    AND
    every Task admitted into G has reached an M5 terminal disposition (Completed or Cancelled; Suspended tasks require Root disposition)
```

`GenerationSettled(G)` is the `close_generation` gate, so it is deliberately false
once `G` has advanced to `CLOSED`. The derived `GenerationView.is_settled` field is
a distinct terminal projection: it is true for a settled `FROZEN` generation and
remains true for its terminal `CLOSED` successor
(`state == CLOSED || GenerationSettled(G)`). A `CLOSED` generation therefore
MUST NOT read `is_settled = false`.

Mechanical work (retries, recovery, adapter reconciliation, lease renewals) remains inside the originating semantic Task and Generation. It MUST NOT create a new Generation.

## Task Materialization and Admission

Every semantic Task MUST belong to exactly one Generation via an explicit `GenerationTaskBinding`.

- Who may request materialization: Root (via explicit proposal admission).
- Scheduler MUST persist: atomic creation of the M5 `TaskRecord` and `GenerationTaskBindingRecord` in one transaction.
- Workers and compilers MUST NOT materialize executable Tasks directly.
- **D-GEN-INTRA Resolution**: Root MAY add Tasks to an already `OPEN` or `FROZEN` generation dynamically (subject to information function admission rules). In `FROZEN`, only `COMPRESS_POSITIVE` and `COMPRESS_NEGATIVE` proposals may be admitted.
- **Task Dependencies**: In M6-A, `TaskSpec.dependencies` MUST be empty. Semantic order is expressed by `SemanticInputSet` provenance and Root admission timing.
- **Generation / Batch Orthogonality**: Generation is a semantic frontier barrier, whereas Batch is an aggregate execution barrier. To preserve orthogonality and prevent deadlocks on dynamic admissions, each admitted semantic task is materialized into its own fresh dedicated internal mechanical execution batch (a new batch identity minted per admission, independent of the task id). Prior task completions or cancellations never compromise the eligibility of subsequently admitted tasks in an `OPEN` Generation.

## Information Functions

Every work proposal and task binding is classified under an explicit `InformationFunction`:

- `EXPAND`: Expands semantic breadth or depth (investigations, audits, reproductions, explorations). Can only be admitted while generation is `OPEN`.
- `COMPRESS_POSITIVE`: Synthesizes accepted evidence, architectures, or findings into consumable summaries. May be admitted in `OPEN` or `FROZEN`.
- `COMPRESS_NEGATIVE`: Distills failed paths, invalid assumptions, or rejected alternatives into scoped negative evidence. May be admitted in `OPEN` or `FROZEN`.

## Provenance Model (D-GEN-TOPOLOGY Resolution)

- Each task binding carries an immutable `SemanticInputSet` capturing references to upstream results, seeds, and artifacts.
- Provenance `result_ids` must resolve to existing durable Result rows.
- Provenance forms a clean semantic DAG via `SemanticInputSet` upstream `ResultId`s, artifact references, and seed inputs without requiring an external ontology engine or dedicated generation parent links.

## Ingress and Compilation (D-INTENT-SCHEMA Resolution)

- `RawWorkIntent` is an unprivileged semantic suggestion emitted by workers or Root.
- A deterministic compiler translates `RawWorkIntent` into a durable `CompiledWorkProposal`.
- Proposal deduplication identity is scoped to `(generation_id, source_kind, source_ref, raw_intent_key, compiler_version)` with canonical payload fingerprinting. Content mismatches for the same identity are rejected with Conflict.
- Proposals become executable Tasks only upon explicit Root admission.

## Batch

A Generation MAY contain multiple Batches.
Batch completion MUST NOT auto-admit the next Generation.
