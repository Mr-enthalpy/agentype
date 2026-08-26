# Repository normalization report

Status: Historical Report
Applies to: documentation layout on branch `docs/normalize-layout`
Canonical path: docs/development/repo-normalization.md

This is the record of the documentation-layout change. It is not a Scheduler
specification.

## Old path → new path

| Old path | New path | Operation |
|---|---|---|
| `ARCHITECTURE.md` (content) | `docs/architecture/overview.md` | `git mv`, then status-header edit |
| *(none)* | `ARCHITECTURE.md` | Created 5-line redirect stub after the mv |
| `docs/V0.1_SPEC.md` | `docs/specs/v0.1.md` | `git mv`, then preamble replacement |
| `docs/V0.1_COMPLETION_REPORT.md` | `docs/reports/v0.1.2/correctness-closure.md` | `git mv`, then status-header + relative-link edit |
| `docs/V0.1.1_LIVE_ACCEPTANCE.md` | `docs/acceptance/v0.1.1/live-acceptance.md` | `git mv`, then status header |
| `docs/V0.1.2_LIVE_ACCEPTANCE.md` | `docs/acceptance/v0.1.2/live-acceptance.md` | `git mv`, then status header |
| `docs/V0.1.3_GROK_LIVE_ACCEPTANCE.md` | `docs/acceptance/v0.1.3/grok-live-acceptance.md` | `git mv`, then status header + evidence note; Scope sentence left intact |
| `README.md` | `README.md` | Edited in place (shrink + new links) |
| *(none)* | `docs/README.md` | Created |
| *(none)* | `docs/decisions/README.md` | Created |
| *(none)* | `docs/design/v0.2/README.md` | Created |
| *(none)* | `docs/development/README.md` | Created |
| *(none)* | `docs/development/repo-normalization.md` | Created (this report) |
| *(none)* | `CHANGELOG.md` | Created |

Baseline for the moves: `main` at `729143f615dc4cfdd5fcfb1ff920030fe9faf2f7`.

## Left in place, and why

LICENSE, `pyproject.toml` (version 0.1.3), `src/`, `tests/`, `config/`,
`examples/`, `.github/workflows/ci.yml`, `.gitignore`, `README.md` (edited, not
moved). Python implementation stays at `src/`; it is not a `legacy/` graveyard.
Config examples remain where tests resolve them
(`Path(__file__).resolve().parents[1] / "config"`).

## Removed

None.

## Newly created

`docs/README.md`, `docs/decisions/README.md`, `docs/design/v0.2/README.md`,
`docs/development/README.md`, this report, `CHANGELOG.md`, root
`ARCHITECTURE.md` stub.

## Source-code changes

None. No edits under `src/`, `tests/`, `config/`, `examples/`, or
`.github/`. Package version remains `0.1.3`.

## Validation

Commands run from repo root with `AGENTYPE_GROK_LIVE` unset. Interpreter:
`py -3.13`.

| Command | Result |
|---|---|
| `python -m compileall -q src tests` | exit 0 |
| `python -m unittest discover -s tests -t . -v` | `Ran 162 tests in 9.612s` `OK (skipped=2)` |
| Live Grok cases | skipped (`set AGENTYPE_GROK_LIVE=1 …`); not claimed |
| `git diff --check` | exit 0 (working-copy LF→CRLF warning on `README.md` only) |
| `pyproject.toml` version / readme | `0.1.3` / `README.md` |
| `__version__` | `0.1.3` |
| `git rev-parse v0.1.2^{}` | `e3ff876a6fbfd1c189dc8da2348264345271e032` (tag not moved) |
| `git tag -l` | `v0.1.1`, `v0.1.2` (no `v0.1.3` created by this change) |
| `Cargo.toml` | absent |
| `legacy/` | absent |
| Old markdown *destinations* `](ARCHITECTURE.md)` / `](docs/V0.1…)` / `](V0.1_SPEC.md)` / `](V0.1.*_LIVE…)` | none remaining |
| Display-text hits `V0.1.1_LIVE_ACCEPTANCE.md` / `V0.1.2_LIVE_ACCEPTANCE.md` | historical report link labels, URLs already retargeted; former-path table in `docs/README.md` |

`git log --follow` after this commit (rename detection on the commit
diff):

- `docs/architecture/overview.md` includes pre-move `ARCHITECTURE.md` history
  through `48d6edb` (`git show -M` records `ARCHITECTURE.md =>
  docs/architecture/overview.md`).
- `docs/specs/v0.1.md`, `docs/reports/v0.1.2/correctness-closure.md`, and the
  three acceptance files follow as git renames (83–98% similar).
- Root `ARCHITECTURE.md` remains the same path with a rewritten stub, so
  `git log --follow -- ARCHITECTURE.md` still walks pre-move overview
  history. Isolating the stub as an add-only file would require deleting the
  path in one commit and adding the stub in a second; this change keeps one
  internally consistent SHA.

Post-staging index check: `git show :plan.txt` failed as required.
`plan.txt` remains untracked.

## Remaining debt

Labelled here only; not fixed in this change.

- `docs/architecture/overview.md` body still talks in places as if V0.1 were
  future tense ("V0.1 should…", "Candidate V0.1 states"). Status header
  corrects the phase; a full tense pass is a rewrite and is out of scope.
- V0.1.2 completion-report limitation about "first adapter type, Codex
  app-server" is stale as current product description; it remains a V0.1.2
  historical statement.
- Completion-report prose “146 tests” was written at `b938c12` when
  `def test_` was 130. HEAD `def test_` is 146 by coincidence (130 + 16 Grok).
  Unittest discovery ran 162 cases because `TopologyCase` re-runs
  `SchedulerCase` methods. Leave the historical number.
- Grok acceptance Scope vs later dormant-Root section (labeled in-file, not
  rewritten).
- ADR candidates ADR-0001–0006 not extracted (`docs/decisions/README.md` TODO).
- V0.2 design not written (`docs/design/v0.2/` empty except README).
- `v0.1.3` git tag is a separate release action at `729143f`, not this commit.

## Confirmation

No V0.2 Scheduler semantics were introduced.
No Rust implementation, Cargo.toml, or placeholder crates were introduced.
Package version remains 0.1.3.
Git tags v0.1.1 and v0.1.2 were not rewritten.
v0.1.2 tag still points at e3ff876 and does not include b938c12.
This docs-layout commit did not run git tag. If v0.1.3 exists, it points at
729143f, not this commit.
plan.txt was not committed.
src/ was not renamed to legacy/.
Scheduler source, adapters, RootBridge, tests, and CI behavior are unchanged
except mechanical documentation-path edits in docs and root README.
