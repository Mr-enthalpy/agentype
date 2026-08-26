# 12 — Root Contract

Status: Normative
Canonical path: docs/specs/v0.2/12-root-contract.md
Derived from: [docs/design/v0.2/02-root-operating-model.md](../../design/v0.2/02-root-operating-model.md),
[ROOT_OPERATING_DOCTRINE.md](../../design/v0.2/ROOT_OPERATING_DOCTRINE.md)

Runtime prompt text lives in the doctrine. This file is the MUST/MUST NOT
behavior contract.

Exact review API surface is DEFERRED (D-ROOT-API). Semantics below are not.

## Root MUST

- Maintain one current, revisable positive semantic model.
- Own semantic frontier admission.
- Admit bounded Generations; fix scope and expansion ceiling at admission.
- Accept, reject, or defer compiled proposals.
- Integrate Results as evidence, not as authority.
- Retain compact negative constraints **with applicability conditions**.
- Replace obsolete positive models rather than accumulate them in active context.
- Use evidence reference-first; materialize on demand.
- Remain provider-neutral.

## Root MUST NOT

- Claim Tasks or renew Leases.
- Control retries, heartbeats, or Incarnations.
- Manually revive LogicalAgents as orchestration.
- Poll Scheduler mechanics.
- Interpret RootBridge notification as Result payload.
- Allow a worker proposal to become executable without admission.
- Treat mechanical retry/recovery/reconciliation/revival as a new Generation.
- Treat GenerationPolicy as received frontier-admission authority.
- Grant workers bounded spawning via a Generation.
- Treat worker `validated_delta` as already-canonical MemoryCapsule content.
- Promote unscoped prohibitions from scoped failures.
- Micromanage Scheduler internals.

## Loop (normative intent, not a DSL)

1. State the current positive model.
2. Identify highest-value unresolved uncertainty.
3. Isolate work that benefits from scheduled independence.
4. Admit a bounded Generation (semantic intent only).
5. Continue independent reasoning where isolation is not needed.
6. On REVIEWABLE: integrate scoped positives/negatives, inspect proposals,
   reject/defer/admit — Root, not the Generation.
7. Replace obsolete positive state cleanly.
8. Stop when the user's acceptance condition is satisfied.

## Notifications

Root remains notification-driven. Wakeup carries event id, type, and indexes.
Results remain in the durable Result Queue.
