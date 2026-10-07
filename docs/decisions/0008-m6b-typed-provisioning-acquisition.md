# ADR-0008 — Typed provisioning acquisition and the evidence-authority fence

Status: Accepted (M6-B.4)
Date: 2026-10-06
Canonical path: `docs/decisions/0008-m6b-typed-provisioning-acquisition.md`

## Context

M6-B.3 closed typed semantic commitment and non-authoritative existing-agent
preselection. A `TaskAgentRequirement` pins an exact `AgentTypeRef`, a
`LogicalAgentTypeBinding` pins an agent's semantic identity, and
`match_existing_agents` returns semantic candidates. None of these proves
physical eligibility.

Spec 06 and
[ADR-0007](0007-m6b-can-provision-task-staging.md) moved `can_provision_task` and
the remaining physical conjuncts to the first authority-bearing typed
acquisition path. That path is M6-B.4.

M6-B.1 froze `ResolvedProvisioningEvidence` as the only proof of security-class
enforcement, with no production constructor: `ENFORCED` is imported evidence, not
a catalog label. M6-B.4 owns the real production producer.

## Decision

> **Revision (audit round 20): provisioning is prepare-only.** Items 27, 33,
> 34, 39, 43, 44, 46–51, 56, 58, 60, 61 originally modelled a **side-effectful**
> `SourceConfigIntegration::materialize` that created a physical environment
> before the Execution. That created a second physical lifecycle M5 recovery did
> not own. Per this revision, `materialize` is **pure/read-only `prepare`**: it
> resolves the source-private config into an opaque launch descriptor with no
> physical side effect, and the **exact `ExecutionAdapter` physically materializes
> the described environment inside `start_execution`**, so M5's
> Execution/RequestId/`reconcile_start`/`observe`/`terminate` machinery owns the
> whole physical lifecycle. The pre-Execution `WRITER_QUIESCENCE_UNKNOWN` /
> `PROVISIONING_MATERIALIZATION_PENDING` machinery (items 46, 51, 55, 60, 61) is
> therefore withdrawn: with no pre-Execution side effect, an interrupted
> acquisition is an ordinary M5 orphan. Items 63–67 record the replacement
> decisions; where they conflict with the earlier items, items 63–67 govern.

1. **M6-B.4 owns the first authority-bearing typed acquisition.** The authority
   transaction MUST itself re-load the current Task authority/state, the current
   `LogicalAgentTypeBinding`, the validated source/config/policy revisions, and
   the capability catalog, and re-prove `can_execute` and `can_provision_task`
   (an `ACTIVE` `AdapterBindingPolicy` whose `required_safety` is satisfied by
   imported enforceability, and a coherent exact `AdapterKind` +
   `AdapterBindingKey`) before it creates any typed `Attempt`, `Lease`, or
   `Execution`. A caller-assembled selection that does not satisfy the predicates
   is rejected, not trusted. The M6-B.3 entry boundary requires credential
   availability before authority, but M6-B.4 has no trusted, source-bound
   availability authority, so the authority transaction **fails closed for any
   config that declares `credential_refs`**: a caller must never be able to mint
   that authority, and a mere `bool` parameter is not a proof. Credential
   resolution, availability, and brokering are B.5. Core never sees a secret
   value; the per-Execution snapshot records only a secret-free
   `credential_refs_digest`.

2. **The actual bound AgentType is the executable contract.** Acquisition proves
   `can_execute`/`can_provision_task` over the agent's exact bound `AgentTypeRef`
   (which MAY be a refinement, broader, or otherwise compatible B.3 candidate),
   not over equality with the Task pin; a different revision of the same
   `type_id` stays excluded (`D-TYPE-REV-COMPAT` deferred). The frozen M5
   placement gates (exact partition, tag superset, `Required` continuity
   workstream) are re-checked rather than assumed.

3. **The evidence producer lives on the internal resolution path.** A
   `ResolvedProvisioningEvidence` is produced only by the Scheduler's internal
   provisioning resolver from the imported M5 execution binding plus validated B.2
   catalog reads. The production producer is behind a `provisioning-producer`
   feature that narrows the default surface for a pure `agentype-agent-contract`
   consumer; this is **not** an authority proof, because Cargo feature
   unification means a consumer that also depends on `agentype-runtime` still
   sees the symbol. The load-bearing guarantees are instead that the authority
   transaction re-proves every conjunct from imported facts and that no control
   surface (`SchedulerControl`, `ProvisioningAdmin`, `CatalogAdmin`,
   `RootSemanticControl`, `IntentIngress`) accepts caller-supplied evidence or can
   inject enforcement facts into an authoritative resolver.

