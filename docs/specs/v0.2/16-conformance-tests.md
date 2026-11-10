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
- a `TYPED` Task / type-bound LogicalAgent is excluded from the legacy claim
  path by the pure Core decision (`ClaimTaskSnapshot.typed`,
  `ClaimAgentSnapshot.type_bound`), not merely by the storage query; a typed Task
  births no legacy consumer, acquires no Attempt/Lease/Execution, and stays
  `QUEUED`, and a legacy Task cannot be retrofitted to `TYPED`
- exact typed replay returns the same Task while a conflicting requirement is a
  `Conflict`; an exact pin stays replayable after deprecation, an already-admitted
  `Latest` command replays against its committed pin even after new revisions are
  published, a genuinely new `Latest` commitment resolves to the current revision,
  and a legacy replay of a typed Task (or vice versa) is a `Conflict`
- a different revision of the same `type_id` is not a substitute; a compatible
  type that is neither a strict refinement nor strictly broader remains a
  candidate (ranked after broader ones), because `more_specific_for` is a
  preference relation and never an eligibility gate
- typed candidate discovery loads every usable agent and runs the authoritative
  binding coherence read, so a `BOUND` agent whose row was lost fails the whole
  discovery closed instead of being silently skipped or falling back to another
  candidate
- matching composes the frozen M5 placement gates (exact partition, tag superset,
  `Required` workstream) with `can_execute`, and returns only bound, M5 `READY`,
  unassigned agents; broader/general candidates are ordered per the spec 06
  preference, and matching writes nothing
- ranking orders exact, narrower, broader, then other compatible, applies
  candidate-vs-candidate dominance before the `Preferred` workstream and the
  frozen M5 availability/identity tie-break, and never uses nominal inheritance
  depth
- the authoritative requirement read resolves its exact pin through the validated
  catalog read (a corrupt pin fails closed, a deprecated one does not) and, when
  the generation carries a policy, requires the stored requirement to be the
  exact fixed point of re-folding that policy (covering network, isolation,
  sandbox, budget, and anchor, not just the Task-owned mirrors)
- `logical_agent_type_bindings` is write-once and a fresh binding requires a
  `PUBLISHED` revision and a `READY`, unassigned agent; `task_agent_requirements`
  and `generation_policies` are immutable and write-once, enforced mechanically by
  SQLite including the `INSERT OR REPLACE` path
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

M6-B.4 provisioning binding and exact launch MUST cover:

- a typed Task stays `QUEUED` and acquires no Attempt/Lease/Execution until the
  single authority-bearing typed acquisition re-proves every mandatory conjunct
  (`can_execute`, `can_provision_task`, active `AdapterBindingPolicy` with
  satisfied `required_safety`, exact binding) over the current catalog
- a SUSPENDED sibling Task and QUEUED typed Task share a SUSPENDED Batch: both
  existing/new acquisition reject without Attempt/Lease or newborn side rows.
  Explicit Root resolution restores ACTIVE and the same Task can be acquired;
- acquisition proves `can_execute`/`can_provision_task` over the agent's actual
  bound type (a refinement, broader, or otherwise compatible B.3 candidate is
  accepted; a different revision of the same `type_id` is not), re-checks M5
  placement (partition, tag superset, `Required` workstream), and rejects a
  caller-assembled selection that does not satisfy the predicates
- imported `ResolvedProvisioningEvidence` is produced only by the internal
  resolver behind the `provisioning-producer` feature, is never injected through
  a control surface, and a `DECLARED` label never satisfies a security class
- the physical `execution_target`/`execution_profile` come from the
  partition/anchor; the exact `(adapter_kind, adapter_binding_key)` the resolver
  proved is carried into the acquisition and the snapshot, not re-supplied by a
  caller, and a target/policy `adapter_kind` mismatch is ineligible
- the `binding_ref` resolves to exactly one installed binding; a missing or
  ambiguous `binding_ref` fails closed and never falls back to another source of
  the same kind; a candidate-local ineligibility skips only that candidate while
  catalog corruption fails the whole resolution
- `provisioning_bindings` is write-once and per Incarnation; a B.3 semantic
  binding alone is never physical eligibility; a source/config change mints a new
  Incarnation rather than rebinding in place; a `PROVISIONED` Incarnation without
  its row, or a `LEGACY` Incarnation with one, fails closed
- `binding_snapshots` is created atomically with its `Execution` and must be
  coherent with it (adapter key, target/profile, provisioning-binding provenance,
  and validated config digest); a crash before the transaction leaves neither, a
  crash after leaves both; a `SNAPSHOT` execution without its row, or a `NONE`
  execution with one, fails closed
- the write-once guards reject `INSERT OR REPLACE` keyed by the durable unique
  identity (a new binding id with the same incarnation, or a new snapshot id with
  the same execution), and durable parent identities cannot be replaced to inject
  a marker
