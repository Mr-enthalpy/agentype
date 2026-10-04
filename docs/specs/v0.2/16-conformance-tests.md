# 16 — Conformance Test Contract

Status: Normative
Canonical path: docs/specs/v0.2/16-conformance-tests.md

These tests are part of the spec. Rust MUST implement them before claiming
the corresponding milestone. They are not an afterthought.

This document does not ship the tests.

## A. V0.1 Core correctness (M4)

MUST cover: fencing; stale ACK; duplicate writer prevention; restart
**authority** recovery; Result durability; Batch partial completion;
topology recovery; writer quiescence / omitted execution_id;
`collect_outcome` MUST NOT inherit `reconcile_start` quiescence; RETIRE
blocked by open writer-safety obligation; **RUNNING confirmation + first
Lease renewal in one fenced transaction before supervision admission**
(oracle `test_running_confirmation_atomically_renews_near_deadline_lease`).

Physical Execution transitions (oracle `record_physical_outcome`):
UNKNOWN → RUNNING; LOST → TERMINATED; STARTING → UNKNOWN; identity-preserving
refine after Task authority ended MUST NOT mutate Task/Result.

LogicalAgent: excess unassigned INITIALIZING/READY/REVIVING retire without
DRAINING; only ASSIGNED drains; safety-resolved SUSPENDED → REVIVING or
RETIRED. Semantic retirement MUST fence every STARTING/WARM/COLD Incarnation
to LOST in the same transaction (oracle
`test_semantic_retirement_fences_idle_reusable_incarnation` and
assignment-boundary retirement fencing).

Outbox: first `Batch → COMPLETED` inserts exactly one `BATCH_RESULTS_READY`
in the same transaction; PENDING or DELIVERED → ACKED.

M4 MUST NOT require Generation membership on Task.

Python V0.1.2/0.1.3 suite is the behavior oracle.

## A2. Runtime / adapter parity (M5)

MUST cover: notifier isolation from dispatcher/heartbeat; notifier backoff
measured from delivery **completion**; recovery startup failure clears
threads and in-memory admissions; empty ExecutionProfile registry is
authoritative (`RESOURCE_UNAVAILABLE`, no adapter default); configuration
`dispatcher_poll_seconds <= heartbeat_seconds < lease_seconds`; daemon
single-run; adapter absolute deadlines including cleanup; first-adapter
runtime/live parity for **one named** reference adapter (opaque handles
only; which adapter is IMPLEMENTATION-DEFINED). **M5.8:** OS process
singleton for one Scheduler store; physical freshness gates heartbeat
renewal; `collect_outcome` / physical end MUST NOT mint a Task Result;
second process MUST fail before recovery mutation; the store-path invariant —
a production store is a literal, UTF-8 representable, file-backed filesystem
path, three SQLite special filenames (a `file:` URI filename, including one
whose prefix is only visible in raw filename bytes rather than in a UTF-8
string; bare `:memory:`; and the empty filename) MUST be refused at both the
config boundary and the store-open boundary, a non-UTF-8 path — which is not
a SQLite special filename — MUST be refused as well, the config boundary and
the store boundary MUST agree on which paths are accepted, and an explicit
`open_memory`-style path remains the only ephemeral store. M5 MUST NOT
require both V0.1.3 transports.

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

M6-B.2 catalog persistence MUST cover:

- a published exact `(ref, content digest)` is immutable; a different canonical
  content for the same exact revision fails closed
- canonical content is permutation-stable (claims/credential refs sorted and
  deduped) and `Bool(false)` is one canonical absence
- two claims at one exact reference with the same value but different
  `declaration_provenance_ref` fail closed, and an `ENFORCED` declaration
  supersedes `DECLARED` deterministically
- golden digest vectors pin the canonical byte format
- a derived AgentType `based_on` provenance is verified and a widening
  refinement is rejected at publication
- a dependent immutable publication (AgentType `based_on`, SpawnSource
  `adapter_policy`) fails closed when the referenced revision is corrupt
  (missing overlay or invalid canonical content), and creates no new row
- a repeated disposition command is an idempotent no-op that preserves the
  transition timestamp, for AgentType, SpawnSource, SourceConfig, and
  AdapterBindingPolicy (including an initial `ACTIVE -> ACTIVE`)
- SQLite mechanically rejects a direct `UPDATE`/`DELETE` of an immutable
  revision row, a direct disposition status reversal, and an overlay
  delete / `INSERT OR REPLACE` / primary-key rewrite that would resurrect a
  prior disposition, without the Kernel
- once a disposition is `DEPRECATED`/`DISABLED`, no direct SQL form can make
  the exact revision `PUBLISHED`/`ACTIVE` again

M6-B.3 typed admission and matching MUST cover:

- a typed admission creates the Task, `GenerationTaskBinding`, and immutable
  `TaskAgentRequirement` in one transaction; a failure leaves none of the three
