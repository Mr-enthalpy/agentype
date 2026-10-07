# RIIR M6-B.4 — Provisioning Binding and Exact Launch Implementation Record

Status: Historical Report
Applies to: V0.2 / M6-B.4
Canonical path: `docs/reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md`
Not a specification.

This records the M6-B.4 implementation of the first authority-bearing typed
acquisition on top of the frozen M6-B.3 schema v7. It is a boundary record, not
a specification; it does not supersede
[spec 06](../../specs/v0.2/06-agent-type-and-matching.md),
[spec 07](../../specs/v0.2/07-spawn-source-and-adapter-contract.md), or
[spec 13](../../specs/v0.2/13-storage-and-transactions.md).

## Status

```text
M5 execution/runtime substrate           FROZEN
M6-A semantic frontier kernel            FROZEN
M6-B.1 AgentType ontology                FROZEN
M6-B.2 durable catalog (schema v6)       FROZEN
M6-B.3 typed admission (schema v7)       FROZEN
M6-B.4 provisioning binding (schema v8)  IMPLEMENTED, NOT FROZEN
                                         (locally verified; daemon typed-dispatch
                                         wiring pending; the independent freeze
                                         audit is the remaining gate)
```

The running daemon's control loop still dispatches the legacy claim path; the
typed acquisition primitives are landed and tested at the storage/runtime
library level, and wiring them into the daemon loop is the remaining integration
step before the freeze gate can be declared closed.

## Frozen surface

```text
SCHEMA_VERSION = 8

ProvisioningBinding        per-Incarnation immutable exact source/config/policy
BindingSnapshot            per-Execution immutable exact physical choice
ResolvedProvisioningSelection
                           durable acquisition input (re-proved in-transaction)
SourceProvisioningCandidate
                           one eligible source/config candidate carrying the
                           exact resolved (adapter_kind, adapter_binding_key)
ImportedEnforcementFacts   (neutral) adapter-imported enforceability

agent-contract
    ResolvedProvisioningEvidence::from_imported_binding   (behind
        `provisioning-producer`; a default-surface narrowing, not an authority
        proof under Cargo feature unification)
    credential_refs_digest
    canonical_provisioning_binding_bytes / provisioning_binding_content_digest
    canonical_binding_snapshot_bytes / binding_snapshot_content_digest
    provisioning_binding_from_canonical_json / binding_snapshot_from_canonical_json
    BindingSnapshot / ProvisioningBinding / RESOLVER_VERSION

adapter-api
    NetworkEnforcement
    ImportableAdapter::import_enforceable_workspace / import_enforceable_network
        (additive, conservative empty defaults)

runtime
    AdapterSafetyEnvelope::enforceable_workspace / enforceable_network
    AdapterRegistry::{import_source_with_binding_ref, resolve_binding_ref}
    provisioning_resolver::{resolve_source_candidates,
                            resolve_source_candidates_for_agent, select_candidate,
                            SourceProvisioningCandidate, ProvisioningResolutionError}
        (acquisition consumes the ExecutionRegistry to check the partition target
         kind; the AdapterBindingPolicy is re-loaded in the transaction)
    acquire_typed_task -> TypedAcquisitionOutcome{acquisition, candidate}
    prepare_typed_execution_launch (daemon-internal Execution handoff)
    CatalogAdmin::{publish_adapter_binding_policy, publish_spawn_source,
                   publish_source_config, set_*_status}
    ProvisioningAdmin::{provisioning_binding, binding_snapshot}

storage / kernel
    provisioning_bindings / binding_snapshots (+ parent presence markers)
    acquire_typed_task_existing / acquire_typed_task_new_agent
    create_execution_with_snapshot
    get_provisioning_binding / get_binding_snapshot
    list_active_spawn_source_refs / list_active_source_config_refs
```

## Frozen invariants

