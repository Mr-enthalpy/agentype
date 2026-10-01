//! Trusted provisioning evidence.
//!
//! `ENFORCED` is not an enum label a caller can mint: it is an imported fact
//! produced by the physical adapter integration (M5.7's `ImportableAdapter`
//! path, wired in M6-B.4). The predicate consumes a policy-bound
//! [`ResolvedProvisioningEvidence`], so adapter A's enforcement facts can never
//! be applied to source B.

use crate::capability::{CapabilityRef, CapabilityValue};
use crate::error::ContractError;
use crate::records::{AdapterBindingKey, AdapterPolicyRef, PhysicalSafety};
use std::collections::BTreeMap;

/// Implemented by the trusted adapter integration. Every value here is produced
/// together by the importer, not assembled by a composition caller.
pub trait ProvisioningEvidenceSource {
    fn adapter_policy(&self) -> AdapterPolicyRef;
    fn adapter_kind(&self) -> String;
    fn adapter_binding_key(&self) -> AdapterBindingKey;
    fn enforceable_safety(&self) -> PhysicalSafety;
    fn enforced_capabilities(&self) -> Vec<(CapabilityRef, CapabilityValue)>;
}

/// Imported, policy-bound enforcement evidence for one exact adapter policy.
pub struct ResolvedProvisioningEvidence {
    adapter_policy: AdapterPolicyRef,
    adapter_kind: String,
    adapter_binding_key: AdapterBindingKey,
    enforceable_safety: PhysicalSafety,
    enforced_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
}

impl ResolvedProvisioningEvidence {
    pub fn from_source(source: &dyn ProvisioningEvidenceSource) -> Result<Self, ContractError> {
        let adapter_kind = source.adapter_kind();
        if adapter_kind.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "adapter kind cannot be empty".into(),
            });
        }

        let mut enforced_capabilities = BTreeMap::new();
        for (reference, value) in source.enforced_capabilities() {
            if enforced_capabilities
                .insert(reference.clone(), value)
                .is_some()
            {
                return Err(ContractError::InvariantViolation(format!(
                    "duplicate enforced capability {}@{}",
                    reference.capability_id().as_str(),
                    reference.revision()
                )));
            }
        }

        Ok(Self {
            adapter_policy: source.adapter_policy(),
            adapter_kind,
            adapter_binding_key: source.adapter_binding_key(),
            enforceable_safety: source.enforceable_safety(),
            enforced_capabilities,
        })
    }

    pub fn adapter_policy(&self) -> &AdapterPolicyRef {
        &self.adapter_policy
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn adapter_binding_key(&self) -> &AdapterBindingKey {
        &self.adapter_binding_key
    }

    pub fn enforceable_safety(&self) -> &PhysicalSafety {
        &self.enforceable_safety
    }

    pub fn enforced_capability(&self, reference: &CapabilityRef) -> Option<&CapabilityValue> {
        self.enforced_capabilities.get(reference)
    }
}
