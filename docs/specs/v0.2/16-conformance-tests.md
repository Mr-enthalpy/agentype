# 16 — Conformance Test Contract

Status: Normative
Canonical path: docs/specs/v0.2/16-conformance-tests.md

These tests are part of the spec. Rust MUST implement them before claiming
the corresponding milestone. They are not an afterthought.

This document does not ship the tests.

## A. V0.1 correctness parity (M4)

MUST cover: fencing; stale ACK; duplicate writer prevention; restart
recovery; Result durability; Batch partial completion; topology recovery;
writer quiescence / omitted execution_id; notifier isolation from
dispatcher/heartbeat; `collect_outcome` MUST NOT inherit
`reconcile_start` quiescence; RETIRE blocked by open writer-safety
obligation.

Physical Execution transitions (oracle `record_physical_outcome`):
UNKNOWN → RUNNING; LOST → TERMINATED; STARTING → UNKNOWN; identity-preserving
refine after Task authority ended MUST NOT mutate Task/Result.

LogicalAgent: excess unassigned INITIALIZING/READY/REVIVING retire without
DRAINING; only ASSIGNED drains; safety-resolved SUSPENDED → REVIVING or
RETIRED.

Outbox: first `Batch → COMPLETED` inserts exactly one `BATCH_RESULTS_READY`
in the same transaction; PENDING/DELIVERED → ACKED.

Python V0.1.2/0.1.3 suite is the behavior oracle.

## B. V0.2 semantic tests (M6)

MUST cover:

- Generation expansion is bounded
- worker cannot directly create next-generation executable Task
- RawWorkIntent compilation is non-authoritative
- compiler cannot admit work
- compiler cannot negatively admit (generic REJECTED_AS_REDUNDANT drop)
- Transform cutover is atomic TARGET_READY → COMPLETED (no durable split-brain CUTTING_OVER)
- audit Generation is non-expansive
- AgentType refinement cannot widen sandbox
- SpawnSource ineligible if enforcement insufficient
- Transform creates successor, not in-place type mutation
- revival preserves LogicalAgent identity
- native session loss falls back to Scheduler continuity floor
- mechanical retry does not create a new Generation
- worker `validated_delta` does not auto-write MemoryCapsule

## C. Provider/frontend neutrality (M7)

A second adapter MUST be addable without Core state-machine changes.

## D. Crash/restart

MUST cover restart during: Generation, Transform, Result AVAILABLE,
compilation, revival. Resume MUST NOT mint a new Generation or identity.

## E. Persistence invariants

- stale authority cannot mutate canonical state
- exactly one authoritative Result per completed Task
