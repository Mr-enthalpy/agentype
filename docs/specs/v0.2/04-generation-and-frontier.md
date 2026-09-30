# 04 — Generation and Semantic Frontier

Status: Normative
Canonical path: docs/specs/v0.2/04-generation-and-frontier.md
Conformance: **M6 only**. M4 MUST NOT implement Generation.

Generation is a **semantic frontier barrier**, not an organizational level
and not a Batch.

## Ownership

Root MUST retain semantic frontier admission.
Admitting a Generation MUST materialize a bounded slice whose **scope and
expansion ceiling are fixed by Root** at admission.
GenerationPolicy MUST constrain admitted work.
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
     - Writes outbox event `generation.frozen` in the same transaction.
   - Race condition rule (Race A): If an admission commits before freeze, the resulting task is part of the generation and must settle before closure. If freeze commits before admission, any pending EXPAND admission is rejected.
3. **`close_generation(generation_id, expected_revision)`**: Transitions `FROZEN -> CLOSED`.
   - Requires `is_generation_settled` barrier predicate:
     - `generation.state == FROZEN`.
     - Every task admitted into the generation has reached a terminal M5 disposition (`Completed`, `Failed`, or `Cancelled`).
   - Atomic SQLite transaction:
     - Verifies `expected_revision` matches current revision.
     - Transitions state to `CLOSED` and increments `revision`.
     - Atomically expires any remaining pending proposals with reason `GENERATION_CLOSED`.
     - Writes outbox event `generation.closed` in the same transaction.

### Settled Barrier Predicate

```text
GenerationSettled(G) :=
    G.state == FROZEN
    AND
    every Task admitted into G has reached an M5 terminal disposition (Completed, Failed, Cancelled)
```

Mechanical work (retries, recovery, adapter reconciliation, lease renewals) remains inside the originating semantic Task and Generation. It MUST NOT create a new Generation.

## Task Materialization and Admission

Every semantic Task MUST belong to exactly one Generation via an explicit `GenerationTaskBinding`.

- Who may request materialization: Root (via explicit proposal admission).
- Scheduler MUST persist: atomic creation of the M5 `TaskRecord` and `GenerationTaskBindingRecord` in one transaction.
- Workers and compilers MUST NOT materialize executable Tasks directly.
- **D-GEN-INTRA Resolution**: Root MAY add Tasks to an already `OPEN` or `FROZEN` generation dynamically (subject to information function admission rules). In `FROZEN`, only `COMPRESS_POSITIVE` and `COMPRESS_NEGATIVE` proposals may be admitted.

## Information Functions

Every work proposal and task binding is classified under an explicit `InformationFunction`:

- `EXPAND`: Expands semantic breadth or depth (investigations, audits, reproductions, explorations). Can only be admitted while generation is `OPEN`.
- `COMPRESS_POSITIVE`: Synthesizes accepted evidence, architectures, or findings into consumable summaries. May be admitted in `OPEN` or `FROZEN`.
- `COMPRESS_NEGATIVE`: Distills failed paths, invalid assumptions, or rejected alternatives into scoped negative evidence. May be admitted in `OPEN` or `FROZEN`.

## Provenance Model (D-GEN-TOPOLOGY Resolution)

- Each task binding carries an immutable `SemanticInputSet` capturing references to upstream results, seeds, and artifacts.
- Provenance forms a clean semantic DAG via parent generation references and task input bindings without requiring an external ontology engine.

## Ingress and Compilation (D-INTENT-SCHEMA Resolution)

- `RawWorkIntent` is an unprivileged semantic suggestion emitted by workers or Root.
- A deterministic compiler translates `RawWorkIntent` into a durable `CompiledWorkProposal`.
- Proposals become executable Tasks only upon explicit Root admission.

## Batch

A Generation MAY contain multiple Batches.
Batch completion MUST NOT auto-admit the next Generation.
