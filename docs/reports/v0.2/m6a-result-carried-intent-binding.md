# RIIR M6-A — Result-Carried Intent Binding (Re-freeze)

Status: Historical Report
Applies to: V0.2 / M6-A re-freeze (result-carried intent binding)
Canonical path: `docs/reports/v0.2/m6a-result-carried-intent-binding.md`
Not a specification.

This report records a narrow correctness closure on top of the M6-A freeze
([m6a-semantic-frontier-freeze](m6a-semantic-frontier-freeze.md)). It does not
rewrite that report; it adds the finding that motivated this fix and the
invariant that now holds. The re-freeze claim takes effect when the closure PR
merges.

## Finding (P1)

The frozen design states that a worker-originated `RawWorkIntent` is durable
through the source `Result` that carried it, and that
`IntentSource::Result` is mechanically anchored. The implementation at the time
of the freeze anchored only the Result **identity**: `IntentIngress` accepted a
caller-supplied `RawWorkIntent` plus any existing `ResultId`, and storage merely
verified `SELECT 1 FROM results WHERE id = ?`. An unrelated intent could
therefore be labelled with a Result that never expressed it, and a worker intent
that had not yet been handed to the compiler was lost if the process crashed
after the Result committed.

This did not grant Task authority — Root admission was still required — so it
was P1, not P0. But it defeated the crash-durability and provenance reason for
carrying intents in Results.

## Closure

Result-backed compilation is now anchored to Result **content**:

```text
RawWorkIntent values are written into the ordinary Result payload under
_agentype.raw_work_intents, keyed by raw_intent_key.

compile_result_intent(result_id, raw_intent_key) loads the intent from the
immutable Result payload inside the same BEGIN IMMEDIATE transaction that
persists the proposal.
```

- The free `compile_intent(intent, source)` entry is gone. Root uses
  `compile_root_intent(intent, command_ref)`; external callers use
  `IntentIngress::compile_from_result(result_id, raw_intent_key)` and cannot
  supply an arbitrary intent.
- A missing Result, a Result without the reserved envelope, a missing key, or a
  malformed entry each fail closed and create no commitment.
- No schema change: `results.payload_json` already exists and M5 keeps the
  payload opaque; only the M6 ingress interprets the reserved namespace.

Decode strictness is scoped to the envelope and the Agentype-owned fields.
`raw_work_intent_from_json` rejects unknown fields in an intent entry, and
`semantic_input_set_from_json` rejects unknown top-level fields; enum-valued
fields fail closed on unknown values. The carried `suggested_task_spec`
(including its nested `payload` / `acceptance`) and the remainder of the worker
payload are intentionally opaque: `task_spec_from_json` validates the
authority-bearing fields and their contract but does not reject every extra
nested key. Unifying a strict nested unknown-field policy across all decoders is
left as future hardening and is not claimed here.

The resulting worker path is:

```text
Worker Result commits (payload carries _agentype.raw_work_intents)
    ⇒ the intent is recoverable across a crash
    ⇒ compile_result_intent(result_id, raw_intent_key)
    ⇒ durable CompiledWorkProposal (PENDING)
    ⇒ explicit Root admission
    ⇒ M5 Task + GenerationTaskBinding
```

## Regressions

```text
test_result_backed_intent_source_must_exist
test_result_carried_intent_is_loaded_from_result_payload
test_result_without_requested_intent_fails_closed
test_malformed_result_intent_envelope_fails_closed
test_result_carried_intent_replay_is_idempotent
test_result_carried_expand_after_generation_freeze_is_not_compiled
test_result_carried_intent_survives_file_backed_restart_before_compile
```

The restart regression is genuinely file-backed (`CARGO_TARGET_TMPDIR`), commits
a Result carrying an intent, drops and reopens the Kernel, then compiles by
`(ResultId, raw_intent_key)` and confirms the proposal is durable across a
second reopen.

## Documentation

Spec [05](../../specs/v0.2/05-work-intent-compilation.md) and the
[m6a-semantic-frontier-kernel](../../architecture/v0.2/m6a-semantic-frontier-kernel.md)
architecture note now define the `_agentype.raw_work_intents` envelope, the
`(ResultId, raw_intent_key)` selection, and the fail-closed rules.

## Freeze-gate resolution

The result-carried intent closure was audited at head:

```text
Audited P1 closure head: 23b7727ac3310de7c3b7a588f9d2f5d6eefc53bc
P0 = 0
P1 = 0
P2 = non-blocking
```

P2 hardening (set semantics, fault-injection rollback proof, root README status)
was then added on the same branch; those changes do not alter M6-A authority or
the frozen frontier contract.

## Still queued (non-blocking P2)

```text
schema relational hardening (admission_seq uniqueness, lifecycle CHECKs,
    ON DELETE RESTRICT)
v4 -> v5 SQLite migration policy (D-DB-MIGRATE)
strict nested unknown-field policy unification across all decoders
```

These remain out of scope for this closure.

## Baseline

```text
Base (main): 4c04aa76579b076325e14f2c18a558f30613d303
SCHEMA_VERSION = 5 (unchanged)

M6-A re-freeze is the merge of the result-carried intent closure PR. If project
convention requires, the merge commit is recorded by a follow-up commit on main.
```