- a typed admission MUST pin an exact AgentType revision resolved before commit;
  there is no durable unpinned typed requirement
- the requirement's TaskSpec-owned dimensions (information function, affinity,
  workspace, continuity) are derived from the admitted TaskSpec, not duplicated
  by the caller
- a Generation policy folds into the requirement as a spec 10 authority ceiling:
  a Task exceeding the workspace/network ceiling fails closed, and a stricter
  Task under a wider ceiling keeps its own value
- a `POLICY` Generation rejects legacy `admit_proposal` before any write, while a
  `NONE` Generation keeps M6-A legacy admission working, and each marker is a
  positive durable fact
- a `TYPED` Task is invisible to `ensure_task_consumers` and
  `claim_next_available`: it births no legacy consumer, acquires no
  Attempt/Lease/Execution, and stays `QUEUED`; losing the requirement row for a
  `TYPED` marker MUST NOT downgrade it to a legacy Task, and losing a `POLICY`
  row MUST NOT make the Generation unconstrained
- a mixed queue still dispatches the legacy Task past a quarantined typed Task
- exact typed replay returns the same Task while a conflicting requirement is a
  `Conflict`; an exact pin stays replayable after deprecation, and a `Latest`
  retry after catalog drift fails closed
- matching composes the frozen M5 placement gates (exact partition, tag superset,
  `Required` workstream) with `can_execute`, and returns only bound, M5 `READY`,
  unassigned agents; broader/general and non-ready candidates are ordered or
  filtered per the spec 06 preference, and matching writes nothing
- ranking orders exact, narrower, equivalent/incomparable, then broader, applies
  candidate-vs-candidate dominance before continuity/availability/identity, and
  never uses nominal inheritance depth
- `logical_agent_type_bindings` is write-once and a fresh binding requires a
  `PUBLISHED` revision; `task_agent_requirements` and `generation_policies` are
  immutable, enforced mechanically by SQLite
- the canonical `TASK_AGENT_REQUIREMENT` and `GENERATION_POLICY` document kinds
  have pinned golden digest vectors
- a `SourceConfigRevision` exposes its validated opaque body without a second
  read path, is unforgeable outside the validated read (private fields, no
  public constructor; compile-fail witness), and its `body` always matches the
  validated mode (`OpaqueJson` => payload/no locator, `ExternalRef` =>
  locator/no payload)
- a fresh publication persists the caller's initial disposition (a `Disabled`
  source/config/policy is not silently stored as `Active`)
- disposition changes never alter revision content and are monotonic
- an opaque `OpaqueJson` config body is validated against its declared
  `config_digest`; an `ExternalRef` stores a locator separately from the digest,
  and a locator whose value equals the digest is accepted (distinct fields) and
  round-trips distinctly
- an invalid `config_digest` grammar (not `sha256:<64 lowercase hex>`) is
  rejected for both body modes
- an `ExternalRef` locator is stored verbatim (whitespace is byte-significant)
- a drifted SourceConfig relational column (locator, digest, or OpaqueJson
  payload) fails every SourceConfig read closed through the single validated
  `SourceConfigRevision` path
- a read recomputes the stored content digest, so a tampered row fails closed
- a self-consistent but non-canonical row (duplicate/permuted claims, an
  explicit `Bool(false)`, duplicated/permuted credential refs) fails closed
  because the read re-canonicalizes before re-encoding
- a blank `AdapterBindingPolicy` `adapter_kind`/`binding_ref` is rejected at
  publication and at decode
- an idempotent republish of an existing exact revision fails closed if that
  stored revision is corrupt
- `AgentTypeSelector` exact and latest resolution fails closed when a published
  revision is corrupt (digest drift, non-canonical content, or a relational
  `based_on` drift), not just `get_agent_type`
- a revision row with a missing disposition overlay fails closed as corruption
  in reads, disposition setters, and idempotent republish; publication does not
  recreate the overlay, and `Latest` does not fall back to an older revision
- a disposition setter distinguishes an absent revision (`NotFound`) from a
  revision whose overlay is missing (corruption)
- the complete durable SourceConfig revision identity includes the body mode and
  `ExternalRef` locator: two configs with equal metadata but different locators
  are different revisions
- an old schema version (v5 and earlier) is rejected at open (D-DB-MIGRATE)

## C. Provider/frontend neutrality (M7)

A **second independent** adapter MUST be addable without Core state-machine
changes. Completing M5 with one reference adapter does not satisfy M7.

## D. Crash/restart

MUST cover restart during Result AVAILABLE (M4). Generation, Transform,
compilation, and revival restarts are **M6**. Resume MUST NOT mint a new
Generation or identity.

## E. Persistence invariants

- stale authority cannot mutate canonical state
- exactly one authoritative Result per completed Task
