# Consolidated V0.2 Architecture Invariants

Status: Frozen direction unless explicitly revised
Canonical path: docs/design/v0.2/12-normative-invariants.md

## Organizational

1. Agent topology is flat.
2. Information dependencies may be arbitrarily structured; command hierarchy may not.
3. Temporary teams emerge from type affinity, anchors, dependencies, and message flow.
4. No manager-agent layer is required for ordinary scheduling.

## Root

5. Root maintains one clean, current, revisable positive semantic model.
6. Root owns semantic frontier admission.
7. Root integrates evidence; workers do not become semantic authorities by producing Results.
8. Root expresses semantic intent, not Lease/Attempt/Incarnation mechanics.
9. Obsolete semantic models are replaced, not indefinitely accumulated in active context.

## Information

10. Explorers expand uncertain space.
11. Positive-semantic agents compress surviving/current structure.
12. Negative-semantic agents compress eliminated structure.
13. Long-lived memory is structured bounded semantics, not transcript retention.
14. Preserve conclusions and constraints, not narrative history.
15. Evidence is reference-first and materialized on demand.

## Generation/frontier

16. Every semantic Task belongs to a Generation.
17. Generation is a semantic frontier barrier, not an organizational level.
18. Workers may emit RawWorkIntent only if Generation policy permits.
19. Workers do not directly expand the executable semantic frontier.
20. Every Generation has bounded expansion policy.
21. Audit/verification Generations may be explicitly non-expansive.
22. Batch remains distinct from Generation.

## WorkIntent

23. Domain agents emit domain-semantic RawWorkIntent, not scheduler-native TaskSpec.
24. RawWorkIntent compilation is an ordinary typed function.
25. Compilation does not imply admission.
26. Compiler has no authority hierarchy.
27. Compilation is non-expansive by default.
28. Architectural ambiguity is returned to Root, not recursively converted into more work.

## Type/provisioning

29. AgentType is defined by organizational/informational/security function, not model quality tier.
30. AgentType and SpawnSource are orthogonal.
31. Type derivation is not scheduling inheritance.
32. Root-created refinements may narrow authority, never enlarge it.
33. A SpawnSource may provision many narrower AgentTypes if it can enforce them.
34. Model economics affect provisioning policy, not Core semantic identity.

## Sandbox

35. Claimed AgentType restrictions must be mechanically enforceable.
36. Prompt-only sandbox restrictions do not count.
37. Effective permission is the intersection of type, Generation, Task, and source ceilings.
38. A source unable to enforce the requested sandbox is ineligible.

## Transform/memory

39. Transform changes semantic responsibility and does not mutate LogicalAgent type identity in place.
40. Transform creates a successor LogicalAgent in the same AgentLineage.
41. MemoryCapsule is Scheduler-owned, bounded, versioned, and externally readable under policy.
42. Runtime transcript is not MemoryCapsule.
43. Scheduler does not silently synthesize memory through hidden LLM work.
44. Type garbage collection is separate from AgentTransform.

## Revival/terminal

45. Terminal-native UX is never a correctness dependency.
46. Revival is an internal Scheduler lifecycle event.
47. Revival preserves LogicalAgent semantic identity.
48. Physical Incarnation/session loss does not imply LogicalAgent loss.
49. Scheduler structured continuity is the mandatory recovery floor.
50. Native terminal/session persistence may improve fidelity but never replace that floor.
51. Continuity affinity is not hard execution affinity unless explicitly required by AgentType.
52. Revival and Transform are distinct.

## V0.1 correctness preservation

53. At-least-once execution remains the model.
54. Attempt/Lease fencing remains authoritative.
55. Stale attempts can refine only their own physical history.
56. Task success and authoritative Result creation remain atomic.
57. Root Result ACK is consumption, not worker completion.
58. Lease expiry alone never proves writer quiescence.
59. Unsafe duplicate writers remain prohibited.
60. Scheduler remains sole claim/state authority.
61. Generation/AgentType/Compiler layers may not bypass the Task/Attempt/Lease/Result correctness kernel.
