# ADR-0012 — M6-B.4 ends at typed physical execution and the durable ACK interface

Status: Accepted (M6-B.4 scope decision; not a milestone freeze)
Date: 2026-11-07
Canonical path: `docs/decisions/0012-m6b4-scope-typed-physical-execution-ack.md`

## Context

M6-B.4 has been described both as

```text
typed physical execution → durable ACK interface
```

and as

```text
typed physical execution → worker-originated Result → durable ACK settlement
```

The two are not equivalent. The second requires a production worker Result
transport, worker authentication, delivery retry, and ingress. The implementation
on PR #24 provides the first: a production typed daemon dispatch, a real source
integration, real OS-enforced workspace jails, exact Execution/adapter binding,
cross-daemon physical recovery, and the durable Kernel Result-ACK command. Its
end-to-end tests read the committed Attempt/Lease/Execution in the harness and
call `Kernel::ack_success` directly; no worker sends a Result.

Leaving the difference in PR comments lets a later reader infer a closed
worker-originated data plane that does not exist. The acceptance boundary
therefore has to be a normative decision rather than a descriptive note.

## Decision

1. **M6-B.4's accepted boundary is `typed physical execution → durable ACK
   interface`.** The milestone must prove typed acquisition, exact binding,
   physical start under real OS enforcement, observation and recovery, and that
   the durable Kernel ACK command settles a Result with lease-epoch, authority
   and writer-safety checks.

2. **A worker-originated Result data plane is outside M6-B.4.** It comprises
   worker Result transport, worker authentication, durable ingress, delivery
   retry, and settlement initiated by the worker rather than by the trusted
   runtime. These remain deferred with B.5 credentials and the wider worker data
   plane. M6-B.4 MUST NOT claim that an independent worker completed an attempt
   and that its Result settled the Task end to end.

3. **Harness-injected ACK is an interface proof, not a data-plane proof.** The
   E2E suite may invoke the durable Kernel command after reading committed
   state. The evidence it produces covers the ACK contract: lease-epoch
   validation, authority enforcement, `ReadOnly` completion, `Write` quiescence
   refusal, and late physical end as history. ADR-0004's separation of durable
   Result transport from Root notification is unchanged and remains deferred for
   the worker side.

4. **A different scope needs a different decision.** If a later milestone or a
   revised M6-B.4 definition includes the worker-originated data plane, it MUST
   add the transport, authentication and retry contract, its failure dispositions
   and its tests, and it MUST NOT be recorded as a mere extension of this
   boundary. Conversely, this ADR cannot be cited to claim that work is done.

5. **Freeze still requires independent re-audit.** This decision fixes scope; it
   is not a milestone freeze. The M6-B.4 record remains `NOT FROZEN` until an
   independent re-audit accepts the implementation at an exact final SHA with
   green Linux and Windows CI.

## Non-claims

- No production worker transport, worker authentication, delivery retry, queue
  or network ingress exists in M6-B.4.
- No claim of same-user read confidentiality, metadata immutability on Linux, or
  a general malicious-code sandbox. Windows claims workspace/desktop write
  confinement; Linux claims a content-write boundary.
- No v7→v8 migration, B.5 credential/security import, cold/revivable matching,
  full typed topology, or executed-WARM replacement is included.

## Consequences

- A later reader cannot treat the harness ACK as proof of the worker data plane;
  the report, PR description, ADR-0008 and ADR-0011 carry the same label.
- The typed physical path remains the milestone's authority-relevant work, and
  the deferred data plane does not reopen frozen M4–M6-B.3 authority,
  physical/semantic separation or writer-safety contracts.
- Implementing the worker data plane later is additive above the durable ACK
  interface rather than a redesign of the Scheduler/Adapter boundary.

## Relationship to other decisions

- [ADR-0004](README.md) keeps Result transport
  durable and separate from Root notification; this ADR does not implement its
  worker side.
- [ADR-0008](0008-m6b-typed-provisioning-acquisition.md) owns the typed
  acquisition design.
- [ADR-0011](0011-m6b-acquisition-contract-parity.md) owns the shared placement,
  isolation and physical-phase contract, including the same ACK boundary.
- Evidence: [M6-B.4 implementation and acceptance
  record](../reports/v0.2/riir-m6b.4-provisioning-binding-freeze.md).
