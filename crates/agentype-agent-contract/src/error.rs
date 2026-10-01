//! Typed M6-B resolution errors.
//!
//! These are deliberately separate from M5 `FailureClass`: a resolution error
//! is a provisioning-time fact, not an execution failure. The caller decides
//! whether another candidate may be tried before the Execution commitment.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    /// The exact AgentType revision does not exist (or is not published).
    AgentTypeNotFound { type_id: String, revision: u64 },
    /// A selector or pinned revision is stale relative to the catalog.
    AgentTypeRevisionConflict { type_id: String },
    /// A derived type would enlarge authority.
    InvalidRefinement { reason: String },
    /// A hard capability requirement is not met by the candidate.
    CapabilityMismatch { capability: String },
    /// A security/authority requirement has no `ENFORCED` proof.
    SecurityUnenforceable { reason: String },
    /// A source/config is not usable for new provisioning.
    SourceConfigInvalid { reason: String },
    /// A credential reference cannot be satisfied by preflight.
    CredentialUnavailable { reference: String },
    /// No imported adapter binding matches the required exact domain.
    AdapterBindingMissing,
    /// A selector carried no usable identity.
    EmptySelector,
    /// A durable relation that must hold does not.
    InvariantViolation(String),
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentTypeNotFound { type_id, revision } => {
                write!(f, "agent type {type_id}@{revision} not found")
            }
            Self::AgentTypeRevisionConflict { type_id } => {
                write!(f, "agent type {type_id} revision conflict")
            }
            Self::InvalidRefinement { reason } => write!(f, "invalid refinement: {reason}"),
            Self::CapabilityMismatch { capability } => {
                write!(f, "capability mismatch: {capability}")
            }
            Self::SecurityUnenforceable { reason } => {
                write!(f, "security not enforceable: {reason}")
            }
            Self::SourceConfigInvalid { reason } => write!(f, "source config invalid: {reason}"),
            Self::CredentialUnavailable { reference } => {
                write!(f, "credential unavailable: {reference}")
            }
            Self::AdapterBindingMissing => write!(f, "exact adapter binding missing"),
            Self::EmptySelector => write!(f, "empty selector"),
            Self::InvariantViolation(msg) => write!(f, "invariant violation: {msg}"),
        }
    }
}

impl std::error::Error for ContractError {}
