# ADR-0013 — M6-B.5 policy authority, sandbox algebra, and durable security identity

Status: Accepted (M6-B.5 / B5.0 design baseline; not a milestone freeze)
Date: 2026-11-08
Canonical path: `docs/decisions/0013-m6b5-policy-authority-algebra-identity.md`

## Context

M6-B.4 is frozen within the [ADR-0012](0012-m6b4-scope-typed-physical-execution-ack.md)
scope: typed physical execution → durable ACK interface. Its sandbox policy is
pinned by **exact reference equality** (`sandbox_policy_no_wider` and
`sandbox_fold` reject any two distinct references), its resolver imports no
sandbox enforcement facts (`produce_evidence` deliberately passes empty
enforced-policy and enforced-capability sets), and every credential-bearing
SourceConfig is rejected until trusted, source-bound availability exists.

[Spec 10](../specs/v0.2/10-sandbox-and-security.md) nonetheless requires the
effective permission of an Execution to be the intersection of the AgentType,
Generation, Task and SpawnSource ceilings, requires root-created refinements to
only narrow, and requires the concrete effective policy to reach the physical
start request. `D-SANDBOX-ORDER` and `D-SANDBOX-INTERSECTION` are still open,
and a single partial order over "allowed" sets cannot express them: ceilings and
Task floors have opposite polarity.

The remaining B5.0 decisions therefore have to fix the authority model, the
algebra, and the durable identity of a computed policy **before** any B5.1
implementation can be reviewed. This ADR records those decisions; it does not
claim that the implementation exists.

## Decision

### 1. Four objects, and no fifth security authority

M6-B.5 separates four objects:

| Object | Question answered | Lifetime |
|---|---|---|
| `SecurityCeiling` | What upper bound did an author grant? | authored, immutable revision |
| `SecurityRequirement` | What does this Task minimally require, and what is its own ceiling? | Task |
| `EnforcementSupport` | What can this exact physical domain mechanically enforce? | imported domain/version |
| `EffectiveExecutionSecurity` | What does this Execution commit to enforce? | Execution |

The effective policy is

```text
P_eff = P_actual_agent ∩ P_generation ∩ P_task ∩ P_source
```

where `P_actual_agent` MUST come from the AgentType revision actually bound to
the LogicalAgent, not from the Task's matching anchor. There is **no**
`P_config` term.

`P_eff` is the authorization component only. `EffectiveExecutionSecurity` also
carries the execution guarantees (notably `I_required` from §3), which are
composed by disjunction and are never part of the policy intersection.

A SourceConfig is not an independent authored `SecurityCeiling`. It may carry
source-private non-secret configuration, functional capability declarations,
lifecycle narrowing, and credential references. A physical effect of a config
is admissible only through the source integration and `EnforcementSupport`
evidence; it never becomes a second authored security authority. If such a
physical restriction cannot satisfy the committed effective policy and the
Task minimum, the candidate is ineligible.

Adding an authored ceiling to a SpawnSource requires publishing a new versioned
source contract. An existing immutable source revision MUST NOT gain a security
meaning it did not have at publication.

### 2. `R_min` is a typed requirement, not one permission set

B5.0 does **not** change M6-A `TaskSpec` canonicalization, digests, or replay.
`R_min` is derived from existing structured Task fields; the only new catalog
property is the explicit capability-to-policy projection below, which removes any
implementer guess. `R_min` is a product of
five typed components:

```text
RequiredAccess(T)          // formal security operation floors only
RequiredCapabilities(T)    // catalog-validated capability obligations
RequiredGuarantees(T)      // isolation, continuity, ...
CoarseExecutionModes(T)    // workspace mode, network mode
PlacementConstraints(T)    // M5 placement predicate
```

- `RequiredAccess` is the only component compared by set inclusion. It is
  constructed only from a security-class capability for which the capability
  catalog provides an **explicit, normative mapping** to a policy access
  dimension. The catalog property is

  ```text
  CapabilityDefinition.policy_projection: Option<PolicyAccessDimension>
  PolicyAccessDimension = FileOperationClass | ToolCommandSet
                        | NetworkEndpoint | EnvironmentName
  ```

  and only `Some(dimension)` permits inclusion in `RequiredAccess`. The mapping
  is a catalog-owned, versioned property introduced with the B5.1 schema; it is
  never inferred from `MatcherKind`, `SecurityClass`, identifier naming, or
  polarity. A mapping is valid only when the capability's value shape denotes an
  allowed-operation set for that dimension; `Quantity`, `Ordered` and functional
  `Exact` values MUST NOT carry a `policy_projection`. Until B5.1 publishes it, no
  capability-to-`RequiredAccess` projection is permitted.

  Because `CapabilityPolarity` is independent of `MatcherKind` and
  `SecurityClass`, a `Set` capability may be a `Restriction`, where a larger set
  is a stronger limit rather than more ability. The v1 mapping validity condition
  is therefore explicit:

  ```text
  policy_projection != None
      ⇒ matcher_kind == Set
      ∧ polarity == Ability
      ∧ security_class ∈ {Authority, Sandbox}
      ∧ every member has a valid operation mapping
  ```

  A `Restriction` never enters the positive `RequiredAccess` floor, even when its
  value is a `Set`; its relation to a policy ceiling is a separate consistency
  projection, never an implicit floor conversion. `Functional` and `Continuity`
  capabilities likewise never project.

  The conversion is deterministic and exhaustive. For a mapped capability with
  dimension `d`, the catalog also declares a versioned member map
  `policy_member_map: member -> operation(d)`, and

  ```text
  ProjectAccess(ref, value) =
      Some({ dimension = d, allowed = { map(m) | m ∈ value.members } })
      iff policy_projection = Some(d) and value is a Set
  ```

  - `Set` is the only projecting value shape in v1. Every member MUST be present
    in `policy_member_map` for that exact `(capability_id, revision)`; an
    unmapped member, a member from another dimension, or a non-positive member
    makes the catalog entry invalid and admission MUST reject. It MUST NOT be
    silently dropped, because dropping would weaken the floor.
  - `Bool` MUST NOT project. `Bool(true)` asserts that the capability is present;
    it does not name allowed operations, so a dimension enum alone cannot
    derive an access set. A capability that needs an operation floor must author
    its members as a `Set`.
  - `Set(∅)` projects to the empty allowed set, which is legal: it is a
    zero-floor requirement, not a conflict.
  - `Ordered`, `Quantity`, and functional/boolean `Exact` never project; their
    obligation stays in `RequiredCapabilities`.

  A quantity such as
  `memory_mb >= 4096`, an `Ordered` rank, or a functional `Exact` capability MUST
  NOT be projected into `ProjectAccess`. B.5 defines no generic
  capability-to-permission-set conversion; absent a catalog mapping, the
  obligation stays in `RequiredCapabilities`. An empty `RequiredAccess` is legal:
  restrictions are still enforced by `P_eff ⊆ every ceiling` and by the capability
  obligations themselves.
- `RequiredCapabilities` is validated by the existing `CapabilityCatalog`
  matcher for the exact `(capability_id, revision)`, preserving `Bool`, `Set`,
  `Ordered`, `Quantity` and `Exact` semantics unchanged. It is never checked by
  `⊆` and never projected into `ProjectAccess`.
- `RequiredAccess` MUST NOT be constructed from `required_workspace`,
  `required_network`, or `Restriction` values.
- `CoarseExecutionModes` carries the workspace and network modes. They are
  compared by the existing dimension-specific predicates (workspace rank and
  network polarity/enforcement mode), never by set inclusion. `ReadOnly` is an
  upper bound on content and directory-entry modification authority (write and
  delete), `Disabled` is a network restriction, and `Restricted` is an execution
  mode; none of them is a positive access requirement.