- after `BindingSnapshot` commits, an adapter start failure follows M5
  retry/recovery and MUST NOT start a second source; a missing exact binding
  during recovery is `RESOURCE_UNAVAILABLE`, not a fallback
- active source/config enumeration loads every immutable revision through the
  validated catalog read, so a missing disposition overlay fails closed rather
  than silently disappearing
- `reconcile_pool` does not count or retire typed (`BOUND`) agents, while untyped
  capacity behavior, MOVE/MERGE, and retirement fencing are unchanged
- the durable store and logs contain no provider secret material; only a
  `credential_refs_digest` is snapshotted, and credential availability is a B.5
  conjunct
- the authority transaction re-loads the `AdapterBindingPolicy` and rejects a
  policy that is no longer `ACTIVE`, a kind that disagrees with the resolved
  selection, or a `required_safety` the imported enforceability does not satisfy
- the partition target's `adapter_kind` is compared with the policy's before any
  Attempt/Lease is created, not deferred to Execution creation
- `ProvisioningBinding` freezes the exact `(adapter_kind, adapter_binding_key)`,
  so after a restart between acquisition and Execution the proven physical
  domain is reconstructable and never silently re-derived
- a resident Incarnation with a matching `ProvisioningBinding` is reused; a
  differing source/config or exact domain rolls over to a fresh Incarnation; a
  legacy Incarnation is adopted only when its prior physical hosting matches
- `BindingSnapshot` semantic/security fields are cross-checked against durable
  authority (the admitted requirement, the provisioning binding, the validated
  config credential digest), so the snapshot cannot carry a fabricated payload
- authoritative reads of the two objects cross-check the parent Incarnation/
  Execution and re-resolve the catalogue provenance, failing closed on incoherence
- the B.4 selection rule is the deterministically first eligible candidate; a
  richer operator policy is a deferred seam
- the imported evidence subject includes the exact `adapter_kind` and opaque
  `adapter_binding_key`, and the authority transaction rejects evidence whose
  domain disagrees with the selected binding
- a legacy Incarnation with any execution history is never requalified into a new
  SourceConfig provenance; it rolls over to a fresh Incarnation, and such a
  legacy Incarnation is fenced
- `ProvisioningBinding` is stable provisioning provenance (no per-Task
  requirement digest); a changed resolved enforceability rolls over
- the cross-record authority validators used at commitment are the same ones
  used by the authoritative reads, so a self-consistent but incoherent row fails
- `AdapterSafetyEnvelope::enforces_workspace` is exact-set membership (no
  `Write⇒ReadOnly` inference)
- a config with credential references gets no Attempt/Lease in B.4 (credential
  availability is B.5; no caller-supplied fact can grant authority)
- the authority facade commits the freshly resolved winner; more than one
  candidate tied on continuity (with no availability/cost model) fails closed as
  ambiguous
- a selected SourceConfig is eligible only when the source integration attests a
  materialization digest for the exact physical domain; the digest is frozen and
  required, and an unattested `ExternalRef` config is ineligible
- the single `acquire_typed_task` applies the spec 06 existing-first order and
  provisions a new agent only when no existing candidate can be acquired
- the unsandboxed local-process adapter imports no enforceable workspace/network
  mode, so it is never a `required_workspace=Write` enforcement proof
- a changed materialization attestation for the same source/config/domain rolls
  over to a new Incarnation
- the active-candidate frontier digest includes adapter-policy disposition and
  uses the canonical structured encoding
- eligibility uses a pure `attest`; the selected winner is resolved by a pure
  `prepare` (no physical side effect — the exact adapter materializes the
  descriptor during `start_execution`)
- an empty materialization attestation is ineligible before selection
- fallback to the next candidate/route happens only on the narrow explicit
  candidate-local `ContractError` set; an `InvariantViolation` surfaced through
  evidence, a storage failure, or a recovery-required error terminates the
  acquisition
- an ambiguous selection is a pre-acquisition resolution deferral: it creates no
  Attempt and emits no `RESOURCE_UNAVAILABLE` NACK, stays QUEUED for re-probe,
  and never falls through to a lower-ranked agent or a new agent in the same
  resolution
- the selected winner is prepared only AFTER its claim/binding is durably
  committed; losers are never prepared, and neither `attest` nor `prepare` creates
  a physical resource (no side effect precedes the Execution)
- adapter termination is scoped to the proven direct process: no process group,
  cgroup or Windows Job Object is created, so descendant lifetime and process-tree
  cleanup are not claimed, and process death remains non-quiescence
- `attest` and `prepare` each receive one absolute `AdapterDeadline`; the
  enumeration deadline and the winner-preparation deadline are distinct, so
  candidate probes cannot consume the preparation budget
- `acquire_typed_task` and the resolution mechanics are not reachable through the
  supported runtime surface (compile-fail witness)
