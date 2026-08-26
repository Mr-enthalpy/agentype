# Agentype V0.2 Normative Specification

Status: Normative
Version: V0.2
Canonical path: docs/specs/v0.2/README.md
Derived from: [docs/design/v0.2/](../../design/v0.2/)
Does not replace: [docs/specs/v0.1.md](../v0.1.md) (V0.1 Python-line executable contract)

This directory defines **what MUST be true** for Agentype V0.2.

[docs/design/v0.2/](../../design/v0.2/) remains architecture rationale (**why**).
Rust implementation decides **how**. This spec MUST NOT be used as a license to
start RIIR by transliterating Python modules.

The current Python package `local-agent-scheduler` 0.1.3 remains a correctness
oracle for the V0.1 kernel. It MUST NOT be modified to implement V0.2 semantics.

## Normative language

| Keyword | Meaning |
|---|---|
| MUST / MUST NOT | Correctness or security obligation |
| SHOULD / SHOULD NOT | Strong default; deviation needs explicit review |
| MAY | Permitted |
| POLICY-DEFINED | Chosen by Generation/Task/AgentType policy, within this spec |
| IMPLEMENTATION-DEFINED | Representation/mechanism may vary if semantics hold |
| DEFERRED | Unresolved; see [17-deferred-open-questions.md](17-deferred-open-questions.md). MUST NOT be silently decided |

## Reading order

1. [00-scope-and-conformance.md](00-scope-and-conformance.md)
2. [01-domain-model.md](01-domain-model.md)
3. [02-authority-and-correctness.md](02-authority-and-correctness.md)
4. [03-task-attempt-lease-result.md](03-task-attempt-lease-result.md)
5. [04-generation-and-frontier.md](04-generation-and-frontier.md)
6. [05-work-intent-compilation.md](05-work-intent-compilation.md)
7. [06-agent-type-and-matching.md](06-agent-type-and-matching.md)
8. [07-spawn-source-and-adapter-contract.md](07-spawn-source-and-adapter-contract.md)
9. [08-logical-agent-lineage-transform.md](08-logical-agent-lineage-transform.md)
10. [09-memory-continuity-revival.md](09-memory-continuity-revival.md)
11. [10-sandbox-and-security.md](10-sandbox-and-security.md)
12. [11-pool-topology.md](11-pool-topology.md)
13. [12-root-contract.md](12-root-contract.md)
14. [13-storage-and-transactions.md](13-storage-and-transactions.md)
15. [14-recovery-and-reconciliation.md](14-recovery-and-reconciliation.md)
16. [15-rust-implementation-contract.md](15-rust-implementation-contract.md)
17. [16-conformance-tests.md](16-conformance-tests.md)
18. [17-deferred-open-questions.md](17-deferred-open-questions.md)
19. [matrices.md](matrices.md)

## Distinctions that MUST NOT collapse

Root semantic authority vs Scheduler mechanical authority;
AgentType vs SpawnSource;
LogicalAgent vs Incarnation vs Execution vs Attempt;
Generation vs Batch;
RawWorkIntent vs CompiledWorkProposal vs admitted Task;
compilation vs admission;
MOVE/MERGE vs TRANSFORM;
revival vs Transform;
information function vs lifecycle;
Scheduler continuity floor vs native resume;
correctness / continuity / experience capabilities;
information graph vs authority hierarchy.

## What an implementer may choose

Rust crate names, struct field layout, SQL table names, async runtime,
serialization library, and channel topology are IMPLEMENTATION-DEFINED unless a
section explicitly freezes them.

An implementer MUST NOT choose who owns authority, what a Generation is, whether
a worker proposal is executable, whether a compiler may admit work, whether
revival changes identity, whether Transform mutates type in place, whether a
prompt is a sandbox, whether Result creation is atomic, whether an unsafe
writer may be retried, or whether native terminal resume is required for
correctness.
