# 13 — Storage and Transactions

Status: Normative
Canonical path: docs/specs/v0.2/13-storage-and-transactions.md

SQL table names, column names, and indexes beyond the semantic unique
constraints are IMPLEMENTATION-DEFINED.

## Source of truth

Scheduler durable state MUST have a single-machine authoritative store for
the first Rust V0.2 line. For M4 that store MUST be SQLite with WAL and
`synchronous=FULL` (UNCHANGED from V0.1). Core domain types MUST NOT leak
SQL types into semantic APIs where avoidable. A later storage backend is
out of M4 scope.

V0.1 database migration/import vs new database is DEFERRED (D-DB-MIGRATE).
M3 MAY start a new DB. Import MUST be specified before any claim of
in-place upgrade.

Times MUST be UTC epoch seconds or equivalent unambiguous UTC.
IDs MUST be unique durable strings (UUID recommended, IMPLEMENTATION-DEFINED).

## Schema version gate (MUST)

The Rust-era store carries an exact schema version in `schema_migrations`.
A database whose version is newer **or** older than the running binary's
supported `SCHEMA_VERSION` MUST be rejected at open, fail closed. There is
**no** in-place upgrade while `D-DB-MIGRATE` is unresolved, so v5 -> v6 (and
v6 -> v7) is deliberately **not** a migration; each stage uses a fresh database
at its own version.

M6-B.3 (schema v7) adds the typed admission surface on top of the frozen v6
catalog:

- `task_agent_requirements` is immutable and write-once, created only inside the
  same transaction as its M6-A `Task` and `GenerationTaskBinding`. It stores the
  canonical requirement document, its Core-computed digest, and a **mandatory**
  exact `(type_id, revision)` pin; the pin MUST resolve through the catalog's
  validated read, and the relational mirror MUST be cross-checked against the
  canonical document on every read. The parent `tasks.agent_requirement_mode` and
  this row are one invariant: a `TYPED` Task without its row, or a `LEGACY` Task
  with one, is corruption that MUST fail closed. The read MUST also resolve the
  pin through the catalog's validated read and cross-check the duplicated
  TaskSpec-owned dimensions (information function, affinity, workspace, folded
  continuity) against the authoritative Task and `GenerationTaskBinding`. A
  requirement row may only be inserted under a parent Task whose
  `agent_requirement_mode` is already `TYPED`, so a legacy Task cannot be
  retrofitted.
- `logical_agent_type_bindings` is immutable and write-once: one exact
  `(type_id, revision)` per LogicalAgent, resolved through the validated read. A
  change is a new binding, never an in-place type mutation. A binding may only be
  minted for a `READY`, unassigned LogicalAgent.
- `generation_policies` is immutable: a generation-wide authority ceiling is
  fixed at generation creation and applied as the spec 10 intersection on each
  typed admission (a Task exceeding it fails closed). A policy row may only be
  inserted under a `POLICY` generation, and admission reads policy presence only
  through the authoritative `get_generation_policy`.
- `tasks.agent_requirement_mode` (`LEGACY`/`TYPED`) and
  `generations.policy_mode` (`NONE`/`POLICY`) are positive parent-side presence
  markers fixed at creation/admission and immutable thereafter. A missing child
  row for a positive marker, or an unexpected child row for a negative marker, is
  corruption, never a silent downgrade to legacy or an unconstrained Generation;
  SQLite triggers forbid mutating a Task's typedness and a generation's policy
  mode.
- all three tables MUST be physically write-once: SQLite triggers reject
  `UPDATE`, `DELETE`, and a duplicate/`INSERT OR REPLACE` that would rewrite an
  existing exact identity, so direct SQL cannot rewrite a requirement, rebind an
  agent, or mutate a policy.
- typed/legacy scheduling eligibility MUST be a pure Core decision
  (`ClaimTaskSnapshot.typed`, `ClaimAgentSnapshot.type_bound`), not a storage
  query convention: coarse SQL filtering is a performance optimization only and
  MUST NOT change scheduler behavior.

The M6-B.2 Agent Contract catalog (schema v6) MUST persist immutable revision
content **separately** from the mutable disposition overlay, so a disposition
change never alters a revision content digest:

- immutable revision content: capability definitions, AgentType revisions,
  SpawnSource revisions, SourceConfig revisions, AdapterBindingPolicy
  revisions, each with a Core-computed canonical content digest;
