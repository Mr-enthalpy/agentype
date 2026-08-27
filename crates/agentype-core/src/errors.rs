//! Domain / authority errors. Distinct from mechanical Task failure classes
//! and from storage I/O failures.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidTransition(String),
    StaleAuthority(String),
    InvalidAuthority(String),
    InvariantViolation(String),
    NotFound(String),
    Conflict(String),
    ConfigurationUnavailable(String),
    RecoveryRequired(String),
    StorageFailure(String),
}

impl Error {
    pub fn stale(msg: impl Into<String>) -> Self {
        Self::StaleAuthority(msg.into())
    }

    pub fn invalid_transition(msg: impl Into<String>) -> Self {
        Self::InvalidTransition(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn invariant(msg: impl Into<String>) -> Self {
        Self::InvariantViolation(msg.into())
    }

    pub fn invalid_authority(msg: impl Into<String>) -> Self {
        Self::InvalidAuthority(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    pub fn configuration_unavailable(msg: impl Into<String>) -> Self {
        Self::ConfigurationUnavailable(msg.into())
    }

    pub fn recovery_required(msg: impl Into<String>) -> Self {
        Self::RecoveryRequired(msg.into())
    }

    pub fn storage_failure(msg: impl Into<String>) -> Self {
        Self::StorageFailure(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition(m) => write!(f, "invalid transition: {m}"),
            Self::StaleAuthority(m) => write!(f, "stale authority: {m}"),
            Self::InvalidAuthority(m) => write!(f, "invalid authority: {m}"),
            Self::InvariantViolation(m) => write!(f, "invariant violation: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::ConfigurationUnavailable(m) => write!(f, "configuration unavailable: {m}"),
            Self::RecoveryRequired(m) => write!(f, "recovery required: {m}"),
            Self::StorageFailure(m) => write!(f, "storage failure: {m}"),
        }
    }
}

impl std::error::Error for Error {}
