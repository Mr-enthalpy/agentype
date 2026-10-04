//! M6-B.3 Task agent requirements and the Generation policy fold.
//!
//! A `TaskAgentRequirement` is the durable, exact-revision agent requirement
//! attached (additively) to an admitted Task. It is pure: it never selects a
//! `SpawnSource`, never resolves a credential, and never calls an adapter.
//!
//! It is deliberately separate from `agentype-core::TaskSpec`, which M6-A froze:
//! the requirement is an additive side table, so M6-B.3 does not reopen M6-A
//! canonicalization.
//!
//! `GenerationPolicy` closes the `D-GEN-POLICY` interface for M6-B.3: a Generation
//! MAY carry an immutable, generation-wide requirement ceiling, and the effective
//! hard requirement is the deterministic *join* (stronger of the two) of that
//! policy with the Task's own requirement. A Task can never widen its Generation.

use crate::capability::{
    join_requirement_values, CapabilityCatalog, CapabilityRef, CapabilityValue,
};
use crate::error::ContractError;
use crate::records::{
    network_rank, workspace_rank, AgentTypeRef, Budget, ContinuityMode, NetworkPolicy,
    SandboxPolicyRef, TaskRequirement,
};
use crate::selector::AgentTypeSelector;
use agentype_core::{InformationFunction, WorkspaceMode};
use std::collections::{BTreeMap, BTreeSet};

/// The pre-commit admission draft for a typed Task.
///
/// The dimensions a `TaskSpec` already owns (`information_function`, affinity,
/// workspace, continuity) are NOT duplicated here: the kernel derives them from
/// the admitted TaskSpec and resolves `required_type` from a loose selector to an
/// exact revision before anything is written. This keeps the TaskSpec the single
/// authority for those dimensions.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRequirementDraft {
    /// A pre-commit selector; `None` means no type pin.
    pub required_type: Option<AgentTypeSelector>,
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    pub required_network: NetworkPolicy,
    pub required_attempt_isolation: bool,
    pub sandbox_policy: Option<SandboxPolicyRef>,
    pub required_anchor: Option<String>,
    pub budget: Budget,
    pub preferred: AgentRequirementPreferences,
}

impl AgentRequirementDraft {
    /// Build the durable requirement once the selector and TaskSpec-derived
    /// dimensions are known.
    pub fn into_requirement(
        self,
        required_type: Option<AgentTypeRef>,
        information_function: InformationFunction,
        required_affinity: BTreeSet<String>,
        required_workspace: WorkspaceMode,
        required_continuity: ContinuityMode,
    ) -> TaskAgentRequirement {
        TaskAgentRequirement {
            required_type,
            hard: TaskRequirement {
                information_function,
                required_capabilities: self.required_capabilities,
                required_affinity,
                required_workspace,
                required_network: self.required_network,
                required_attempt_isolation: self.required_attempt_isolation,
                required_continuity,
                sandbox_policy: self.sandbox_policy,
                required_anchor: self.required_anchor,
                budget: self.budget,
            },
            preferred: self.preferred,
        }
    }
}

/// Soft ranking preferences. Hard constraints live in [`TaskRequirement`]; these
/// only order already-eligible candidates and MUST NOT make an ineligible
/// candidate eligible.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRequirementPreferences {
    /// Tags a candidate is preferred to advertise (never a hard filter).
    pub preferred_affinity: BTreeSet<String>,
    /// A stronger continuity guarantee is preferred when otherwise equal.
    pub preferred_continuity: Option<ContinuityMode>,
    /// Prefer the most specific compatible AgentType over a broader one.
    pub prefer_specificity: bool,
}

impl Default for AgentRequirementPreferences {
    fn default() -> Self {
        Self {
            preferred_affinity: BTreeSet::new(),
            preferred_continuity: None,
            prefer_specificity: true,
        }
    }
}

impl AgentRequirementPreferences {
    pub fn normalize(&mut self) {
        self.preferred_affinity = std::mem::take(&mut self.preferred_affinity);
    }
}