- mutable disposition overlays: AgentType `PUBLISHED`/`DEPRECATED`,
  SpawnSource/SourceConfig/AdapterBindingPolicy `ACTIVE`/`DRAINING`/`DISABLED`,
  monotonic and never entering revision content or digests. A fresh publication
  MUST persist the caller's initial disposition (never silently substitute
  `ACTIVE`); re-publishing an existing exact revision is content-idempotent and
  MUST NOT mutate the live disposition.

A published exact revision's content digest is a durability witness: a read at
the catalog authority boundary MUST recompute the digest from the stored
canonical document, re-canonicalize the decoded record (decode -> canonicalize
against the catalog -> re-encode -> byte equality), and MUST NOT accept a
document that is not the unique canonical encoding of the record it decodes to.
Every duplicated relational column (identity, `based_on`, adapter policy,
capability semantics, config body/mode/locator/digest) MUST be cross-checked
against the canonical document, so no mirror column becomes a second authority.
An idempotent republish of an existing exact revision MUST re-run this validated
read before reporting success, and every aggregate's publication boundary MUST
enforce that aggregate's own value invariant (for example a non-blank
`AdapterBindingPolicy` `adapter_kind`/`binding_ref`), storing the value verbatim.
A revision row with a missing disposition overlay is corruption that MUST fail
closed with one meaning everywhere: reads, disposition setters, idempotent
republish, and selector resolution. A publication MUST NOT create or repair an
overlay for an existing exact revision; only a fresh immutable publication
creates one, in the same transaction. A disposition setter MUST distinguish an
absent revision (`NotFound`) from an existing revision whose overlay is missing
(corruption). Resolving a selector MUST NOT silently drop such a revision or
fall back to another revision.

Pre-commit `AgentTypeSelector` resolution MUST build its published set from the
same validated read, so selector resolution is not a second, weaker authority.
Any corrupt published revision MUST fail the whole lookup closed (no silent drop
and no fallback to another revision).

A new immutable revision that references another catalog revision (an AgentType
`based_on`, a SpawnSource `adapter_policy`) MUST resolve the referenced revision
through its validated read, not merely verify that the row exists; a corrupt
dependency MUST fail closed rather than propagate into new durable facts. A
non-`ACTIVE`/`DEPRECATED` dependency may still be referenced; only corruption is
rejected. Repeating the current disposition MUST be an idempotent no-op that
preserves the transition timestamp, for every disposition overlay (AgentType,
SpawnSource, SourceConfig, AdapterBindingPolicy).

The immutable-revision / monotonic-disposition boundary MUST also be enforced by
SQLite itself, not only by the Kernel API: `BEFORE UPDATE` and `BEFORE DELETE`
triggers MUST reject any mutation of an immutable revision row; a disposition
overlay row MUST have immutable identity and lifetime, so SQLite MUST reject its
deletion, its re-insertion (including the `INSERT OR REPLACE` conflict path,
which resolves before a delete trigger fires while `recursive_triggers` is off),
and any rewrite of its primary-key columns; and `BEFORE UPDATE OF status`
triggers on the overlays MUST reject a status reversal. A direct SQL statement
that bypasses the Kernel transaction therefore cannot mutate frozen content,
reverse a disposition, or delete-and-recreate an overlay to resurrect a prior
state.

A config digest MUST have the canonical grammar `sha256:` followed by 64
lowercase hexadecimal characters, for both `OpaqueJson` and `ExternalRef`
configs, before it enters a durable revision.

The complete durable SourceConfig revision content is the `SourceConfigRevision`
(`SourceConfig` metadata + body `mode` + `ExternalRef` `locator`); its canonical
content digest is the only durable revision identity. A metadata-only relation
(such as `SourceConfig::same_config_contract_content`) MUST NOT be used as, or
named as, durable revision content identity, because it cannot distinguish two
configs that differ only by locator.