- a global authority-snapshot invalidation (`StaleAuthority`, e.g. a changed
  catalog frontier) re-resolves the whole acquisition and never falls through to
  a lower-ranked agent, while a per-candidate storage rejection may fall through
- the prepared descriptor's re-derived digest must exact-match the durably
  committed `attested_materialization_digest`, so an attest/prepare TOCTOU cannot
  produce an Execution; the digest is a canonical `sha256:<64 lowercase hex>`
  value (a source integration cannot inject an arbitrary string), and the
  mismatch error never echoes a source-produced value
- a pure `prepare` failure or handoff failure is provably side-effect-free: a
  static/liveness failure settles the committed Attempt/Lease/assignment as
  `RESOURCE_UNAVAILABLE`, an authority loss is left to recovery (no Task-level
  nack), and a durable persistence/corruption fault is fatal and propagated,
  never surfaced as a Task-level `RESOURCE_UNAVAILABLE`
- a `SourceConfigIntegration` result obtained after the absolute `AdapterDeadline`
  yields a non-candidate-local `ProvisioningDeadlineExceeded` for both `attest`
  and `prepare`
- a process death after the authority COMMIT but before the Execution commitment
  (there is no pre-Execution side effect) is recovered by frozen M5
  `recover_authority` as an ordinary interrupted acquisition
  (`CLAIM_ORPHANED`/`ExecutionLost`); `materialize` creates no environment
- authority is re-qualified (`claim_authority_is_current`) both BEFORE and after
  the pure `prepare`; a stale claim is settled as authority loss, not a reported
  success
- the same operator `binding_ref` may resolve within two different
  `adapter_kind`s without collision, while remaining unique within one kind
- the acquisition carries its winner candidate out, and the Execution handoff
  commits a snapshot-bearing Execution (`create_execution_with_snapshot`) whose
  `BindingSnapshot` carries the acquisition's exact `adapter_kind`/
  `adapter_binding_key`/`attested_materialization_digest` and the resolved environment's
  isolation; a candidate that disagrees with the authoritative partition target
  fails closed rather than committing an Execution
- a binding whose capability does not satisfy its policy `required_safety`, or a
  snapshot whose effective workspace/network is not enforceable by the binding
  capability, fails the authoritative read
- the effective `attempt_isolation` is authoritative from the ExecutionRegistry
  target; a Task requiring isolation on a non-isolating target is ineligible, and
  the snapshot isolation equals the frozen Execution isolation
- the effective network policy is carried on the provider-neutral
  `EnvironmentStartRequest`
- an `OpaqueJson` config is eligible once its body digest validates; an
  `ExternalRef` config is eligible only when a source integration attests the
  declared content identity under the exact physical domain, and is ineligible
  until then
- a credential-bearing config is filtered before deterministic selection, so a
  stronger credential-bearing candidate can never shadow a credential-free one;
  likewise a wrong-target-kind candidate is filtered before selection
- the partition's full target+profile is validated (missing profile, or
  incompatible `allowed_targets`) before the authority transaction, so the Task
  stays `QUEUED` with no Attempt/Lease/ProvisioningBinding and `prepare` is
  never invoked
- `MOVE_CAPACITY` moves only `UNBOUND` LogicalAgents; a `BOUND` typed agent is
  never relocated by a legacy capacity command (ADR-0009)
- cancelling a prepared-but-not-started typed claim simply releases the agent
  (there is no pre-Execution side effect and therefore no writer-quiescence
  obligation)
- the descriptor is causally bound to the launch: the opaque launch descriptor
  returned by `prepare` is frozen in the `BindingSnapshot` and carried on the
  `EnvironmentStartRequest` delivered to the exact adapter, which physically
  materializes it during `start_execution`
- the descriptor protocol is a proven composition relation: eligibility requires
  the source-local integration's protocol to equal the exact adapter binding's
  `import_provisioning_protocol`, the protocol is frozen in the
  `ProvisioningBinding`, preparation re-checks the integration before/after the
  call (including failed calls), and handoff re-checks the current adapter;
  observed protocol drift is fatal, invokes no Task NACK and preserves authority
- provisioning capability is optional: an adapter that does not declare a
  non-blank `import_provisioning_protocol` is not provisioning-capable and is
  ineligible for typed acquisition (the reference local-process adapter declares
  none, because its `start_execution` ignores a descriptor)
- the candidate universe is source-local: an active source whose
  `SpawnSourceRef` is not routed to an integration by the composition root is
  ineligible, so one source's opaque config is never interpreted by another
  source's integration
- production source routing has no wildcard constructor; a new revision remains
  unrouted until explicitly registered, even when an integration supports both
- attestation None explicitly makes a candidate unavailable, while Some contains
  a validated canonical digest; raw/empty successful strings cannot satisfy SPI
