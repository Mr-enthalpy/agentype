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
    /// A derived type would enlarge authority.
    InvalidRefinement { reason: String },
    /// A hard capability requirement is not met by the candidate.
    CapabilityMismatch { capability: String },
    /// A security/authority requirement has no `ENFORCED` proof.
    SecurityUnenforceable { reason: String },
    /// A source/config is not usable for new provisioning.
    SourceConfigInvalid { reason: String },
    /// Imported enforcement evidence does not belong to the source's policy.
    EvidencePolicyMismatch { expected: String, actual: String },
    /// Imported enforcement evidence was resolved for a different candidate.
    EvidenceSubjectMismatch { reason: String },
    /// A numeric contract value was non-finite or out of range.
    InvalidNumber { field: String },
    /// An identity was empty or a revision was zero.
    InvalidRef { reason: String },
    /// A capability revision was redefined with different semantics.
    CapabilityDefinitionConflict { capability: String, revision: u64 },
    /// A durable relation that must hold does not.
    InvariantViolation(String),
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentTypeNotFound { type_id, revision } => {
                write!(f, "agent type {type_id}@{revision} not found")
            }
            Self::InvalidRefinement { reason } => write!(f, "invalid refinement: {reason}"),
            Self::CapabilityMismatch { capability } => {
                write!(f, "capability mismatch: {capability}")
            }
            Self::SecurityUnenforceable { reason } => {
                write!(f, "security not enforceable: {reason}")
            }
            Self::SourceConfigInvalid { reason } => write!(f, "source config invalid: {reason}"),
            Self::EvidencePolicyMismatch { expected, actual } => write!(
                f,
                "enforcement evidence policy {actual} does not match source policy {expected}"
            ),
            Self::EvidenceSubjectMismatch { reason } => {
                write!(f, "enforcement evidence subject mismatch: {reason}")
            }
            Self::InvalidNumber { field } => write!(f, "invalid number for {field}"),
            Self::InvalidRef { reason } => write!(f, "invalid ref: {reason}"),
            Self::CapabilityDefinitionConflict {
                capability,
                revision,
            } => write!(
                f,
                "capability {capability}@{revision} redefined with different semantics"
            ),
            Self::InvariantViolation(msg) => write!(f, "invariant violation: {msg}"),
        }
    }
}

impl std::error::Error for ContractError {}