4. **The physical `execution_target`/`execution_profile` come from the M5
   partition/anchor**, not from `SpawnSource`. `SpawnSource`/`AdapterBindingPolicy`
   select the exact installed binding (`binding_ref` -> `AdapterKind` +
   `AdapterBindingKey`) of the matching kind; the target's `adapter_kind` MUST
   agree with the policy's `adapter_kind`, or the candidate is ineligible. The
   exact binding identity is carried from resolution into the acquisition and the
   snapshot; a caller does not re-supply it. This avoids changing the frozen B.2
   canonical `SpawnSource` content.

5. **A B.3 binding is semantic identity only.** An agent bound in M6-B.3 is not
   physical eligibility evidence. The first typed acquisition MUST mint a
   `ProvisioningBinding` for the Incarnation it selects (requalifying a
   previously-legacy Incarnation), or reuse a `ProvisioningBinding` whose frozen
   provenance still satisfies the requirement. Changing source/config requires a
   new Incarnation.

6. **Fallback is a pre-commit concern.** A candidate-local ineligibility skips
   only that candidate, so it cannot hide a later eligible source; catalog
   corruption fails the whole resolution. Fallback is allowed only before the
   `ProvisioningBinding`/`Execution` commitment. After the `BindingSnapshot`
   commits, a missing exact binding is a recovery/configuration failure, never a
   silent re-selection.

7. **`BindingSnapshot` is an atomic, coherent commitment.** It is written with
   the `Execution` row, and its adapter key/kind, target/profile,
   provisioning-binding provenance, and validated config digest MUST agree with
   the Execution and its `ProvisioningBinding`, or the transaction fails. It
   records target/profile, exact `adapter_kind`/`adapter_binding_key`,
   source/config provenance, the admitted requirement's capability values, the
   resolved security, and a secret-free `credential_refs_digest`. The
   cross-record coherence is shared by the commitment path and the authoritative
   read, so a self-consistent but incoherent durable row is never accepted.

8. **The evidence subject binds the exact physical domain.**
   `ResolvedProvisioningEvidence` carries the `adapter_kind` and the opaque
   `adapter_binding_key` it was imported from. The authority transaction requires
   `evidence.adapter_kind == policy.adapter_kind == selection.adapter_kind` and
   `evidence.adapter_binding_key == selection.adapter_binding_key`, so an
   enforceability proof imported from one installed domain can never authorize
   another. `ResolvedProvisioningEvidence` construction is not an authority
   proof, so the transaction never treats the evidence value itself as a token.

9. **Legacy physical history is never promoted to SourceConfig provenance.**
   M5 Executions freeze only `adapter_kind`/`adapter_binding_key`; they never
   freeze the `SpawnSource`/`SourceConfig` a legacy Incarnation was created from,
   and SourceConfig is Core-opaque. A legacy Incarnation is therefore adopted in
   place only when it is truly fresh (no execution history); one with any
   physical history rolls over to a new Incarnation. A future trusted legacy
   provenance attestation would be a separate bridge.

10. **`ProvisioningBinding` is stable provisioning provenance.**
    Its scope is the Incarnation's physical/source provenance, not a per-Task
    requirement: it does NOT carry a Task requirement digest. The per-Task
    commitment (requirement capabilities, resolved security) lives on the
    Execution's `BindingSnapshot`. `qualifies()` compares the exact domain,
    source/config/policy, and the resolved enforceability, so any change rolls
    over to a new Incarnation.

11. **One enforceability algebra.** `AdapterSafetyEnvelope::enforces_workspace`
    is exact-set membership, matching the frozen M6-B.1 `PhysicalSafety`; B.4
    imports enforceability facts and never infers one workspace mode from
    another.

12. **`attempt_isolation` is M4-authoritative.** Adapter import describes a
    domain's capability, but the isolation of a concrete launch comes from the
    `ExecutionRegistry` target. B.4 requires the target's `attempt_isolation`
    when the Task requires isolation, and the `BindingSnapshot` records the
    authoritative value, which the validator requires to equal the frozen
    Execution. A `BindingSnapshot` may never claim a stricter isolation than the
    physical launch.

13. **The effective network policy reaches the request.** The neutral
    `NetworkEnforcement` is carried on the `ExecutionLaunchSnapshot` and the
    provider-neutral `EnvironmentStartRequest`, so a proven-but-unrequested
    network policy cannot be silently widened at launch.

14. **The committed selection is Scheduler-derived.** The runtime acquisition
    facade re-enumerates the eligible set, selects the winner, re-resolves the
    exact `binding_ref` from the winner, and builds the durable selection from
    that winner immediately before the authority transaction; no caller-supplied
    candidate or enforcement fact enters the transaction, and a stale binding is
    rejected.