- producer-controlled diagnostic sentinels from attest and prepare, including
  nested storage/contract errors, appear in neither public Display/Debug nor
  durable text/blob cells; availability, authority loss and fatal dispositions
  and candidate-local versus global fallback retain their original semantics
- adding an equally continuous config to an existing reusable WARM host returns
  SelectionAmbiguous before authority, preserves the host/provenance and leaves
  the Task QUEUED; removing the tie restores reuse. WARM provenance priority
  remains an explicit policy gap (ADR-0010), not an identity-sort tie-break
- successful prepare output with malformed digest or blank/whitespace descriptor
  is fatal and preserves Attempt/Lease/provenance; it creates no failure, retry,
  Execution or snapshot. Valid changed content remains an availability rejection
- the typed dispatch handoff retains the verified adapter instance and deadlines;
  replacing a registry entry at the same kind/key cannot redirect that request
- cross-target MERGE rejects READY and assigned pre-Execution typed WARM hosts
  atomically, preserving Task routing/topology/assignment/provenance. Old pending
  cutover after pure failure, cancellation or recovery closes the claim but
  preserves the host, original partition/retention and suspended pending agent.
  Same-target WARM and never-executed cross-target reservations succeed; missing
  provisioning children cannot downgrade to legacy cutover;
- provenance change on a previously executed WARM host fails closed before a
  new claim, preserving its state and immutable provenance; terminal Execution
  rows alone cannot authorize SQL-only resource replacement
- a typed-handoff failure is classified: only a pre-start availability failure
  settles as `RESOURCE_UNAVAILABLE`; authority loss is left to recovery; durable
  corruption/persistence is fatal; and every fallible composition check runs
  before the Execution is committed
- the source/config effective lifecycle must contain the member's realized M4
  `Retention` (`Resident`/`Ephemeral`), proved in the resolver and the authority
  transaction
- the prepare fence is the full committed-claim authority check
  (`AuthoritySnapshot` + Claim-identity coherence)
- the handoff re-qualifies the currently installed enforceability against the
  committed capability: an importer that keeps the exact `(kind, key)` but
  weakens workspace/network/isolation must not produce an Execution (settled as a
  pre-start `RESOURCE_UNAVAILABLE`)
- the typed handoff error exposes no Task failure class; a settlement failure
  that is not a confirmed authority loss is surfaced as fatal, never normalized
  to `RESOURCE_UNAVAILABLE`
- `ProvisioningBinding.agent_type` must equal the agent's frozen
  `LogicalAgentTypeBinding`
- the authoritative read accepts any `agentype-resolver/*` resolver version
- the effective attempt-isolation requirement is the OR of the AgentType and Task
  flags, checked against the authoritative target before authority AND re-proved
  at Execution commitment, so an `unisolated` safety that lowers it fails
- restart reconciliation validates each Execution's snapshot marker/child
  coherence; a `SNAPSHOT` execution with a missing/incoherent snapshot aborts
- the authority transaction rejects a selection whose active-candidate frontier
  changed since resolution (concurrent publish)
- a domain that can isolate but does not use it is representable: the
  ProvisioningBinding capability and the BindingSnapshot effective isolation are
  distinct and both valid
- a TYPED Task cannot commit through the legacy `create_execution`
- the authority facade commits the deterministic selection, not a caller-chosen
  eligible candidate
- `reconcile_pool` aborts on a marker/child binding mismatch rather than
  silently excluding the agent from capacity
- `INSERT OR REPLACE` is rejected for every durable UNIQUE identity
  (`request_id`, active execution per Incarnation, active Incarnation per agent,
  ...)
- an old schema version (v7 and earlier) is rejected at open (`D-DB-MIGRATE`)

The M6-B.4 inherited-contract acceptance matrix MUST additionally cover:

- unreadiness due to Task state, suspended Batch, inactive partition or future
  backoff before acquisition/direct enumeration: zero attest/prepare calls and
  no new Attempt/Lease/binding or population rows on existing/new paths, even
  with an integration that would return a fatal attestation error; a ready Task
  with due backoff attests, prepares and acquires successfully;
- a separate controller changes each readiness fact during attestation:
  transactional rejection or newborn rollback, zero prepare calls and no new
  authority/population/fault rows; preflight never replaces authoritative recheck;
- B.3/existing/new parity for Required continuity with no workstream and with a
  concrete matching workstream; rejection creates no Attempt/Lease/binding or
  newborn agent;
- configured target isolation independently of semantic requirements and
  imported isolation capability; an incapable source is excluded before
  attestation/selection/authority, and a capable alternative remains selectable;
- Execution/snapshot isolation cannot exceed the committed binding capability;
  rejection atomically rolls back Execution and Attempt physical association;
