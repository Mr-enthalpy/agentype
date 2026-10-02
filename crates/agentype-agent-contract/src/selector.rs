//! Pre-commit AgentType selectors. Only exact refs survive commitment.

use crate::error::ContractError;
use crate::ids::AgentTypeId;
use crate::records::{AgentTypeLookup, AgentTypeRef};

/// A selector is allowed to be loose *before* commitment; it MUST resolve to an
/// exact `AgentTypeRef` before any durable binding is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentTypeSelector {
    Exact(AgentTypeRef),
    Latest(AgentTypeId),
}

impl AgentTypeSelector {
    /// Build a `Latest` selector from a non-empty type id.
    pub fn latest(type_id: impl Into<String>) -> Result<Self, ContractError> {
        let type_id = AgentTypeId::from_string(type_id);
        if type_id.as_str().trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "selector type id cannot be empty".into(),
            });
        }
        Ok(Self::Latest(type_id))
    }
}

pub fn resolve_selector(
    selector: &AgentTypeSelector,
    catalog: &impl AgentTypeLookup,
) -> Result<AgentTypeRef, ContractError> {
    match selector {
        AgentTypeSelector::Exact(reference) => {
            if catalog.is_published(reference) {
                Ok(reference.clone())
            } else {
                Err(ContractError::AgentTypeNotFound {
                    type_id: reference.id().as_str().to_string(),
                    revision: reference.revision(),
                })
            }
        }
        AgentTypeSelector::Latest(type_id) => {
            if type_id.as_str().trim().is_empty() {
                return Err(ContractError::InvalidRef {
                    reason: "selector type id cannot be empty".into(),
                });
            }
            match catalog.latest_revision(type_id) {
                Some(revision) => AgentTypeRef::from_id(type_id.clone(), revision),
                None => Err(ContractError::AgentTypeNotFound {
                    type_id: type_id.as_str().to_string(),
                    revision: 0,
                }),
            }
        }
    }
}
