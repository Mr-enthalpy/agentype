# M6-B Agent Contract and Spawn Resolution

Status: Architecture
Applies to: V0.2 / M6-B
Canonical path: `docs/architecture/v0.2/m6b-agent-contract-and-spawn-resolution.md`
Not a specification. Normative contracts remain
[spec 06](../../specs/v0.2/06-agent-type-and-matching.md) and
[spec 07](../../specs/v0.2/07-spawn-source-and-adapter-contract.md).
Runtime execution and storage substrate remain frozen from M5.8; semantic
admission remains frozen from M6-A
([m6a-result-carried-intent-binding](../../reports/v0.2/m6a-result-carried-intent-binding.md)).

This note grows across the M6-B stages. It currently records **M6-B.1**, the
pure AgentType ontology and the four matching predicates.

---

## Frozen phrases

AgentType expresses contract, never vendor configuration.

The four relations stay four; they MUST NOT collapse into one subtype operator.

Hard constraints are filtered before any candidate is ranked; cost never
overrides correctness or security.

`DECLARED` never produces an isolation or authority proof.

Agentype must preserve unmodeled agency outside itself.

---

## 1. Ownership chain

```text
M6-A                    owns semantic commitment (WHAT becomes real)
AgentType               owns semantic/security/lifecycle/continuity contract
LogicalAgent            owns durable semantic identity
SpawnSource             owns provisioning possibility
SourceConfig            owns source-specific configuration freedom
ProvisioningBinding     freezes one Incarnation's chosen provisioning
AdapterBindingPolicy    connects stable operator intent to runtime binding
BindingSnapshot         freezes one Execution's exact physical choice
ExecutionAdapter        owns physical environment mechanics
External environment    owns harness/model/provider/tools/credentials
```

M6-B answers *what contract must execute an already-admitted Task, and from
what physical source may it be materialized*. It never admits Task (M6-A) and
never creates a physical environment (M5).

---

## 2. Ontology (M6-B.1)

Pure value types in `agentype-agent-contract`; no SQLite, Tokio, Runtime, or
ExecutionAdapter dependency.

```text
AgentTypeRef              exact (type_id, revision); revisions immutable
AgentType                 published contract revision; optional based_on provenance
AgentTypeContract         information functions, capability envelope, affinity,
                          budget ceiling, security, lifecycle, continuity,
                          sandbox policy, anchor constraint
CapabilityRef             exact (capability_id, revision); private fields
CapabilityCatalog / CapabilityDefinition
                          the single authority for a capability's matcher kind,
                          security class, and refinement polarity; redefining a
                          revision fails closed
CapabilityValue           BOOL | SET | ORDERED | QUANTITY | EXACT
CapabilityClaim           (exact reference, value, assurance declaration, and a
                          diagnostic provenance ref; the ref is NOT a proof)
TaskRequirement           information function, required capabilities, affinity,
                          workspace/network, continuity, sandbox policy, anchor,
                          budget
SpawnSource               advertised lifecycle/continuity envelopes, the
                          provisionable functional envelope (ceiling), source-wide
                          declarations, status
SourceConfig              opaque source-private config: exact source ref,
                          identity, digest, config declarations and optional
                          lifecycle/continuity overrides (which MUST stay within
                          the source envelope), credential refs, status
AdapterBindingPolicy      stable alias -> physical binding, required safety
SandboxPolicyRef          exact reference to the full spec-10 sandbox vocabulary
PhysicalSafety            private, validated enforceable sets: isolation,
                          workspace modes, network modes
ResolvedProvisioningEvidence
                          imported enforcement facts bound to one exact candidate
                          tuple: SpawnSourceRef + SourceConfigRef + config digest +
                          AdapterPolicyRef; carries enforceable safety, enforced
                          sandbox policies, and enforced capabilities; NO public
                          production constructor in B.1
AgentTypeSelector         Exact(ref) | Latest(id), resolved to exact pre-commit
```

A capability's security class is defined once by the [`CapabilityCatalog`], not
per AgentType, so two AgentTypes cannot disagree about whether `network.lock@1`
is `Sandbox` or `Functional`. The physical adapter binding key is **not**
modeled here: B.4 consumes the canonical `agentype-execution-config`
`AdapterBindingKey`. `ResolvedProvisioningEvidence` has no constructor on the
supported default production surface in B.1 (it is available only via
`for_tests` under `test-support`). This is a **supported-surface** boundary, not
an unforgeable capability: a consumer that explicitly enables `test-support` can
still construct it, so the wording MUST stay "not on the supported default
production surface", never "unforgeable". B.4 owns the real production producer
(M5 imported-binding / safety authority) and MUST add a production compile-fail
boundary witness, exactly as `FrozenExecutionSafety` is owned by
`agentype-execution-config`.