15. **`ExternalRef` configs require `attest` I/O; `OpaqueJson` is Core-verified.**
    Candidate resolution MAY perform bounded, read-only attestation I/O through
    `SourceConfigIntegration::attest` (resolving and hashing an `ExternalRef`
    locator so its declared digest is attested); it performs no side-effectful
    materialization before the authority commitment. An `OpaqueJson` body digest
    is verified by Core through the validated read. An `ExternalRef` is eligible
    only when the integration attests it; an unattested `ExternalRef` fails
    closed.

16. **A `ProvisioningBinding` is coherent with the LogicalAgent type binding.**
    The authoritative validator requires `binding.agent_type` to equal the
    agent's frozen `LogicalAgentTypeBinding`, not merely an existing revision.

17. **Resolver version is an audit family, not an equality.** The authoritative
    read accepts any `agentype-resolver/*` version so immutable historical
    snapshots do not become corruption on a producer bump.

18. **Effective isolation is the OR of both sides.** The effective
    attempt-isolation requirement is
    `AgentType.requires_attempt_isolation OR Task.required_attempt_isolation`,
    and it is checked against the authoritative M4 target before authority.

19. **Capability and effective policy are separate.** `ProvisioningBinding`
    records the imported **enforceability capability** (`PhysicalSafety`).
    `BindingSnapshot` records that capability separately from the per-execution
    **effective** isolation/workspace/network, so a domain that can isolate but
    does not use it is representable; the validator cross-checks the effective
    values against the authoritative Execution and requirement.

20. **Typed executions cannot bypass the snapshot.** The legacy
    `create_execution` fails closed for a `TYPED` Task, so every typed Execution
    commits through `create_execution_with_snapshot` with its provenance.

21. **Selection is Scheduler-derived and fails closed on ambiguity.** The
    authority-bearing runtime facade re-enumerates the eligible set, applies the
    continuity dimension, and commits the winner; when more than one candidate is
    tied and the frozen availability/cost dimensions cannot disambiguate, the
    selection is unresolved and fails closed rather than using an invented
    tie-break ([ADR-0010](../../decisions/0010-m6b-selection-order-scope.md)).

22. **`reconcile_pool` validates binding coherence.** Every live agent is
    resolved through the authoritative binding-coherence read; a marker/child
    mismatch aborts the reconcile transaction, never silently excluded.

23. **Every durable UNIQUE identity is guarded against `INSERT OR REPLACE`.**
    Guards cover the primary key and every secondary/partial unique identity
    (`request_id`, active execution per Incarnation, active Incarnation per
    agent), not only `attempt_id` and `(agent, generation)`.

24. **The effective isolation is re-proved at Execution commitment.** The
    `BindingSnapshot` authority validator recomputes
    `AgentType.requires_attempt_isolation OR Task.required_attempt_isolation`
    from the bound AgentType and the admitted requirement and requires the frozen
    Execution to enforce it. A self-consistent Execution+snapshot pair that
    lowers an admitted hard security requirement fails the transaction.

25. **Restart recovery does not bypass the snapshot marker.** The M5
    reconciliation-candidate read validates the Execution provisioning-snapshot
    marker/child coherence through the same authority read the supported surface
    uses, so a `SNAPSHOT` Execution with a missing/incoherent snapshot aborts
    recovery rather than reconciling without provenance.

26. **Selection is fenced to the commitment.** The resolver captures a
    deterministic active-candidate-frontier digest; the authority transaction
    recomputes it and rejects a mismatch, so a concurrently published active
    source/config cannot bypass the deterministic selection.

27. **SourceConfig must be materialized, not just named.** A
    `SourceConfigIntegration` consumes the validated `SourceConfigRevision` and
    attests, bound to the exact `(adapter_kind, adapter_binding_key)`, a
    secret-free materialization digest. That digest is frozen in the
    `ProvisioningBinding`/`BindingSnapshot` and required; a config identity alone
    never becomes a physical environment. This is also the B.4 `ExternalRef`
    content attestation obligation: an `ExternalRef` is eligible only when the
    integration resolves and attests it.

28. **The existing-vs-new decision is Scheduler-owned.** A single
    `acquire_typed_task` applies the frozen spec 06 order: the B.3 ranked READY
    candidates are tried in order with a full B.4 physical-eligibility proof, and
    only when none can be acquired does the Scheduler provision a new agent.

29. **The production reference adapter declares its honest exact set.** The
    local-process adapter does not sandbox, so it imports NO enforceable
    workspace mode or network policy; a typed Task requiring any workspace/network
    mode is ineligible for it until a real sandboxed adapter exists.

30. **The authoritative reads re-prove the safety relation.**
    `policy.required_safety` must be satisfied by `binding.effective_security`,
    and a `BindingSnapshot`'s effective workspace/network must be enforceable by
    that capability.

