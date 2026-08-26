# Rust Rewrite Boundary

Status: Implementation strategy
Canonical path: docs/design/v0.2/10-rust-rewrite-boundary.md

## 1. V0.2 is RIIR, not continuation of Python

The Python V0.1.2 implementation is:

- a correctness oracle;
- a behavior/test reference;
- a historical implementation.

It should not remain a co-equal implementation line.

Do not transliterate `core.py` function by function.

Re-implement from normative semantics and regression behavior.

## 2. Milestone order

### M0 — Freeze V0.1.2

- merge release closure;
- final acceptance;
- tag;
- no new Python Core semantics.

### M1 — Repository/document normalization

- stable docs taxonomy;
- V0.1.2 historical freeze;
- V0.2 design landing zone.

### M2 — Freeze V0.2 semantic design

- AgentType;
- SpawnSource;
- Generation;
- WorkIntent;
- Transform;
- Lineage;
- Memory;
- Revival;
- Sandbox.

### M3 — Rust workspace/bootstrap

- domain types;
- SQLite layer;
- migrations;
- test harness.

### M4 — Reproduce V0.1.2 correctness kernel

Before implementing V0.2 semantic expansion, reproduce:

- Task/Attempt/Lease;
- fencing;
- Result Queue;
- Batch;
- writer safety;
- recovery;
- LogicalAgent;
- Incarnation/Execution;
- topology;
- outbox.

### M5 — Runtime + first adapter parity

Reach previous live acceptance with Rust runtime.

### M6 — Add V0.2 semantic layer

Only after M4/M5 correctness parity.

### M7 — Second adapter

Demonstrate frontend/provider neutrality without Core semantic changes.

## 3. Suggested workspace boundaries

Conceptually:

- `agentype-core`
- `agentype-storage-sqlite`
- `agentype-runtime`
- `agentype-adapter-api`
- `agentype-adapter-codex`
- `agentype-root-bridge`
- `agentype-cli`

Exact crate names are not frozen by this bundle.

## 4. Async boundary

Core state transitions and SQLite authority should remain transaction-oriented and deterministic.

Async concerns primarily belong in runtime/adapters:

- process I/O;
- observation;
- notification;
- timeout;
- supervision.

Do not spread async through domain logic without need.

## 5. Likely implementation tools

Possible choices include tokio, rusqlite, serde, thiserror, tracing, clap, toml, and uuid.

These are implementation suggestions, not architecture contracts.

## 6. Main RIIR rule

Do not simultaneously change language, redefine the correctness kernel, and add new semantic architecture without staged gates.

First reproduce correctness, then add V0.2 semantics.
