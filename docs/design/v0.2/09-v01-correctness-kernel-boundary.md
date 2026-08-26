# V0.1 Correctness Kernel Boundary

Status: Frozen compatibility constraint for V0.2
Canonical path: docs/design/v0.2/09-v01-correctness-kernel-boundary.md

## 1. V0.2 must not replace the proven execution authority model with semantic abstractions

V0.1 established the core execution-correctness kernel.

Important objects remain conceptually valid:

- Task;
- Batch;
- Attempt;
- Lease;
- Result;
- Failure;
- Escalation;
- LogicalAgent;
- Incarnation;
- Execution;
- Workstream;
- Checkpoint;
- PoolPartition;
- notification outbox.

V0.2 may evolve schemas and representation, but must preserve the correctness properties.

## 2. Task authority

Claim creates an Attempt and Lease transactionally.

At-least-once execution remains the model.

Exactly-once execution is not promised.

## 3. Fencing

Every authority-bearing completion/failure/checkpoint action must be fenced by Attempt/Lease authority.

Stale executions may update their own physical history but may not mutate current Task/Result authority.

## 4. Result Queue

Task success atomically creates one authoritative durable Result.

Task completion and Result creation are one correctness boundary.

Root Result consumption is later and independent.

Root notification is wakeup/control, not Result transport.

## 5. Writer safety

Lease expiry is not proof that a writer stopped.

Replacement requires confirmed quiescence or attempt-scoped isolation.

Otherwise:

`Task/Batch suspend + Escalation + no duplicate writer`

This rule survives V0.2.

## 6. Revival vs replacement

Physical replacement may occur without changing LogicalAgent semantic identity.

Transform is the V0.2 semantic mechanism that intentionally creates a successor identity.

## 7. Scheduler sole authority

Scheduler remains sole authority for claim, Attempt, Lease, Task state, Result, retry, recovery, and suspension.

No AgentType compiler, RootBridge, terminal UI, or worker may bypass it.

## 8. Recovery

Restart recovery must reconcile ambiguous physical state before unsafe duplicate writer dispatch.

V0.2 semantic layers do not weaken this recovery barrier.

## 9. Batch and Generation

Batch remains an execution barrier.

Generation adds a semantic frontier barrier above it.

Generation must not replace Batch correctness semantics.