31. **The typed exact launch is defined but not yet wired.** Spec 07 describes
    the source-resolved exact-binding launch path as defined; the production
    dispatch composition still uses `resolve_unique` until the daemon integration
    lands, and the milestone is not frozen.

32. **No false enforceability evidence.** The reference local-process adapter
    does not sandbox, so it declares NO enforceable workspace/network mode; it
    must not claim `WorkspaceMode::Write` merely because an OS process can write.

33. **Materialization is part of Incarnation qualification.**
    `ProvisioningBinding::qualifies` compares the materialization digest, so a
    changed attestation for the same source/config/domain rolls over to a new
    Incarnation instead of reusing a binding frozen with the old materialization.

34. **Eligibility attestation and winner materialization are split.**
    `SourceConfigIntegration::attest` is side-effect-free (bounded read-only I/O
    is allowed) and used during candidate eligibility (an empty attestation is
    ineligible); `materialize` is side-effectful, idempotent, and called only for
    the durably committed winner, AFTER the authority transaction (see item 39),
    and it returns the digest it actually materialized, which the caller requires
    to exact-match the committed digest. Losers are never materialized. The rule
    is: candidate resolution may perform bounded read-only attestation I/O, but
    performs no side-effectful materialization before the authority commitment.

35. **Only explicit candidate-local ineligibility permits fallback.** The
    Scheduler falls through to the next candidate/route only for enumerated
    candidate-local ineligibilities; every other error (invariant violation,
    storage failure, recovery-required, unsupported resolver state, missing
    revision) terminates the acquisition.

36. **The selection frontier is canonical and disposition-complete.** The
    active-candidate frontier digest uses the canonical structured encoding
    (not a delimiter grammar) and includes each source's adapter-policy
    disposition, since disposition affects durable eligibility.

37. **The fallback classifier is narrow and fail-closed.** Only the explicit
    candidate-local ineligibilities permit falling through to the next route; a
    `ContractError` is skippable only when it is one of
    `CapabilityMismatch`/`SecurityUnenforceable`/`EvidencePolicyMismatch`/
    `EvidenceSubjectMismatch`/`SourceConfigInvalid`, and every other
    `ContractError` (notably `InvariantViolation`) terminates the acquisition.
    One authoritative classifier serves both enumeration and the facade.

38. **An ambiguous selection is task-terminal.** `SelectionAmbiguous` is never a
    candidate-local fallback: if the B.3-best agent's source selection is
    unresolved, the acquisition fails closed rather than silently preferring a
    lower-ranked agent whose source choice happens to be unique.

39. **Materialization follows the durable commitment.** No side effect precedes
    the authority transaction: eligibility uses the pure `attest`, the winner's
    provision binding/claim is durably committed, and only then is the winner
    materialized under that identity (idempotent), so a crash or a losing route
    cannot leave an orphan materialization and losers are never materialized.

40. **The source integration obeys the absolute-deadline discipline.** `attest`
    and `materialize` receive one absolute `AdapterDeadline`; a physical
    integration cannot block the Scheduler acquisition indefinitely (the same
    liveness contract the M5 adapter boundary enforces).

41. **The authority acquisition stays daemon-internal.** `acquire_typed_task`
    and the candidate-resolution mechanics are `pub(crate)`; only the
    `SourceConfigIntegration` SPI and the diagnostic candidate value are exposed,
    until the ControlLoopService owns the path and the supported surface is
    deliberately widened.

42. **A global authority-snapshot failure is not candidate-local.** The catalog
    frontier fence returns a distinct `StaleAuthority`; the fallback classifier
    treats it (and any `StaleAuthority`/`Conflict`) as a whole-acquisition
    re-resolution, never as "this agent is ineligible". Only the per-candidate
    storage rejections (`InvalidAuthority`/`InvalidTransition`/`NotFound`/
    `ConfigurationUnavailable`) permit falling through to a lower-ranked agent.

43. **The materialization receipt must match the committed digest.**
    `SourceConfigIntegration::materialize` returns the digest it actually
    materialized; the caller requires it to exact-match the durably committed
    `materialization_digest` before returning the acquisition, so a TOCTOU
    between `attest` and `materialize` can never produce an Execution.

44. **A post-commit acquisition failure settles its authority.** Because the
    Attempt/Lease/assignment is committed before materialization, a failure there
    MUST mechanically settle that committed authority, and it MUST NOT leave a
    dangling claim that recovery would later mis-classify as `ExecutionLost`. The
    settlement depends on whether the side-effectful invocation was entered
    (item 51): a provably side-effect-free pre-invocation failure is
    `RESOURCE_UNAVAILABLE` (frozen M5 pre-start composition-failure semantics);
    a failure after the invocation began is physical-writer-unknown and is
    suspended as `WRITER_QUIESCENCE_UNKNOWN`.

