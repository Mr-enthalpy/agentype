# Changelog

## 0.1.3 — 2026-08-26

Python package version on `main`. Product SHA `729143f` (Merge PR #2).
The documentation-normalization commit is not this release.

- Grok ACP stdio ExecutionAdapter (`src/local_agent_scheduler/adapters/grok.py`).
- Grok ACP RootBridge: `session/load` + notification `session/prompt`; never `session/new`.
- Opt-in live tests behind `AGENTYPE_GROK_LIVE=1`.
- Example config `config/scheduler.grok.toml`.

This documentation-normalization change does not bump the package version.

## 0.1.2 — 2026-08-25

Correctness-closure release line. Annotated tag `v0.1.2` points at `e3ff876`.

- Provider-free correctness closure of Task/Attempt/Lease fencing, writer quiescence,
  topology MOVE/MERGE/RETIRE, notifier isolation, Codex SandboxMode mapping, schema v7.
- Tag incompleteness: Core follow-up `b938c12` (writer safety from the Attempt Execution
  even when `execution_id` is omitted) landed after the tag and is on `main` via PR #1.
  The tag was not moved. No `v0.1.2.1` tag.

## 0.1.1 — 2026-08-12

Annotated tag `v0.1.1` points at `db991f8`.

- Physical Incarnation lifecycle hardening on top of the initial V0.1 implementation
  (`48d6edb`).