`AgentType` MUST NOT be defined by model, provider, terminal, price tier, prompt
alias, `SourceConfig`, or `AdapterBinding`. `SourceConfig` is a **selection
identity, not a config body**: `SourceConfigRef` + revision + `ConfigDigest` are
the Core-visible identity, the source integration owns the actual
model/provider/CLI/config-file semantics out of Core, and the source-private
body/locator is not part of AgentType semantics and need not be stored in
`agentype-agent-contract`. If B.2 persists an opaque locator or body, Core MUST
never interpret it. Core recognizes identity, digest, credential references, and
config-specific capability declarations only. All numeric contract values are validated finite
non-negative newtypes (`Budget`, `Quantity`); every ref has private fields and a
validated constructor rejecting empty ids / zero revisions, so a durable digest
can never form from an invalid value.

---

## 3. The four predicates

### `can_execute(AgentType, TaskRequirement)`

```text
task information_function ∈ agent allowed_information_functions
task required_capabilities ⊆ agent capability envelope (under the matcher)
agent workspace/network policy >= task requirement (no narrowing)
agent continuity          >= task continuity minimum
task anchor requirement satisfies agent anchor constraint (None = unconstrained)
task budget               <= agent budget_ceiling
```

### `can_provision(SpawnSource, SourceConfig, AgentType, ResolvedProvisioningEvidence)`

```text
config.config_ref.source == source.source_ref        (exact revision)
source/config active
evidence.adapter_policy == source.adapter_policy     (adapter A's facts never fit source B)
evidence.subject == (source_ref, config_ref, config_digest)
                                                     (config A's evidence never covers config B)
agent.sandbox_policy (if pinned) ∈ evidence.enforced sandbox policies
config lifecycle/continuity overrides (if present) MUST be a subset of the source
effective lifecycle  = config override else source lifecycle_modes
effective continuity = config override else source continuity_modes
agent lifecycle  ⊆ effective lifecycle
effective continuity advertises a mode >= agent continuity
for each exact capability revision the agent requires:
    definition = CapabilityCatalog(reference)        (missing definition fails closed)
    functional: the source envelope is the provisionable CEILING; source and
                config declarations MUST stay within it; provided is the config
                declaration, else the source declaration, else the envelope
    security class: provided = evidence.enforced_capability(exact revision)
    value must satisfy the definition matcher
required isolation / workspace / network enforcement comes from evidence.enforceable_safety
```

`ENFORCED` is imported **evidence**, not an enum label: a source/config that
merely declares `ENFORCED` (or supplies a functional envelope) never satisfies a
sandbox/authority/continuity requirement. There is no string-keyed bypass, and a
capability claim at a different revision never satisfies a requirement.

### `more_specific_for(A, B, TaskRequirement)`

Only defined once both `can_execute`, and only when A is no wider than B on
*every* relevant dimension (information functions, affinity, budget,
**lifecycle**, workspace/network, isolation, sandbox policy, anchor, and
capability values) with at least one dimension strictly narrower. Two types with
equivalent authority are **incomparable**, not mutually more specific. Ranking
MUST NOT use nominal inheritance depth.

`continuity` is deliberately excluded: it is a candidate-ranking dimension
applied after compatibility, not semantic specificity. Two types differing only
in continuity are incomparable for `more_specific_for`.

### One authoritative sandbox vocabulary

There is no second, semantic-only representation of a security restriction:

```text
capabilities (evidence-proven for Authority/Sandbox/Continuity)
    -> the refinement polarity decides whether growing narrows (Restriction)
       or widens (Ability)
workspace / network / attempt_isolation
    -> ResolvedProvisioningEvidence.enforceable_safety
full filesystem/tool/visibility policy
    -> SandboxPolicyRef, enforced by imported evidence
```

The contract has no free-form `tools`/`roots`/`visibility`/permission lists: a
field that looks like a security restriction MUST have a proof path, a derived
type MUST NOT become narrower without an equal-or-narrower mechanically
enforceable policy, and prompt/semantic labels are never enforcement.

Capability polarity is a catalog-owned field **independent of the security
class**: `SecurityClass` answers "does this need imported evidence?" while
`CapabilityPolarity` answers "which direction narrows?". `Ability`
(Functional/Authority) narrows by shrinking — remove or lower, never add or
raise; `Restriction` (Sandbox/Continuity) narrows by growing — add or raise,
never drop or lower. The `lifecycle` set is a required source envelope (the
source must support every listed mode).

### `is_valid_refinement(Base, Derived)`

```text
DerivedAffinity   ≤ BaseAffinity (Any is the top; Only(A) ≤ Only(B) iff A ⊆ B)
DerivedBudget     ≤ BaseBudget
DerivedInformationFunctions ⊆ BaseInformationFunctions
DerivedLifecycle ⊆ BaseLifecycle (lifecycle mode set MUST NOT widen)
continuity MAY strengthen but MUST NOT weaken the base guarantee
anchor MUST satisfy base anchor constraint
sandbox policy MUST NOT widen (a base None may be pinned; a base Some must match)
workspace/network/required-isolation MUST NOT weaken base
capability values follow catalog polarity: Ability must not grow, Restriction
    must not shrink; matcher kind and security class are catalog-global
```

