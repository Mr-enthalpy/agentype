# RIIR M6-A — Result-Carried Intent Binding (Re-freeze)

Status: Historical Report
Applies to: V0.2 / M6-A re-freeze (result-carried intent binding)
Canonical path: `docs/reports/v0.2/m6a-result-carried-intent-binding.md`
Not a specification.

This report records a narrow correctness closure on top of the M6-A freeze
([m6a-semantic-frontier-freeze](m6a-semantic-frontier-freeze.md)). It does not
rewrite that report; it adds the finding that motivated this fix and the
invariant that now holds.

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

## Still queued (non-blocking P2)

```text
SemanticInputSet multiplicity canonicalization (set vs multiset)
schema relational hardening (admission_seq uniqueness, lifecycle CHECKs)
broader transaction fault-injection rollback tests
root README Rust-status banner
v4 -> v5 SQLite migration policy (D-DB-MIGRATE)
```

These remain out of scope for this closure.

## Baseline

```text
Base (main): 4c04aa76579b076325e14f2c18a558f30613d303
SCHEMA_VERSION = 5 (unchanged)
```
