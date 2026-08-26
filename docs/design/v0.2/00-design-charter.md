# V0.2 Design Charter

Status: Architecture direction
Canonical path: docs/design/v0.2/00-design-charter.md

## Purpose

V0.2 extends Agentype from a reliable local Agent Scheduler into a typed semantic work-generation and scheduling runtime while preserving the V0.1 correctness kernel.

V0.2 is also the first Rust implementation line. It is not a line-by-line Python transliteration.

## Scope

V0.2 design covers:

- AgentType and typed logical specialization;
- SpawnSource as a provisioning abstraction orthogonal to AgentType;
- Root semantic operating policy;
- positive / negative / exploratory information functions;
- flat agent topology;
- task generations and bounded frontier expansion;
- RawWorkIntent → CompiledWorkProposal compilation;
- AgentTransform, AgentLineage, MemoryCapsule;
- revival transparency and continuity capability tiers;
- sandbox/capability enforcement;
- typed work requirements and scheduler matching;
- preservation of the V0.1 correctness kernel under RIIR.

## Non-goals

V0.2 should not expand into:

- distributed consensus or HA;
- generic workflow DSL;
- vector-memory platform;
- transcript database;
- autonomous scheduler LLM;
- provider credential pool;
- dashboard-first architecture;
- arbitrary hierarchical agent organizations;
- model-per-task hardcoding.

## Design test

The architecture should still make sense if tomorrow all available models become equally capable, cheap, fast, long-context, tool-capable, and broadly general.

If a core abstraction exists only because one model is currently expensive and another is cheap, it belongs in SpawnSource selection policy, not in Core semantics.

## Core principles

> Structure cognition; do not merely route models.

> Flat actors, structured messages.

Complexity is allowed in information dependencies and semantic transformations. It must not be duplicated into multi-level command hierarchy.
