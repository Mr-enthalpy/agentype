# 07 — SpawnSource and Adapter Contract

Status: Normative
Canonical path: docs/specs/v0.2/07-spawn-source-and-adapter-contract.md

## SpawnSource

SpawnSource is a physical provisioning source. It sits above V0.1
ExecutionTarget × ExecutionProfile (those remain adapter-bound).

It MUST NOT be AgentType identity or Core vendor semantics.

Suggested categories (representation IMPLEMENTATION-DEFINED): source_id,
adapter_ref, target_selector, profile_selector, provisionable capability
envelope, enforceable sandbox features, lifecycle modes, supported continuity
modes, source_tags, availability.

## Selection order (MUST)

1. correctness constraints
2. sandbox enforceability
3. AgentType compatibility (`can_provision`)
4. continuity value
5. availability
6. cost / resource policy

Cost MUST NOT override correctness or security eligibility.
A source that cannot enforce the required sandbox MUST be ineligible.

## ExecutionAdapter (correctness required)

Narrow interface, UNCHANGED from V0.1:

- `start_execution`
- `observe_execution`
- `interrupt_execution`
- `terminate_execution`
- `collect_outcome`
- `reconcile_start`

Every call MUST return within a configured absolute deadline, including
exception cleanup. Cleanup consumes remaining time; a depleted deadline MAY
only kill or abandon without a fresh wait budget.

`StartObservation` MUST carry `terminal_confirmed` and `quiescent_confirmed`
(default false). Dispatcher MUST NOT derive those proofs from a
terminal-looking enum state.

`collect_outcome` is authoritative for ACK/NACK proof. A nonterminal collect
MUST NOT inherit terminal/quiescence proof from earlier `reconcile_start`.

Runtime locators (thread id, session id, turn id) MUST be opaque handles on
Incarnation/Execution. Core MUST NOT interpret vendor enums.

Process death is not quiescence proof.

## TerminalExperienceAdapter (optional)

MAY display child agents, conversations, status, workstreams.
MUST NOT be required for Core correctness.
MUST NOT grant claim, Result, or frontier authority.

## RootBridge

Independent of worker execution. See [03](03-task-attempt-lease-result.md)
outbox rules. Bridge MUST NOT `session/new` or otherwise own Root identity
when the transport has a load/resume primitive. Notifications MUST NOT
include Result payload.
