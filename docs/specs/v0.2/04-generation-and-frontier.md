# 04 — Generation and Semantic Frontier

Status: Normative
Canonical path: docs/specs/v0.2/04-generation-and-frontier.md
Conformance: **M6 only**. M4 MUST NOT implement Generation.

Generation is a **semantic frontier barrier**, not an organizational level
and not a Batch.

## Ownership

Root MUST retain semantic frontier admission.
Admitting a Generation MUST materialize a bounded slice whose **scope and
expansion ceiling are fixed by Root** at admission.
GenerationPolicy MUST constrain admitted work.
A Generation MUST NOT receive independent frontier-admission authority.
Workers MUST NOT receive spawning authority from a Generation.

## States

These names are the V0.2 normative machine. Exact extra flags on policy are
DEFERRED (D-GEN-POLICY).

| From | Operation | To | Who |
|---|---|---|---|
| (none) | Root admits bounded slice | OPEN | Root intent; Scheduler persists |
| OPEN | first Task materialized or work running | ACTIVE | Scheduler |
| ACTIVE | drained + durable Results + intents + compilation-if-configured | REVIEWABLE | Scheduler |
| REVIEWABLE | Root reject/defer/close without successor | CLOSED | Root |
| REVIEWABLE | Root admits successor slice | CLOSED (this) + OPEN (next) | Root |
| OPEN/ACTIVE | Root or policy cancel | CANCELLED | Root |
| OPEN/ACTIVE | blocking Escalation / safety stop | SUSPENDED | Scheduler |
| SUSPENDED | Root/scheduler resume | ACTIVE or OPEN | Scheduler |

`CLOSED` and `CANCELLED` are terminal for that id.

### REVIEWABLE (MUST)

A Generation becomes REVIEWABLE only when:

1. no Task in the Generation can still run (all terminal, or only blocked in a
   way the drain definition treats as stopped — drain exactness DEFERRED
   D-GEN-POLICY if it depends on policy encoding);
2. authoritative Results for completed Tasks are durable;
3. generated RawWorkIntents are durable;
4. WorkIntent compilation pass is complete if configured.

Root Result ACK MUST NOT be required for REVIEWABLE.

### Mechanical work MUST NOT advance Generation

Retry, recovery, adapter reconciliation, and revival MUST remain inside the
originating semantic Task/Generation. They MUST NOT create a new Generation.

## Task materialization

Every semantic Task MUST belong to exactly one Generation.

Who may request materialization: Root (admission / review).
Scheduler MUST persist.
Workers and compilers MUST NOT materialize executable Tasks.

Whether Root MAY add Tasks to an already OPEN/ACTIVE Generation after initial
admission is DEFERRED (D-GEN-INTRA). Until resolved, implementations SHOULD
treat initial admission as the Task set unless a later spec row says otherwise.

## Expansion bound

Every Generation MUST have a bounded expansion policy.
Audit/verification Generations MAY be non-expansive: read-only, no mutation,
no RawWorkIntent, no frontier expansion. Follow-ups remain findings for Root.

Workers MAY emit RawWorkIntent only if that Generation's policy permits.
`proposal != admission`.

Bounds MUST be mechanical (counts/budgets), not prompt reminders.
Exact budget representation is DEFERRED (D-GEN-POLICY).

## Provenance

A Generation MAY record `parent_generation_id`. Whether the graph is a chain
or a DAG is DEFERRED (D-GEN-TOPOLOGY). Implementations MUST still persist
enough provenance to explain successor admission.

## Batch

A Generation MAY contain multiple Batches.
Batch completion MUST NOT auto-admit the next Generation.