- `RequiredGuarantees` carries guarantees that are not allowed-operation sets:
  `required_attempt_isolation` (the disjunction defined in §3, not a per-object
  intersection), continuity, and any other execution guarantee.
  It is checked by the corresponding guarantee predicate, not by `⊆`.
- `PlacementConstraints` continues to be checked by the frozen M5 placement
  predicate, and only when a candidate Agent exists (see §5).

`R_min` contains **positive obligations only**. `WorkspaceMode::Write` is a
`CoarseExecutionModes` requirement, and a security-class capability can enter
`RequiredAccess` only through its catalog mapping. Negative constraints
(`ReadOnly`, `Disabled`, no-widening) are covered by `P_eff ⊆ every ceiling` and
by the coarse-consistency checks, not by `RequiredAccess`.

`CredentialRef` lives in SourceConfig, not in `TaskRequirement`. It creates a
credential qualification obligation on the selected config; it is not a Task
floor and MUST NOT be presented as a Task-declared credential set.

Credential authorization is namespaced. The frozen `CredentialRef` is an opaque,
non-empty source-local string; it is not a global identifier and string equality
does not establish that two references name the same credential. The AgentType
ceiling grants a stable `CredentialAuthorizationId` = an explicit credential
authority namespace plus a logical reference, resolved by the trusted source
integration under the exact source/config revision. Two brokers that both use
`prod-key` keep distinct identities, and a source-local ref MUST NOT gain
AgentType authorization solely by matching another authority's string. Core does
not interpret provider internals, and the namespace is not derived from adapter
kind or mutable provisioning identity. Re-provisioning the same logical
credential keeps the same id; a different logical credential requires a new
AgentType revision rather than silent reuse. When no credential is requested the
dimension adds no obligation.

B.5 v1 does **not** claim path-level minimum requirements. `Task requires
workspace Write` is mechanically checkable; `Task must write Workspace/src/main.rs`
is not, without a new structured Task field, and MUST NOT be inferred from the
natural-language payload. A future path-level floor requires a separate ADR that
extends `TaskSecurityRequirement`/admission rather than silently changing M6-A.

### 3. Coarse fields are projections or consistency checks, per object

In `SECURITY_V1`, the coarse and fine fields cannot be two independent grants.
The relation is defined per object, because they have different authorship:

| Object | Relation |
|---|---|
| AgentType | `project_agent(P) == SecurityContract` (equality) |
| Generation | `project_gen(P) == (max_workspace, max_network)`, and `P` entails `requires_attempt_isolation` |
| Task | **consistency**, not projection equality |

`project_workspace(P)` is `Write` when `P` authorizes writing **or deleting** any
logical workspace region, and `ReadOnly` otherwise. Deletion modifies workspace
content and directory entries, so a policy that can delete cannot project to
`ReadOnly`.

`requires_attempt_isolation` is an execution guarantee, not part of the
file/network allowed set, so the normalized policy carries it explicitly.
`P_eff` is an **authorization** intersection; execution guarantees are not
intersected with it and MUST NOT be composed by `⊆`. Attempt isolation is a
monotone requirement with an explicit disjunction:

```text
I_required = I_AgentType ∨ I_Requirement ∨ I_SourceRequirement
I_Requirement = TaskAgentRequirement.hard.required_attempt_isolation
```

- `I_AgentType` is `AgentTypeContract.security.requires_attempt_isolation` for the
  actually bound revision. `I_Requirement` is the durable
  `TaskAgentRequirement.hard.required_attempt_isolation`
  (`TaskRequirement.required_attempt_isolation`). The frozen path produces it in
  two distinct stages, and the attribution matters:
  `AgentRequirementDraft::into_requirement` only carries the Task-authored
  `required_attempt_isolation` forward; `fold_generation_policy` performs the
  disjunction `GenerationPolicy.requires_attempt_isolation
  || TaskRequirement.required_attempt_isolation`, and the folded value is what
  becomes durable in `TaskAgentRequirement.hard`. B5.1 MUST NOT change
  `into_requirement` to fold the Generation term itself in order to "match" this
  ADR; the Generation term belongs to the fold, and the stored requirement MUST
  remain the exact fixed point of that fold.
- `required_attempt_isolation` is **not** a `TaskSpec` field. B5.0/B5.1 MUST NOT
  add one to the frozen M6-A `TaskSpec` to match an ADR formula; the authored
  value lives in `AgentRequirementDraft`, which becomes the durable
  `TaskAgentRequirement.hard`.
- `I_SourceRequirement` exists only when the SpawnSource contract for that
  provisioning explicitly requires isolation. No v1 source contract has such a
  field, so it is `false` at this baseline; adding one requires a new versioned
  source contract, never an inference.
- `EnforcementSupport` saying the adapter/domain *can enforce* isolation is a
  capability, not a request. It MUST NOT set any `I_*`, and "supports isolation"
  MUST NOT be read as "this execution has isolation enabled".
- `I_required` is carried separately from `P_eff` in
  `EffectiveExecutionSecurity`; it is not a policy dimension and cannot be
  narrowed by meeting another policy.

`I_required` discharges through one ordered chain at the candidate stage:

```text
I_required
    → ExecutionTarget.attempt_isolation = true
    → exact adapter proves enforceability for the binding
    → committed execution requests isolation
```

Any failed step makes the candidate ineligible. It is never repaired by
intersecting policies, by an AgentType claim, or by adapter self-assertion. An
isolating `ExecutionTarget` alone does not discharge `I_required`: the committed
execution must request isolation for the requirement to be met, and an
`I_required = false` execution gains no semantic isolation claim from its target.

For a Task, `required_workspace` is fixed by the TaskSpec before
`into_requirement`, while `sandbox_policy` comes from the requirement draft. They
are independent authored inputs. Demanding `project_task(P) == required_workspace`
would either violate TaskSpec's single-authority rule or be unimplementable.
Define

```text
P_task = TaskSpec.workspace_mode            // coarse workspace, M6-A
       ∩ requirement_draft.required_network // coarse network, typed draft
       ∩ optional requirement_draft.sandbox_policy
```

`P_task` must then satisfy:

```text
required_workspace = Write    ⇒ P_task allows write
required_workspace = ReadOnly ⇒ P_task contains no write and no delete
required_network follows the existing rank/polarity
required_attempt_isolation    ⇒ the candidate carries the isolation guarantee
```

The two coarse terms have different sources: M6-A `TaskSpec` carries only
`workspace_mode`; `required_network` comes from the typed requirement draft, and
the fine policy is the draft's optional `sandbox_policy`. The spec MUST NOT
attribute `required_network` to `TaskSpec`. `required_attempt_isolation` is also
carried by the draft (and folded with the Generation policy, see §3).

Consequently a coarse/fine pair such as `workspace = ReadOnly` with a fine write
or delete root MUST be rejected.

The Task-local check runs before any cross-object meet and before normalization:

```text
validate_task_local_coarse_fine(TaskSpec, TaskPolicy)
```

It MUST run on the un-normalized authored inputs, so `None` and an explicit
authored policy remain distinguishable. Validation MUST NOT be deferred to the
normalized meet, where `ReadOnly + None` and `ReadOnly + explicit write policy`
could produce indistinguishable bytes.

A conflict between the TaskSpec coarse fields and the Task policy MUST be
rejected even when a stricter Generation/AgentType/SpawnSource ceiling would
hide it in `P_eff`. For example, `required_workspace = ReadOnly` with a Task
policy that allows `write Workspace/src` is invalid on its own; a Generation
`ReadOnly` ceiling does not repair it. `SECURITY_V1` admission MUST fail closed
when this check fails.

