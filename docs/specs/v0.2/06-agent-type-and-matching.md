# 06 — AgentType and Matching

Status: Normative
Canonical path: docs/specs/v0.2/06-agent-type-and-matching.md

## AgentType

AgentType MUST represent organizational, informational, security, lifecycle,
and continuity semantics.

AgentType MUST NOT be defined by model name, provider, terminal, price tier,
prompt alias, or PoolPartition identity.

Suggested descriptor categories (representation IMPLEMENTATION-DEFINED):
type_id, revision, affinity, capabilities, sandbox policy ref, lifecycle,
continuity, anchor constraint, spawn requirements, information-function set
and memory policy, transform policy, optional `based_on_type_id`.

Revisions MUST be immutable once published. A committed LogicalAgent, and any
admitted Task agent requirement, MUST pin an exact immutable
`AgentTypeRef(type_id, revision)`. `latest`, ranges, and aliases are pre-commit
selectors only and MUST resolve to an exact revision before any durable binding
is written. Cross-revision compatibility and Transform across revisions remain
DEFERRED (D-TYPE-REV-COMPAT); M6-B v1 performs no automatic cross-revision
upgrade (D-TYPE-REV-PIN resolved to exact-pin).

## Relations (MUST remain four)

Implementations MUST provide distinct predicates:

- `can_execute(AgentType, TaskRequirement)`
- `can_provision(SpawnSource, SourceConfig, AgentType, imported enforcement evidence)`
- `more_specific_for(A, B, TaskRequirement)`
- `is_valid_refinement(Base, Derived)`

They MUST NOT be collapsed into one subtype/inheritance operator.

A broad SpawnSource MAY provision a narrower AgentType if it can enforce it.
A more specific AgentType MAY be preferred for assignment over a broader
compatible one. These directions differ and MUST NOT be inverted.

Concrete encodings of the four relations are FROZEN in M6-B.1 for the defined
dimensions (D-TYPE-REL resolved; sandbox-policy order/intersection remains
deferred under D-SANDBOX-ORDER / D-SANDBOX-INTERSECTION) and implemented in
`agentype-agent-contract`:

- `can_execute` checks the information function, task affinity (an agent with
  the `Any` affinity accepts any requirement; `Only(S)` requires the Task tags to
  be a subset of `S`), workspace/network policy, continuity minimum, sandbox
  policy, anchor, and budget. For `Ability` task capabilities the AgentType
  envelope must cover the requirement; `Restriction` task capabilities need not
  be pre-advertised by the AgentType, but where the AgentType already carries the
  same restriction the pure contract join MUST exist (an incompatible pair fails
  closed here rather than only at provisioning). The physical enforcement of the
  joined restriction is still proven at provisioning.
- There is exactly one authoritative sandbox vocabulary. Restrictions are
  expressed as security-class capabilities (evidence-proven) plus workspace,
  network, attempt isolation, and the evidence-enforced `SandboxPolicyRef`; the
  contract has no free-form `tools`/`roots`/`visibility`/permission lists that
  look like security but have no enforcement path. A security/authority
  restriction MUST NOT exist without a proof path, and a derived type MUST NOT
  become narrower without an equal-or-narrower mechanically enforceable policy.
  In M6-B.1 the sandbox policy is pinned by **exact reference** (equality);
  ordering two policy references and intersecting AgentType with Task policy is
  deferred (`D-SANDBOX-ORDER` / `D-SANDBOX-INTERSECTION`), so the coarse
  workspace/network/isolation fields and restriction capabilities are the
  B.1-composable restrictions.
- `can_provision` checks that the config belongs to the exact source revision,
  source/config activity, lifecycle coverage and the continuity minimum against
  the **effective** envelope (a config may narrow, never widen, the source's
  `lifecycle_modes`/`continuity_modes`; the effective set is the config override
  when present, else the source envelope; continuity requires a mode at least as
  strong as required), every required
  capability **at its exact revision** (the source envelope is the provisionable
  ceiling; config/source declarations MUST stay within it), the imported
  `ENFORCED` evidence for security-class capabilities, mechanically enforceable
  workspace/network modes and isolation, the pinned sandbox policy, and the
  policy-bound evidence **subject** (the exact `SpawnSourceRef`, `SourceConfigRef`
  and config digest it was resolved for). A capability's matcher kind and
  security class are defined once by a canonical `CapabilityCatalog`; a
  `DECLARED`/`ENFORCED` claim label is never a proof, and restrictions MUST NOT
  be inferred from a stronger capability.
