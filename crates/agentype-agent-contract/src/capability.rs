//! Capability vocabulary: a small, provable matcher set.
//!
//! M6-B v1 deliberately rejects arbitrary expression DSLs (CEL, JS, LLM
//! matchers, provider-specific predicate languages). A capability is one of a
//! handful of shapes that can be decided mechanically.

use crate::ids::CapabilityId;
use serde_json::Value;
use std::collections::BTreeSet;

/// The five matcher shapes supported in M6-B v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatcherKind {
    /// Exact boolean (e.g. `terminal.attach = true`).
    Bool,
    /// Required subset of a provided set (e.g. `tools ⊇ {git,ripgrep}`).
    Set,
    /// Enum lattice rank (e.g. `continuity >= logical`).
    Ordered,
    /// Numeric lower bound (e.g. `memory_mb >= 4096`).
    Quantity,
    /// Exact equality.
    Exact,
}

/// A capability value. The shape is determined by the capability spec.
#[derive(Clone, Debug, PartialEq)]
pub enum CapabilityValue {
    Bool(bool),
    Set(BTreeSet<String>),
    Ordered { class: String, rank: u32 },
    Quantity(f64),
    Exact(Value),
}

impl CapabilityValue {
    pub fn matcher_kind(&self) -> MatcherKind {
        match self {
            Self::Bool(_) => MatcherKind::Bool,
            Self::Set(_) => MatcherKind::Set,
            Self::Ordered { .. } => MatcherKind::Ordered,
            Self::Quantity(_) => MatcherKind::Quantity,
            Self::Exact(_) => MatcherKind::Exact,
        }
    }
}

/// Whether a capability participates in functional features or in a
/// security/authority proof (which then requires `ENFORCED`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecurityClass {
    Functional,
    Authority,
    Sandbox,
    Continuity,
}

/// A claim is either merely declared by a source/config or enforced by the
/// imported physical domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assurance {
    Declared,
    Enforced,
}

impl SecurityClass {
    /// Functional features may be satisfied by a `DECLARED` claim when policy
    /// allows; authority/sandbox/continuity claims MUST be `ENFORCED`.
    pub fn requires_enforced(self) -> bool {
        !matches!(self, Self::Functional)
    }
}

/// Declares the shape and security class of a capability id + revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilitySpec {
    pub capability_id: CapabilityId,
    pub revision: u64,
    pub matcher_kind: MatcherKind,
    pub security_class: SecurityClass,
}

/// A source's (or config's) concrete claim about one capability.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityClaim {
    pub capability_id: CapabilityId,
    pub capability_revision: u64,
    pub value: CapabilityValue,
    pub assurance: Assurance,
    pub evidence_ref: Option<String>,
}

/// Decide whether `provided` satisfies `required` under `matcher`.
pub fn value_satisfies(
    matcher: MatcherKind,
    required: &CapabilityValue,
    provided: &CapabilityValue,
) -> bool {
    match (matcher, required, provided) {
        (MatcherKind::Bool, CapabilityValue::Bool(r), CapabilityValue::Bool(p)) => !*r || *p,
        (MatcherKind::Set, CapabilityValue::Set(r), CapabilityValue::Set(p)) => r.is_subset(p),
        (
            MatcherKind::Ordered,
            CapabilityValue::Ordered {
                class: rc,
                rank: rr,
            },
            CapabilityValue::Ordered {
                class: pc,
                rank: pr,
            },
        ) => rc == pc && pr >= rr,
        (MatcherKind::Quantity, CapabilityValue::Quantity(r), CapabilityValue::Quantity(p)) => {
            p >= r
        }
        (MatcherKind::Exact, r, p) => r == p,
        _ => false,
    }
}

/// `DECLARED` never satisfies a security/authority requirement.
pub fn assurance_satisfies(class: SecurityClass, assurance: Assurance) -> bool {
    !class.requires_enforced() || assurance == Assurance::Enforced
}
