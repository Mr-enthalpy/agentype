//! Trusted provisioning evidence.
//!
//! `ENFORCED` is not an enum label a caller can mint: it is an imported fact
//! owned by the physical adapter integration. In M6-B.1 this type has **no
//! public production constructor** — exactly as `agentype-execution-config`
//! owns `FrozenExecutionSafety` — so a downstream consumer cannot self-report
//! isolation or enforced capabilities. The M5 imported-binding bridge (M6-B.4)
//! will own the production constructor; tests use `for_tests`.

use crate::capability::{CapabilityRef, CapabilityValue};
#[cfg(any(test, feature = "test-support"))]
use crate::error::ContractError;
use crate::records::{AdapterPolicyRef, PhysicalSafety};
use std::collections::BTreeMap;

/// Imported, policy-bound enforcement evidence for one exact adapter policy.
pub struct ResolvedProvisioningEvidence {
    adapter_policy: AdapterPolicyRef,
    adapter_kind: String,
    enforceable_safety: PhysicalSafety,
    enforced_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
}

impl ResolvedProvisioningEvidence {
    /// Test-support constructor. Not a production surface: production evidence
    /// must come from the M5 imported-binding bridge (M6-B.4).
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests(
        adapter_policy: AdapterPolicyRef,
        adapter_kind: impl Into<String>,
        enforceable_safety: PhysicalSafety,
        enforced_capabilities: Vec<(CapabilityRef, CapabilityValue)>,
    ) -> Result<Self, ContractError> {
        let adapter_kind = adapter_kind.into();
        if adapter_kind.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "adapter kind cannot be empty".into(),
            });
        }
        let mut enforced = BTreeMap::new();
        for (reference, value) in enforced_capabilities {
            if enforced.insert(reference.clone(), value).is_some() {
                return Err(ContractError::InvariantViolation(format!(
                    "duplicate enforced capability {}@{}",
                    reference.capability_id().as_str(),
                    reference.revision()
                )));
            }
        }
        Ok(Self {
            adapter_policy,
            adapter_kind,
            enforceable_safety,
            enforced_capabilities: enforced,
        })
    }

    pub fn adapter_policy(&self) -> &AdapterPolicyRef {
        &self.adapter_policy
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn enforceable_safety(&self) -> &PhysicalSafety {
        &self.enforceable_safety
    }

    pub fn enforced_capability(&self, reference: &CapabilityRef) -> Option<&CapabilityValue> {
        self.enforced_capabilities.get(reference)
    }
}