```text
a typed Task acquires no Attempt/Lease/Execution without the mandatory conjuncts,
    re-proved inside the authority transaction over the current catalog
acquisition proves can_execute/can_provision_task over the ACTUAL bound AgentType
    (a refinement/broader/compatible candidate is accepted; cross-revision is not)
the frozen M5 placement gates are re-checked, not assumed from B.3 preselection
the AdapterBindingPolicy is re-loaded in the transaction (ACTIVE, kind match,
    required_safety satisfied) and the partition target kind must match the policy
the partition's full authoritative target+profile is validated (missing profile,
    or incompatible allowed_targets) in the pure pre-commit resolver and re-proved
    in the authority transaction, BEFORE any authority commit or materialization
ResolvedProvisioningEvidence is produced only by the internal resolver; the
    provisioning-producer feature narrows the default surface (not a proof)
a DECLARED label never satisfies a security class
a B3Candidate / LogicalAgentTypeBinding is never physical eligibility
a ProvisioningBinding is write-once, scoped to one Incarnation, and freezes the
    exact (adapter_kind, adapter_binding_key) so recovery never re-derives it
a resident Incarnation + matching ProvisioningBinding is reused; a differing
    source/config/domain rolls over to a fresh Incarnation
a legacy Incarnation is adopted only if it has NO Execution history; otherwise it
    rolls over to a fresh Incarnation (physical hosting alone never proves the
    Core-opaque SourceConfig provenance)
a BindingSnapshot is created atomically with its Execution and is coherent with
    it (key, target/profile, provenance, config digest, requirement, capability,
    effective isolation/workspace/network), and never rewritten
a ProvisioningBinding records an enforceability capability; a BindingSnapshot
    records the per-execution effective policy separately (no hybrid PhysicalSafety)
the effective isolation is AgentType.requires OR Task.required, checked against
    the authoritative target before authority and re-proved at commitment
a typed Task cannot commit through the legacy create_execution path
the authority facade commits the deterministic selection, not a caller's pick
the selected SourceConfig carries a required source-integration materialization
    digest bound to the exact physical domain (also the ExternalRef attestation)
the existing-vs-new acquisition decision is Scheduler-owned (spec 06 order)
the authoritative reads re-prove the safety relation between the policy, the
    binding capability, and the snapshot effective policy
the materialization digest is part of Incarnation qualification and the frontier
    digest is canonical and disposition-complete
eligibility uses a pure attest; the winner's claim/binding commits first and then
    the winner is prepared under that durable identity; losers are never prepared
attest and prepare are PURE/read-only: no physical side effect before the
    Execution (M6-B never creates a physical environment; the exact adapter
    materializes the descriptor inside start_execution, so M5 owns the lifecycle)
the prepared descriptor's re-derived digest must exact-match the committed
    attested_materialization_digest; attested_materialization_digest is a canonical
    sha256:<64 lowercase hex> value and the mismatch error is redacted
the descriptor is causally bound to the launch and to the exact adapter by an
    OPTIONAL provisioning protocol identity proven at eligibility and re-checked at
    handoff; an adapter that declares none is not provisioning-capable
the source integration is source-local: an active source unrouted by the
    composition root is ineligible, so one source's config is never interpreted by
    another's integration
a typed-handoff failure is classified (pre-start availability -> RESOURCE_UNAVAILABLE,
    authority loss -> recovery, durable corruption/persistence -> fatal), and every
    fallible composition check precedes the Execution commit
the realized M4 retention must be contained in the source/config effective
    lifecycle (proved in the resolver and the authority transaction)
the enumeration (attest) deadline and the winner (prepare) deadline are distinct
    absolute endpoints; each integration call is bounded and re-qualified
claim authority is re-qualified before and after prepare using the full
    AuthoritySnapshot + Claim-identity check (not a lease-only read)
a pure prepare/handoff failure is classified pre-start: static/liveness ->
    RESOURCE_UNAVAILABLE, authority loss -> left to recovery, fatal persistence ->
    propagated; the typed handoff error carries no Task failure class
the handoff re-qualifies the current installed enforceability against the committed
    capability (weakening -> no Execution)
fallback happens only on explicit candidate-local ineligibility (narrow
    ContractError set); an ambiguous selection is task-terminal and a global
    authority-snapshot failure (StaleAuthority) re-resolves the whole acquisition
deterministic selection runs on the fully hard-eligible set: credential-bearing
    configs and wrong-partition-target adapter_kind candidates are filtered before
    ranking
a config that declares credential_refs is ineligible (no credential authority)
binding_ref resolves within the policy's adapter_kind
the authority acquisition stays daemon-internal (pub(crate)) until wired
the acquisition carries its winner candidate out; the daemon-internal handoff
    freezes the snapshot-bearing Execution and stops at the assembled start request
restart recovery validates the Execution snapshot marker/child coherence
selection is fenced by an active-candidate-frontier digest checked at commitment
reconcile_pool validates binding coherence and aborts on marker/child mismatch
every durable UNIQUE identity is guarded against INSERT OR REPLACE
after the BindingSnapshot commits there is no source fallback
a missing exact binding is a recovery/configuration failure
a candidate-local ineligibility skips only that candidate; catalog corruption
    fails the whole resolution
the durable store and logs carry no provider secret material
V0.1 capacity counts and retires only untyped LogicalAgents; MOVE_CAPACITY moves
    only UNBOUND agents, so a BOUND typed agent is never relocated by a legacy
    capacity command (ADR-0009)
```

