# Flat Organization, Generations, and Bounded Frontier Expansion

Status: Architecture direction
Canonical path: docs/design/v0.2/03-flat-organization-and-generations.md

## 1. Flat actor topology

Agentype must not evolve into a persistent hierarchy such as:

`Root → manager → team lead → worker → temporary worker`

Information flow may have arbitrary depth, but authority hierarchy should remain flat.

Persist causal relations such as:

- caused_by;
- depends_on;
- derived_from;
- audits;
- supersedes;
- evidence_for;
- evidence_against.

Do not create Core semantics such as reports_to, manager_of, or subordinate_of.

## 2. Workers do not own frontier expansion

A worker may discover that additional work is useful.

It must not directly create an executable Task merely because it discovered that need.

Worker output first enters Result as RawWorkIntent.

> Workers may propose work. They do not expand the work frontier.

Root owns semantic frontier admission.

## 3. Generation

Generation is a semantic frontier barrier, not an organizational level.

Possible fields:

- generation_id;
- parent_generation_id;
- objective_ref;
- policy;
- state;
- admitted_at;
- closed_at.

Every Task belongs to a Generation.

A Generation may represent a coherent wave such as EXPLORE, IMPLEMENT, VERIFY, AUDIT, or REFINE. These are policy modes, not AgentType names.

## 4. GenerationPolicy

A Generation applies a small global constraint set to all work in that frontier.

Possible fields:

- semantic_mode;
- allow_work_intents;
- expansion_budget;
- allowed task operations;
- mutation policy;
- sandbox ceiling;
- acceptance policy.

Effective restriction is approximately:

`AgentType policy ∩ Generation policy ∩ Task policy`

## 5. Audit Generation

A pure audit Generation is a convergence operation.

Typical constraints:

- read-only;
- no implementation mutation;
- no frontier expansion;
- no RawWorkIntent emission.

It may emit PASS, FAIL, CONTRADICTION, UNCOVERED_RISK, or INSUFFICIENT_EVIDENCE.

Any recommended follow-up remains a finding for Root to consider later.

## 6. Explore Generation

Exploration may be allowed to emit bounded RawWorkIntents because discovering unknowns is part of exploration.

But:

`proposal != admission`

Boundedness can be enforced mechanically with limits such as max intents per Result or per Generation.

## 7. Review barrier

A Generation becomes REVIEWABLE when:

- no more Tasks in the Generation can run;
- authoritative Results are durable;
- generated RawWorkIntents are durable;
- WorkIntent compilation pass is complete if configured.

Root then performs one semantic review and chooses stop, reject, defer, or admit the next Generation.

## 8. Batch vs Generation

Batch answers:

> Which Tasks form an execution/synchronization completion unit?

Generation answers:

> Which work belongs to one semantic frontier?

Therefore:

`Batch = execution barrier`

`Generation = semantic frontier barrier`

A Generation may contain multiple Batches.

## 9. Temporary teams are emergent

A set of agents becomes a temporary team because of shared Generation, anchor, task dependencies, type affinity, and Result flow.

No durable Team manager object is required.

> Change the topology of semantic attraction, not the hierarchy of command.
