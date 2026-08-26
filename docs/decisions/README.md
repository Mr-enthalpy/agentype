# Architecture decisions

Status: Index
Canonical path: docs/decisions/README.md

This folder records **why** a durable choice exists (ADRs).

- Architecture invariants stay in [docs/architecture/overview.md](../architecture/overview.md).
- Versioned testable contracts stay in [docs/specs/](../specs/).
- V0.2 design drafts stay in [docs/design/v0.2/](../design/v0.2/).
- Python how-to stays in [docs/development/](../development/).

No ADRs are extracted in the repository-normalization change. The decisions
below already live as Core Invariants; copying them now would duplicate
normative text.

## Extraction TODO (reserved IDs)

| ID (reserved) | Decision already established |
|---|---|
| ADR-0001 | LogicalAgent identity is distinct from physical execution/session identity (invariants 6–14). |
| ADR-0002 | Task execution is at-least-once with fencing, not exactly-once (invariants 15–20). |
| ADR-0003 | SQLite is the V0.1 single-machine authority, not a permanent semantic contract (invariant 65). |
| ADR-0004 | Result transport is durable and separate from Root notification (invariants 25–28, 61). |
| ADR-0005 | Runtime/Frontend adapters do not define Core scheduling semantics (invariants 46–57). |
| ADR-0006 | Physical writer quiescence is required before unsafe replacement (invariants 21–23, §31). |

New ADRs start at `docs/decisions/0007-short-title.md` when a **new** durable
choice is made (expected during V0.2 design, not this layout change).

Template for a new ADR:

- title
- date
- status
- context
- decision
- consequences