45. **The runtime re-qualifies the absolute deadline after every integration
    call.** Before computing any conjunct, and after `attest`/`materialize`
    returns, the runtime checks the same absolute `AdapterDeadline`; a result
    obtained after the endpoint yields a non-candidate-local
    `ProvisioningDeadlineExceeded` (M5.6 evidence-admission semantics), never a
    successful observation.

46. **Frozen M5 recovery recognizes the post-commit materialization phase as a
    legitimate durable state.** The staging order (authority COMMIT, then
    `materialize`, then the later `Execution`+`BindingSnapshot` commitment)
    introduces a state M5 never had: an `ACTIVE` Attempt+Lease with a
    `PROVISIONED` Incarnation and a durable `ProvisioningBinding` but no
    `Execution`. A process death in the window between the COMMIT and the
    `Execution` commitment leaves exactly that state. `recover_authority` /
    `expire_leases(recover_unstarted=true)` MUST detect it (a typed Task with a
    `task_agent_requirements` row whose incarnation has a `ProvisioningBinding`,
    and no Execution) and treat it as **physical-writer-unknown**, NOT the M5
    `CLAIM_ORPHANED`/`ExecutionLost` orphan path and NOT a
    provably-absent execution. The materialization side effect may already exist
    even though no `Execution` records it, so the frozen M5 rule "lease expiry /
    process death is not quiescence" applies: the claim is suspended as
    `WRITER_QUIESCENCE_UNKNOWN` (`PROVISIONING_MATERIALIZATION_PENDING`) and is
    NEVER automatically retried, so a fresh source selection cannot race a
    possibly-live second tuple against the unconfirmed first. It can only be
    retried after confirmed quiescence or isolation, via the existing M5
    escalation machinery. The immutable exact ProvisioningBinding remains durable
    as the identity for that re-drive. `writer_is_safe_to_replace` MUST NOT be
    consulted with `execution_exists=false` for this phase, because that would
    wrongly read "no side effect". CI green cannot cover this process-death
    window; it is closed by this classification plus explicit crash regressions.

47. **Materialization re-qualifies Lease authority, before and after.** The runtime
    re-checks that the committed Attempt/Lease is still `ACTIVE`, at the same
    epoch, and unexpired (`claim_authority_is_current`) BOTH immediately before
    invoking the side-effectful `materialize` (so a stale claim never starts the
    side effect) and again after it returns. A pre-invocation staleness is
    provably side-effect-free and settles as `RESOURCE_UNAVAILABLE`; an authority
    that expires while the side effect was in flight is physical-writer-unknown
    and is suspended as `WRITER_QUIESCENCE_UNKNOWN` (item 51). Either way the
    acquisition MUST NOT be reported as successful: it returns a terminal
    `PostCommitMaterializationFailed`, rather than letting a later `Execution`
    commitment fail on stale authority after the side effect already happened.

48. **Deterministic selection operates on the fully eligible set.** Candidate
    enumeration applies every hard, per-candidate eligibility predicate before
    `select_candidate`: `can_provision_task` over the exact candidate tuple, the
    config carries no `credential_refs` (B.4-ineligible), the source's stable
    `binding_ref` still resolves to the exact installed binding, and the
    attestation is non-empty. In addition, the pre-commit resolver filters by the
    partition's authoritative execution-target `adapter_kind` before ranking, so a
    wrong-kind or credential-bearing, higher-continuity candidate can never shadow
    a truly eligible candidate (or manufacture a spurious `SelectionAmbiguous`).
    The authority transaction still re-proves every conjunct (defense in depth);
    the point is that the ranking input set, not just the winning element, is
    complete.

49. **The physical materialization contract is narrowed to crash-safe idempotency
    with no cross-tuple conflict.** Because `materialize` is a physical side effect
    with no Execution-level fencing token or observe/reconcile protocol, its
    contract is narrowed: it MUST be crash-safe and idempotent for the exact
    `(source config, adapter_kind, adapter_binding_key)` tuple, and materializing a
    different tuple MUST NOT conflict with — or silently reuse — the resources of
    an earlier tuple whose completion was not confirmed. Combined with item 46
    (no automatic re-selection while the outcome is unknown), this is the B.4
    physical-authority model: a fully durable materialization *operation* with
    observe/reconcile/quiescence remains deferred to a later milestone.