`sandbox_policy = None` in `SECURITY_V1` means "no additional Task-level fine
restriction". It is an **absent optional layer**, not an explicit top policy: it
adds no restriction and MUST NOT be represented as `Top` for the meet. The
credential dimension is a positive grant set rather than a `Top`-shaped lattice
element, so an explicit top would silently erase the Generation's credential
grants. With no Task layer, `P_eff` is `P_generation`, and the coarse fields
remain authoritative in their own sources. AgentType and
Generation `SECURITY_V1` contracts MUST carry an authored policy; an absent
policy there would make `project_agent`/`project_gen` undefined. `COARSE_V1`
keeps its frozen `None` semantics and replay. `SECURITY_V1` MUST NOT bypass the
projection/consistency check to enter the old dispatch path.

When a capability and a policy dimension express the same restriction, there is
exactly one authoritative value. A formally mapped capability is derived from
and validated against the policy; a security-class capability with no defined
mapping keeps the existing capability catalog rules and MUST NOT be guessed
into a policy dimension.

### 4. Authored policies are catalogued; derived policies are inline and content-addressed

```text
Authored SandboxPolicyRevision
    immutable catalog
           ↓
        normalized meet
           ↓
Derived Canonical Policy
    inline bytes + digest (NOT a catalog row)
```

The catalog stores **only author-defined anchor policies** with an immutable
revision and a canonical content digest. The product of an intersection is not
inserted into the catalog, and there is no API to "publish a derived policy".

Two durable commitments are recorded:

- `TaskSecurityBinding` at typed admission for a `SECURITY_V1` Task (a
  `COARSE_V1` Task has no such row): inline admitted policy bytes,
  policy digest, authored input refs/digests, and derivation identity.
- `ExecutionSecuritySnapshot` at `SECURITY_V1` Execution commitment: inline final
  effective policy bytes, policy digest, exact source/binding provenance, security
  plan bytes/digest, and the actual bound AgentType identity.

The Task stage computes `Generation ∩ Task`; the Execution stage adds the actual
AgentType and the SpawnSource ceiling. Neither stage invents a new authored
policy identity.

Derivation identity records `normalization_version` and `meet_algorithm_version`
separately, not one vague "derivation version". Golden vectors are keyed by
`(canonical_version, algebra_version)`. Existing rows are read under the version
recorded with them and MUST NOT be reinterpreted by a newer algorithm.

`SecurityDerivation` proves the canonical **output content** and the unique
**set** of authored/derived input references and digests used to produce it. Its
input list is unordered: each `(reference, digest)` pair is sorted and
deduplicated, and it carries no per-input author role. Consequently a Generation
and a Task that author the same exact policy contribute one input entry, not two
role-tagged entries. The Task author identity is carried by
`TaskAgentRequirement.sandbox_policy` and re-read jointly with the Generation
policy and the derivation: a binding is accepted only when the independently
recomputed derivation, the durable requirement, and the catalog agree. A reader
MUST NOT treat the derivation digest alone as proof of which author contributed
which layer. Role-tagged inputs would change the canonical bytes and therefore
require a new derivation canonical version; B5.1 does not adopt them.

#### Read-side revalidation of release invariants

Publication-time validation is not a substitute for a validated read. The schema
guards make the relevant columns immutable, but the project's corruption
discipline requires that a bypassing direct-SQL writer still fails closed:

- `spawn_source_security_declaration` validates its parent SpawnSource first
  (canonical content and disposition overlay), then re-reads the declared
  `P_source` and rejects it when `attempt_isolation = true`, matching
  `validated_source_declaration`. A missing or corrupt parent MUST NOT be
  reported as a legal `COARSE_V1` source with no declaration, and a repointed
  declaration MUST NOT acquire an isolation guarantee.
- The Task/Generation `security_contract_mode` agreement is re-proved
  **symmetrically** by the validated requirement read
  (`verify_requirement_mirrors` → `verify_generation_mode_agreement`), not only
  by `get_task_security_binding`. Admission requires the two modes to be equal in
  both directions, and the mode columns are immutable under the schema guards, so
  a mismatch can only be injected corruption. The symmetric check matters because
  a `SECURITY_V1`-only guard leaves the mirror image open: a `COARSE_V1` Task
  whose Generation is repointed to `SECURITY_V1` would still refold as a legal
  coarse requirement, and the frozen coarse acquisition gate would then treat it
  as dispatchable. A read that sees a locally valid but cross-record
  contradictory pair treats it as corruption, never as execution qualification.
- The sandbox-policy and capability-document budgets are re-checked by their
  decoders, and a stored binding policy is re-encoded and compared byte-for-byte
  with its canonical form.

These checks are error paths, not alternate semantics: they return an invariant
or contract error and never repair, downgrade, or reinterpret the stored value.

### 5. Admission proves satisfiability; candidates prove satisfaction

There is exactly one `satisfies_requirement` relation. It is total over `R_min`
and returns `Satisfied`, `Unsatisfied`, or `Pending(component)` for a component
whose subject does not exist yet. A caller MUST NOT treat `Pending` as
`Satisfied`.

```text
satisfies_requirement(R_min, P, stage, candidate?)
    stage = Admission | Candidate | Commitment
```

| Stage | Evaluated | Deliberately deferred |
|---|---|---|
| Typed admission | `RequiredAccess ⊆ ProjectAccess(TaskAuthority(meet(P_generation, P_task)))`, `CoarseExecutionModes` compatibility with the Generation ceiling, Task-local coarse/fine consistency, and the pinned AgentType's authored policy compared against `TaskAuthority(meet(...))` | `RequiredCapabilities` (they need the actual AgentType), `PlacementConstraints`, and guarantees that require a physical candidate |
| Candidate matching/acquisition | `RequiredCapabilities` against the actual AgentType claims through their catalog matchers, `RequiredGuarantees` physical eligibility, `PlacementConstraints` through the frozen M5 placement predicate, whole-policy `FeatureObligations` coverage, **and every positive Task requirement (`RequiredAccess`, `RequiredCapabilities`, `CoarseExecutionModes`) re-evaluated against the complete `P_eff`** | — |
| Execution commitment | exact binding and the complete security contract, re-proven against the committed snapshot, including every positive Task requirement and `CoarseExecutionModes` against the committed `P_eff` | — |

Only `Unsatisfied` rejects. For a `SECURITY_V1` Task, `Pending` components are
recorded in `TaskSecurityBinding` as an explicit obligation set and MUST be
discharged at the candidate stage; a `COARSE_V1` Task has no binding and no such
obligation set. Admission MUST NOT assume that a candidate Agent exists: a Task
may name a workstream that no future Agent matches, and that is a candidate
ineligibility, not an admission contradiction.

At the Candidate stage every previously `Pending` component MUST resolve to
`Satisfied` or `Unsatisfied`; at Execution commitment no `Pending` may remain at
all. A remaining `Pending` at Candidate is a whole-round defer when its subject is
still legitimately unavailable, or a control-plane fault when the subject should
exist; it is never treated as `Satisfied` and never authorizes execution
commitment.

The admission predicate MUST NOT use the fine-derived meet as if it were the
complete Task authority. The durable `TaskSecurityBinding.policy` remains the
fine-derived meet and its canonical bytes are unchanged, but every admission
check uses the pure coarse-constrained view

```text
TaskAuthority(P) = P with write := ∅ and delete := ∅   if required_workspace = ReadOnly
                   P with network := ∅                 if required_network = Disabled
```

