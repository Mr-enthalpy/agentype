# System Thesis and Information Functions

Status: Architecture direction
Canonical path: docs/design/v0.2/01-system-thesis-and-information-functions.md

## 1. Stable drivers

The architecture is not fundamentally driven by “expensive Root / cheap worker” routing.

That may be a useful current provisioning policy, but it is too contingent to justify the architecture.

The stable drivers are organizational and informational:

- exploration should be isolated from canonical reasoning;
- successful structure and failed structure require different compression;
- independent audit reduces correlated error;
- long-lived responsibilities should retain bounded domain continuity;
- permissions and visible state should follow role responsibility, not raw model power;
- search-space expansion must be bounded;
- one semantic integrator must maintain the current decision model;
- physical execution lifecycle must not define logical identity.

A universal AGI does not remove these requirements.

## 2. Homogeneous intelligence does not imply homogeneous agents

The same underlying model can instantiate:

- Root;
- positive-semantic maintainer;
- negative-semantic auditor;
- explorer;
- coder;
- verifier.

The distinction comes from semantic responsibility, information selection, affinity, anchor, sandbox, continuity, lifecycle, and authority.

Agent specialization does not imply model specialization.

## 3. Three recurring information operations

### EXPAND

Explore uncertain information space.

Typical outputs:

- candidate explanations;
- experiments;
- counterexamples;
- implementation attempts;
- evidence.

### COMPRESS-POSITIVE

Compress surviving/current structure.

Typical retained information:

- accepted facts;
- current invariants;
- current implementation model;
- accepted decisions;
- current plan.

### COMPRESS-NEGATIVE

Compress eliminated structure.

Typical retained information:

- disproven assumptions;
- rejected designs;
- invalid combinations;
- boundary conditions;
- counterexamples;
- known failure signatures.

Root integrates these outputs into a single current decision model.

## 4. Positive-semantic long-lived agents

Compression direction:

`large implementation/history space → what remains valid now`

They retain current valid structure, not narrative history.

## 5. Negative-semantic long-lived agents

Compression direction:

`large failed exploration space → what must not be forgotten`

Their value is persistent search-space pruning. Negative memory should not become a raw failure log.

## 6. Exploratory agents

Exploratory work naturally fits short-lived agents:

`spawn → explore → produce Result/evidence → retire`

The durable value is normally the Result, not the exploratory agent identity.

## 7. Shared long-lived requirements

Positive and negative long-lived agents both need:

- bounded structured memory;
- evidence-backed statements;
- versioned durable capsules;
- external readability under authorization;
- checkpoint support;
- revival from physical loss;
- Transform when compression quality degrades;
- no dependence on transcript retention.

Their difference is primarily what information they preserve.

## 8. Different Transform pressures

Positive-semantic pollution often comes from superseded implementations and stale formerly-correct facts.

Positive Transform tends to remove superseded truth.

Negative-semantic pollution often comes from duplicate failures, overgeneralized prohibitions, mixed applicability scopes, and obsolete constraints.

Negative Transform tends to deduplicate, generalize correctly, narrow applicability, split domains, and retire obsolete exclusions.

Semantic specialization is a topology choice, not an inheritance hierarchy.