- a resident host made WARM by a real committed Execution and valid ACK keeps
  its identity, state and immutable binding after a second pre-Execution claim's
  config failure, Task/Batch cancel, expiry, restart, NACK or ACK;
- a fresh STARTING reservation never fabricates WARM/terminal physical evidence
  on pre-Execution cancellation and can be reused;
- WARM security-context reuse: an unchanged `ProvisioningBinding` does not by
  itself authorize reusing a WARM host for a Task with a different write scope,
  secret scope, or execution guarantee; reuse requires a fresh per-Execution
  isolation proof or a reset/quiescence witness covering the earlier Task's
  possible writes, and otherwise fails closed before any physical start; a fresh
  per-Execution write jail proves write confinement only, so credential-bound
  reuse additionally needs execution-bound broker verification, proof the old
  capability cannot be inherited or continued, or a verified physical reset
  witness;
- after pure prepare failure or cancellation, a STARTING reservation with a
  validated ProvisioningBinding and no Execution history can transactionally
  roll to another config/source once its claim closes; tests MUST cover
  same-Task retry, one active reservation, immutable old provenance and zero
  physical adapter starts;
- rollover MUST reject ACTIVE claims on the hosting LogicalAgent even when
  Attempt.incarnation_id is NULL, and MUST reject STARTING with any Execution
  history; rejection leaves existing provenance and Task authority intact;
- integration-returned availability errors settle as RESOURCE_UNAVAILABLE,
  while authority loss and fatal/control-plane faults produce no Task failure
  row; StorageFailure, InvariantViolation, RecoveryRequired and missing revisions
  propagate without closing the committed claim;
- confirmed fatal preparation facts survive close/reopen/recovery: Task becomes
  SUSPENDED without ExecutionLost, worker failure, automatic retry, host death
  or invented quiescence; crash before fault observation retains ordinary orphan
  retry. Include permitted ExecutionLost retry with max_attempts > 1, malformed
  success, protocol drift, storage/invariant faults and failed fault persistence;
  the same suspension transaction creates exactly one preparation-cause
  Escalation and DECISION_REQUIRED event, with no worker FailureClass. Rejecting
  the event INSERT rolls back claim closure, release, Task/Batch suspension and
  decision. Repeated recovery is idempotent; explicit retry/cancel resolves it
  without fake quiescence, and release_cancelled_writer is rejected. A latest
  already-suspended prior-v8 fault gains the missing decision once; resolved or
  cancelled work cannot reopen. Cause/child/fault mismatch fails closed;
- confirmed fatal typed handoff target/candidate inconsistency, rejected
  snapshot validation/constraint and failed availability settlement persist the
  same fault. Restart creates one actionable Root decision/event, with no
  ExecutionLost, worker Failure, retry or adapter start. Fault-write failure
  propagates fatally; authority loss/availability keep their distinct semantics.
  A real committed Execution and RequestId remain intact after a simulated lost
  COMMIT result, without a pre-Execution fault;
- preparation fault facts reject update/delete/replace, forged ownership,
  subsequent execution/worker settlement and closed/replaced claims; an elapsed
  but still-owned ACTIVE claim can record a late fact without gaining activity
  authority. Explicit cancellation works, and prior PR-era v8 reopening installs
  additive guards. A stored executed WARM host survives this recovery;
- attestation A ConfigurationUnavailable / B valid commits B, while A
  StorageFailure/InvariantViolation aborts before B with no claim;
- committed descriptor A rejects explicit B, empty/whitespace and legacy typed
  construction; exact A and ordinary legacy requests succeed;
- an earlier credential-bearing config and a wrong-kind source whose attestation
  would fail the whole search are skipped without invoking that attestation.

### M6-B.5 effective security and credential gates

`SECURITY_V1` MUST add:

- policy algebra properties: `meet` is the greatest lower bound, the order is
  transitive, `normalize` is idempotent, and canonical digests are stable under
  permutation and duplicate removal;
- equality regression: every contract accepted by the B.1–B.4 exact-reference
  relation remains accepted by the pure policy order, while no new physical
  eligibility is thereby granted;
- coarse projection/consistency: every `SECURITY_V1` coarse/fine conflict,
  including `ReadOnly` plus a write root and Task minimums unmet by the
  Generation meet, is rejected at the correct stage; a Task-local coarse/fine
  conflict is rejected even when another ceiling would hide it, and
  `sandbox_policy = None` follows the `SECURITY_V1` rule;
- `R_min` typing: `RequiredAccess ⊆ ProjectAccess` only for catalog-mapped
  security operation requirements, and only a `Set` value projects through the
  versioned member map (`Bool(true)` projects to nothing, an unmapped member
  fails closed, `Set(∅)` is a legal empty floor); `RequiredCapabilities` are
  `Pending` at admission and validated at the candidate stage against the actual
  AgentType through their existing
  `Bool`/`Set`/`Ordered`/`Quantity`/`Exact` matchers,
  never through `ProjectAccess`; `CoarseExecutionModes`
  use workspace/network predicates; `RequiredGuarantees` and
  `PlacementConstraints` go through the single staged `satisfies_requirement`
  relation, and `Pending` is never treated as satisfied;
