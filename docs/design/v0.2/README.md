# Agentype V0.2 Design Bundle

Status: Design consolidation
Canonical path: docs/design/v0.2/README.md
Scope: V0.2 semantic architecture and RIIR preparation
Implementation status: no Rust API or storage schema is frozen by this bundle

This directory is the repository landing zone for the V0.2 design bundle.
It is not the V0.1 executable contract (`docs/specs/v0.1.md`) and it is not a
rewrite of `docs/architecture/overview.md`.

The central premise is that Agentype is not primarily a model-routing
optimization. It is an organizational and information-theoretic runtime for
structuring search, compression, memory, authority, continuity, and semantic
integration.

The V0.1 correctness kernel remains the execution foundation: Task / Attempt /
Lease / Result / LogicalAgent / Incarnation / Execution / Batch / Escalation /
Pool correctness must not be weakened by V0.2 semantic features.

Unresolved questions — do not invent answers during ingestion or kernel work:
[11-open-questions.md](11-open-questions.md).

If repository code or V0.1 documentation conflicts with this bundle: state the
conflict, classify it as historical V0.1 vs current V0.2, and do not silently
preserve the old design or rewrite the frozen direction.

Two operational contracts live beside the numbered documents and are not
substitutes for each other:

- [AGENT_INGESTION_GUIDE.md](AGENT_INGESTION_GUIDE.md) teaches an
  architecture/RIIR agent how to absorb Agentype.
- [ROOT_OPERATING_DOCTRINE.md](ROOT_OPERATING_DOCTRINE.md) teaches a runtime
  Root how to use Agentype to solve a user problem.

Latest ingestion *evidence* (historical report, not canonical input):
[docs/reports/v0.2/design-ingestion-e71f8ec.md](../../reports/v0.2/design-ingestion-e71f8ec.md).
An earlier incomplete absorption remains at
[docs/reports/v0.2/design-ingestion-d1cfc458.md](../../reports/v0.2/design-ingestion-d1cfc458.md).
Every new architecture/RIIR agent must produce its own comprehension from this
bundle. A previous agent's report must not be treated as source design.

## Reading order

1. `00-design-charter.md`
2. `01-system-thesis-and-information-functions.md`
3. `02-root-operating-model.md`
4. `03-flat-organization-and-generations.md`
5. `04-work-intent-compilation.md`
6. `05-agent-type-and-spawn-source.md`
7. `06-transform-lineage-and-memory.md`
8. `07-revival-continuity-and-terminal-boundary.md`
9. `08-sandbox-and-capability-enforcement.md`
10. `09-v01-correctness-kernel-boundary.md`
11. `10-rust-rewrite-boundary.md`
12. `11-open-questions.md`
13. `12-normative-invariants.md`

`AGENT_INGESTION_GUIDE.md` is the operational instruction for a future
architecture/implementation agent to absorb the bundle without flattening it
into a simplistic hierarchy or model-routing design.

`ROOT_OPERATING_DOCTRINE.md` is the runtime Root prompt contract. Read it
after `02-root-operating-model.md`. It does not replace the architecture
model and does not grant Root Scheduler mechanics.

## Normative intent

This bundle distinguishes:

- **Frozen direction**: architectural rules that should constrain V0.2.
- **Proposed representation**: names/data shapes that may still change.
- **Deferred detail**: implementation choices intentionally left to the Rust
  design/implementation phase.

Exact crate names, SQLite schema, and GenerationPolicy encoding are not frozen
here. Do not create a Cargo workspace from this landing.

If future implementation contradicts a frozen direction, the contradiction
should be explicit and reviewed rather than silently introduced.

## Current executable contract

Until open questions are resolved and a V0.2 spec is written:

- Language-independent V0.1 invariants: [docs/architecture/overview.md](../../architecture/overview.md)
- Testable V0.1 contract: [docs/specs/v0.1.md](../../specs/v0.1.md)