## Mechanical enforcement

```text
provisioning_bindings / binding_snapshots
    BEFORE UPDATE -> abort
    BEFORE DELETE -> abort
    BEFORE INSERT -> abort on duplicate primary key
    BEFORE INSERT -> abort on duplicate incarnation_id / execution_id
                     (covers the INSERT OR REPLACE unique-conflict path)
    require a PROVISIONED incarnation / SNAPSHOT execution parent
incarnations / executions
    BEFORE INSERT -> abort on same-id and same-unique-identity replacement
    incarnations.provisioning_mode cannot downgrade
    executions.binding_snapshot_mode cannot downgrade
reads read the parent marker first, so a PROVISIONED/SNAPSHOT parent without its
    child row is corruption (never None) and a LEGACY/NONE parent with one fails
reads re-encode the canonical document and cross-check every relational column
```

## Reused unchanged

M6-B.4 adds no semantics to, and does not redefine, the frozen substrate:

```text
M5   Task / Attempt / Lease / Result authority, claim fencing, Execution
     commitment, AdapterKind / AdapterBindingKey freezing, resolve_exact
     recovery, ExecutionAdapter physical-only contract, deadlines, supervision,
     writer safety, process singleton, recover-before-dispatch
M6-A Generation, RawWorkIntent, CompiledWorkProposal, GenerationTaskBinding,
     RootSemanticControl, IntentIngress, atomic admission
M6-B.1 the four relations, AgentType purity
M6-B.2 immutable exact-revision catalog; validated reads; disposition overlays
M6-B.3 typed admission, requirement/binding durability, matching semantics
```

## Negative space

M6-B.4 deliberately does not implement:

```text
credential resolution / availability / brokering              (B.5; B.4 fails
    closed for any config that declares credential_refs)
enforced sandbox-policy / security-class capability import     (B.5)
concrete source-specific SourceConfigIntegration                (interface
    provided; a deployment supplies the real integration)
typed cold/revivable matching, MemoryCapsule, Transform
typed dispatch wiring into the running ControlLoopService
full typed-population targets                              (D-TOPOLOGY remainder)
v7 -> v8 in-place migration                                (D-DB-MIGRATE)
```

## Audit and resolution

M6-B.4 was hardened through twenty-one independent freeze-audit rounds (every one
`P0=0`; each `REQUEST_CHANGES` was closed in place), followed by a deliberate
post-audit handoff-contract step. All findings are resolved. The durable
decisions are recorded in
[ADR-0008](../../decisions/0008-m6b-typed-provisioning-acquisition.md) (items
1–70) and reflected in the invariants above. The corrections with the most
architectural weight were:

