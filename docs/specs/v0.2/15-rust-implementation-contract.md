# 15 — Rust Implementation Contract

Status: Normative
Canonical path: docs/specs/v0.2/15-rust-implementation-contract.md

Crate names are IMPLEMENTATION-DEFINED. Dependency **direction** is not.

## Required direction

```text
domain/core
    MUST NOT depend on adapter, vendor, runtime I/O, or CLI

storage
    persists core state; MUST NOT define scheduling semantics

runtime
    dispatcher, reconciler, recovery, notifier isolation

adapter-api
    provider-neutral execution contract:
    traits, adapter-facing DTOs, and deterministic worker execution protocol

adapter implementations
    depend on adapter-api / core DTOs; MUST NOT leak into core

root bridge
    wakeup transport; independent of execution correctness

cli / composition root
    wires configuration; MUST NOT become Core
```

Suggested names (`agentype-core`, `agentype-storage-sqlite`, …) are
IMPLEMENTATION-DEFINED.

## Hard rules

- Core MUST NOT import Codex/OpenCode/Claude/Grok/provider-specific concepts.
- Core MUST NOT depend on terminal UI semantics.
- SpawnSource/provider configuration MUST remain outside AgentType identity.
- Runtime I/O MAY be async.
- Core transaction/state logic SHOULD be deterministic and testable without
  an async runtime.
- SQLite types MUST NOT leak into pure domain semantics where avoidable.
- Python `core.py` MUST NOT be transliterated function-by-function.

## Type-system expectations (guidance, not crate API)

SHOULD: newtypes for IDs; enums for closed states; explicit epochs/authority
tokens; typed errors; no magic strings for correctness-critical states;
frozen snapshots of execution authority; separate Root-facing vs
adapter-facing DTOs.

MUST NOT overuse typestate so that persistence/recovery becomes impractical.
Persisted state MUST remain inspectable and migration-friendly.

Do not freeze serde/tokio/rusqlite/clap choices.
