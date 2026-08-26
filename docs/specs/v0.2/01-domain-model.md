# 01 — Domain Model

Status: Normative
Canonical path: docs/specs/v0.2/01-domain-model.md

Objects below are semantic. Concrete Rust field layouts and SQL names are
IMPLEMENTATION-DEFINED unless a field is marked architecturally significant.

Legend: **RV** Root-visible · **SI** Scheduler-internal · **AB** Adapter-bound ·
**SR** survives restart · **Ver** versioned.

| Object | Identity | Owner | Lifetime | Mutability | RV | SI | AB | SR | Ver |
|---|---|---|---|---|---|---|---|---|---|
| Objective | id | Root (semantic) | problem scope | revisable positive model | Y | N | N | SHOULD | MAY |
| Generation | id | Scheduler record; Root admits | frontier slice | policy frozen at admit | Y | Y | N | Y | N |
| Batch | id | Scheduler | execution barrier | state machine | Y | Y | N | Y | N |
| Task | id | Scheduler | until terminal | state machine | Y | Y | N | Y | N |
| TaskRequirement | part of Task/Proposal | Root intent / compiler | with Task or proposal | immutable after materialize | Y | Y | N | Y | N |
| RawWorkIntent | id | Scheduler record; worker proposes | until Root disposition | immutable evidence | Y | Y | N | Y | N |
| CompiledWorkProposal | id | Scheduler record; compiler produces | until Root decision | immutable | Y | Y | N | Y | N |
| AgentType | type_id + revision | Scheduler registry; Root intent | until GC | revisions immutable | Y | Y | N | Y | Y |
| LogicalAgent | id | Scheduler | until RETIRED | identity immutable | Y | Y | N | Y | N |
| AgentLineage | id | Scheduler | across successors | append-only | Y | Y | N | Y | N |
| AgentTransform | id | Scheduler | saga until terminal | state machine | Y | Y | N | Y | N |
| MemoryCapsule | id + version | Scheduler | with agent/lineage | versioned replace | limited | Y | N | Y | Y |
| PoolPartition | id / name | Scheduler | until RETIRED | V0.1 immutability of structure | Y | Y | partial | Y | topology rev |
| SpawnSource | source_id | config / composition | deployment | config | N | Y | Y | config | N |
| Incarnation | id | Scheduler | one physical hosting | state machine | N | Y | Y | Y | gen |
| Execution | id | Scheduler | one Task turn | physical history | N | Y | Y | Y | N |
| Attempt | id | Scheduler | one claim try | state machine | N | Y | N | Y | epoch |
| Lease | id | Scheduler | one Attempt authority | state machine | N | Y | N | Y | epoch |
| Result | id | Scheduler | durable | ACK only after AVAILABLE | Y | Y | N | Y | N |
| Failure | id | Scheduler | durable | append-only | Y | Y | N | Y | N |
| Escalation | id | Scheduler | until resolved | open/resolved | Y | Y | N | Y | N |
| Checkpoint | id | Scheduler | fenced promotion | versioned | limited | Y | N | Y | Y |
| ContinuityBinding | opaque handle | Scheduler stores; Adapter interprets | until invalid | opaque | N | Y | Y | Y | N |
| Outbox event | event_id | Scheduler | until ACKED | at-least-once | indexes | Y | via bridge | Y | N |

## Meaning (MUST)

**Objective** MAY be represented as Root-owned problem scope. Exact schema
DEFERRED ([17](17-deferred-open-questions.md) D-OBJECTIVE).

**Generation** is a semantic-frontier barrier, not an organizational layer. See
[04](04-generation-and-frontier.md).

**Batch** is an execution/synchronization completion unit. See [03](03-task-attempt-lease-result.md).
A Generation MAY contain multiple Batches.

**Task** is durable schedulable work. Creating a Task MUST NOT establish
execution authority.

**TaskRequirement** states type/affinity/anchor/capability/sandbox needs. It
MUST NOT encode a model name as identity.

**RawWorkIntent** is domain-semantic and architecture-light. Workers MUST NOT
be required to understand Scheduler internals to emit one. Lifetime lasts
until **Root** disposition (admit / reject / defer / accept a redundancy
candidate). Compiler rejection MUST NOT end the intent's Root-visible life.

**CompiledWorkProposal** is architecture-aware and execution-unbound. It MUST
NOT normally bind logical_agent_id, incarnation_id, attempt_id, lease_id, or a
concrete SpawnSource.

**AgentType** is responsibility/capability/security/lifecycle/continuity. It
MUST NOT be a model, provider, terminal, price tier, prompt alias, or
PoolPartition.

**LogicalAgent** is long-lived semantic identity. **Incarnation** is one
physical hosting period. **Execution** is one Task-scoped runtime turn.
**Attempt** is Scheduler authority over one try to complete a Task.

**AgentLineage** preserves continuity across Transform successors.

**AgentTransform** is an intentional semantic transition, not MOVE.

**MemoryCapsule** is Scheduler-owned bounded structured continuity. Transcript
MUST NOT be MemoryCapsule.

**PoolPartition** in the V0.1 kernel is desired resident capacity plus
retention and (historically) one ExecutionTarget/Profile. V0.2 SHOULD evolve
it toward desired population of an AgentType at an anchor. Until that
evolution is specified, kernel MOVE/MERGE/RETIRE rules in [11](11-pool-topology.md)
remain. Exact V0.2 partition-vs-type split remainder is DEFERRED (D-TOPOLOGY).

**SpawnSource** is physical provisioning capability. It MUST NOT be semantic
identity.

**Lease** plus matching fencing epoch is execution authority.

**Result** is the unique authoritative durable outcome of a completed Task.

**ContinuityBinding** is an opaque adapter handle. Core MUST NOT embed vendor
thread/session semantics. Storage/security details DEFERRED (D-CONTINUITY-BIND).

**Outbox event** is wakeup/control. It MUST NOT carry Result payload.

## Provenance (MUST be representable)

- Task belongs to one Generation and one Batch.
- Attempt belongs to one Task; Lease belongs to one Attempt.
- Execution belongs to one Attempt; Incarnation hosts Executions.
- RawWorkIntent originates from a Result when policy allows.
- CompiledWorkProposal originates from a RawWorkIntent.
- Transform successor LogicalAgent `supersedes` source; same AgentLineage.
- Result `derived_from` Task; evidence refs are first-class.
- Causal graph MAY include caused_by, depends_on, derived_from, audits,
  supersedes, evidence_for, evidence_against.
- Core MUST NOT persist reports_to, manager_of, or subordinate_of.