An opaque SourceConfig body or locator is source-private: Core stores it
without interpreting it. An `OpaqueJson` body's declared `config_digest` MUST
equal its canonical body digest, and it is durable **non-secret** configuration
material: provider/vendor secrets MUST NOT be persisted there but belong behind
`ExternalRef`/`CredentialRef`, so the Scheduler store never becomes a
provider-secret store. An `ExternalRef` freezes **both** an opaque `locator`
(where the configuration lives) and the declared `config_digest` (which content
version is behind it). Location and identity are distinct fields/columns; their
*values* are source-private and may coincide (for example a content-addressed
locator), so Core MUST NOT reject a value equality while it MUST NOT store one in
place of the other. Core MUST reject an all-whitespace locator but MUST store a
non-empty locator exactly as supplied (it MUST NOT trim a source-owned opaque
identity). A single validated `SourceConfigRevision` read MUST be the only
SourceConfig read path; getters MUST NOT read the duplicated columns
independently, and a later stage MUST read the opaque body through that validated
record rather than issuing its own query. The validated revision type MUST be
unforgeable: private fields, no public constructor, read-only accessors, and a
`body` accessor dispatched on the validated body mode, so a workspace sibling
cannot construct a record that merely looks validated.

## Kernel unique constraints (MUST)

- one ACTIVE lease per Task
- one ACTIVE Attempt per Task
- one ACTIVE Attempt per LogicalAgent
- one Execution history row per Attempt
- one authoritative Result per Task
- one open Escalation per suspended Task
- one assigned LogicalAgent per Task
- one STARTING/WARM/COLD Incarnation per LogicalAgent
- at most one STARTING/RUNNING/UNKNOWN Execution per Incarnation

## Transaction boundaries (MUST be atomic)

| Operation | Includes |
|---|---|
| Batch submit | Batch + Task graph + dependencies + initial BLOCKED/QUEUED |
| Claim | fencing epoch increment + Attempt + Lease + LogicalAgent ASSIGNED |
| Execution create | Execution associated with Attempt and Incarnation |
| Confirm RUNNING | Positive RUNNING transition **and first Lease renewal** in one fenced Core transaction **before** daemon supervision admission. MUST NOT commit Execution RUNNING then renew later. |
| Success ACK | Attempt SUCCEEDED, Lease RELEASED, Task COMPLETED, exactly one Result AVAILABLE, dependency release, Batch recompute. If this transaction is the **first** `Batch → COMPLETED`, it MUST also insert **exactly one** `BATCH_RESULTS_READY` outbox row. MUST NOT complete Batch in tx1 and enqueue wakeup in tx2. |
| Retryable NACK | Failure, Attempt FAILED, Lease RELEASED, Task RETRY_WAIT, agent release |
| Suspend | Task SUSPENDED, Lease REVOKED, Escalation, Batch SUSPENDED, and the decision/control outbox event in the **same** transaction |
| Result ACK | Result state only |
| Checkpoint promote | matching Attempt + epoch |
| Topology mutation | revision + desired membership (V0.1 rules) |
| LogicalAgent RETIRED | RETIRED observable only with every STARTING/WARM/COLD Incarnation of that agent fenced LOST in the **same** transaction (excess retire, assignment-boundary retire, Transform source retire) |
| Generation REVIEWABLE | drain predicates + durable intents/proposals flags |
| Proposal persist | intent → proposal outcome; MUST NOT admit |
| Transform cutover | single transaction: successor create + lineage + topology cutover + source RETIRED + source live Incarnations fenced LOST + writer safety held. Durable state jumps TARGET_READY → COMPLETED. No persisted split-brain CUTTING_OVER. **Explicit freeze (option A)**, not a literal copy of the design saga's CUTTING_OVER row. |
| MemoryCapsule version | MUST NOT be hidden LLM; promotion protocol DEFERRED so this tx MUST NOT auto-apply worker deltas |
| Catalog publish | immutable revision content + canonical content digest + initial disposition in one transaction. Republishing the same exact `(ref, content digest)` is idempotent; a different digest for an already-published exact revision fails closed as an invariant violation |
| Typed admission | M5 Task + GenerationTaskBinding + immutable TaskAgentRequirement in one transaction, with the exact AgentType pin resolved through the validated catalog read and the Generation policy folded in. A failure leaves none of the three; no SpawnSource is selected and no external I/O occurs |
| Generation policy | generation row + immutable `generation_policies` row in one transaction |
| Agent type binding | write-once `logical_agent_type_bindings` row for an already-materialized LogicalAgent; no physical provisioning |

Stale writes MUST fail closed (no canonical mutation). Physical-only history
MAY still record on the old Execution/Incarnation.