50. **The acquisition carries its winner candidate out, and the Execution handoff
    is a defined daemon-internal step.** `acquire_typed_task` returns
    `TypedAcquisitionOutcome { acquisition, candidate }` — the committed
    Attempt/Lease/ProvisioningBinding plus the exact `SourceProvisioningCandidate`
    the authority transaction proved. The Execution handoff
    (`prepare_typed_execution_launch`) resolves the Attempt's environment, resolves
    the adapter by the **exact** `(adapter_kind, adapter_binding_key)` from the
    acquisition (never `resolve_unique`), freezes the `BindingSnapshot` from the
    same candidate evidence, and commits the snapshot-bearing Execution through
    `create_execution_with_snapshot`, returning the assembled
    `EnvironmentStartRequest`. It deliberately stops at the request: the physical
    `start_execution` plus the immediate-observation commit are the daemon
    dispatch step's responsibility, so this milestone does not duplicate M5.4
    dispatch semantics. Dispatcher wiring (acquiring typed work and calling this
    handoff) remains deferred; the supported surface is unchanged, and typed work
    is still not dispatched by the production daemon.

51. **Materialization settlement splits on whether the side-effectful invocation
    was entered.** A failure BEFORE the invocation (deadline already spent, stale
    authority, missing revision) is provably side-effect-free, so the committed
    authority is settled with the frozen pre-start `RESOURCE_UNAVAILABLE`
    semantics. A failure AFTER the invocation began (the integration returned an
    error, or a post-call deadline/receipt/authority check failed) has an unknown
    physical outcome — a side effect may exist with no Execution recording it — so
    it is settled as `WRITER_QUIESCENCE_UNKNOWN`: the Task and LogicalAgent are
    suspended, an escalation is opened, and the claim is never auto-retried or
    re-selected until quiescence or isolation is confirmed. This is the frozen M5
    rule that a failed invocation is not proof of physical absence or quiescence,
    and it matches the process-death recovery classification (item 46). Both cases
    surface a terminal `PostCommitMaterializationFailed`; the distinction is only
    in the settlement, and the `RESOURCE_UNAVAILABLE` path is reachable only when
    the side effect is provably impossible.

52. **`materialization_digest` is a constrained canonical digest, not an opaque
    string.** `MaterializationDigest` enforces the frozen grammar
    `sha256:<64 lowercase hex>` at construction, mirroring `ConfigDigest` but
    without a pre-commit draft phase (it only ever enters durable provenance).
    This keeps the B.2 source-private-to-durable-Core seam intact: a source
    integration cannot inject an arbitrary string into an immutable
    `ProvisioningBinding`/`BindingSnapshot`. The receipt-mismatch error is redacted
    and never echoes a source-produced value.

53. **The full target+profile is validated BEFORE any authority commit or physical
    work.** The frozen M5 environment resolution rejects a missing profile or an
    incompatible `allowed_targets`; B.4 must not defer that purely static check
    behind its new pre-Execution physical stage. The pre-commit resolver and the
    authority transaction both call a pure, Attempt-independent
    `resolve_target_profile` (missing target/profile, or incompatible
    `allowed_targets` → configuration failure), so a static misconfiguration
    leaves the Task `QUEUED` with no Attempt/Lease/ProvisioningBinding and never
    invokes `materialize`. Checking only the target (or only after materialize)
    would promote a static config error into a physical-quiescence incident.

54. **Legacy `MOVE_CAPACITY` moves only `UNBOUND` LogicalAgents.** Per ADR-0009,
    V0.1 operator capacity applies to the untyped population only. `move_capacity`
    now selects its cutover candidates through the same authoritative
    `LogicalAgentTypeBinding` read `reconcile_pool` uses: a `BOUND` typed agent is
    never relocated (which would silently change its frozen B.3 partition
    placement), and an incoherent marker/child aborts the transaction. A real
    typed MOVE/capacity split remains `D-TOPOLOGY`, not half-implemented here.

55. **Every authority-closing path excludes the provisioning-pending state.** The
    shared predicate `is_provisioning_pending_claim` (a typed Task whose
    incarnation durably committed a `ProvisioningBinding`, with no Execution) is
    the single authority-state rule: *post-commit provisioning with no Execution
    is NOT proof of physical absence*. Every path that would otherwise read
    `Execution == None` as "no side effect" must fence instead of release:
    - `report_provisioning_quiescence_unknown` performs a dedicated settlement
      (validate authority → record the failure → suspend) that NEVER consults the
      frozen `RetryPolicy` or the `writer_is_safe_to_replace(execution_exists=
      false)` decision. A Task whose policy legitimately lists
      `WRITER_QUIESCENCE_UNKNOWN` is still suspended, never `RETRY_WAIT`.
    - `cancel_task` / `cancel_batch` mark the Task `CANCELLED` but keep the
      LogicalAgent `SUSPENDED` with a `WRITER_QUIESCENCE_UNKNOWN` obligation; an
      explicit `quiescence_confirmed` attests only the (nonexistent) Execution
      writer and therefore does NOT cover the provisioning side effect, so
      provisioning-pending wins and the agent is not released.
    - `expire_leases` (item 46) already applies the same rule.
    Audited and unchanged because they cannot reach this state: the generic `nack`
    (no longer reachable for pending once the dedicated settlement exists),
    `nack_preserving_physical_history` / `ack_success` (require an Execution),
    `abort_before_physical_start` (requires a STARTING Execution), and the idle
    drain paths in `reconcile_pool` / `merge_partitions` (no active Attempt).