- Credentials are not a caller-minted authority fact: B.4 fails closed for any
  config that declares `credential_refs` (resolution/availability/brokering are
  B.5), and the public credential seam was removed.
- Selection follows the frozen spec 06/07 order with hard filters + continuity; a
  tie on the unmodelled availability/cost dimensions fails closed as
  `SelectionAmbiguous` (no lexical tie-break), and ranking runs on the fully
  hard-eligible set.
- SourceConfig body eligibility is `OpaqueJson` (Core validates the body digest)
  or an `ExternalRef` attested by the source integration under the exact physical
  domain; an unattested `ExternalRef` is ineligible.
- `attest` is pure and used for eligibility; the side-effectful, idempotent
  `materialize` runs only for the durably committed winner under the exact frozen
  identity and must return the committed digest. One absolute `AdapterDeadline`
  bounds every call and is re-qualified after it.
- The acquisition returns its winner candidate; the daemon-internal handoff
  freezes the snapshot-bearing Execution and stops at the assembled start
  request.
- Materialization settlement splits on whether the side-effectful invocation was
  entered: a provably side-effect-free pre-invocation failure is
  `RESOURCE_UNAVAILABLE`; any failure after the invocation began is
  physical-writer-unknown and is suspended as `WRITER_QUIESCENCE_UNKNOWN`
  (ADR-0008 item 51).
- `materialization_digest` is a constrained canonical `sha256:<64 hex>` value
  (`MaterializationDigest`), and the mismatch error is redacted (ADR-0008 item
  52).
- The full target+profile is validated before any authority commit or
  materialization, so a static profile misconfiguration cannot be promoted into a
  physical-quiescence incident (ADR-0008 item 53).
- Legacy `MOVE_CAPACITY` moves only `UNBOUND` agents, matching ADR-0009, so a
  typed agent is never relocated across its frozen B.3 placement gate (ADR-0008
  item 54).
- The realized M4 retention is re-proved against the source/config lifecycle
  (ADR-0008 item 57); the handoff re-qualifies the current installed enforceability
  (item 62). Round 20 replaced the side-effectful materialization with a pure
  `prepare` and moved the physical work into `ExecutionAdapter::start_execution`
  (ADR-0008 items 63–67): the descriptor is bound to the exact adapter by a proven
  protocol identity, the digest is renamed `attested_materialization_digest`, the
  attest/prepare deadlines are separate, and pre-start failures are classified
  (static → `RESOURCE_UNAVAILABLE`, authority loss → recovery, persistence →
  fatal). The pre-Execution `WRITER_QUIESCENCE_UNKNOWN` machinery (items 55, 60,
  61 as originally written) is withdrawn. Round 21 added source-local integration
  routing (ADR-0008 item 68), classified the typed-handoff failure and moved every
  fallible check before the Execution commit (item 69), and made
  provisioning-capability optional and honest (item 70).

## Verification (local)

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p agentype-storage-sqlite --features test-support  # b3 40 / b4 34 / catalog 32 / frontier 49 / store 4 / m4_kernel 71 / topology 16 / recovery 11 / reconciliation 5 / supervision 12 / outbox 17
cargo test -p agentype-runtime --lib             # provisioning_resolver 23; 258 passed
cargo test --doc -p agentype-public-api-boundary # 33 probes
```

The storage integration targets run locally with `--features test-support` (with
`TEMP` redirected onto the workspace). The only local failures are the daemon /
process-lock runtime unit tests, which need OS-temp and process-lock facilities
unavailable in the session sandbox; they are environment-only and pass in CI.

## Baselines

```text
M6-B.3 frozen base (main):
cbd5a7518395247dc7abfef996eb307e7a526f25

SCHEMA_VERSION = 8 at M6-B.4 implementation.
```
