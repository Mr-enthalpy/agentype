//! Trusted provisioning evidence.
//!
//! `ENFORCED` is not an enum label a caller can mint: it is an imported fact
//! owned by the physical adapter integration. In M6-B.1 this type has **no
//! public production constructor** — exactly as `agentype-execution-config`
//! owns `FrozenExecutionSafety` — so a downstream consumer cannot self-report
//! isolation or enforced capabilities. The M5 imported-binding bridge (M6-B.4)
//! will own the production constructor; tests use `for_tests`.
//!
//! Evidence is bound to the **exact candidate** it was resolved for: the
//! `SpawnSourceRef`, the `SourceConfigRef` + config digest, and the
//! `AdapterPolicyRef`. Evidence resolved for one config must never authorize a
//! different config under the same source.

use crate::capability::{CapabilityRef, CapabilityValue};
#[cfg(any(test, feature = "test-support"))]
use crate::error::ContractError;
use crate::records::{
    AdapterPolicyRef, ConfigDigest, PhysicalSafety, SandboxPolicyRef, SourceConfigRef,
    SpawnSourceRef,
};
use std::collections::{BTreeMap, BTreeSet};

/// Imported, candidate-bound enforcement evidence for one exact provisioning
/// tuple `(SpawnSourceRef, SourceConfigRef + digest, AdapterPolicyRef)`.
pub struct ResolvedProvisioningEvidence {
    adapter_policy: AdapterPolicyRef,
    adapter_kind: String,
    source_ref: SpawnSourceRef,
    source_config_ref: SourceConfigRef,
    config_digest: ConfigDigest,
    enforceable_safety: PhysicalSafety,
    enforced_sandbox_policies: BTreeSet<SandboxPolicyRef>,
    enforced_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
}

impl ResolvedProvisioningEvidence {
    /// Test-support constructor. Not a production surface: production evidence
    /// must come from the M5 imported-binding bridge (M6-B.4).
    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::too_many_arguments)]
    pub fn for_tests(
        adapter_policy: AdapterPolicyRef,
        adapter_kind: impl Into<String>,
        source_ref: SpawnSourceRef,
        source_config_ref: SourceConfigRef,
        config_digest: ConfigDigest,
        enforceable_safety: PhysicalSafety,
        enforced_sandbox_policies: Vec<SandboxPolicyRef>,
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
            source_ref,
            source_config_ref,
            config_digest,
            enforceable_safety,
            enforced_sandbox_policies: enforced_sandbox_policies.into_iter().collect(),
            enforced_capabilities: enforced,
        })
    }

    pub fn adapter_policy(&self) -> &AdapterPolicyRef {
        &self.adapter_policy
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn source_ref(&self) -> &SpawnSourceRef {
        &self.source_ref
    }

    pub fn source_config_ref(&self) -> &SourceConfigRef {
        &self.source_config_ref
    }

    pub fn config_digest(&self) -> &ConfigDigest {
        &self.config_digest
    }

    pub fn enforceable_safety(&self) -> &PhysicalSafety {
        &self.enforceable_safety
    }

    pub fn enforces_sandbox_policy(&self, policy: &SandboxPolicyRef) -> bool {
        self.enforced_sandbox_policies.contains(policy)
    }

    pub fn enforced_capability(&self, reference: &CapabilityRef) -> Option<&CapabilityValue> {
        self.enforced_capabilities.get(reference)
    }
}