56. **The materialization is causally bound to the physical launch.**
    `SourceConfigIntegration::materialize` returns a `MaterializedSource`
    — the committed digest plus an **opaque, secret-free environment handle**.
    The handle is frozen in `BindingSnapshot.materialization_handle` (created
    atomically with the Execution) and forwarded to the provider-neutral
    `EnvironmentStartRequest` (`from_launch_with_materialization`), so the adapter
    consumes the exact environment this Execution's provenance names. Core never
    interprets the handle. This closes `SourceConfig → materialization →
    Execution launch`: two configs under one `AdapterBindingKey` can no longer
    both launch against an ambient/default environment.

57. **The realized M4 lifecycle is re-proved at acquisition.** B.3 deliberately
    left the realized-lifecycle conjunct to B.4. The member's actual M4
    `Retention` (`Resident` / `Ephemeral`) requires the effective source/config
    lifecycle (`SourceConfig::effective_lifecycle`) to contain the corresponding
    `LifecycleMode`. Both the pre-commit resolver and the authority transaction
    prove it, so a `Resident` partition can never be provisioned by an
    `Ephemeral`-only source/config.

58. **The pre-materialization fence is the full committed-claim authority check.**
    `claim_authority_is_current(claim)` reuses the same `AuthoritySnapshot`
    validation as every authority-bearing transaction (Attempt/Lease `ACTIVE`,
    lease epoch, lease unexpired, `Task.current_attempt_id`, `Task.fencing_epoch`)
    and additionally requires the Claim's identity copies to match the durable
    Attempt. It is run before **and** after the side-effectful `materialize`, so
    the new physical side-effect boundary is fenced no more weakly than M5's.

59. **The pending predicate uses authoritative marker/child reads.**
    `is_provisioning_pending_claim` reads `get_task_agent_requirement` and
    `get_provisioning_binding` (positive-marker coherence), not naked child-row
    `EXISTS`. A `TYPED` marker with a missing requirement row, a `PROVISIONED`
    incarnation with a missing binding, or their inverse, is durable corruption
    that fails the caller's transaction closed — never a silent downgrade to a
    legacy/orphan claim.

60. **Pre-materialization failures are classified by fault kind.** A
    provably side-effect-free deadline failure settles as `RESOURCE_UNAVAILABLE`;
    a stale/lost authority is `AuthorityLost` and is **not** nacked as a Task
    configuration failure (recovery settles it); a durable-corruption /
    persistence fault (including a committed immutable revision that has
    vanished) is **fatal** and propagates without any Task-level `nack`. Only a
    failure after the invocation is entered becomes `WRITER_QUIESCENCE_UNKNOWN`.

61. **The typed handoff settles a post-materialize failure.** Because
    `materialize` has already run when `prepare_typed_execution_launch` executes,
    any handoff failure settles the committed authority as
    `WRITER_QUIESCENCE_UNKNOWN` (best-effort; a stale authority is cleaned up by
    recovery) before the original error is returned, instead of leaving a
    possibly-live provisioning side effect with no Execution while the Task stays
    leased.

62. **The handoff re-qualifies the current enforceability.** After
    `resolve_exact`, the handoff re-checks the CURRENT installed
    `AdapterSafetyEnvelope` against the committed
    `ProvisioningBinding.effective_security`: exact `(kind, key)` identity is not
    enough — an importer that kept the key but weakened its workspace/network/
    isolation capability must not launch (frozen M5.1: a stale safety proof cannot
    authorize a later launch after registry reconfiguration). Any weakening is a
    pre-start handoff failure, never committed as an Execution (see item 60/66 for
    its settlement).

63. **`materialize` is pure `prepare`; the exact adapter owns the physical
    work.** `SourceConfigIntegration` exposes `attest` (pure) and `prepare`
    (pure/read-only), both bounded by one absolute deadline and neither creating
    any physical resource. `prepare` resolves the source-private config into an
    opaque, secret-free **launch descriptor** (re-deriving the attested digest,
    which must match the committed one). The descriptor is frozen in the
    `BindingSnapshot` and carried on the `EnvironmentStartRequest`; the exact
    `ExecutionAdapter` physically materializes the described environment inside
    `start_execution`. M5's Execution/RequestId/`reconcile_start`/`observe`/
    `terminate` machinery therefore owns the entire physical lifecycle, and there
    is no second lifecycle before the Execution. This restores the frozen
    "M6-B never creates a physical environment" boundary and withdraws the
    pre-Execution `WRITER_QUIESCENCE_UNKNOWN` / `PROVISIONING_MATERIALIZATION_PENDING`
    machinery (items 46, 51, 55, 60, 61 as originally written): with no
    pre-Execution side effect, an interrupted acquisition is an ordinary M5 orphan
    (`ExecutionLost`), and cancellation simply releases the agent.