which preserves reads, execute, tools, environment, credentials and resource
ceilings. `ReadOnly` and `Disabled` are upper bounds carried by the Task itself,
so they apply to every layer the Task admits, including the Generation layer.
`Delete` is included because deletion modifies workspace content and directory
entries; a Task that declares `ReadOnly` cannot be satisfied by a policy that
still authorizes deletion. This closes both directions of the
combined-obligation error: a `ReadOnly` Task whose required capability projects
to a write or delete, or a `Disabled` Task whose required capability projects to
an endpoint, is unsatisfiable even when the fine layer shows the operation; and
a legal `ReadOnly` Task under a `Write` Generation is not rejected merely because
the fine meet retains Generation write paths that the Task ceiling removes before
the pinned AgentType comparison. A later Candidate or Execution stage re-applies
the same view to the complete `P_eff` because a later meet can only narrow it.

RequiredAccess is therefore checked as
`RequiredAccess ⊆ ProjectAccess(TaskAuthority(meet(P_generation, P_task)))`, and
`can_execute_security_v1` compares `TaskAuthority(meet(...))` with the pinned
AgentType's authored policy. B5.1 does not weaken the `SECURITY_V1` strict
ordering to obtain this result; it supplies the comparison with the complete,
explainable Task constraint.

#### The binding policy is not the complete execution authority

`TaskSecurityBinding.policy` is exactly `meet(P_generation, P_task_fine)`; it does
not include the Task's coarse `workspace`/`network` upper bounds. For example a
`ReadOnly` Task under a `Write` Generation keeps the Generation's write paths in
`binding.policy()`, while the Task's actual authority permits neither writing nor
deletion. B5.1 handles this correctly by applying `TaskAuthority` in every
admission check, but the raw binding value MUST NOT become the physical compiler
input.

B5.2 MUST introduce a typed composite authority (for example
`EffectiveTaskAuthority` or `QualifiedSecurityContract`) whose only constructor
combines:

```text
TaskSecurityBinding.policy
+ durable Task coarse modes (required_workspace, required_network)
+ the actual bound AgentType authored ceiling
+ the SpawnSource authored ceiling
```

and whose type is what `compile_execution_security` accepts. Passing
`binding.policy()` directly is forbidden. The composite MUST also record the
exact source, binding and AgentType identities so Execution commitment can
re-prove them. This is a B5.2 API requirement, not B5.1 runtime behavior; B5.1
adds no `EffectiveTaskAuthority` type.

`RequiredCapabilities` is deferred at admission because admission has only the
Generation and Task policy objects, while the existing M6-B.2 matcher compares a
requirement against the actual AgentType's capability claims. Moving it earlier
would require inventing a candidate, and collapsing it into `ProjectAccess` is
exactly the quantity/rank/exact confusion this split forbids.

Only the `RequiredAccess` component is a set inclusion. `RequiredCapabilities`
use their catalog matchers, and `RequiredGuarantees`, `CoarseExecutionModes`, and
`PlacementConstraints` use their own predicates. There MUST be exactly one
relation, not two implementations that disagree about which `R_min` dimensions
are set-like or about which stage owns placement. In particular, network
`RequiredAccess` uses the same union containment as `is_within`: a required
endpoint rule is satisfied when the union of the effective rules covers it, not
only when one effective rule covers the whole rule. `ANY:*:443` is therefore
satisfied by `TCP:*:443 ∪ UDP:*:*` at every stage.

Admission checks only `Generation ∩ Task`. Provisioning adds the actual
AgentType and the SpawnSource ceiling, and its failure is a different class
(candidate ineligibility / typed unavailability) rather than an admission
rejection.

Satisfying a component at admission does **not** make it satisfied after a later
meet. Intersection can remove the capability that a mode depended on:

```text
Task requires WorkspaceMode::Write
AgentType allows writing Workspace/src
SpawnSource allows writing Workspace/tests
P_eff = four-way meet
    → no writable path
```

The composite policy may still be physically enforceable, but it can no longer
satisfy the Task's `Write` mode. Candidate matching and Execution commitment MUST
therefore re-evaluate `CoarseExecutionModes` and every positive Task requirement
against the complete `P_eff`; they MUST NOT reuse the admission-time verdict.
This adds no path-level floor: it only prevents a satisfied coarse requirement
from being silently erased by a later intersection.

`WorkspaceMode::Write` is satisfied only when the complete `P_eff` retains at
least one allowed write operation **inside the committed logical workspace
region** of this Task. A writable location that is unrelated to that workspace —
a temporary directory, a bootstrap-manifest path outside the workspace, or any
other writable area that the Task cannot use as its workspace — MUST NOT satisfy
`Write`. This stays a coarse-mode predicate over the logical workspace region; it
does not introduce a path-level `R_min` floor, and it MUST NOT be inferred from
an adapter's ability to create files somewhere.

This matches the existing coarse rejection of
`workspace_rank(task) > generation.max_workspace`; the sandbox dimension MUST
fail at the same stage, not later during provisioning. An unsatisfiable Task
must never become a durable semantic commitment.

Admission itself MUST apply this re-evaluation to the admitted
`meet(P_generation, P_task)` before the Task or its `TaskSecurityBinding` becomes
durable. A Task whose `required_workspace` is `Write` MUST be rejected when the
admitted policy has no writable region; `Restricted`/`Enabled` network modes MUST
be rejected when the admitted policy allows no endpoint or is not unrestricted.
A legal deny-all intersection is not a `PolicyConflict`; it is an unsatisfiable
positive requirement at the admission stage.

### 6. Dimension rules for `meet`

The algebra is defined dimension by dimension; "intersect the allowed sets" is
not sufficient:

| Dimension | Composition rule |
|---|---|
| allowed file operations and path ranges | set intersection per operation class |
| hidden/prohibited ranges | keep common restrictions; normalize to equivalent allowed/prohibited regions; visibility is an independent observability dimension, not an operation precondition (see §7) |
| tools, commands | intersect the effective `ToolCommandSet` union (see §7) |
| network endpoints/protocols/ports | meet in that dimension |
| environment variables | allowed-name intersection; conflicting required values reject |
| credential scope | refs, use, delivery mode and audience all narrow; an omitted grant in the AgentType ceiling is empty, not `Top`, and other ceilings can only narrow |
| resource ceilings | stricter value within the same unit and scope |
| working directory | validate as a launch constraint; conflict rejects rather than choosing one |
| execution guarantees (`requires_attempt_isolation`) | **disjunction, not intersection**: `I_required = I_AgentType ∨ I_Requirement ∨ I_SourceRequirement`; any true disjunct makes isolation required, and no meet can remove it |

`P_eff` covers allowed operations and ceilings only. Guarantees live beside it:
meeting two policies can never clear an isolation requirement, and an
`EnforcementSupport` capability cannot create one (see §3).

The authored policy isolation bit and the object's durable guarantee field are a
single authority, not two independent representations. Consequently
`validate_generation_coarse_fine` requires

```text
Generation.requires_attempt_isolation
    ⇔ authored_policy.attempt_isolation
```

and the AgentType analogue requires the same equality. A Generation that authors
isolation without declaring the guarantee, or declares it without authoring it,
is rejected at publication; neither direction may leave `P_eff.attempt_isolation
= true` while no requirement asks a committed execution to request isolation. A
Task's optional fine policy MAY repeat the Task's own
`required_attempt_isolation = true`, but MUST NOT introduce the guarantee when
the Task does not declare it. `SpawnSource` has no isolation guarantee field, so
its `P_source` MUST NOT set `attempt_isolation`; an unattributed source-level
guarantee is rejected at publication rather than left for B5.2/B5.4 to
interpret. Making fine-policy `true` *create* an isolation requirement would
change the `I_required` composition and therefore requires a new ADR revision
and algebra version.