/// A Task's durable agent requirement: an optional exact AgentType pin plus the
/// hard derived `TaskRequirement` and soft preferences.
///
/// `required_type` is always an exact immutable revision once durable. A loose
/// selector is resolved before commitment (spec 06 `D-TYPE-REV-PIN`).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskAgentRequirement {
    /// Exact AgentType pin, or `None` for a requirement with no type constraint
    /// (still potentially constraining capabilities/security).
    pub required_type: Option<AgentTypeRef>,
    pub hard: TaskRequirement,
    pub preferred: AgentRequirementPreferences,
}

impl TaskAgentRequirement {
    /// Shape-check every required capability and the preference shape. Unknown
    /// or malformed capabilities fail closed. Does not consult any source.
    pub fn validate(&self, catalog: &CapabilityCatalog) -> Result<(), ContractError> {
        validate_requirement_capabilities(&self.hard.required_capabilities, catalog)
    }

    /// Canonicalize the in-memory record (sorted affinity, shape-normalized
    /// capability map). Digests MUST be computed over a canonicalized record.
    pub fn normalize(&mut self, catalog: &CapabilityCatalog) -> Result<(), ContractError> {
        self.validate(catalog)?;
        let mut out = BTreeMap::new();
        for (reference, value) in std::mem::take(&mut self.hard.required_capabilities) {
            if !value.is_present() {
                continue;
            }
            out.insert(reference, value);
        }
        self.hard.required_capabilities = out;
        self.hard.required_affinity = std::mem::take(&mut self.hard.required_affinity);
        self.preferred.normalize();
        Ok(())
    }
}

fn validate_requirement_capabilities(
    map: &BTreeMap<CapabilityRef, CapabilityValue>,
    catalog: &CapabilityCatalog,
) -> Result<(), ContractError> {
    for (reference, value) in map {
        // Resolve and shape-check before any absence normalization: `Bool(false)`
        // is absence only for a capability the catalog defines as `Bool`.
        let definition =
            catalog
                .get(reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                })?;
        if definition.matcher_kind != value.matcher_kind() {
            return Err(ContractError::InvariantViolation(format!(
                "requirement value shape does not match the definition for {}@{}",
                reference.capability_id().as_str(),
                reference.revision()
            )));
        }
    }
    Ok(())
}

/// An immutable, generation-wide hard requirement ceiling.
///
/// It uses the same coarse dimensions as [`TaskRequirement`]; the fold keeps the
/// stronger of the policy and the Task on every dimension. It MUST NOT be edited
/// after `create_generation`: a change is a new Generation.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationPolicy {
    /// Information functions this Generation admits. MUST be non-empty.
    pub allowed_information_functions: Vec<InformationFunction>,
    pub required_capabilities: BTreeMap<CapabilityRef, CapabilityValue>,
    pub min_workspace: WorkspaceMode,
    pub min_network: NetworkPolicy,
    pub requires_attempt_isolation: bool,
    pub min_continuity: ContinuityMode,
    pub sandbox_policy: Option<SandboxPolicyRef>,
    pub budget_ceiling: Budget,
    /// `None` = any affinity; `Some(S)` = Task affinity tags MUST be a subset.
    pub allowed_affinity: Option<BTreeSet<String>>,
    /// `None` = any anchor; `Some(a)` = the Task anchor MUST equal `a`.
    pub anchor_constraint: Option<String>,
}

impl GenerationPolicy {
    pub fn validate(&self, catalog: &CapabilityCatalog) -> Result<(), ContractError> {
        if self.allowed_information_functions.is_empty() {
            return Err(ContractError::InvariantViolation(
                "a generation policy must allow at least one information function".into(),
            ));
        }
        validate_requirement_capabilities(&self.required_capabilities, catalog)
    }

