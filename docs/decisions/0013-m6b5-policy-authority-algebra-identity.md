# ADR-0013 — M6-B.5 policy authority, sandbox algebra, and durable security identity

Status: Proposed (M6-B.5 / B5.0 normative design; pending maintainer audit; not a milestone freeze)
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
  upper bound on write authority, `Disabled` is a network restriction, and
  `Restricted` is an execution mode; none of them is a positive access
  requirement.
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
required_workspace = ReadOnly ⇒ P_task contains no write
required_network follows the existing rank/polarity
required_attempt_isolation    ⇒ the candidate carries the isolation guarantee
```

The two coarse terms have different sources: M6-A `TaskSpec` carries only
`workspace_mode`; `required_network` comes from the typed requirement draft, and
the fine policy is the draft's optional `sandbox_policy`. The spec MUST NOT
attribute `required_network` to `TaskSpec`. `required_attempt_isolation` is also
carried by the draft (and folded with the Generation policy, see §3).

Consequently a coarse/fine pair such as `workspace = ReadOnly` with a fine
write root MUST be rejected.

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
restriction", so the Task policy layer is the explicit top
and `P_task` reduces to the two coarse terms — `TaskSpec.workspace_mode` and
`requirement_draft.required_network` (see §3). It never widens another ceiling,
and the coarse fields remain authoritative in their own sources. AgentType and
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
| Typed admission | `RequiredAccess ⊆ ProjectAccess(meet(P_generation, P_task))`, `CoarseExecutionModes` compatibility with the Generation ceiling, Task-local coarse/fine consistency | `RequiredCapabilities` (they need the actual AgentType), `PlacementConstraints`, and guarantees that require a physical candidate |
| Candidate matching/acquisition | `RequiredCapabilities` against the actual AgentType claims through their catalog matchers, `RequiredGuarantees` physical eligibility, `PlacementConstraints` through the frozen M5 placement predicate, whole-policy enforceability, **and every positive Task requirement (`RequiredAccess`, `RequiredCapabilities`, `CoarseExecutionModes`) re-evaluated against the complete `P_eff`** | — |
| Execution commitment | exact binding and the complete security contract, re-proven against the committed snapshot, including every positive Task requirement and `CoarseExecutionModes` against the committed `P_eff` | — |

Only `Unsatisfied` rejects. For a `SECURITY_V1` Task, `Pending` components are
recorded in `TaskSecurityBinding` as an explicit obligation set and MUST be
discharged at the candidate stage; a `COARSE_V1` Task has no binding and no such
obligation set. Admission MUST NOT assume that a candidate Agent exists: a Task
may name a workstream that no future Agent matches, and that is a candidate
ineligibility, not an admission contradiction.

`RequiredCapabilities` is deferred at admission because admission has only the
Generation and Task policy objects, while the existing M6-B.2 matcher compares a
requirement against the actual AgentType's capability claims. Moving it earlier
would require inventing a candidate, and collapsing it into `ProjectAccess` is
exactly the quantity/rank/exact confusion this split forbids.

Only the `RequiredAccess` component is a set inclusion. `RequiredCapabilities`
use their catalog matchers, and `RequiredGuarantees`, `CoarseExecutionModes`, and
`PlacementConstraints` use their own predicates. There MUST be exactly one
relation, not two implementations that disagree about which `R_min` dimensions
are set-like or about which stage owns placement.

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

### 6. Dimension rules for `meet`

The algebra is defined dimension by dimension; "intersect the allowed sets" is
not sufficient:

| Dimension | Composition rule |
|---|---|
| allowed file operations and path ranges | set intersection per operation class |
| hidden/prohibited ranges | keep common restrictions; normalize to equivalent allowed/prohibited regions |
| tools, commands | allowed-set intersection |
| network endpoints/protocols/ports | meet in that dimension |
| environment variables | allowed-name intersection; conflicting required values reject |
| credential scope | refs, use, delivery mode and audience all narrow; an omitted grant in the AgentType ceiling is empty, not `Top`, and other ceilings can only narrow |
| resource ceilings | stricter value within the same unit and scope |
| working directory | validate as a launch constraint; conflict rejects rather than choosing one |
| execution guarantees (`requires_attempt_isolation`) | **disjunction, not intersection**: `I_required = I_AgentType ∨ I_Requirement ∨ I_SourceRequirement`; any true disjunct makes isolation required, and no meet can remove it |

`P_eff` covers allowed operations and ceilings only. Guarantees live beside it:
meeting two policies can never clear an isolation requirement, and an
`EnforcementSupport` capability cannot create one (see §3).

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
  safety checks and adds whole-policy enforceability.
- `more_specific_for` uses proven policy narrowing, never nominal inheritance
  depth or continuity strength.
- `is_valid_refinement` requires a provable non-widening relation; an unresolved
  "new policy ID" cannot bypass it.
- `can_provision_task` remains the mandatory authority-bearing conjunct.

The new order is monotone over the old one:

```text
equality ⊆ new_order
```

Every contract accepted under B.1–B.4 exact equality MUST remain accepted. A
previously rejected "different but narrower" policy becomes legal only through
an explicit catalog declaration. A differential test MUST pin this.

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

Mode declarations are per-object immutable facts, fixed when the object is
created or published — never a mutable "supports" label chosen at admission:

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
  narrowing.

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
  exceed the Generation's own author limit; and a SpawnSource revision carrying an
  authored `P_source` MUST NOT be selected through the coarse path — it may only be
  used by the complete `SECURITY_V1` provisioning and launch path, which proves
  its restriction. A "supports both modes" label is not a proof and never
  substitutes for mode-specific contracts;
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