The credential dimension is the one dimension where absence is not `Top`. A
`SECURITY_V1` AgentType ceiling that does not explicitly grant a secret scope
authorizes no credential use at all, because the alternative would let an
unrelated `SourceConfig` request — not an authored security contract — decide
which sensitive credential enters the execution chain. A `Generation` or source
ceiling that declares the dimension may only narrow the grant, never create or
widen one, and `SourceConfig.credential_refs` stays a request. This is ordinary
meet behavior: an absent AgentType grant is a legal empty allowed set, so `meet`
returns `Ok` and the credential obligation is `Unsatisfied`, which makes the
candidate ineligible. It adds no `P_config` term and no `TaskSpec` field. When no
credential is requested, the dimension imposes no obligation and policy-only
execution is unaffected.

"Authorizes no credential use" is scoped to Agentype-managed credential
authorization and delivery. It does not mean the worker is physically unable to
obtain a credential: host credentials reachable through harness-native
configuration, inherited environment or ordinary file permissions are outside the
managed grant, are not certified by this ADR, and neither satisfy nor are implied
by a `CredentialAuthorizationId`. Absence of a grant and physical impossibility of
credential access are different claims at every layer.

The canonical form is the identity of the policy's authorization semantics. A
`CredentialGrant` with an empty `uses`, `audience` or `delivery_modes` facet
authorizes nothing, so normalization **drops** it instead of serializing an inert
grant; the corresponding `CredentialAuthorizationId` is then simply absent from
the canonical map. An explicitly authored no-authority grant and an absent grant
therefore share one canonical byte sequence and one content digest. Dropping such
a grant loses no authority and no later reader may reintroduce authority from a
digest. `is_within` is a preorder on normalized policies, and canonical digest
equality is a sufficient but not the only way to observe no-wider in both
directions; it MUST NOT be used as a substitute for the algebra when comparing
policies that were not produced by the same normalization path.

`meet` is **partially defined** over the grammar, not total:

```text
meet(P, Q) -> Result<NormalizedPolicy, PolicyConflict>
```

A dimension rule that says "reject" or "conflict rejects" returns
`PolicyConflict`. The greatest-lower-bound property holds only for the `Ok`
case: if it returns `Ok(N)`, then `N` is the greatest representable policy below
both inputs. `PolicyConflict` is a deterministic unsatisfiability result of the
two contracts — not a compiler fault, not a transient unavailability, and not a
candidate-scoring signal. It MUST NOT be reported as `RESOURCE_UNAVAILABLE`, and
v1 does not persist a `Bottom`/`Unsatisfiable` policy; callers hold the error
type in memory and reject the candidate or admission at their own stage.

An **empty allowed set is not a conflict.** The two outcomes are different and
MUST NOT be conflated:

- an empty intersection in a permission dimension (`file`, `tool/command`,
  `network endpoint`, ...) yields `Ok` with that dimension empty — a valid
  deny-all sub-policy. It is the greatest lower bound, and a requirement-stage
  check may later report the Task obligation `Unsatisfied` (for example,
  `WorkspaceMode::Write` with no allowed write);
- `PolicyConflict` is reserved for **required launch/structural constraints
  that cannot both hold**: two different required working directories,
  conflicting required environment values for the same name, incompatible
  required resource scope or unit declarations, or a coarse/fine contradiction
  within one object. It is not "no permission in a dimension".

Consequently a conformance test must distinguish "meet produced deny-all" from
"meet produced no representable lower bound", and GLB tests may not reinterpret
an empty set as a conflict or vice versa.

A resource ceiling's **scope is part of the contract**: a per-process memory
limit and an execution-domain limit are different guarantees, and a semantic
token budget is not an OS resource limit.

### 7. Bounded v1 policy grammar

B5.1 MUST NOT implement an authorization DSL with arbitrary regexes, shell
conditions, or script evaluation. V1 syntax is limited to exact identifiers,
finite sets, bounded numeric values, anchored path-component trees, finite
network rules, and explicit versioned extensions.

Logical paths are contract-kernel values. Binding them to real filesystem
objects belongs to physical enforcement. The following shortcuts are invalid:

```text
string-prefix comparison      ≠ filesystem containment proof
one canonicalize at launch    ≠ permanent race elimination
entrypoint in an allowlist    ≠ every later command is confined
```

Symlinks, reparse points, aliases, inherited handles, and post-launch path
changes require evidence from the exact enforcement profile. Runtime
dependencies, temporary directories, and launch-required files enter an
explicit bootstrap manifest. A compile failure MUST NOT add an unrestricted
"needed to run" exception.

#### v1 network universe, canonical universal rule, and bounded host tokens

The B5.1 network operation domain is exactly

```text
{TCP, UDP} × Host × Port,   Port = {1 ..= 65535}
```

`NetworkTransport::Any` is the union of `TCP` and `UDP`, and
`NetworkEndpointSet::Any` is the whole domain. The finite rule `ANY:*:*` denotes
exactly the same domain, so normalization MUST collapse it to `Any`. A canonical
policy MUST NOT retain both spellings, and `project_network` together with
`is_within` MUST agree that the explicit universal rule and `Any` are the same
top policy. A protocol outside `TCP`/`UDP` requires a new policy canonical or
algebra version; it MUST NOT be silently folded into the v1 `Any` value.

An exact host token is 1 to 253 ASCII bytes drawn from `[A-Za-z0-9._-]` and MUST
contain at least one alphanumeric byte. A token containing `:` MUST parse as an
IPv6 literal, so `example.com:443` and other `host:port` fragments are invalid;
the port is always a separate selector. `*` is the only wildcard and MUST be the
entire host token; `*.example.com`, `example.*`, and any other embedded `*` are
invalid. NUL, control bytes, whitespace, path or URL syntax, and non-ASCII text
are invalid. Port 0 is not an endpoint selector. Each normalized policy network
set and each `RequiredAccess` endpoint set is limited to 256 distinct rules, and
the decoder rejects a longer authored array; a larger collection is rejected
before it reaches the finite-partition containment algorithm. These are input
grammar rules, not B5.2 physical-compiler policy.

The meet of two bounded endpoint sets is also bounded. Each side has at most 256
rules, so the rule-wise product has at most 65,536 candidates; normalization
groups candidates by host and port and removes covered rules in `O(n log n)`
rather than with a global pairwise scan. A normalized result that still exceeds
256 rules is rejected as a `PolicyConflict` by policy construction. Thus a
legitimate 256 × 256 input can fail closed on the representable-output bound but
cannot force unbounded CPU work, and an intermediate product that simplifies to
a representable policy remains admissible.

This collapse is part of defining the initial policy canonical, not a
reinterpretation of durable data: B5.1 is the first build to write policy rows,
and the pinned `(canonical_version, algebra_version) = (1, 1)` golden vectors
are defined by this initial canonical. No vector keeps a second spelling of the
explicit universal rule. A post-B5.1 change to these bytes MUST bump
`SANDBOX_POLICY_SCHEMA_VERSION` and add new vectors rather than let an existing
digest mean something new.

The containment predicate `network_allowed_union_within` uses the same finite
partition. If the two sets together name `H` distinct exact hosts and `P`
distinct exact ports, the partition has `2 · (H + 1) · (P + 1)` atoms. The
per-set bound limits each side to 256 rules, so across one comparison `H ≤ 512`
and `P ≤ 512`: at most `2 · 513 · 513 ≈ 526,338` atoms. The predicate marks the
outer union once and then requires every atom an inner rule covers to be marked.
Each rule first projects to the host and port atoms it covers, so an exact-port
rule touches one port column per covered host instead of rescanning every port
atom to reject it. The cost is bounded by the input grammar, and a 256 × 256
adversarial regression pins it. This is pure in-transaction policy evaluation;
B5.1 does not add a separate wall-clock admission budget.

