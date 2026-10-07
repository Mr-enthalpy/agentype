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
- an ambiguous selection is task-terminal (it never falls through to a
  lower-ranked agent or a new agent)
- the selected winner is prepared only AFTER its claim/binding is durably
  committed; losers are never prepared, and neither `attest` nor `prepare` creates
  a physical resource (no side effect precedes the Execution)
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
  `ProvisioningBinding`, and the handoff re-checks the current adapter's protocol
  against it
- provisioning capability is optional: an adapter that does not declare a
  non-blank `import_provisioning_protocol` is not provisioning-capable and is
  ineligible for typed acquisition (the reference local-process adapter declares
  none, because its `start_execution` ignores a descriptor)
- the candidate universe is source-local: an active source whose
  `SpawnSourceRef` is not routed to an integration by the composition root is
  ineligible, so one source's opaque config is never interpreted by another
  source's integration
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