- `can_provision_task` is the contract/sandbox eligibility predicate and a
  **mandatory eligibility conjunct**; it is NOT the complete physical candidate
  eligibility decision. The conjunction
  `can_execute(agent, task) && can_provision(agent, source, config, evidence)` is
  **necessary but not sufficient**: the imported environment MUST also be able to
  enforce the Task's effective (stricter) workspace/network, attempt isolation,
  and pinned sandbox policy, not merely the AgentType ceiling. Effective attempt
  isolation is `AgentType.requires_attempt_isolation OR
  Task.required_attempt_isolation` (a Task may tighten it; the AgentType flag
  stays a per-task requirement), and when effective it MUST be physically
  enforceable. Full eligibility additionally
  requires an active adapter policy, satisfied `required_safety`, a coherent
  adapter kind/binding, a currently resolvable exact binding, and available
  required credential refs (B.4/B.5); no second implicit eligibility relation may
  be introduced for these. For `Restriction` capabilities the
  effective value is the deterministic join of the AgentType and Task values
  (`Bool` OR, `Set` union, `Ordered`/`Quantity` max, `Exact` equality). `Bool`
  uses presence semantics (`false` absent, `true` present); a present value
  satisfies an absent requirement but not the converse, so refinement, join, and
  satisfaction stay mutually monotone. `Bool(false)` means absence only after the
  catalog has resolved the exact capability and confirmed it is a `Bool`
  capability with a matching value shape; a `Bool(false)` for an unknown
  capability or a non-`Bool` definition MUST fail closed, never silently vanish.
  Once so normalized, an omitted entry and `Bool(false)` are the same state in
  every relation (`can_execute`, `more_specific_for`, `is_valid_refinement`) and
  in validation; the durable canonical encoding is omission, so `{}` and
  `{cap: Bool(false)}` MUST NOT be persisted as two distinct contract forms. The
  joined value is proven by imported
  evidence for security classes and by the effective source/config functional
  value otherwise — `SecurityClass` keeps
  controlling proof authority independently of polarity. Every Task capability
  value MUST match its catalog definition shape and fail closed otherwise, even
  for `Restriction` capabilities the AgentType does not pre-advertise. B.3/B.4
  MUST include `can_provision_task` as a mandatory conjunct, not the bare
  conjunction.
- `more_specific_for` is defined only once both types are executable, over the
  semantic/authority/scope dimensions (affinity, budget, lifecycle, information
  functions, workspace/network, isolation, sandbox policy, anchor, and capability
  values). It MUST be strict (two types with equivalent authority are
  incomparable) and MUST NOT rank by nominal inheritance depth. `continuity`
  strength is deliberately **not** part of semantic specificity: it is a
  candidate-ranking dimension applied after compatibility (a stronger guarantee
  lets a type execute more continuity-requiring Tasks).
- `is_valid_refinement` enforces monotonic narrowing of affinity, budget,
  information functions, sandbox policy, anchor, security policy, and capability
  values; a derived type MUST NOT widen the lifecycle mode set, weaken required
  isolation, or **weaken a continuity
  guarantee** (`derived.continuity >= base.continuity` — strengthening is
  allowed). Affinity is an allowed-tag ceiling with an explicit top: `Any` is
  general and unconstrained, `Only(A) ≤ Only(B)` iff `A ⊆ B`, and
  `Any ≤ Only(B)` is false. Capability values use the catalog-owned **polarity**,
  which is an independent field: `SecurityClass` decides whether imported
  evidence is required, while `CapabilityPolarity` decides which direction
  narrows. An `Ability` capability narrows by shrinking — a derived type may
  remove or lower it but never add or raise it; a `Restriction` capability
  narrows by growing — a derived type may add or raise it but never drop or
  lower it. Functional/Authority and Sandbox/Continuity are only conventional
  defaults, not bindings. The executable-Task-set-subset
  invariant holds for the ability/scope dimensions; continuity is deliberately a
  guarantee-strengthening dimension outside it. The `lifecycle` set is a
  **required source envelope** (the source must support every listed mode), not
  a set of alternative instantiation choices.

`SourceConfigRef` + revision + `ConfigDigest` are the Core-visible **selection
identity** of an opaque source-private configuration. The configuration body or
locator (model/provider/CLI/config-file semantics) belongs to the source
integration and is not part of AgentType semantics; Core MUST NOT interpret it,
and it need not be stored in `agentype-agent-contract`.

## Catalog persistence (M6-B.2)

