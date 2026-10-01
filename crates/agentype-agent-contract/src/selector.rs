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
                    type_id: reference.type_id().as_str().to_string(),
                    revision: reference.revision(),
                })
            }
        }
        AgentTypeSelector::Latest(type_id) => match catalog.latest_revision(type_id) {
            Some(revision) => AgentTypeRef::new(type_id.as_str(), revision),
            None => Err(ContractError::AgentTypeNotFound {
                type_id: type_id.as_str().to_string(),
                revision: 0,
            }),
        },
    }
}