`continuity` is the one **provisioning-guarantee** dimension: strengthening it
is a valid refinement (`derived >= base`) even though it lets the type execute
more continuity-requiring Tasks. The executable-Task-set-subset invariant is
therefore scoped to the authority/scope dimensions. Continuity is **not** part
of `more_specific_for`; it is a candidate-ranking dimension applied after
compatibility.

### Physical eligibility seam

```text
can_execute(agent, task) && can_provision(agent, source, config, evidence)
    => necessary but NOT sufficient for physical eligibility

can_provision_task(agent, source, config, evidence, task)
    = the above
      AND evidence can enforce the Task's effective (stricter)
          workspace / network / pinned sandbox policy / continuity
```

`can_provision_task` is a mandatory eligibility conjunct (the contract/sandbox
eligibility predicate); it is NOT the complete physical candidate eligibility
decision. B.3/B.4 MUST include it, and MUST additionally require an active
adapter policy, satisfied `required_safety`, a coherent adapter kind/binding, a
currently resolvable exact binding, and available required credential refs
(B.4/B.5) — without inventing a second implicit eligibility relation. A source
that can realize the AgentType ceiling but not the Task's stricter policy is
ineligible; this is the composition rule that prevents an "both predicates pass,
but the sandbox cannot run the Task" hole.

---

## 4. Security class and assurance

```text
SecurityClass::Functional   a source/config declaration or envelope may suffice
SecurityClass::Authority    requires imported evidence
SecurityClass::Sandbox      requires imported evidence
SecurityClass::Continuity   requires imported evidence
```

`DECLARED`/`ENFORCED` on a claim is declaration metadata only. A source that
merely declares sandbox support produces no proof; only the imported,
policy-bound `ResolvedProvisioningEvidence` (produced by the adapter
integration, not assembled by a composition caller) satisfies a security class,
and its enforceable safety must realize the required isolation / workspace /
network.

There is deliberately **no** `assurance_satisfies`-style helper: no public API
may express "a claim label satisfies a security class". `Assurance` exists only
for catalog/diagnostic vocabulary and MUST NOT be consulted by any security
resolution.

---

## 5. M6-B.1 negative space

M6-B.1 does not implement:

```text
catalog persistence or schema v6                (M6-B.2)
TaskAgentRequirement / typed admission          (M6-B.3)
LogicalAgent matching / ranking engine          (M6-B.3)
ProvisioningBinding / BindingSnapshot           (M6-B.4)
AdapterRegistry.resolve_exact launch wiring     (M6-B.4)
CredentialRef resolution / brokering            (M6-B.5)
REST/gRPC wire API                              (unspecified)
```

Wild agency stays external: observing a harness-native subagent never yields a
LogicalAgentId, Generation membership, Lease, or Result authority.

B.2 obligations recorded here (not implemented in B.1): a published
`(ref, canonical content digest)` MUST be immutable — the same exact ref with a
different canonical content is an invariant violation, not a Rust caller
discipline; `config_digest` MUST be a validated canonical representation before
it enters schema v6; and the immutable catalog schema MUST enforce a canonical
claim representation (at most one value per exact `CapabilityRef`, or a
deterministic dedup rule) so an exact revision cannot encode in more than one
way; the durable encoding MUST normalize `Bool(false)` to omission so absence has
exactly one representation; publication MUST verify `based_on` provenance
(`derived.based_on ==
base.type_ref`) rather than trusting the field; and schema v6 MUST persist
immutable revision content separately from the mutable
`SpawnSourceDisposition`/config-disposition overlay, so status never enters the
content digest (spec 07 "Revision and disposition ownership").

B.4 binding-bridge obligations: `ResolvedProvisioningEvidence.adapter_kind` and
`AdapterBindingPolicy.required_safety`/`status` are not consumed by the B.1
relations; the launch bridge MUST consume them (exact binding, safety
intersection) so they do not remain dead metadata. The production evidence
authority fence is the Scheduler's internal resolution path, which may receive
evidence only from the trusted imported-binding/safety authority; there MUST NOT
be any production control API that injects caller-supplied evidence into an
authoritative resolver. A constructor being absent on the default surface is not
an authority proof, and B.4 MUST add a stronger conformance test than the
`test-support` visibility.

---

## 6. Inherited invariants relevant here

```text
INV-B1  AgentType purity
INV-B2  exact type revision; selectors resolve to exact pre-commit
INV-B3  no silent type mutation (type change is Transform)
INV-B4  source multiplicity (many AgentType <-> many SpawnSource <-> many config)
INV-B5  SourceConfig opacity
INV-B8  filter before score
INV-B9  enforced != declared
```

M5-B* (adapter physical-only, exact binding frozen at Execution, no
provisioning side effects in transactions) and M6A-B* (no Task creation, no
Generation expansion, provisioning after admission) remain in force.
