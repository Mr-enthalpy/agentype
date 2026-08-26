# Development notes

Status: Index
Canonical path: docs/development/README.md

This directory is **current Python how-to** plus the repository-normalization
report. It is not the V0.2 / RIIR design home; that is
[docs/design/v0.2/](../design/v0.2/).

## Current implementation

- Python 3.11+, package `local-agent-scheduler`
- Layout: `src/` + `tests/` + `config/`
- Do not name `src/` `legacy/`. Git tags archive the Python V0.1 line.

Provider-free verification matches CI:

```powershell
python -m compileall -q src tests
python -m unittest discover -s tests -t . -v
```

Live Grok tests require `AGENTYPE_GROK_LIVE=1`. Credentials stay in CLI auth.
Grok RootBridge must never `session/new`.

A later RIIR is expected to add a Cargo workspace beside (then instead of) this
Python tree. `docs/` does not move. Crate-layout and V0.2 semantic drafts go in
[docs/design/v0.2/](../design/v0.2/), not here.

`plan.txt` is untracked task input and is not part of the product tree.

Remaining documentation debt: [repo-normalization.md](repo-normalization.md)
(single home).
