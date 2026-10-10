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
//! `SpawnSourceRef`, the `SourceConfigRef` + config digest, the
//! `AdapterPolicyRef`, and the exact physical domain `(adapter_kind,
//! adapter_binding_key)`. Evidence resolved for one config or one physical
//! domain must never authorize a different config or domain. Binding the exact
//! key into the subject means a downstream acquisition cannot substitute another
//! installation's enforceability proof for the selected one.

use crate::capability::{CapabilityRef, CapabilityValue};
#[cfg(any(test, feature = "provisioning-producer"))]
use crate::error::ContractError;
use crate::records::{
    AdapterPolicyRef, ConfigDigest, PhysicalSafety, SandboxPolicyRef, SourceConfigRef,
    SpawnSourceRef,
};
use std::collections::{BTreeMap, BTreeSet};

/// Imported, candidate-bound enforcement evidence for one exact provisioning
/// tuple `(SpawnSourceRef, SourceConfigRef + digest, AdapterPolicyRef)`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProvisioningEvidence {
    adapter_policy: AdapterPolicyRef,
    adapter_kind: String,
    adapter_binding_key: String,
    source_ref: SpawnSourceRef,
    source_config_ref: SourceConfigRef,
    config_digest: ConfigDigest,
    enforceable_safety: PhysicalSafety,
    enforced_sandbox_policies: BTreeSet<SandboxPolicyRef>,
    enforced_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
}

impl ResolvedProvisioningEvidence {
    /// M6-B.4 production producer: build imported, candidate-bound enforcement
    /// evidence from facts supplied by the M5 imported-binding/safety authority.
    ///
    /// This is the only production construction path. It is deliberately called
    /// exclusively by the Scheduler's internal provisioning resolver, which
    /// sources `enforceable_safety`/`enforced_*` from the imported adapter
    /// binding rather than from a catalog claim or a control-surface caller. No
    /// supported control surface accepts caller-supplied evidence. Per M6-B.1
    /// this is a **supported-surface** fence, not an unforgeable capability; the
    /// authoritative boundary is the internal resolution path
    /// ([ADR-0008](../../../decisions/0008-m6b-typed-provisioning-acquisition.md)).
    ///
    /// Compiled only under the `provisioning-producer` feature, which the
    /// internal runtime resolver enables; the default supported surface keeps no
    /// evidence constructor.
    #[cfg(any(test, feature = "provisioning-producer"))]
    #[allow(clippy::too_many_arguments)]
    pub fn from_imported_binding(
        adapter_policy: AdapterPolicyRef,
        adapter_kind: impl Into<String>,
        adapter_binding_key: impl Into<String>,
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
        let adapter_binding_key = adapter_binding_key.into();
        if adapter_binding_key.trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "adapter binding key cannot be empty".into(),
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
            adapter_binding_key,
            source_ref,
            source_config_ref,
            config_digest,
            enforceable_safety,
            enforced_sandbox_policies: enforced_sandbox_policies.into_iter().collect(),
            enforced_capabilities: enforced,
        })
    }

    /// Test-support constructor (B.1). Delegates to the production producer so
    /// the two cannot drift; it stays gated on `test-support` so the default
    /// surface exposes only the internal-resolver path.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::too_many_arguments)]
    pub fn for_tests(
        adapter_policy: AdapterPolicyRef,
        adapter_kind: impl Into<String>,
        adapter_binding_key: impl Into<String>,
        source_ref: SpawnSourceRef,
        source_config_ref: SourceConfigRef,
        config_digest: ConfigDigest,
        enforceable_safety: PhysicalSafety,
        enforced_sandbox_policies: Vec<SandboxPolicyRef>,
        enforced_capabilities: Vec<(CapabilityRef, CapabilityValue)>,
    ) -> Result<Self, ContractError> {
        Self::from_imported_binding(
            adapter_policy,
            adapter_kind,
            adapter_binding_key,
            source_ref,
            source_config_ref,
            config_digest,
            enforceable_safety,
            enforced_sandbox_policies,
            enforced_capabilities,
        )
    }

    pub fn adapter_policy(&self) -> &AdapterPolicyRef {
        &self.adapter_policy
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    /// Opaque exact physical execution domain the evidence was imported from.
    pub fn adapter_binding_key(&self) -> &str {
        &self.adapter_binding_key
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
