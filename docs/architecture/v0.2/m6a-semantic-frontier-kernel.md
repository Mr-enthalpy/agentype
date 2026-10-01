# M6-A Semantic Frontier Kernel

Status: Architecture
Applies to: V0.2 / M6-A
Canonical path: `docs/architecture/v0.2/m6a-semantic-frontier-kernel.md`
Not a specification. Normative frontier specification remains
[spec 04](../../specs/v0.2/04-generation-and-frontier.md).
Runtime execution and storage substrate remains frozen from M5.8
([m5.8-runtime-daemon](m5.8-runtime-daemon.md), [spec 14](../../specs/v0.2/14-recovery-and-reconciliation.md)).

---

## Frozen phrases

Semantic depth may be arbitrarily deep while durable authority topology remains flat.

No semantic expansion occurs merely because an agent suggested more work.

Compression is a derived artifact, never an authoritative replacement for its inputs.

The Scheduler governs durable commitments, not Root's cognitive attention or thought process.

Mechanical execution authority belongs to M5; semantic frontier admission belongs to Root.

A frozen generation halts expansion to allow convergence; it does not freeze execution in progress.

---

## 1. System Role and Separation of Concerns

M6-A does not introduce a manager agent, a conversation tree, or a working memory database.

Root is the single clean, revisable positive semantic integrator. Its internal reasoning chains, attention levels, and context windows belong to the host environment, never to the Scheduler. The Scheduler's role is strictly limited to governing durable commitments:

```text
untrusted semantic suggestion
        ↓
RawWorkIntent
        ↓ deterministic compile
CompiledWorkProposal
        ↓ explicit admission (Root)
M5 Task + GenerationTaskBinding
        ↓
ordinary M5 execution/result authority
        ↓
Result + provenance
        ↓
Root pulls / zooms / integrates
```

### Negative Space (Strictly Prohibited in M6-A)

1. **No AgentType or SpawnSource selection**: Deferred to M6-B.
2. **No Transform or MemoryCapsule**: Deferred to M6-D.
3. **No Root Context Persistence**: The database stores no `root_thought`, `attention_level`, or `transcript_cache`.
4. **No Autonomous Spawning**: Workers and compilers cannot directly materialize executable Tasks.
5. **No Degradation of M5 Substrate**: Leases, attempts, results, process locks, and observer loops remain 100% authoritative and unchanged.

---

## 2. Core Ontology

1. **`Generation`**: A durable semantic frontier barrier answering which semantic tasks belong to the current exploration slice.
2. **`InformationFunction`**: The designated role of a work proposal in information space:
   - `EXPAND`: Expands problem breadth or investigates depth.
   - `COMPRESS_POSITIVE`: Synthesizes accepted evidence and architectures into consumable summaries.
   - `COMPRESS_NEGATIVE`: Distills failed paths, invalid assumptions, and dead ends into scoped negative knowledge.
3. **`RawWorkIntent`**: An unprivileged ingress value representing a suggestion for future work.
4. **`CompiledWorkProposal`**: A deterministically validated, normalized, and durable candidate work item.
5. **`GenerationTaskBinding`**: Durable many-to-one mapping tying an M5 Task to its parent Generation and freezing its immutable `SemanticInputSet`.
6. **`SemanticInputSet`**: Immutable value object capturing explicit upstream `ResultId`s, seeds, or artifact references.
7. **`GenerationView`**: Derived read-only projection summarizing generation state, admitted task IDs, proposal counts, and settled status.

---

## 3. Generation Lifecycle and Invariants

The Generation state machine contains three durable states:

```text
OPEN  ──(freeze_generation)──>  FROZEN  ──(close_generation)──>  CLOSED
```

### State Matrix

| State | Semantic Invariant | Permitted Admissions |
|---|---|---|
| `OPEN` | Semantic breadth may expand. | `EXPAND`, `COMPRESS_POSITIVE`, `COMPRESS_NEGATIVE` |
| `FROZEN` | Semantic breadth is sealed. Expansion forbidden. | `COMPRESS_POSITIVE`, `COMPRESS_NEGATIVE` |
| `CLOSED` | Terminal. All admitted work has settled. | None |

### Atomic Transitions

1. **`create_generation`**:
   - Creates a generation in `OPEN` state with `revision = 0` and an initial seed payload.
2. **`freeze_generation(generation_id, expected_revision)`**:
   - Enforces optimistic concurrency (`revision == expected_revision`).
   - Atomically updates state to `FROZEN` and increments `revision`.
   - Atomically marks all unadmitted `PENDING` proposals with information function `EXPAND` as `EXPIRED` (`reason: GENERATION_FROZEN`).
   - Emits an outbox event `GENERATION_FROZEN` in the same transaction.
   - **Race A (admit vs freeze)**: Serialized by SQLite transaction. If admission commits first, the task is bound to the generation and must settle before closure. If freeze commits first, subsequent EXPAND admissions fail immediately with `GenerationFrozen`.