- stage separation: `PlacementConstraints` are not evaluated at admission; a
  candidate that fails them is ineligible, and admission never assumes a
  candidate Agent exists;
- composite mode re-check: constructing `P_eff` can leave no writable path even
  though the Task and each ceiling individually allowed `Write`; candidate
  matching and commitment MUST reject that candidate, and MUST NOT reuse the
  admission-time `CoarseExecutionModes` verdict;
- `Write` region: a composite policy whose only remaining write is a temporary
  directory or any location outside the committed logical workspace region does
  not satisfy `WorkspaceMode::Write`, even though some writable path exists;
- guarantee composition: attempt isolation is the disjunction
  `AgentTypeContract.security.requires_attempt_isolation OR
  TaskAgentRequirement.hard.required_attempt_isolation OR
  SourceRequirement.requires_attempt_isolation`;
  no policy meet can clear it, an `EnforcementSupport` capability that merely
  supports isolation does not create it, and a required isolation without an
  isolating `ExecutionTarget`, exact-adapter enforceability, and a committed
  isolation request makes the candidate ineligible;
- isolation source staging: `AgentRequirementDraft::into_requirement` carries only
  the Task-authored `required_attempt_isolation`, and `fold_generation_policy`
  performs the Generation disjunction into the durable
  `TaskAgentRequirement.hard`; a test asserts the stored requirement is the exact
  fixed point of the re-fold, so B5 cannot "fix" the ADR by changing the frozen
  `into_requirement` behavior;
- admission mode: explicit `COARSE_V1 | SECURITY_V1` is durable and never
  inferred from `sandbox_policy != None`; a `SECURITY_V1` Task cannot downgrade,
  a `COARSE_V1` AgentType cannot execute it, and `SECURITY_V1 + None` is tested as
  legal;
- mode-marker ownership: `task_agent_requirements.security_contract_mode` is the
  only mode authority; `task_security_bindings` exists exactly when the mode is
  `SECURITY_V1` and is absent for `COARSE_V1` (no independent mode in either
  case); `generation_task_bindings` carries none. A `SECURITY_V1` requirement
  without its binding, a binding on a `COARSE_V1` requirement, a `task_id`
  mismatch, more than one binding, and two candidate authority rows are all
  rejected as corruption — and a legal `COARSE_V1` Task must be admitted with no
  security row, so the biconditional is tested in both directions;
- Generation mode coverage: a `NONE` Generation has no `generation_policies` row,
  admits only `COARSE_V1` Tasks, rejects a `SECURITY_V1` Task at admission, and is
  never read as unconstrained or implicitly `SECURITY_V1`; a `POLICY` Generation
  derives its mode from the immutable row, and admission never backfills a row for
  a `NONE` Generation;
- cross-mode symmetry: a `SECURITY_V1` Generation MUST reject a `COARSE_V1` Task
  at admission even when the Generation also lists `COARSE_V1` support, and a
  SpawnSource revision carrying an authored `P_source` ceiling MUST NOT be
  selected by the coarse path — both are negative cases, and a "supports both
  modes" label grants no execution;
- `P_source` top: a `SECURITY_V1` SpawnSource revision with no authored fine
  ceiling has `P_source = Top`, adds no restriction, and still fails the
  candidate when imported enforcement coverage, the feature gate, or credential
  qualification fails; `Top` is never a bypass;
- policy algebra result type: `meet(P, Q) -> Result<NormalizedPolicy,
  PolicyConflict>`; the GLB property is tested only on `Ok`, and a conflicted meet
  is a deterministic unsatisfiability that is never surfaced as
  `RESOURCE_UNAVAILABLE` and never persisted as a `Bottom` policy;
- empty-vs-conflict: an empty allowed-set intersection is `Ok` with a deny-all
  dimension (a Task obligation may then be `Unsatisfied`), while `PolicyConflict`
  is reserved for required launch/structural clashes such as two different
  required working directories or conflicting required environment values; tests
  must not accept one as the other;
- `RequiredAccess` projection: only a catalog-mapped `Set` with `polarity = Ability`
  and `security_class` in `Authority`/`Sandbox` projects, through the versioned
  member map; `Bool(true)` projects to nothing, a `Restriction` or an ineligible
  class projects to nothing even as a `Set`, an unmapped member fails closed, and
  `Set(∅)` yields a legal empty floor; a `Restriction` ceiling is never converted
  into a positive floor;
- canonical golden vectors keyed by `(canonical_version, algebra_version)`;
- compile budget: exhausting the budget mid-round, or an unknown credential
  candidate not proven continuity-non-decisive, cannot produce partial
  selection; a failed winner does not fall back;
