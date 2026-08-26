# Agentype documentation

Status: Index
Canonical path: docs/README.md

## One-minute map

| Question | Answer |
|---|---|
| What is Agentype? | A single-machine, local-first, restart-safe Scheduler control plane. Root README and [docs/architecture/overview.md](architecture/overview.md). |
| What behavior is normative in V0.1.2 / the V0.1 Python line? | [docs/specs/v0.1.md](specs/v0.1.md). V0.1.2 is the correctness-closure baseline. 0.1.3 adds Grok ACP adapter/RootBridge transport without changing Core state machines. Frozen V0.1.2 bytes: `git show v0.1.2:docs/V0.1_SPEC.md`. |
| Which documents are historical evidence rather than specification? | Everything under [docs/acceptance/](acceptance/) and [docs/reports/](reports/). |
| Where do I add a new architecture decision? | [docs/decisions/](decisions/) (see that README). Do not add ADRs into `architecture/overview.md`. |
| Where will V0.2 design live? | Canonical bundle: [docs/design/v0.2/](design/v0.2/). Latest ingestion evidence: [docs/reports/v0.2/](reports/v0.2/) (historical, not source). Open questions in the bundle are unresolved; the executable contract remains [docs/specs/v0.1.md](specs/v0.1.md). |
| Can Python later be replaced by Rust without moving docs again? | Yes. `docs/` stays. `src/` is the current Python implementation, not a legacy graveyard. |

Normative: [docs/architecture/overview.md](architecture/overview.md) (invariants) and [docs/specs/v0.1.md](specs/v0.1.md) (testable V0.1 contract).
Informational: acceptance, reports, development notes, CHANGELOG, this index.
Layout notes and remaining documentation debt: [docs/development/repo-normalization.md](development/repo-normalization.md) (full list there only).

## Taxonomy (where a new file goes)

| Directory | Holds | Does not hold |
|---|---|---|
| `architecture/` | Durable, language-independent invariants. Survives RIIR. | Versioned MUST/SHOULD contracts, ADRs, evidence, V0.2 drafts. |
| `specs/` | Versioned testable contracts (MUST/SHOULD/MAY). Current file: `v0.1.md`. | Live smoke records, completion claims, V0.2 design. |
| `decisions/` | Why a durable choice exists (ADRs). Empty except this README until extraction. | Architecture overview text, V0.2 design drafts, Python how-to. |
| `acceptance/` | Empirical evidence: live smokes, environment observations. Never normative. | Specs, reports of what a release claimed. |
| `reports/` | Historical completion/closure claims and derived ingestion/audit reports. | Live evidence (that is `acceptance/`), current spec, canonical V0.2 design. |
| `design/v0.2/` | Canonical V0.2 / RIIR design bundle (numbered docs, MANIFEST, ingestion guide, Root operating doctrine). | Previous agents' comprehension reports, Python how-to, ADRs. |
| `development/` | Current Python implementation how-to **plus this cleanup’s** `repo-normalization.md`. | V0.2 design drafts (those go in `design/v0.2/`), ADRs (those go in `decisions/`). |

Rule: V0.2 design drafts MUST NOT land in `development/` or `decisions/`. ADRs MUST NOT be appended to `architecture/overview.md`. Remaining documentation debt lives only in [docs/development/repo-normalization.md](development/repo-normalization.md); this map points at that report in one line and does not duplicate the list.

## Status-header legend

```text
Status: Architecture | Normative | Historical Evidence | Historical Report | Redirect | Index
Version: <id or "independent unless stated">
Applies to: <release line, if evidence/report>
Canonical path: <repo-relative path>
```

Normative documents use `Status: Architecture` or `Status: Normative`.
Acceptance files use `Status: Historical Evidence`. Completion reports use
`Status: Historical Report`.

## Former paths

| Former path on `main` before this layout | Canonical path |
|---|---|
| `ARCHITECTURE.md` (content) | [docs/architecture/overview.md](architecture/overview.md). Root `ARCHITECTURE.md` is a redirect stub. |
| `docs/V0.1_SPEC.md` | [docs/specs/v0.1.md](specs/v0.1.md) |
| `docs/V0.1_COMPLETION_REPORT.md` | [docs/reports/v0.1.2/correctness-closure.md](reports/v0.1.2/correctness-closure.md) |
| `docs/V0.1.1_LIVE_ACCEPTANCE.md` | [docs/acceptance/v0.1.1/live-acceptance.md](acceptance/v0.1.1/live-acceptance.md) |
| `docs/V0.1.2_LIVE_ACCEPTANCE.md` | [docs/acceptance/v0.1.2/live-acceptance.md](acceptance/v0.1.2/live-acceptance.md) |
| `docs/V0.1.3_GROK_LIVE_ACCEPTANCE.md` | [docs/acceptance/v0.1.3/grok-live-acceptance.md](acceptance/v0.1.3/grok-live-acceptance.md) |

Frozen V0.1.2 spec bytes remain `git show v0.1.2:docs/V0.1_SPEC.md` (the old path on that tag is correct).

ADR extraction TODO: [docs/decisions/README.md](decisions/README.md).
