# Local Agent Scheduler

Local Agent Scheduler is a single-machine, local-first, restart-safe control
plane for durable agent work. It owns Tasks, Attempts, Leases, Results,
LogicalAgents, Batches, pool topology, recovery, escalation, and durable Root
notifications. Physical execution belongs to replaceable adapters.

The current implementation is the Python package `local-agent-scheduler` 0.1.3:
SQLite WAL, a Codex `app-server` adapter, and a Grok ACP stdio adapter.
Scheduler Core owns semantics; adapters and RootBridge own transport.
`config/scheduler.grok.toml` selects the Grok worker. Live Grok tests are
opt-in: set `AGENTYPE_GROK_LIVE=1`.

Documentation map: [docs/README.md](docs/README.md).
Architecture: [docs/architecture/overview.md](docs/architecture/overview.md).
V0.1 contract: [docs/specs/v0.1.md](docs/specs/v0.1.md).
Release history: [CHANGELOG.md](CHANGELOG.md).

## Development

```powershell
python -m pip install -e .
python -m unittest discover -s tests -t . -v
local-agent-scheduler --config config/scheduler.example.toml init
local-agent-scheduler --config config/scheduler.example.toml status
```

The CLI is diagnostic and control-oriented. It is not the authoritative state;
SQLite is.

## Minimal operation

Review `config/scheduler.example.toml`, then initialize the database, submit a
Task graph, recover persisted activity, and run the daemon:

```powershell
local-agent-scheduler --config config/scheduler.example.toml init
local-agent-scheduler --config config/scheduler.example.toml batch submit --file examples/batch.json
local-agent-scheduler --config config/scheduler.example.toml recover
local-agent-scheduler --config config/scheduler.example.toml daemon
```

Diagnostics are available through `status` and each entity's `list`/`show`
commands. Explicit controls include Task/Batch cancellation, Result ACK,
Escalation resolution, Execution interrupt/terminate, pool reconciliation,
partition creation/idempotent structural upsert, resize/capacity movement,
MERGE, and guarded partition retirement.

The example configuration uses `root_bridge.kind = "filesystem"` so local
testing does not wake a real Root. Provider RootBridge settings live in the
example configs and the spec.
