# V0.1.3 Grok Worker Live Round

Status: Historical Evidence
Applies to: 0.1.3
Canonical path: docs/acceptance/v0.1.3/grok-live-acceptance.md
Not normative.

Evidence note: the original Scope paragraph records the first worker-round
observation (filesystem RootBridge; Grok RootBridge then not in scope),
committed in f760fbd. A later section in this same file records the
subsequent dormant-Root wakeup after d8ffa69 added GrokAcpRootBridge.
Both observations are retained. Later evidence does not rewrite earlier
observations.

Date: 2026-08-26

Environment: Windows, Grok CLI 1.0.5, existing authenticated user deployment.
The round used the concrete `grok.exe` entrypoint with
`grok --sandbox read-only agent --always-approve --model grok-build stdio`.
It did not read, copy, modify, or persist credentials, `auth.json`, session
IDs, or event IDs. Temporary SQLite, configuration, and inbox files were
deleted after execution.

## Scope

One Scheduler control-plane round through the Grok ACP adapter. Filesystem
RootBridge delivered the outbox envelope. There is no Grok RootBridge; dormant
Root wakeup was not attempted.

## Outcome

```text
init → submit → daemon --once → Result AVAILABLE → Batch COMPLETED
→ BATCH_RESULTS_READY DELIVERED → result ack ACKED
= PASS
```

Sanitized authoritative Result payload:

```json
{"evidence":"v0.1.3-grok-live","status":"ok"}
```

Observed durable state after `run_until_idle` and Result ACK:

* Task: `COMPLETED`
* Batch: `COMPLETED`
* Execution: `SUCCEEDED`
* Result: `AVAILABLE` then `ACKED`
* outbox: exactly one `BATCH_RESULTS_READY`, `DELIVERED`
* active Leases: 0
* open Escalations: 0
* SQLite integrity: `ok`

Elapsed wall time for the unittest round: 5.254 seconds. `max_attempts=1`.
No replacement worker. Result content traveled only through the durable
Result Queue.

## How to repeat

```powershell
$env:AGENTYPE_GROK_LIVE = "1"
py -3.13 -m unittest tests.test_grok_live -v
```

Optional: `AGENTYPE_GROK_BIN` points at a concrete `grok.exe`. Without the
env flag the test is skipped, so CI stays provider-free.

## Classification

```text
REAL GROK WORKER ROUND = PASS
TASK → RESULT → BATCH = PASS
BATCH-LEVEL-ONLY OUTBOX = PASS
RESULT ACK = PASS
```

## Dormant Root wakeup (2026-08-26)

A dedicated Root ACP session was bootstrapped by the test fixture (`session/new`
+ one idle prompt), then left inactive. The worker used a **different** ACP
session. The notifier loaded the dormant Root with `session/load` and started
exactly one notification `session/prompt`. The Bridge did not call
`session/new`.

Observed after `run_until_idle` (12.449 seconds wall time):

* worker Task `COMPLETED`, Result evidence `v0.1.3-grok-live`
* outbox: exactly one `BATCH_RESULTS_READY`, state `DELIVERED`
* Root path methods: `session/load` + `session/prompt` only
* active Leases: 0
* open Escalations: 0

```text
DORMANT ROOT + DAEMON NOTIFIER = PASS
GROK ROOTBRIDGE EXACT PROMPT = PASS
MAIN ROOT POLLING = NOT INTRODUCED
WORKER SESSION USED AS ROOT = NO
```
