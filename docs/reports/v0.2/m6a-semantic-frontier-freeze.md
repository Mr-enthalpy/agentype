# RIIR M6-A — Semantic Frontier Kernel Freeze

Status: Historical Report
Applies to: V0.2 / M6-A closure (PR #16)
Canonical path: `docs/reports/v0.2/m6a-semantic-frontier-freeze.md`
Not a specification.

This is a milestone boundary marker. It records that M6-A is closed and frozen
on top of the frozen M5 runtime substrate, and that it satisfies the freeze gate
in the M6-A design. It adds no feature and supersedes no specification.

## Status

```text
M6-A  semantic frontier kernel     FROZEN
```

M6-A is closed.

The next design/implementation phase is M6-B (Agent Contract & SpawnSource
Resolution), which is not specified here.

## Frozen surface

```text
Generation lifecycle              OPEN -> FROZEN -> CLOSED (strictly monotonic)

InformationFunction               EXPAND
                                  COMPRESS_POSITIVE
                                  COMPRESS_NEGATIVE

Generation                        durable semantic admission frontier
CompiledWorkProposal              durable, unprivileged candidate work item
GenerationTaskBinding             task <-> generation membership + frozen inputs

RawWorkIntent                     unprivileged ingress value (no authority, no
                                  independent durable identity; durability
                                  inherited from the source Result)
IntentSource                      typed ingress provenance
                                  Root   -> Root command reference
                                  Result -> durable ResultId (verified)
SemanticInputSet                  immutable provenance value object
ArtifactRef                       content-addressed artifact reference
GenerationView                    derived, rebuildable read model

RootSemanticControl               create_generation
                                  compile_root_intent
                                  admit_proposal
                                  reject_proposal
                                  freeze_generation
                                  close_generation
                                  read_generation_view
                                  read_proposal

IntentIngress                     compile_from_result   (proposal-only; no
                                  admit / reject / freeze / close)
```

## Frozen invariants

The M6-A kernel freezes INV-A1 through INV-A20 as stated in the design. The
load-bearing ones:

```text
every M6 semantic Task belongs to exactly one Generation
legacy M4/M5 Tasks need not belong to any Generation
RawWorkIntent and CompiledWorkProposal carry zero execution authority
admission atomically creates the M5 Task and the GenerationTaskBinding
a proposal admits at most one Task
no EXPAND admission after FROZEN; no admission of any function after CLOSED
Generation close never manufactures Task terminality
the barrier reads M5 Task authority, never physical Execution state
every compression Task has an immutable SemanticInputSet
compression never replaces or invalidates its source Results
provenance edges carry no scheduling authority
semantic depth creates no durable manager hierarchy
worker-generated expansion is non-expansive until explicitly admitted
Generation state changes and wakeup outbox insertion are atomic
Root attention/focus is not Scheduler durable state
```

## Admission and replay contract

Frontier state governs creation of new semantic commitments; it does not govern
replay of commitments that already durably exist.

```text
compile_intent resolves the durable proposal on its stable identity
(generation_id, source_kind, source_ref, raw_intent_key, compiler_version)
before consulting current Generation state, so a proposal committed before
freeze or close replays to its final state instead of failing.
```

Intent provenance is typed, not free-form: `IntentSource::Root` is anchored to a
Root command reference, and `IntentSource::Result` is mechanically anchored to a
durable `Result` (a missing result fails closed). The proposal-only
`IntentIngress::compile_from_result` exposes exactly that result-backed path and
holds no admission, freeze, or close authority; `RootSemanticControl` keeps the
sole `compile_root_intent` plus frontier authority.

`RawWorkIntent` is an ingress value, not a durable Scheduler record:
worker-originated durability derives from the durable source `Result`, and
`CompiledWorkProposal` is the first Scheduler-owned durable review object for
it. Admission override consistency and crash replay compare `TaskSpec`s under
canonicalization (`canonical(A) == canonical(B)`), never Rust struct equality,
so a committed spec replayed with its original, differently-ordered affinity
tags, retry classes, or dependencies returns the same `TaskId`. The M5 retry
contract is preserved unchanged: `base_backoff_seconds >= 0` and
`max_backoff_seconds >= base_backoff_seconds` remain legal.

The derived `GenerationView.is_settled` field is a terminal projection distinct
from the `close_generation` gate: it is true for a settled `FROZEN` generation
and remains true for its `CLOSED` successor
(`state == CLOSED || GenerationSettled(G)`).

## Reused unchanged from M5

M6-A adds no semantics to, and does not redefine, the frozen M5 substrate:

```text
Task / Attempt / Lease / Result authority
Lease fencing and writer safety
Execution and physical observation
retry / recovery / reconciliation
Batch (execution barrier)
notification outbox and RootBridge
LogicalAgent / Incarnation
production daemon composition root
```

Each admitted semantic Task is materialized into a fresh dedicated mechanical
execution batch, so completion or cancellation of prior tasks never blocks a
dynamic admission into an `OPEN` Generation.

## Negative space

M6-A deliberately does not implement:

```text
AgentType / SpawnSource contract system
provider / model / config abstraction
Transform
MemoryCapsule
Root context / attention / zoom persistence
manager-agent tree or worker-to-worker delegation
transcript ingestion or model-based truth adjudication
automatic policy-governed admission
```

## Correctness authority

M6-A correctness remains defined by the V0.2 normative specs and architecture
documents. This report does not supersede them.

```text
spec 01 domain-model                   normative ingress/durability model
spec 04 generation-and-frontier        normative frontier contract
spec 05 work-intent-compilation        normative compilation contract
spec 17 deferred-open-questions        D-GEN-* / D-INTENT-* / D-ROOT-API resolved
architecture v0.2/m6a-semantic-frontier-kernel
```

## Freeze-gate resolution

The M6-A freeze audit of head `ae34364` returned:

```text
P0 = 0
P1 = 2   closed in this PR (compile replay durability; CLOSED settled projection)
P2 = 3   closed in this PR (batch naming doc; D-ROOT-API surface; control boundary comment)
```

Both P1 defects concerned durability of the first durable semantic authority
surface and were closed by making `compile_intent` replay stable and by deriving
the `GenerationView` settled projection from terminality, with regressions for
EXPAND-after-freeze, ADMITTED/REJECTED-after-close, COMPRESS-after-close replay,
and the CLOSED view projection.

A second closure review of
`main@c174f1585a82d6b3fac3ba30c4c374485d25cb9e` returned:

```text
P0 = 0
P1 = 2   closed in the closure PR (canonical TaskSpec replay; normative RawWorkIntent model)
P2 = 1   closed in the closure PR (zero retry backoff held legal for M5 parity)
```

The canonical-replay defect made `admit_proposal` compare a canonically-stored
`TaskSpec` against a raw override with Rust struct equality, so a committed spec
replayed with differently-ordered vectors wrongly conflicted; it is closed by
comparing canonical forms. The normative-ingress defect is closed by stating in
specs 01 and 05 that `RawWorkIntent` is an ingress value with no independent
durable identity, and that `CompiledWorkProposal` is the first durable review
object. The M6-A freeze claim holds only after this closure lands.

A third review of
`main@d4bf49a1ade4d1ef2002a6c47dc6872f72a32e68` returned `PASS WITH P1 CLOSURE`
with two thematic P1 items and four non-blocking P2 items, closed by two narrow
PRs on top of that baseline:

```text
P1  intent ingress / provenance boundary   closed (typed IntentSource; proposal-only IntentIngress)
P1  normative intent / proposal scope      closed (D-INTENT-SCHEMA resolution; M6-A TASK-only fence)
P2  schema hardening, rollback/restart proof, SemanticInputSet
    multiplicity, root README status       queued, non-blocking
```

The ingress boundary is now typed and result-anchored, and the normative docs no
longer present a deferred intent schema or non-Task admission kinds as M6-A
behavior. M6-A remains a TASK-only semantic frontier.

## Baseline

```text
M5 frozen base (main):
c1629e42ec142f73c7981f10972809bc2ef9b4de

First audited M6-A closure head:
ae3436416687c4681a0e5b5541d59dd9783462b9

M6-A baseline is the merge of PR #16 into main. The M6-A freeze claim is
confirmed by the subsequent closure PRs (fix/m6a-audit-closure on
c174f1585a82d6b3fac3ba30c4c374485d25cb9e, then the intent-ingress and
normative-scope closures on
d4bf49a1ade4d1ef2002a6c47dc6872f72a32e68).

SCHEMA_VERSION = 5 at M6-A freeze.
```

## M6-B entry boundary

M6-B may now assume the semantic frontier kernel exists. It answers a different
question, and MUST NOT fold agent provisioning back into semantic admission:

```text
M6-A answers:
"WHAT semantic work becomes real?"

M6-B answers:
"WHAT CONTRACT must execute it, and FROM WHAT physical source may it be
materialized?"
```

M6-B MUST NOT weaken M5 mechanical authority or M6-A semantic admission
authority. If it appears to require changing either, that is an explicit
cross-milestone architecture review.
