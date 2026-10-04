# RIIR M6-B.2 — Agent Contract Catalog Freeze

Status: Historical Report
Applies to: V0.2 / M6-B.2 closure (PR #22)
Canonical path: `docs/reports/v0.2/riir-m6b.2-agent-catalog-freeze.md`
Not a specification.

This is a milestone boundary marker. It records that the M6-B.2 durable Agent
Contract catalog is closed and frozen on top of the frozen M6-B.1 ontology, and
that it satisfies its freeze gate. It adds no feature and supersedes no
specification.

## Status

```text
M5 execution/runtime substrate     FROZEN
M6-A semantic frontier kernel      FROZEN
M6-B.1 AgentType ontology          FROZEN
M6-B.2 durable catalog (schema v6) FROZEN
```

The next design/implementation phase is M6-B.3 (TaskAgentRequirement / typed
admission / LogicalAgent matching), which is not specified here.

## Frozen surface

```text
SCHEMA_VERSION = 6

immutable revision content tables
    capability_definitions
    adapter_binding_policies
    agent_types
    spawn_sources
    source_configs

mutable disposition overlays
    adapter_binding_policy_dispositions
    agent_type_dispositions            PUBLISHED <-> DEPRECATED (forward only)
    spawn_source_dispositions          ACTIVE -> DRAINING -> DISABLED
    source_config_dispositions         ACTIVE -> DRAINING -> DISABLED

canonical encoding                 agentype-contract/1
content digest                     sha256:<64 lowercase hex>
canonical format owner             agentype-agent-contract::canonical
validated read owner               agentype-storage-sqlite::catalog

Kernel catalog surface
    publish_capability_definition
    publish_adapter_binding_policy
    publish_agent_type
    publish_spawn_source
    publish_source_config
    load_capability_catalog
    get_agent_type / get_spawn_source / get_source_config / get_adapter_binding_policy
    get_source_config_revision / get_source_config_mode / get_source_config_locator
    resolve_agent_type_selector
    set_agent_type_status / set_spawn_source_status
    set_source_config_status / set_adapter_binding_policy_status
```

## Frozen invariants

```text
a published exact (ref, content digest) is immutable
republishing the same exact revision is idempotent
a different digest for an existing exact revision fails closed
the canonical document is the single authority for a revision
every read re-canonicalizes and cross-checks duplicated relational columns
a missing disposition overlay is corruption, never "not found" or repaired
dispositions only advance; repeating the current one is a durable no-op
a fresh publication persists the caller's initial disposition
catalog corruption never propagates into new immutable facts
pre-commit AgentType selectors resolve through the validated read only
SourceConfigRevision is an unforgeable, mode-consistent validated fact
SourceConfig location and content identity are distinct frozen fields
based_on provenance is validated and re-checked with is_valid_refinement
AdapterBindingPolicy keeps operator intent above physical adapter binding
```

## Mechanical enforcement

The immutable-revision / monotonic-disposition boundary is enforced by SQLite
itself, not only by the Kernel transaction API:

```text
immutable revision rows
    BEFORE UPDATE  -> abort
    BEFORE DELETE  -> abort

disposition overlays
    BEFORE DELETE  -> abort               (overlay lifetime == revision lifetime)
    BEFORE INSERT  -> abort on duplicate  (covers INSERT OR REPLACE conflict path)
    BEFORE UPDATE  -> abort on primary-key rewrite (identity immutable)
    BEFORE UPDATE OF status -> abort on status reversal
```

Direct SQL therefore cannot mutate frozen content, reverse a disposition, or
delete and recreate an overlay to resurrect a prior state.

## Validated read contract

```text
stored canonical JSON
    -> recompute content digest
    -> strict decode
    -> canonicalize against the exact CapabilityCatalog
    -> re-encode
    -> byte equality with the stored document
    -> cross-check every duplicated relational column
```

`SourceConfigRevision` is the single validated SourceConfig read authority; it
has private fields, no public constructor, read-only accessors, and a `body`
accessor dispatched on the validated body mode. The only production constructor
is `load_source_config_revision`.

## Canonical format

One canonical format namespace (`agentype-contract/1`) with Core-computed
`sha256:` digests. Object keys are recursively sorted so the bytes do not depend
on `serde_json`'s map feature configuration. Claims and credential references are
sorted/deduped, `Bool(false)` is normalized to omission only after exact catalog
resolution and shape validation, the surviving claim per exact `CapabilityRef`
is unique, and golden digest vectors pin every document kind (capability
definition, AgentType, SpawnSource, SourceConfig with/without locator,
AdapterBindingPolicy, and the opaque JSON body digest).

## Reused unchanged

M6-B.2 adds no semantics to, and does not redefine, the frozen substrate:

```text
M5   Task / Attempt / Lease / Result authority, claim, Execution commitment,
     AdapterKind / AdapterBindingKey freezing, resolve_exact recovery,
     ExecutionAdapter physical-only contract, supervision, writer safety,
     process singleton, recover-before-dispatch
M6-A Generation, RawWorkIntent, CompiledWorkProposal, GenerationTaskBinding,
     RootSemanticControl, IntentIngress, atomic admission
```

Catalog publication cannot create a Task or expand a Generation. Catalog state
is durable configuration authority, not provisioning authority.

## Negative space

M6-B.2 deliberately does not implement:

```text
TaskAgentRequirement / typed admission        (M6-B.3)
LogicalAgent matching / ranking               (M6-B.3)
ProvisioningBinding / BindingSnapshot         (M6-B.4)
AdapterRegistry.resolve_exact launch wiring   (M6-B.4)
production ResolvedProvisioningEvidence       (M6-B.4)
CredentialRef resolution / brokering          (M6-B.5)
sandbox-policy ordering / intersection        (M6-B.5)
v5 -> v6 in-place migration                   (D-DB-MIGRATE, unresolved)
```

`D-DB-MIGRATE` remains unresolved; M6-B.2 uses a fresh schema-v6 database, and
older files are rejected at open (fail closed). Cross-revision AgentType
compatibility remains deferred (`D-TYPE-REV-COMPAT`).

## Correctness authority

M6-B.2 correctness remains defined by the V0.2 normative specs and architecture
documents. This report does not supersede them.

```text
spec 06 agent-type-and-matching          canonical identity, refinement
spec 07 spawn-source-and-adapter-contract revision/disposition ownership
spec 13 storage-and-transactions         schema-v6 authority, mechanical guards
spec 16 conformance-tests                M6-B.2 conformance cases
spec 17 deferred-open-questions          D-DB-MIGRATE / D-TYPE-REV-COMPAT open
architecture v0.2/m6b-agent-contract-and-spawn-resolution
```

## Freeze-gate resolution

The catalog was reviewed across several rounds, each returning REQUEST CHANGES
with narrow closures, before the final PASS:

```text
claim canonicalization determinism          closed
ExternalRef locator / digest separation     closed
initial disposition fidelity                closed
canonical byte independence                 closed
read-side digest integrity                  closed
read-side re-canonicalization               closed
ConfigDigest canonical grammar              closed
single validated SourceConfig read          closed
AdapterBindingPolicy value invariant        closed
selector validated-read authority           closed
dependent-reference validation              closed
all-overlay setter idempotency              closed
SQLite immutable-revision guards            closed
SQLite overlay lifetime / identity guards   closed
INSERT OR REPLACE resurrection path         closed
```

The final audit of head
`f5b03bb595f5d8806dca812d290cb1dd12f80438` returned:

```text
P0 = 0
P1 = 0
P2 = 0

AUDIT: PASS
MERGE: PASS
```

This freeze report is a documentation commit added after that audited head; it
does not alter the frozen behavior.

## Baseline

```text
M6-B.1 frozen base (main):
9b1aa8f1bf72032a1f7470d7c4e7ea45fc23ba39

Final audited M6-B.2 head:
f5b03bb595f5d8806dca812d290cb1dd12f80438

Exact-head CI (run 37207374071):
rust / ubuntu-latest    PASS
rust / windows-latest   PASS
Python 3.11/3.13 (ubuntu/windows) PASS

SCHEMA_VERSION = 6 at M6-B.2 freeze.
```

## M6-B.3 entry boundary

M6-B.3 may treat the schema-v6 catalog as the frozen exact-revision authority
and MUST NOT reopen its durable identity or disposition design:

```text
catalog existence          != Task eligibility
SpawnSource ACTIVE         != physical eligibility
SourceConfig ACTIVE        != enforcement proof
AdapterBindingPolicy       != exact AdapterBinding
DECLARED / ENFORCED label  != ResolvedProvisioningEvidence
```

Physical provisioning authority, exact binding, credentials, and enforcement
evidence remain M6-B.4/M6-B.5 concerns.