#### Network canonical identity is deterministic but not union-minimal

The v1 canonical form is deterministic for a given rule syntax: the same
endpoint array always normalizes to the same rule set, bytes and digest. It is
not a complete union-minimal form. A rule covered only by the *union* of several
other rules is retained, because removing it would require canonicalizing the
whole wildcard product. For example

```text
A = TCP:*:* ∪ UDP:*:443
B = TCP:*:* ∪ ANY:*:443
```

permit exactly the same operations, because `TCP:*:*` already contains the TCP
part of `ANY:*:443`. `is_within` proves `A ⪯ B` and `B ⪯ A`, and `meet` handles
them correctly, but `A` and `B` keep different canonical bytes and digests.

This is a documented limitation of the initial canonical, not a permission
difference. B5.2/B5.4 MUST compare policy authority with `is_within` (or with
the normalized meet result), never by digest equality. A future canonical that
collapses semantic unions MUST bump `SANDBOX_POLICY_SCHEMA_VERSION` and add new
golden vectors; it MUST NOT silently change the identity of already-published
policies.

#### v1 path bounds

Path selectors are bounded by the same resource argument. One selector has at
most 32 components, one component is at most 255 bytes, one normalized path set
has at most 256 selectors, and one policy has at most 512 selectors across its
five path sets (`read`, `write`, `execute`, `delete`, `visible`). The decoder and
`PathSet::try_allowed` reject an oversized authored collection before pairwise
cover removal, and `NormalizedSandboxPolicy::from_parts` re-checks both the
per-set and aggregate counts. The bounds are part of the v1 grammar: raising them
changes the accepted input language, not the canonical bytes of an already
accepted policy, and still requires an explicit version decision rather than a
silent relaxation.

`PathSet::allowed` remains an infallible constructor for trusted,
already-bounded callers. It runs pairwise cover removal before any later
`from_parts` budget check, so a caller that hands it unbounded authored input
performs quadratic work before the value is rejected. Every current boundary
(the decoder, `NormalizedSandboxPolicy::from_parts`, the catalog publisher) uses
`try_allowed` or an equivalent count check first, so this is an API-misuse risk
rather than a reachable path. Narrowing the constructor to crate-private use, or
making it fallible everywhere, is a non-breaking-to-behavior API cleanup that
does not gate B5.1.

#### v1 name, map and document bounds

The remaining set and map dimensions use the same resource argument. One
allowed-name set (`tools`, `commands` or allowed environment names) has at most
256 distinct members; a policy's `required_values` map, credential grant map and
resource ceiling map each have at most 256 entries; and one credential grant has
at most 256 `uses` and 256 `audience` members. `NameSet::allowed`,
`EnvironmentPolicy::new`, `CredentialGrant::new`, `RequiredAccess::new` and
`NormalizedSandboxPolicy::from_parts` reject an oversized collection, and the
decoder rejects it before constructing a partial policy.
`RequiredAccess::try_with_credentials` re-checks the merged credential set, so a
value cannot cross the bound through a post-construction update; there is no
infallible credential setter.

Those per-dimension bounds alone still allow a maximal policy to be physically
large, so one canonical sandbox policy document is additionally limited to
1,048,576 bytes. The limit is enforced by the value constructor
`NormalizedSandboxPolicy::from_parts`, after normalization: an authored policy, a
decoded policy and the result of `meet` all pass the same check before the value
exists. `sandbox_policy_from_canonical_json` checks the document length before
parsing, and the catalog publisher checks the canonical encoding before hashing
or storing it; those are boundary repetitions of the value invariant, not the
only enforcement point. Consequently a `meet(P_generation, P_task)` whose result
is not representable fails as a `PolicyConflict` inside admission, before any
durable Task, Batch, binding or requirement row is committed. A durable
commitment can never be written that the validated reader would refuse to read.
As with the path bounds, these are v1 input-grammar limits: they do not change
the canonical bytes of an accepted policy, and relaxing them requires an
explicit version decision.

A capability definition with a v2 `PolicyProjection` is bounded as authored
catalog input as well. One projection map has at most 256 members, one member
name or operation token is at most 256 bytes and contains no NUL or `=`, and one
canonical capability definition document is at most 1,048,576 bytes.
`PolicyProjection::new` enforces the map and token bounds,
`capability_definition_from_canonical_json` checks the whole-document budget
before parsing, and the publisher checks the canonical encoding before hashing
or storing it. An oversized definition is therefore rejected at publication and
cannot make a later `load_capability_catalog` traverse unbounded input.

#### v1 `ToolCommandSet` is one abstract operation namespace

The v1 projection dimension is `ToolCommandSet`, not separate `Tool` and
`Command` operations. A policy stores two authoring allowlists, `tools` and
`commands`, but they do not grant two independent capabilities. The effective
`ToolCommandSet` is their union: a required operation named `"git"` is satisfied
by `tools = {"git"}`, by `commands = {"git"}`, or by both. v1 does not
distinguish a tool invocation from a command invocation that share a name, and
it does not promise that a physical compiler can enforce that distinction.

Because the union is the authority, normalization MUST give that union one
canonical spelling: the union of the two finite sets is stored in `tools` and
`commands` is explicit deny-all. If either field is `Any`, the whole namespace is
authorized, so both fields remain `Any`. Consequently

```text
{tools: {git}, commands: {}}  ==  {tools: {}, commands: {git}}
                              ==  {tools: {git}, commands: {git}}
```

as values, canonical bytes and content digests. `meet` and `is_within` operate on
this same effective union, so `{tools: {git}, commands: {}}` and
`{tools: {}, commands: {git}}` are mutually within and their meet still allows
`"git"`. Intersecting the two storage fields separately would violate the
greatest-lower-bound property and make `PolicyAccess` disagree with the ordering.

Because the canonical union is one allowed-name set, the 256-member bound of
§7 applies to the union itself: an authored pair whose combined distinct names
exceed it is rejected at construction rather than split across two fields.

This canonical spelling is part of the initial B5.1 policy canonical, not a
reinterpretation of durable data: B5.1 is the first build to write policy rows.
Any later change to how the union is spelled MUST bump
`SANDBOX_POLICY_SCHEMA_VERSION` and add new golden vectors.

If B5.2/B5.4 need different enforceability guarantees for tool and command
invocation, that requires a new policy canonical/schema version and separate
versioned projection operations. The same string MUST NOT be reinterpreted as a
tool or a command only by the physical compiler, because that would silently
change the meaning of an already-published capability projection.

#### Visibility is observability, not an access precondition

`FilePolicy.visible` is the set of path regions the execution may observe while
enumerating a directory. It is an independent authorization dimension and MUST
NOT be interpreted as a precondition for reading, writing, executing or deleting
an explicitly named path. A hidden path is one that no `visible` selector covers;
there is no separate negation.

Consequently:

- `project_workspace` and `PolicyAccess` derive operation authority only from the
  non-empty `read`, `write`, `execute` and `delete` sets. A deny-all `visible`
  set does not erase a writable region, and `Write` remains satisfied while a
  write operation is authorized;
- `meet` intersects `visible` with the other dimensions independently, and
  `is_within` compares it as its own dimension;
- physical enforcement of directory observability belongs to the B5.2
  enforcement compiler. It MUST NOT be used to infer an operation grant or deny
  one: a policy that hides a path while allowing a known-path write means exactly
  that, not "write is denied".

Treating visibility as an access precondition would change the authorization
meaning of an existing dimension and therefore requires a new ADR and a new
canonical/algebra version; it MUST NOT be introduced as an implementation
convenience.

### 8. Capability algebra stays separate