- compile cache identity: the key includes normalization and meet algorithm
  versions explicitly; a different algorithm version cannot reuse a cached plan
  even when the effective-policy bytes digest matches;
- imported coverage: a policy using an unsupported dimension is ineligible even
  when the protocol gate is open, and a source that supports A and B is not
  assumed to support `A ∩ B`;
- reference profile: `SECURITY_V1_POLICY_ONLY_REFERENCE_V1` claims and non-claims
  are tested on both Linux and Windows; a non-claimed dimension makes a candidate
  ineligible rather than partially enforced; B5.4 publishes the OS
  enforcement-mechanism design note and proves receipt integrity,
  non-replaceability, identity binding, crash recovery and worker
  write/delete/replace denial; receipt read confidentiality is tested as a
  non-claim, and no behavior may depend on the receipt path, digest or content
  being unknown to the worker;
- physical root binding: `ResolveRoot(committed descriptor, RequestId)` MUST equal
  `receipt.actual_root` by materialized filesystem identity, not path string;
  root substitution, symlink/reparse-point aliasing, and TOCTOU between
  verification and rule application are negative cases, and a digest match with a
  wrong root cannot confirm RUNNING;
- bootstrap manifest binding: the trusted start path verifies
  `AppliedBootstrapManifest.digest == snapshot.bootstrap_manifest_digest` before
  publishing the sandbox receipt, and the receipt binds the applied manifest
  digest; a substituted or unmeasurable manifest fails closed and never reaches
  `HELD`, and a matching policy/profile/plan triple alone is not accepted as proof
  of the committed bootstrap;
- bootstrap authorization: `validate_bootstrap` rejects a manifest whose digest
  matches but whose content adds a worker-visible environment name, inherited
  handle, start exemption, path, credential or any other dimension beyond the
  compiled plan and `P_eff`; launcher-only privileges must be proven
  non-inheritable and dropped, and manifest content must be recoverable as
  canonical bytes or exact deterministic reconstruction rather than digest-only;
- receipt publication: the platform proof covers durable contents, atomic
  no-replace publication, durable namespace update, and re-read verification of
  the published identity; a process killed after publication but before namespace
  durability MUST NOT be reported as `HELD` or `RELEASED`, and reconciliation
  treats the receipt as not proven;
- restart-time phase classification: after a crash or OS restart the runtime MUST
  distinguish `HELD`, `RELEASED` and `UNKNOWN`, including the window where release
  was reached but the RUNNING transaction was not committed; unprovable release
  state remains `UNKNOWN` and is never inferred as `RELEASED`, and this proof is
  the first implementation gate of the policy-only profile, not a B5.5 finding;
- enforcement matrix completeness: every spec-10 dimension is classified
  proven/limited/unsupported for the exact binding, and an unclassified
  dimension fails the suite;
- credential probe: broker unreachable is not absent, stale or wrong-scope
  proofs are rejected, and epochs are not mixed; an unknown candidate in the
  winner's continuity tier defers, and only a proven continuity-non-decisive
  candidate may be ignored;
- credential request provenance: the request comes from the exact `SourceConfig`
  revision through the trusted resolver as a canonical secret-free
  `CredentialRequest`, is validated as a whole against `P_eff` before the first
  probe, and its digest reaches qualification and the `CredentialAccessPlan`;
  resolution runs during candidate qualification after source attestation, never
  at M6-A semantic admission; a scope learned from a broker response, or a request
  changed during the same observation epoch, is rejected as probe-then-authorize;
  the resolver is pure, read-only, secret-free, deterministic and deadline-bounded,
  and creates no physical resource, broker call or delivery side effect;
- credential reference completeness: over canonicalized references,
  `Refs(request) == Refs(SourceConfig)`; a resolver that omits a ref, adds one,
  returns an empty set for a non-empty config, keeps a duplicate, or mixes
  revisions fails closed, and an empty request never turns a credential-bearing
  config into a credential-free Execution that bypasses the brokered gate;
- credential evidence durability and scope: proof identity/epoch/issuer
  version/validity/subject digests reach the Execution snapshot, no raw broker
  response is persisted, and a requested scope outside `P_eff` or the Source
  ceiling is rejected;
- credential authorization default: a credential-bearing `SourceConfig` whose
  requested ref, use, scope, audience, or delivery mode is not explicitly granted
  by the bound AgentType `SECURITY_V1` ceiling is ineligible even when the broker
  reports `Available`, delivery is brokered, and the adapter supports it; an
  AgentType ceiling that omits the secret-scope dimension grants nothing and is
  never `Top`, while a Generation or source ceiling can only narrow a grant;