3. **`close_generation(generation_id, expected_revision)`**:
   - Enforces the `is_generation_settled` barrier predicate:
     ```text
     GenerationSettled(G) :=
         G.state == FROZEN
         AND
         every Task admitted into G has reached an M5 terminal disposition (Completed, Cancelled; Suspended tasks require Root disposition)
     ```
     `is_generation_settled` is the close gate and is false once the generation is
     `CLOSED`. The derived `GenerationView.is_settled` field is a separate terminal
     projection (`state == CLOSED || is_generation_settled`), so a closed generation
     reads `is_settled = true`.
   - Atomically updates state to `CLOSED` and increments `revision`.
   - Atomically marks any remaining unadmitted `PENDING` proposals as `EXPIRED` (`reason: GENERATION_CLOSED`).
   - Emits an outbox event `GENERATION_CLOSED` in the same transaction.

---

## 4. Admission and Compilation Semantics

- **Typed Intent Source**: an intent is compiled from a typed `IntentSource`, never from free-form text. `IntentSource::Root` is anchored to a Root command reference; `IntentSource::Result` is anchored to a durable `Result`. The persisted `(source_kind, source_ref)` identity (`"root"` / `"result"`) is derived from this type.
- **Result-Carried Intent**: a worker/harness intent is not supplied separately. It is written into the ordinary Result payload under the reserved envelope `_agentype.raw_work_intents`, keyed by `raw_intent_key`, and selected by `(ResultId, raw_intent_key)`. Storage reconstructs the intent from the immutable Result payload, so a caller cannot label an unrelated intent with a Result it did not come from, and a committed Result always makes its carried intents recoverable across a crash. Absent or wrong-typed envelopes fail closed, and unknown fields in an intent entry (or in its top-level `semantic_input_set`) are rejected; the carried `suggested_task_spec` and the surrounding worker payload stay opaque and are not validated for extra keys.
- **Deterministic Compilation**: `compile_intent` maps a `RawWorkIntent` into a unique `CompiledWorkProposal`. It is idempotent on `(generation_id, source_kind, source_ref, raw_intent_key, compiler_version)` with canonical payload fingerprinting (content changes for the same identity are rejected with Conflict).
- **Task-Only Materialization**: the broader V0.2 operation vocabulary (Transform, type refinement, topology change) is not implemented here. Every proposal admissible in M6-A materializes as an ordinary M5 Task; there is no non-Task admission path.
- **TaskSpec Dependencies**: In M6-A, `TaskSpec.dependencies` must be empty. Cross-admission dependency is expressed by `SemanticInputSet` provenance and Root admission order.
- **Generation / Batch Orthogonality**: Generation is a semantic frontier barrier, not an aggregate execution barrier. Each admitted task is materialized into its own fresh dedicated mechanical Batch, ensuring completion or cancellation of prior tasks never blocks dynamic admissions into an `OPEN` Generation.
- **Atomic Admission**: `admit_proposal` executes in a single SQLite transaction:
  1. Checks generation admission rules (`generation_allows_admit`).
  2. Transitions proposal state from `PENDING` to `ADMITTED`. Concurrent duplicate admissions result in exactly one winner (Race B).
  3. Validates that provenance `result_ids` exist in durable storage.
  4. Resolves `TaskSpec` (enforcing explicit Root decision if unspecified in proposal, and detecting conflicts).
  5. Creates an M5 `TaskRecord` in state `Queued` inside a fresh dedicated mechanical execution batch (a new batch identity minted per admission, independent of the task id).
  6. Creates a `GenerationTaskBindingRecord` linking the task, generation, information function, admitted `TaskSpec`, and frozen `SemanticInputSet`.

---

## 5. Provenance and the Semantic DAG

Compression is an analytical convenience, not a replacement of historical truth:
- A compression task consumes an immutable `SemanticInputSet` specifying the exact inputs it summarizes.
- Original raw results are never deleted or mutated upon compression.
- Provenance forms a clean directed acyclic graph (DAG) through `SemanticInputSet` upstream `ResultId`s, artifact references, and seed inputs on each task binding, supporting selective zoom-in and audit without external graph databases or dedicated generation parent links.

---

## 6. Runtime Public API Boundary

Host control interacts with the semantic frontier via `RootSemanticControl`
(full authority) and `IntentIngress` (proposal-only, for external harnesses and
workers):

```rust
let sem = daemon.semantic_control();   // or control.semantic_control()
let ingress = daemon.intent_ingress(); // or control.intent_ingress()

// Root authority surface
let gen = sem.create_generation(json!({ "seed": "data" }))?;
let prop = sem.compile_root_intent(&gen.generation_id, intent, "cli", 1)?;
let task_id = sem.admit_proposal(&prop.proposal_id, 0, None)?;
let view = sem.read_generation_view(&gen.generation_id)?;
sem.freeze_generation(&gen.generation_id, 0)?;
sem.close_generation(&gen.generation_id, 1)?;

// Proposal-only surface: select an intent the Result already carries.
let prop = ingress.compile_from_result(&gen.generation_id, &source_result_id, "audit-session-race", 1)?;
```

### Package Boundary Witness

As verified by `agentype-public-api-boundary`:
- `RootSemanticControl` does **not** expose worker acknowledgement (`ack_success`, `ack_failure`).
- `RootSemanticControl` does **not** expose mechanical lease renewal or dispatch loops.
- `IntentIngress` exposes **only** `compile_from_result(result_id, raw_intent_key, ...)`; it has no `admit_proposal`, `reject_proposal`, `freeze_generation`, or `close_generation`, and it cannot be handed an arbitrary intent — the intent is loaded from the named Result.
- `Kernel` remains internal to the storage layer and cannot be named or instantiated by external consumers.