The existing `Ability`/`Restriction` polarity and matcher rules are preserved.
A Generation capability ceiling may restrict the allowed range; a `Restriction`
continues to strengthen by the existing rules. B.5 MUST NOT rewrite capability
comparison direction while implementing sandbox intersection. A capability that
overlaps a policy dimension uses the single-authority rule in §3.

### 9. Four relations and `can_provision_task`

A read-only `ContractContext` exposes the capability and sandbox policy
catalogs. Only the previously deferred sandbox dimension is added:

- `can_execute` also checks Task minimum access and AgentType/Task policy
  compatibility.
- `can_provision` keeps type-level capability, lifecycle, continuity and coarse
  safety checks and adds type/source-level enforceability for the AgentType and
  SpawnSource/SourceConfig contract. It receives no Task or Generation, so it does
  not decide the full Task-specific `P_eff`.
- `more_specific_for` uses proven policy narrowing, never nominal inheritance
  depth or continuity strength.
- `is_valid_refinement` requires a provable non-widening relation; an unresolved
  "new policy ID" cannot bypass it.
- `can_provision_task` remains the mandatory authority-bearing conjunct.

The responsibility split is therefore explicit:

```text
can_provision
    AgentType + SpawnSource/SourceConfig type- and source-level eligibility
    and their applicable security constraints

can_provision_task / acquisition qualification
    full Task-specific P_eff, including Task and Generation terms

Execution commitment
    re-prove the complete contract against the actual bound identities
```

`can_provision` MUST NOT be described as proving the whole four-way
`P_A ∩ P_G ∩ P_T ∩ P_S`; that claim belongs to the Task-specific stage and is
re-proved at commitment. This is an interface clarification, not a change to the
frozen four relations.

The new order is monotone over the old one:

```text
equality ⊆ new_order
```

Every contract accepted under B.1–B.4 exact equality MUST remain accepted. A
previously rejected "different but narrower" policy becomes legal only through
an explicit catalog declaration. A differential test MUST pin this.

B5.1 applies the mode-aware policy order only where a `SECURITY_V1` revision is
matched: admission of a Task against a pinned `SECURITY_V1` AgentType, and
`based_on` refinement between two `SECURITY_V1` revisions. The frozen
`can_execute`, `more_specific_for` and `is_valid_refinement` relations keep their
exact-reference policy behavior for `COARSE_V1`; a `SECURITY_V1` Task that pins a
`COARSE_V1` AgentType stays eligible at admission and its execution-mode
eligibility is a candidate-stage decision. A `SECURITY_V1` revision may therefore
be refined by another `SECURITY_V1` revision with a different, provably narrower
authored policy, while `SECURITY_V1 -> COARSE_V1` refinement is rejected because
it drops the physical enforcement contract even when the coarse fields and policy
reference are unchanged. B5.1 thus implements this conservative subset of the
§5 ordering; it does not claim the complete secured-execution qualification that
B5.2/B5.4 own.

B5.1 does **not** wire the mode-aware order into existing-agent preselection.
`match_existing_agents_for_task` keeps the frozen `match_existing_agents`
relation, which still compares policy references by equality for both
`COARSE_V1` and `SECURITY_V1` requirements. Consequently a `SECURITY_V1`
AgentType whose authored policy is a strict superset of the Task's effective
policy is admissible under `can_execute_security_v1` but is not returned as an
existing-agent candidate in B5.1.

This is an explicit **scope amendment** to the initial B5.1 slice: B5.1 delivers
the two relations where the policy order is already required for admission and
refinement, and defers the other two to B5.2. It is not an execution-authority
leak, because the coarse acquisition path cannot execute a `SECURITY_V1` Task
and admission already rejects that path. It is nevertheless a hard B5.2
acceptance item, not an optional optimization:

```text
B5.2 MUST deliver, with differential tests:
  1. mode-aware match_existing_agents_for_task over the policy order;
  2. policy-aware more_specific_for (including based_on and preselection);
  3. the same order in can_provision/acquisition when a SECURITY_V1
     existing agent is compared to a SECURITY_V1 Task;
  4. no SECURITY_V1 candidate is silently dropped merely because its
     policy reference differs but its policy is provably narrower.
```

B5.2 MUST NOT be declared complete while a `SECURITY_V1` Task that is admissible
under `can_execute_security_v1` still loses an eligible existing-agent candidate
because preselection uses reference equality. Until those items land, B5.1 MUST
NOT claim that all four relations are policy-order aware.

### 10. `ActualAuthority` is defined by provable equality, not by a claim

`ActualAuthority(E)` is not directly observable. Acceptance uses falsifiable
conditions:

```text
AppliedPlan(E)  ≡ CommittedPlan(E)          // root-relative plan digest equality
AppliedPolicy(E) == P_eff(E)                // effective policy digest equality
+ per-feature positive and negative tests
```

The exact equality tuple and receipt rules are fixed in
[ADR-0014](0014-m6b5-enforcement-compilation-receipt.md). Together the
conditions state that the committed restrictions were enforced on the same
physical instance, not merely that an adapter *could* enforce them.

### 11. Durable identity, explicit admission mode, schema v9, and version gates

The full-security mode is selected **explicitly** at typed admission. It MUST NOT
be inferred from `sandbox_policy.is_some()`, because
`SECURITY_V1 + sandbox_policy = None` is a legal combination.

```text
SecurityContractMode = COARSE_V1 | SECURITY_V1
```

The admission request carries the requested mode. When it is absent the result is
`COARSE_V1`, preserving frozen compatibility. The selected mode is part of the
durable Task contract. Its ownership is fixed, so B5.1 cannot end up with two
competing authority sources:

- `task_agent_requirements` is the **sole owner** of
  `SecurityContractMode` (`security_contract_mode` in schema v9). It is immutable
  and one row per Task;
- `task_security_bindings` existence is a **conditional biconditional**, not an
  unconditional one-per-requirement rule:

  ```text
  security_contract_mode = SECURITY_V1  ⇔  exactly one TaskSecurityBinding
  security_contract_mode = COARSE_V1    ⇔  no TaskSecurityBinding
  ```

  The binding is keyed by the same `task_id` with a foreign key and MUST NOT
  carry an independent mode value. A `SECURITY_V1` requirement without its
  binding, a binding whose requirement is `COARSE_V1`, a `task_id` mismatch, or
  more than one binding is corruption. A `COARSE_V1` Task therefore never gets a
  security row, which is what keeps the frozen coarse path unchanged;
- `generation_task_bindings` is the Generation↔Task admission map and MUST NOT
  carry a second mode marker or act as an alternative authority source. A copy
  kept for readability, if any, is derived and constrained equal; it never
  decides.

`TaskSecurityBinding.policy` is the fine `P_eff` derivation, not the Task's
complete authority. A `ReadOnly` Task under a `Write` Generation that authors no
fine layer retains the Generation's write paths in the binding, while its durable
`required_workspace` still says `ReadOnly`. B5.2/B5.4 execution compilation MUST
therefore compose the binding policy with the durable Task coarse requirement
(and the actual AgentType/SpawnSource terms); it MUST NOT treat the binding policy
alone as the effective Task permission.

Mode declarations are per-object immutable facts, fixed when the object is
created or published — never a mutable "supports" label chosen at admission:

- an AgentType revision's mode and a SpawnSource revision's security declaration
  are written in the same transaction that inserts the immutable revision. There
  is no post-publication declaration API: a revision cannot be published as
  `COARSE_V1` and later reinterpreted in place, and an idempotent republish MUST
  carry exactly the same mode/declaration. Changing either requires a new
  revision;