AgentType, SpawnSource, SourceConfig, capability definitions, and
AdapterBindingPolicy revisions MUST be persisted immutably. Each published exact
revision has a canonical content encoding and a Core-computed content digest:

- republishing the same exact `(ref, content digest)` is idempotent; a different
  canonical content for an already-published exact revision MUST fail closed;
- the canonical encoding MUST be permutation-stable (set-like sequences sorted
  and deduped), MUST recursively sort object keys so the bytes are independent
  of the JSON library's map feature configuration, and MUST encode `Bool(false)`
  as omission, so one logical revision has exactly one digest;
- at most one claim per exact `CapabilityRef` may survive; an `ENFORCED`
  declaration supersedes `DECLARED` ones, and two surviving declarations with
  the same value but a different `declaration_provenance_ref` are ambiguous and
  MUST fail closed (provenance is diagnostic, never an input-order tie-break);
- a derived AgentType's `based_on` provenance MUST be verified against the base
  loaded through its validated read (canonical content, mirror, and present
  overlay), and the derived contract MUST pass `is_valid_refinement` before it is
  published; a corrupt base MUST fail closed and MUST NOT authorize a new
  immutable revision. A new revision that references another catalog revision
  (AgentType `based_on`, SpawnSource `adapter_policy`) MUST likewise resolve that
  dependency through its validated read, not merely check that the row exists;
- publication and deprecation status is a mutable **disposition overlay** that
  MUST NOT enter the revision content or its digest; a fresh publication MUST
  persist the caller's initial disposition (never silently substitute
  `ACTIVE`), dispositions only advance, repeating the current disposition MUST be
  an idempotent no-op that preserves the transition timestamp, and a revision row
  whose overlay is missing is corruption that MUST fail closed with one meaning
  everywhere (reads, setters, idempotent republish, and selector resolution). An
  existing exact revision's overlay MUST NOT be created or repaired by
  publication. The immutable-content and monotonic-disposition boundary MUST
  also be enforced mechanically by SQLite triggers, not only by the Kernel API;
- a read at the catalog boundary MUST verify the stored content digest against
  the stored canonical document, **re-canonicalize** the decoded record (decode
  -> canonicalize against the catalog -> re-encode -> byte equality; a
  self-consistent but non-canonical row MUST fail), and cross-check every
  duplicated relational column against that document, so the digest is a
  durability witness rather than publication-time metadata and no mirror column
  becomes a second authority;
- pre-commit `AgentTypeSelector` resolution MUST use the same validated read for
  its published set, so a corrupt published revision (including a missing
  overlay) fails the whole lookup closed rather than remaining selectable or
  causing `Latest` to fall back through a weaker path;
- an idempotent republish of an existing exact revision MUST re-run the
  validated read before reporting success, and the AdapterBindingPolicy durable
  boundary MUST enforce its own whole-record invariant (non-blank
  `adapter_kind`/`binding_ref`) on both publication and decode;
- a SourceConfig's complete durable revision identity is the
  `SourceConfigRevision` (metadata + body mode + `ExternalRef` locator); a
  metadata-only relation MUST NOT be used or named as durable revision identity;
- a SourceConfig `config_digest` MUST have the canonical grammar
  `sha256:<64 lowercase hex>`, and a single validated `SourceConfigRevision` read
  MUST cross-check its body `config_mode`/`config_payload_json`/`config_locator`
  columns. A locator and a digest are distinct fields whose values are
  source-private and may legitimately coincide, so an `ExternalRef` locator MUST
  NOT be required to differ from its digest. An `OpaqueJson` body is durable
  non-secret material; provider/vendor secrets belong behind `ExternalRef`.

## Refinement monotonicity

A Root-created derived type MUST NOT enlarge authority.

MUST hold:

- DerivedAffinity ≤ BaseAffinity (allowed-tag ceiling)
- DerivedBudget ≤ BaseBudget
- Derived lifecycle mode set ⊆ Base lifecycle mode set
- Derived capability values follow catalog polarity: Ability values must not
  exceed base; Restriction values must not fall below base
- Derived continuity MUST NOT weaken the base guarantee
- Derived workspace/network/attempt-isolation MUST NOT weaken base
- Derived sandbox policy MUST NOT widen
- Derived anchor MUST satisfy base anchor constraints

Anchor MUST satisfy base anchor constraints. There is no free-form
permission/visibility/tools/roots field: those concerns are capabilities
(evidence-proven for security classes) or the sandbox policy reference.