- credential authorization namespace: two distinct credential authorities that
  both use the string `prod-key` resolve to different
  `CredentialAuthorizationId` values; a source-local ref cannot acquire an
  AgentType grant by string equality across authorities, and a re-provisioned
  source keeps the same logical identity while a different logical credential
  requires a new AgentType revision;
- secret sentinel: no raw secret material in Agentype-produced durable JSON,
  logs, control errors, snapshots, or launch descriptors; the test covers the Core
  control path and does not assert that Core can detect a secret inside an opaque
  publisher-supplied `SourceConfigBody::OpaqueJson` payload, which remains a
  source-publisher responsibility;
- recipient identity: wrong SID/birth, wrong pinned instance, and
  cross-Execution delivery are rejected;
- receipt replay: Execution A's receipt cannot confirm B; a missing required
  receipt cannot enter RUNNING;
- hold/release: worker code does not run before sandbox-receipt durability; the
  hold stays closed when the daemon crashes or the control channel disconnects,
  and a parent-death-released pipe is rejected; recovery distinguishes
  awaiting-release from actually started; a receipt alone does not confirm
  RUNNING;
- secured SPI deadline: all three secured operations take one absolute deadline,
  the hold/receipt-publication/release stages share it with no per-stage reset, an
  expired deadline with unknown physical state yields `UNKNOWN` and M5 recovery
  rather than `RELEASED` or RUNNING, and a later `reconcile_secured_start` may
  take a new bounded deadline but cannot change the `RequestId` or create a
  second physical instance;
- secured start phase: `HELD`/`RELEASED`/`UNKNOWN`/`TERMINAL` transitions are
  exact; `HELD` requires a verified sandbox receipt, a credential-bound
  `RELEASED` requires both receipts, `UNKNOWN` permits a missing chain and cannot
  enter RUNNING; a crash-injection test where the controlled
  process is created but the receipt is not yet persisted MUST yield `UNKNOWN`,
  never `HELD`, `RELEASED`, or RUNNING, and the instance is never released or
  acknowledged; release requires a
  Kernel authority proof for the same RequestId and instance witness; PID reuse
  cannot authorize release; uncertain release cannot create a second worker;
- release authority proof: the proof carries `ExecutionId`, `RequestId`, the
  Attempt/Lease fencing identity, the exact physical instance witness, a bounded
  validity and the issuing authority identity; only the trusted Kernel path may
  issue it, a caller-filled or extended proof is rejected, and a stale or replayed
  proof cannot release a still-`HELD` instance after a Lease expiry or fencing
  change; after durable `RELEASED`, a repeated call returns the recorded state
  without a second OS release, while a `HELD` instance requires a fresh valid
  proof; the Kernel-check/OS-release race is not claimed atomic and fails closed
  into M5 recovery;
- cross-mode exclusion: a `SECURITY_V1` AgentType cannot execute a `COARSE_V1`
  Task, because the coarse path cannot prove its authored fine restrictions;
- receipt chain: receipts are append-only, write-once, and bounded to one per
  kind per Execution; the `SandboxAppliedReceipt` establishes `HELD` and is never
  rewritten; a `CredentialDeliveryReceipt` binds the sandbox receipt digest; a
  policy-only Execution has no credential receipt, and a credential-bound
  Execution has both before `RELEASED`; a second row of the same kind, or an
  upsert over one, is corruption;
- credential ordering: a credential-bearing `SECURITY_V1` Execution establishes
  delivery while `HELD`, persists the sandbox and credential-delivery receipts,
  then revalidates authority before `RELEASED`; a request for a different
  credential scope under the same Execution is rejected before any broker call,
  because the committed plan is immutable and one Execution carries at most one
  credential receipt — the different scope needs a new Execution with its own
  qualification, snapshot, and chain;
- partial credential delivery: a plan requesting several references that
  establishes only a subset produces no `CredentialDeliveryReceipt`, never reaches
  `RELEASED`, stays at most `HELD`/`UNKNOWN`, and triggers reconciliation plus
  closure of the partial capabilities; when closure cannot be proven the state is
  `UNKNOWN` and M5 recovery applies, never a "mostly delivered" success;
- receipt durability: file and directory metadata are durable before release, an
  existing different receipt is never replaced, and the worker inherits no
  receipt file/directory handle and no write/delete/replace access; read
  confidentiality is not claimed;
- RUNNING atomicity: observation, required receipt chain, RUNNING and first
  renewal all commit
  or all roll back;
- schema gate: a v8 database is rejected at v9 open and no implicit upgrade
  occurs;
- feature gating: without a declared policy/credential capability, behavior
  remains at B.4 fail-closed;
- crash recovery: an uncertain physical start keeps the original RequestId and
  does not create a second security fact.

These supplement, rather than replace, M4 writer-safety/recovery, M6-A frontier,
B.3 admission/matching and default-feature API-boundary tests (ADR-0011).

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
