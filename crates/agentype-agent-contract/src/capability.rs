//! Capability vocabulary: a small, provable matcher set.
//!
//! M6-B v1 deliberately rejects arbitrary expression DSLs (CEL, JS, LLM
//! matchers, provider-specific predicate languages). A capability is one of a
//! handful of shapes that can be decided mechanically, and it is always keyed
//! by an **exact revision** so a v1 claim can never satisfy a v2 requirement.

use crate::error::ContractError;
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

/// A validated non-negative, finite quantity.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Quantity(f64);

impl Quantity {
    pub fn new(value: f64) -> Result<Self, ContractError> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(ContractError::InvalidNumber {
                field: "quantity".into(),
            })
        }
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// A capability value. The shape is determined by the capability spec.
#[derive(Clone, Debug, PartialEq)]
pub enum CapabilityValue {
    Bool(bool),
    Set(BTreeSet<String>),
    Ordered { class: String, rank: u32 },
    Quantity(Quantity),
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

/// Exact `(capability_id, revision)` identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityRef {
    pub capability_id: CapabilityId,
    pub revision: u64,
}

impl CapabilityRef {
    pub fn new(capability_id: impl Into<String>, revision: u64) -> Result<Self, ContractError> {
        let capability_id = CapabilityId::from_string(capability_id);
        if capability_id.as_str().trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "capability id cannot be empty".into(),
            });
        }
        if revision == 0 {
            return Err(ContractError::InvalidRef {
                reason: "capability revision must be >= 1".into(),
            });
        }
        Ok(Self {
            capability_id,
            revision,
        })
    }
}

/// Declares the shape and security class of an exact capability revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilitySpec {
    pub reference: CapabilityRef,
    pub matcher_kind: MatcherKind,
    pub security_class: SecurityClass,
}

/// A Task's (or AgentType contract's) requirement under an exact revision.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityRequirement {
    pub reference: CapabilityRef,
    pub value: CapabilityValue,
}

/// A source's (or config's) concrete claim about one exact capability revision.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityClaim {
    pub reference: CapabilityRef,
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
            p.get() >= r.get()
        }
        (MatcherKind::Exact, r, p) => r == p,
        _ => false,
    }
}

/// `DECLARED` never satisfies a security/authority requirement.
pub fn assurance_satisfies(class: SecurityClass, assurance: Assurance) -> bool {
    !class.requires_enforced() || assurance == Assurance::Enforced
}
