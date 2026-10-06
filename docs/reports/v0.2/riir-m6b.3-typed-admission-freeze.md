# RIIR M6-B.3 — Typed Semantic Commitment and Existing-Agent Preselection Freeze

Status: Historical Report
Applies to: V0.2 / M6-B.3 closure (PR #23)
Canonical path: `docs/reports/v0.2/riir-m6b.3-typed-admission-freeze.md`
Not a specification.

This is a milestone boundary marker. It records that M6-B.3 is closed and frozen
on top of the frozen M6-B.2 catalog, and that it satisfies the M6-B.3 freeze gate.
It adds no feature and supersedes no specification, except the explicit B.1
staging amendment recorded in
[ADR-0007](../../decisions/0007-m6b-can-provision-task-staging.md).

M6-B.3 freezes as: **typed semantic commitment and existing-agent preselection,
without physical execution authority.**

## Status

```text
M5 execution/runtime substrate           FROZEN
M6-A semantic frontier kernel            FROZEN
M6-B.1 AgentType ontology                FROZEN
M6-B.2 durable catalog (schema v6)       FROZEN
M6-B.3 typed admission + preselection    FROZEN (schema v7)
```

The next design/implementation phase is M6-B.4 (provisioning binding,
authority-bearing typed acquisition, physical eligibility), which is not
specified here.

## Frozen surface

```text
SCHEMA_VERSION = 7

TaskAgentRequirement        exact required AgentTypeRef + hard TaskRequirement
AgentRequirementDraft       pre-commit selector + typed extras
GenerationPolicy            immutable generation-wide authority ceiling
ExistingAgentCandidate      non-authoritative semantic candidate
TaskPlacement               M5 placement facts

schema v7 tables
    task_agent_requirements
    logical_agent_type_bindings
    generation_policies

parent positive markers
    tasks.agent_requirement_mode            LEGACY | TYPED
    logical_agents.agent_type_binding_mode  UNBOUND | BOUND
    generations.policy_mode                 NONE | POLICY

Kernel / storage surface
    admit_typed_proposal
    create_generation_with_policy
    get_task_agent_requirement / get_logical_agent_type_binding / get_generation_policy
    bind_logical_agent_type
    match_existing_agents_for_task

Runtime surface
    RootSemanticControl::admit_typed_proposal / create_generation_with_policy
                        / read_generation_policy
    ProvisioningAdmin::bind_logical_agent_type / task_agent_requirement
                       / logical_agent_type_binding / match_existing_agents_for_task
    CatalogAdmin::publish_capability_definition / publish_agent_type
                  / deprecate_agent_type

pure relations
    match_existing_agents   (semantic preselection, non-authoritative)
    fold_generation_policy  (spec 10 coarse intersection)
```

## Frozen invariants

```text
a committed TaskAgentRequirement pins an exact immutable AgentTypeRef
typed admission is atomic: Task + GenerationTaskBinding + requirement in one txn
a TaskMarker/childRow mismatch is corruption, never a silent downgrade
a policy marker/row mismatch is corruption, never an unconstrained Generation
a binding marker/row mismatch is corruption, never a silent unbinding
legacy eligibility (Task and agent) is a pure Core decision, not a SQL convention
matching grants no Task/Attempt/Lease/Execution authority and no physical eligibility
more_specific_for is a preference relation, never an eligibility gate
a different revision of the pinned type_id is never a substitute
a replay never lets mutable catalog state change a past commitment
Generation policy is an authority ceiling; it never widens a Task
logical_agent type binding is semantic identity only, never physical evidence
```

## Mechanical enforcement

The presence markers and the new child identities are enforced by SQLite itself:

```text
child tables (task_agent_requirements, logical_agent_type_bindings,
              generation_policies)
    BEFORE UPDATE  -> abort
    BEFORE DELETE  -> abort
    BEFORE INSERT  -> abort on duplicate identity (covers INSERT OR REPLACE)

parent identities (tasks, generations, logical_agents)
    BEFORE INSERT  -> abort on same-id insert (identity cannot be replaced)

marker immutability
    tasks.agent_requirement_mode      UPDATE -> abort
    generations.policy_mode           UPDATE -> abort
    logical_agents.agent_type_binding_mode  BOUND -> UNBOUND -> abort
    generation policy row requires a POLICY parent
    a requirement/binding row requires a TYPED/BOUND parent
```

Direct SQL therefore cannot rewrite a requirement, rebind an agent, mutate a
policy, downgrade a typed Task, unbound a bound agent, or replace a durable parent
identity to inject a different presence marker.

## Reused unchanged

M6-B.3 adds no semantics to, and does not redefine, the frozen substrate:

```text
M5   Task / Attempt / Lease / Result authority, claim, Execution commitment,
     AdapterKind / AdapterBindingKey, resolve_exact recovery, ExecutionAdapter
     physical-only contract, absolute deadlines, supervision, writer safety,
     process singleton, recover-before-dispatch
M6-A Generation, RawWorkIntent, CompiledWorkProposal, GenerationTaskBinding,
     RootSemanticControl, IntentIngress, atomic admission
M6-B.1 the four relations (can_execute / can_provision / can_provision_task /
     more_specific_for / is_valid_refinement); AgentType purity
M6-B.2 immutable exact-revision catalog; validated reads; disposition overlays
```

## Negative space

M6-B.3 deliberately does not implement:

```text
SpawnSource / SourceConfig selection
ProvisioningBinding / BindingSnapshot
AdapterRegistry.resolve_exact launch wiring
production ResolvedProvisioningEvidence
credential resolution / brokering
full sandbox-policy ordering / intersection            (B.5)
typed Claim / Attempt / Lease / Execution
cold/revivable matching, MemoryCapsule, Transform
v6 -> v7 in-place migration                            (D-DB-MIGRATE, unresolved)
```

A typed Task stays durable `QUEUED`; a `B3Candidate` is never an
`EligibleExecutionCandidate`. Per
[ADR-0007](../../decisions/0007-m6b-can-provision-task-staging.md),
`can_provision_task` and the remaining physical conjuncts are required at the
first authority-bearing acquisition path (M6-B.4/M6-B.5), not at B.3 preselection.

## Correctness authority

M6-B.3 correctness remains defined by the V0.2 normative specs and architecture
documents. This report does not supersede them.

```text
spec 06 agent-type-and-matching          typed admission, preselection, ranking
spec 07 spawn-source-and-adapter-contract unchanged (source boundary)
spec 10 sandbox-and-security             coarse Generation intersection
spec 13 storage-and-transactions         schema v7 authority, mechanical guards
spec 16 conformance-tests                M6-B.3 conformance cases
spec 17 deferred-open-questions          D-GEN-POLICY-FOLD resolved; D-DB-MIGRATE open
ADR-0007                                 can_provision_task staging amendment
architecture v0.2/m6b-agent-contract-and-spawn-resolution §7
```

## Freeze-gate resolution

An independent M6-B.3 systematic audit of head
`b882da7b3814fe4a96009b8e13ee4a404700ba51` returned:

```text
P0 = 0
P1 = 0
P2 = 0

AUDIT: PASS
MERGE: PASS
```

The audit confirmed the B.1/B.2 boundaries, the atomicity and durability of typed
admission, the Core-owned legacy quarantine for typed Tasks and type-bound agents,
the exact-revision and replay semantics, the non-authoritative matching surface,
the ADR-0007 amendment, and the schema-v7 mechanical guards.

This freeze report is a documentation commit added after that audited head; it
does not alter the frozen behavior.

## Baseline

```text
M6-B.2 frozen base (main):
c7b7a139c7d201d4338e1e2e307e0c2936bf4ffd

Final audited M6-B.3 head:
b882da7b3814fe4a96009b8e13ee4a404700ba51

Exact-head CI (run 37357934141):
rust / ubuntu-latest    PASS
rust / windows-latest   PASS
Python 3.11/3.13 (ubuntu/windows) PASS

SCHEMA_VERSION = 7 at M6-B.3 freeze.
```

## M6-B.4 entry boundary

M6-B.4 may treat the schema-v7 typed requirement and LogicalAgent binding as the
frozen semantic authority, and MUST NOT reopen typed admission, requirement
durability, matching semantics, or ADR-0007. It owns the first authority-bearing
typed acquisition, and MUST prove before creating any typed Claim/Attempt/Lease:

```text
current Task authority/state (a B.3 candidate is not fresh authority)
current LogicalAgent binding
can_execute and can_provision_task
active AdapterBindingPolicy and satisfied required_safety
trusted ResolvedProvisioningEvidence
exact AdapterKind + AdapterBindingKey resolution (frozen at Execution)
available required credentials
recovery via resolve_exact(kind, key)
an explicit V0.1 PoolPartition capacity vs typed population seam (D-TOPOLOGY)
```

A `B3Candidate` and a `LogicalAgentTypeBinding` MUST NEVER become physical
eligibility proof.
