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
AgentTypeContract         information functions, capability envelope, permission
                          ceiling, visibility, tools, roots, budget ceiling,
                          security, lifecycle, continuity, anchor constraint
CapabilitySpec            (capability_id, revision, matcher_kind, security_class)
CapabilityValue           BOOL | SET | ORDERED | QUANTITY | EXACT
CapabilityClaim           (capability, value, assurance DECLARED|ENFORCED, evidence)
TaskRequirement           information function, required capabilities/permissions/
                          tools, workspace/network, continuity, anchor, budget
SpawnSource               advertised lifecycle/continuity envelopes, functional
                          capability envelope, capability claims, status
SourceConfig              opaque source-private config: identity, digest,
                          credential refs, status
AdapterBindingPolicy      stable alias -> exact runtime binding, required safety
PhysicalSafety            imported enforceability: isolation, workspace, network
AgentTypeSelector         Exact(ref) | Latest(id), resolved to exact pre-commit
```

`AgentType` MUST NOT be defined by model, provider, terminal, price tier, prompt
alias, `SourceConfig`, or `AdapterBinding`. `SourceConfig` payloads are opaque:
Core recognizes identity, digest, and credential references only.

---

## 3. The four predicates

### `can_execute(AgentType, TaskRequirement)`

```text
task information_function ∈ agent allowed_information_functions
task required_capabilities ⊆ agent capability envelope (under the matcher)
task required_permissions ⊆ agent permission_ceiling
task required_tools       ⊆ agent tools
agent workspace/network policy >= task requirement (no narrowing)
agent continuity          >= task continuity minimum
task anchor requirement satisfies agent anchor constraint (None = unconstrained)
task budget               <= agent budget_ceiling
```

### `can_provision(SpawnSource, SourceConfig, AgentType, PhysicalSafety, exact binding)`

```text
source/config active
agent lifecycle  ⊆ source lifecycle_modes
agent continuity ∈ source continuity_modes
agent capability envelope ⊆ source functional envelope / claims
required isolation is ENFORCED by the source AND realized by PhysicalSafety
PhysicalSafety workspace/network realize the agent security policy
exact AdapterBinding available
```

### `more_specific_for(A, B, TaskRequirement)`

Only defined once both `can_execute`: A is more specific iff A's authority
envelope is a subset of B's. Ranking MUST NOT use nominal inheritance depth.

### `is_valid_refinement(Base, Derived)`

```text
DerivedPermission ⊆ BasePermission
DerivedVisibility ⊆ BaseVisibility
DerivedTools      ⊆ BaseTools
DerivedRoots      ⊆ BaseRoots
DerivedBudget     ≤ BaseBudget
lifecycle MUST NOT widen; continuity MUST NOT exceed base
anchor MUST satisfy base anchor constraint
workspace/network/tool-roots/required-isolation MUST NOT weaken base
```

---

## 4. Security class and assurance

```text
SecurityClass::Functional   DECLARED may suffice
SecurityClass::Authority     MUST be ENFORCED
SecurityClass::Sandbox       MUST be ENFORCED
SecurityClass::Continuity    MUST be ENFORCED
```

A source that merely declares sandbox support produces no isolation proof; the
imported `PhysicalSafety` fact must also hold.

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