    pub fn normalize(&mut self, catalog: &CapabilityCatalog) -> Result<(), ContractError> {
        self.validate(catalog)?;
        self.allowed_information_functions
            .sort_by_key(|f| f.as_sql());
        self.allowed_information_functions.dedup();
        let mut out = BTreeMap::new();
        for (reference, value) in std::mem::take(&mut self.required_capabilities) {
            if !value.is_present() {
                continue;
            }
            out.insert(reference, value);
        }
        self.required_capabilities = out;
        Ok(())
    }
}

fn sandbox_fold(
    policy: &Option<SandboxPolicyRef>,
    task: &Option<SandboxPolicyRef>,
) -> Result<Option<SandboxPolicyRef>, ContractError> {
    match (policy, task) {
        (None, other) => Ok(other.clone()),
        (Some(_), None) => Ok(policy.clone()),
        (Some(a), Some(b)) if a == b => Ok(Some(a.clone())),
        (Some(_), Some(_)) => Err(ContractError::GenerationPolicyConflict {
            reason: "task sandbox policy is not the generation's exact sandbox policy".into(),
        }),
    }
}

/// Fold a Generation policy into a Task requirement: the effective hard
/// requirement is the stronger of the two on every dimension. A Task can never
/// widen its Generation's authority; a widening attempt fails closed.
pub fn fold_generation_policy(
    policy: &GenerationPolicy,
    task: &TaskRequirement,
) -> Result<TaskRequirement, ContractError> {
    if !policy
        .allowed_information_functions
        .contains(&task.information_function)
    {
        return Err(ContractError::GenerationPolicyConflict {
            reason: format!(
                "generation does not admit information function {}",
                task.information_function.as_sql()
            ),
        });
    }

    if let Some(allowed) = &policy.allowed_affinity {
        if !task.required_affinity.is_subset(allowed) {
            return Err(ContractError::GenerationPolicyConflict {
                reason: "task affinity exceeds the generation affinity ceiling".into(),
            });
        }
    }
    if let Some(constraint) = &policy.anchor_constraint {
        if task.required_anchor.as_ref() != Some(constraint) {
            return Err(ContractError::GenerationPolicyConflict {
                reason: "task anchor does not satisfy the generation anchor constraint".into(),
            });
        }
    }

    let mut required_capabilities = task.required_capabilities.clone();
    for (reference, policy_value) in &policy.required_capabilities {
        if !policy_value.is_present() {
            continue;
        }
        match required_capabilities.get(reference).cloned() {
            None => {
                required_capabilities.insert(reference.clone(), policy_value.clone());
            }
            Some(task_value) => {
                let Some(joined) =
                    join_requirement_values(task_value.matcher_kind(), &task_value, policy_value)
                else {
                    return Err(ContractError::RequirementConflict {
                        reason: format!(
                            "generation policy value for {}@{} is incompatible with the task requirement",
                            reference.capability_id().as_str(),
                            reference.revision()
                        ),
                    });
                };
                required_capabilities.insert(reference.clone(), joined);
            }
        }
    }

    let workspace =
        if workspace_rank(policy.min_workspace) >= workspace_rank(task.required_workspace) {
            policy.min_workspace
        } else {
            task.required_workspace
        };
    let network = if network_rank(policy.min_network) >= network_rank(task.required_network) {
        policy.min_network
    } else {
        task.required_network
    };
    let continuity = policy.min_continuity.max(task.required_continuity);
    let budget = if policy.budget_ceiling <= task.budget {
        policy.budget_ceiling
    } else {
        task.budget
    };

    Ok(TaskRequirement {
        information_function: task.information_function,
        required_capabilities,
        required_affinity: task.required_affinity.clone(),
        required_workspace: workspace,
        required_network: network,
        required_attempt_isolation: policy.requires_attempt_isolation
            || task.required_attempt_isolation,
        required_continuity: continuity,
        sandbox_policy: sandbox_fold(&policy.sandbox_policy, &task.sandbox_policy)?,
        required_anchor: task.required_anchor.clone(),
        budget,
    })
}