- each Generation's `SecurityContractMode` follows its immutable
  `generations.policy_mode`, which the frozen schema restricts to `NONE` or
  `POLICY`:

  ```text
  policy_mode = NONE
      → COARSE_V1 only
      → no generation_policies child row
      → cannot admit a SECURITY_V1 Task

  policy_mode = POLICY
      → security_contract_mode owned by the immutable generation_policies
        row written by create_generation (COARSE_V1 or SECURITY_V1, as authored)
  ```

  For a `NONE` Generation the effective mode is the derived `COARSE_V1`; it is
  never stored as a second marker, and B5 MUST NOT backfill a `generation_policies`
  row for an existing `NONE` Generation (the frozen trigger
  `generation_policies_require_policy_mode` rejects one). `NONE` is never an
  implicit unrestricted `SECURITY_V1` Generation. For a `POLICY` Generation the
  policy row exists and there is no separate Generation revision mechanism, and
  the mode is not editable;
- each AgentType revision declares the `SecurityContractMode` it is authored for;
- each SpawnSource **revision** declares whether it carries an authored
  `P_source` fine ceiling and which enforcement protocol/profile versions its
  integration can import. A source revision that carries an authored ceiling is a
  `SECURITY_V1` source revision. A `SECURITY_V1` source revision that declares
  **no** authored fine ceiling has `P_source = Top` for the authorization meet:
  it contributes no restriction, but it is still a `SECURITY_V1` source and MUST
  pass the complete imported-enforcement coverage check and the feature gate.
  `Top` MUST NOT be read as "skip eligibility", "skip profile coverage", or
  "skip credential qualification"; it only means this object adds no authored
  narrowing. Nor does `Top` change the admissible execution path: the source
  revision declares `SECURITY_V1`, so it remains a full-path source and MUST NOT
  be selected by the coarse path merely because it adds no restriction.

Consequently:

- a `SECURITY_V1` Task requires its exact Generation to be `SECURITY_V1`;
  otherwise admission rejects before any durable commitment;
- it also requires the actual bound AgentType to support `SECURITY_V1`; otherwise
  that candidate is ineligible;
- a `COARSE_V1` AgentType MUST NOT execute a `SECURITY_V1` Task;
- in B.5 v1 a `SECURITY_V1` AgentType MUST NOT execute a `COARSE_V1` Task. The
  coarse path cannot prove that the AgentType's authored fine restrictions (for
  example a command allowlist) are enforced, and its coarse projection is not a
  substitute for the full contract. A future dual-mode AgentType with two
  independently versioned mode-specific contracts is possible only through a new
  ADR that proves coarse execution does not exceed the mode's declared ceiling;
- the same conservative rule is symmetric for the other two objects: a
  `SECURITY_V1` Generation MUST NOT admit a `COARSE_V1` Task, because coarse
  admission and dispatch do not evaluate its authored fine ceiling and would
  exceed the Generation's own author limit; and every `SECURITY_V1` SpawnSource
  revision MUST NOT be selected through the coarse path, whether or not it
  carries an authored `P_source` ceiling. A revision with `P_source = Top` adds no
  restriction but still declares the `SECURITY_V1` protocol mode, so it may only
  be used by the complete provisioning and launch path. `Top` does not make a
  single-mode source revision coarse-compatible. A source revision may be
  reachable from both paths only through an independent, explicitly authored and
  versioned mode-specific contract that proves coarse execution stays within its
  declared ceiling; a "supports both modes" label is not such a contract and
  never substitutes for one;
- existing `COARSE_V1` Generations and Source revisions keep their frozen
  contract; the new security semantics use new immutable records or revisions, not
  an in-place reinterpretation;
- `SECURITY_V1` MUST NOT fall back to `COARSE_V1` once the mode is durable — not
  at matching, provisioning, or recovery. A missing security child is corruption,
  not a downgrade opportunity.

The same rule applies to compatibility: a `SECURITY_V1` parent without its
security child is corruption and MUST NOT degrade to `COARSE_V1`.

B5.1 may introduce schema v9 (final numbering follows the actual table freeze).
The `D-DB-MIGRATE` discipline is unchanged:

- v9 uses fresh databases;
- opening a v8 database as v9 MUST be rejected, never silently reinterpreted;
- no implicit in-place migration;
- the version gate rejects any older or newer schema at open.

New rows use the same guards as B.3/B.4: positive parent-side presence markers,
physical write-once (reject `UPDATE`/`DELETE`/same-identity replace), and
authoritative-read coherence. A stored policy reference MUST resolve through the
validated catalog read, and a stored digest MUST equal a recomputation over the
stored canonical bytes.

Catalog publication remains one transaction with immutable content, canonical
digest, and initial disposition. Republishing the same exact `(ref, content
digest)` is idempotent; a different digest for an already-published exact
revision fails closed. A Source security record is published in the same
transaction as its exact source revision; a change requires a new revision.

The declaration MUST be written before the disposition/published marker in that
transaction. The database enforces publication closure: once an
`agent_type_dispositions` or `spawn_source_dispositions` row exists, the schema
triggers reject a first `INSERT` into the corresponding security-declaration
table. Disposition rows cannot be deleted or reinserted, so a published
`COARSE_V1` revision cannot later gain a `SECURITY_V1` declaration through raw
SQL. This is the storage-level counterpart of the API rule that the security
mode is fixed at revision publication.

Old canonical encodings and digests are never reinterpreted because a new field
or algorithm version was added.

## Non-claims

- No B.5 code exists at this baseline; this ADR is design authority, not an
  implementation or freeze claim.
- No claim that every `SandboxPolicy` can run on every adapter. Per-dimension
  support belongs to the imported enforcement matrix.
- No path-level Task requirement, no M6-A TaskSpec extension, no
  `P_config`, and no fifth authored security source.
- `COARSE_V1` replay, exact-reference equality, capability polarity, continuity
  and affinity semantics are unchanged.
- No v8 → v9 migration and no reinterpretation of v8 security rows.
- No general malicious-code sandbox and no claim of same-user read
  confidentiality.

## Consequences

- A Task that cannot satisfy its own minimum under the Generation ceiling is
  rejected at admission; a Task that can satisfy it there may still be excluded
  at provisioning when the actual AgentType or source ceiling is narrower.
- `PlacementConstraints` and candidate-dependent guarantees stay `Pending` at
  admission and are discharged by candidate matching; admission never invents a
  candidate Agent.
- Attempt isolation is a union of authored requirements, not an intersection, so
  meeting policies can never clear it and an adapter's support capability can
  never create it.
- The admission mode is durable and explicit, so a Task cannot be silently
  downgraded when a later stage finds the security path inconvenient.
- The catalog stays small and auditable; derived policies are content-addressed
  facts, not a second mutable policy namespace.
- Two policies with the same canonical bytes share a digest but retain separate
  authored provenance through the recorded input refs.
- A future algorithm change needs a new algorithm version and golden vectors;
  old rows stay readable under their recorded version.

## Relationship to other decisions

- [ADR-0007](0007-m6b-can-provision-task-staging.md) makes `can_provision_task`
  the authority-bearing conjunct; this ADR defines its new sandbox dimension.
- [ADR-0008](0008-m6b-typed-provisioning-acquisition.md) and
  [ADR-0011](0011-m6b-acquisition-contract-parity.md) own typed acquisition and
  the shared physical-phase contract; this ADR does not create a new authority
  center.
- [ADR-0012](0012-m6b4-scope-typed-physical-execution-ack.md) freezes the M6-B.4
  boundary this design extends.
- [ADR-0014](0014-m6b5-enforcement-compilation-receipt.md) owns enforcement
  compilation, physical binding, and the applied-security receipt chain.
- [ADR-0015](0015-m6b5-credentials-feature-gates-scope-freeze.md) owns
  credential qualification, feature gates, and the scoped freeze ladder.