## Information functions

EXPAND, COMPRESS_POSITIVE, and COMPRESS_NEGATIVE are information operations.

They MUST NOT be a required mutually exclusive AgentType taxonomy and MUST
NOT be a retention mode.

An AgentType MAY declare zero, one, or multiple information functions.
A single small-domain LogicalAgent MAY carry both positive and negative
maintenance.

Lifecycle (short-lived explore-and-retire vs long-lived continuity) is
orthogonal.

Concrete set/trait encoding is RESOLVED in M6-A (D-INFO-FN): the closed
three-value `InformationFunction` enum (`EXPAND`, `COMPRESS_POSITIVE`,
`COMPRESS_NEGATIVE`) with state-gated admission. Implementations MUST NOT ship
`enum AgentKind { Positive, Negative, Explorer }` as the type identity.

Positive and negative semantic memory MUST be scoped and evidence-backed.
Negative entries MUST retain applicability conditions. Implementations MUST
NOT promote “Y failed under C” into “Y must never be used” without scope.

## Matching preference (semantic order)

1. exact / most-specific compatible resident agent
2. compatible narrower anchored type
3. compatible broader/general type
4. cold/revivable compatible logical agent
5. provision a new logical agent from an eligible SpawnSource

Ranking SHOULD consider compatibility, affinity specificity, capability
surplus, anchor distance, continuity value, and warm/revival cost.
Ranking MUST NOT be by nominal inheritance depth.

V0.1 partition matching ([11](11-pool-topology.md)) remains for kernel
conformance until typed matching is implemented in M6.

## Typed admission and existing-agent matching (M6-B.3)

An admitted Task MAY carry a durable `TaskAgentRequirement`. Its `required_type`
is a **mandatory exact immutable `AgentTypeRef`**: `admit_typed_proposal` resolves
a pre-commit selector to an exact revision before commitment, and there is no
durable unpinned typed requirement (`D-TYPE-REV-PIN`). It creates the Task, its
`GenerationTaskBinding`, and the requirement atomically; a Task whose requirement
would widen its Generation policy MUST fail closed. The requirement never selects
a `SpawnSource` and never starts physical work.

Typedness is a positive durable fact: the Task carries an
`agent_requirement_mode` (`LEGACY`/`TYPED`) fixed at admission, and any Task that
is not `LEGACY` MUST be invisible to the legacy untyped consumer/claim path. A
lost requirement row is therefore corruption, never a silent downgrade to a
legacy Task. Until a typed authority-bearing acquisition path exists (M6-B.4), a
typed Task stays durable `QUEUED` and acquires no Attempt, Lease, or Execution.

A Generation carries a `policy_mode` (`NONE`/`POLICY`) fixed at creation. A
`POLICY` Generation MUST NOT be admitted through the legacy `admit_proposal` path
(fail closed), and a missing policy row for a `POLICY` marker is corruption, never
an unconstrained Generation. A `NONE` Generation preserves legacy admission
unchanged. Folding a Generation policy is the spec 10 intersection: the Generation
is an authority ceiling, so a Task requiring more workspace/network authority than
the ceiling MUST fail closed, and a Task requiring less MUST keep its stricter
value (the Generation never widens a Task). Full capability and sandbox
ordering/intersection remains B.5.

Existing-agent matching composes two hard gates: the frozen M5 placement rules
(exact partition, task tags subset of the agent's concrete tags, and the same
workstream for `Required` continuity) **and** the AgentType contract
(`can_execute`). Only M5 `READY`, unassigned, bound agents are candidates;
non-READY states are not treated as cold/revivable; an unbound agent is never
returned. A `Preferred` continuity ranks the same workstream first. The exact pin
is a matching anchor, not a ceiling: among eligible candidates the relation to the
pin orders as exact, then compatible narrower/refinement types, then
equivalent/incomparable compatible types, then compatible broader/general types;
within a relation class, candidate-vs-candidate `more_specific_for` dominates
before the `Preferred` workstream, continuity strength, availability, and stable
identity tie-breaks. Ranking MUST NOT use nominal inheritance depth. Only
exact-selector replay is idempotent; an already-committed exact pin remains
replayable after deprecation, while a `Latest` retry after the catalog advances
MUST fail closed. A fresh `logical_agent_type_bindings` row requires a currently
`PUBLISHED` revision. New-agent provisioning from an eligible SpawnSource, full
physical eligibility (`can_provision_task`), and credentials remain
M6-B.4/M6-B.5.
