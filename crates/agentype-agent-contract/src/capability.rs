//! Capability vocabulary and its canonical catalog.
//!
//! A capability's semantics (matcher kind, security class) are defined **once**
//! by a catalog keyed on the exact `(capability_id, revision)`. An AgentType
//! references a capability; it never owns a private definition that could
//! redefine what a security class means. `ENFORCED` is imported evidence, not
//! an enum label.

use crate::error::ContractError;
use crate::ids::CapabilityId;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The five matcher shapes supported in M6-B v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatcherKind {
    /// Exact boolean: `provided == required`.
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

/// A capability value. The shape is determined by the capability definition.
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
/// security/authority proof (which then requires imported `ENFORCED` evidence).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecurityClass {
    Functional,
    Authority,
    Sandbox,
    Continuity,
}

/// Declaration metadata for a claim. This is *not* a proof: only imported
/// `ResolvedProvisioningEvidence` can satisfy a security requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assurance {
    Declared,
    Enforced,
}

impl SecurityClass {
    pub fn requires_evidence(self) -> bool {
        !matches!(self, Self::Functional)
    }
}

/// Exact `(capability_id, revision)` identity. Fields are private so an
/// unvalidated reference cannot be constructed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityRef {
    capability_id: CapabilityId,
    revision: u64,
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

    pub fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// The canonical semantics of one exact capability revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityDefinition {
    pub matcher_kind: MatcherKind,
    pub security_class: SecurityClass,
}

/// Canonical, single-authority set of capability definitions.
#[derive(Clone, Debug, Default)]
pub struct CapabilityCatalog {
    definitions: BTreeMap<CapabilityRef, CapabilityDefinition>,
}

impl CapabilityCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Define a capability revision. Redefining it with different semantics is
    /// rejected, so a security class can never be redefined per AgentType.
    pub fn define(
        &mut self,
        reference: CapabilityRef,
        matcher_kind: MatcherKind,
        security_class: SecurityClass,
    ) -> Result<(), ContractError> {
        let definition = CapabilityDefinition {
            matcher_kind,
            security_class,
        };
        match self.definitions.get(&reference) {
            Some(existing) if existing == &definition => Ok(()),
            Some(_) => Err(ContractError::CapabilityDefinitionConflict {
                capability: reference.capability_id().as_str().to_string(),
                revision: reference.revision(),
            }),
            None => {
                self.definitions.insert(reference, definition);
                Ok(())
            }
        }
    }

    pub fn get(&self, reference: &CapabilityRef) -> Option<&CapabilityDefinition> {
        self.definitions.get(reference)
    }

    pub fn contains(&self, reference: &CapabilityRef) -> bool {
        self.definitions.contains_key(reference)
    }
}

/// A source's (or config's) declaration about one exact capability revision.
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
        (MatcherKind::Bool, CapabilityValue::Bool(r), CapabilityValue::Bool(p)) => r == p,
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

/// Whether `value` stays within a `ceiling` (used to keep config/source
/// declarations from exceeding a SpawnSource's provisionable envelope).
pub fn value_within(ceiling: &CapabilityValue, value: &CapabilityValue) -> bool {
    match (ceiling, value) {
        (CapabilityValue::Bool(a), CapabilityValue::Bool(b)) => a == b,
        (CapabilityValue::Set(a), CapabilityValue::Set(b)) => b.is_subset(a),
        (
            CapabilityValue::Ordered {
                class: ac,
                rank: ar,
            },
            CapabilityValue::Ordered {
                class: bc,
                rank: br,
            },
        ) => ac == bc && br <= ar,
        (CapabilityValue::Quantity(a), CapabilityValue::Quantity(b)) => b.get() <= a.get(),
        (CapabilityValue::Exact(a), CapabilityValue::Exact(b)) => a == b,
        _ => false,
    }
}

/// Whether a claim's declaration satisfies a security class. Retained for
/// catalog vocabulary; it is never a substitute for imported evidence.
pub fn assurance_satisfies(class: SecurityClass, assurance: Assurance) -> bool {
    !class.requires_evidence() || assurance == Assurance::Enforced
}