64. **The descriptor protocol is a proven composition relation.**
    `ImportableAdapter::import_provisioning_protocol` declares the descriptor
    grammar an exact adapter binding accepts; `SourceConfigIntegration::protocol`
    declares the grammar an integration prepares. Candidate eligibility requires
    `integration.protocol == imported.provisioning_protocol`, the committed
    selection freezes that protocol in the `ProvisioningBinding`, and the handoff
    re-checks the current adapter's protocol against it. A descriptor produced for
    one adapter can therefore never be launched against an adapter that does not
    understand it.

65. **The digest is named for what it is.** `ProvisioningBinding` /
    `BindingSnapshot` carry `attested_materialization_digest` (an expected
    identity resolved by the pure integration), not a claim that materialization
    happened; materialization is the adapter's start operation. The mismatch error
    is redacted.

66. **Pre-start failure classification.** A pure `prepare` failure or a handoff
    failure is provably side-effect-free: a static/liveness failure settles as
    `RESOURCE_UNAVAILABLE`, an authority loss is left to recovery (no Task-level
    nack), and a durable persistence/corruption fault is `Fatal` and propagated,
    never disguised as a Task failure. `TypedLaunchError` exposes no
    `standard_failure_class`; the handoff settlement is not swallowed (only a
    confirmed authority loss is ignored, otherwise a fatal `SettlementFailed`).

67. **Separate attest and prepare deadlines.** The acquisition facade takes the
    enumeration (`attest`) deadline and the winner (`prepare`) deadline
    separately, so unrelated candidate probes cannot consume the winner's
    preparation budget; each physical integration call still uses one absolute
    endpoint that is not renewed mid-call.

68. **The source integration is source-local.** A single global
    `SourceConfigIntegration` would let the caller's choice silently narrow the
    candidate universe and let one source's integration interpret another source's
    opaque config. Instead the composition root holds a
    `SourceIntegrationRegistry` mapping each durable `SpawnSourceRef` to the one
    integration that understands its config grammar; enumeration resolves the
    integration **per source** (an unrouted source is ineligible) and `prepare`
    re-resolves the same source-local integration from the committed
    `ProvisioningBinding.spawn_source`. The candidate universe therefore depends
    only on the durable catalog plus this explicit routing, and the B.3 source
    multiplicity is preserved without Core ever interpreting provider/model.

69. **Typed-handoff failures are classified, and all fallible checks precede the
    Execution commit.** `prepare_typed_execution_launch` runs every fallible
    composition check (binding/environment/target/adapter/protocol/enforceability/
    candidate-vs-`ProvisioningBinding`) as a plan BEFORE committing the Execution;
    only a pre-start availability failure (`Configuration`/`InvalidBinding`/
    `Adapter`/`Weakened`) is settled as the Task-level `RESOURCE_UNAVAILABLE`. A
    `StaleAuthority`/`InvalidAuthority` kernel rejection is left to recovery (no
    Task-level nack), and durable corruption / persistence (`Inconsistent`,
    fatal `Kernel`, settlement failure) is fatal and never disguised as a Task
    failure. Failures after the Execution commit (the infallible-by-construction
    request assembly) are likewise not nacked; recovery reconciles the committed
    Execution.

70. **Typed-provisioning capability is optional and honest.**
    `ImportableAdapter::import_provisioning_protocol` returns `Option<&str>` with
    a `None` default: an ordinary M5 physical adapter is not provisioning-capable
    and is ineligible for typed acquisition. A non-blank protocol is required at
    registry-import time (like kind/key). The reference local-process adapter
    returns `None` because its `start_execution` does not consume a provisioning
    descriptor; it must not claim a protocol it ignores.

## Consequences

- M6-B.1's `ResolvedProvisioningEvidence` gains a production producer without
  reopening its purity contract; `DECLARED != ENFORCED` is preserved. A
  constructor's absence on the default surface is a supported-surface fence, not
  an authority proof: the load-bearing guarantees are the in-transaction re-proof
  and the compile-fail boundary witness.
- The M5.7 imported-binding boundary is additively extended to carry enforceable
  workspace/network (and optional enforced sandbox policies/capabilities); this is
  an explicit additive amendment, with conservative defaults that fail closed.
- A typed Task acquires M5 authority only through this path; until acquisition it
  remains durable `QUEUED` and invisible to the legacy claim path.
- Any later change to the staging or the evidence fence is a new ADR, not an edit
  inside a milestone PR.
