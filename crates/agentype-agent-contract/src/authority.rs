//! The typed composite authority of a `SECURITY_V1` Task (ADR-0013 §5).
//!
//! `TaskSecurityBinding.policy` is exactly `meet(P_generation, P_task_fine)`; it
//! does not include the Task's coarse `workspace`/`network` upper bounds, the
//! actual bound AgentType authored ceiling, or the SpawnSource authored ceiling.
//! Passing the binding policy directly to an execution compiler is therefore
//! forbidden. [`EffectiveTaskAuthority`] is the only value that folds all four
//! terms and records the exact identities, so a later Execution commitment can
//! re-prove the same composition.
//!
//! The type is pure: it performs no I/O and grants no execution authority. It is
//! not a durable record and has no canonical encoding of its own; the durable
//! inputs (binding, requirement, catalog revisions) remain the authority.

use crate::canonical::{
    sandbox_policy_content_digest, security_derivation_content_digest,
    task_agent_requirement_content_digest,
};
use crate::error::ContractError;
use crate::records::{AgentType, AgentTypeRef, NetworkPolicy, SpawnSource, SpawnSourceRef};
use crate::requirement::TaskAgentRequirement;
use crate::sandbox::{
    meet_optional_layer, task_coarse_authority, EnforcementProfileRef, NormalizedSandboxPolicy,
    SecurityContractMode, SpawnSourceSecurityDeclaration, TaskSecurityBinding,
};
use agentype_core::{GenerationId, LogicalAgentId, TaskId, WorkspaceMode};

/// A borrowed, validated view of one AgentType revision's security identity.
///
/// A `SECURITY_V1` revision must declare an authored sandbox policy and must be
/// presented with the policy bytes that exact reference resolves to. A
/// `COARSE_V1` revision carries the frozen exact-reference relation and no fine
/// authored layer.
///
/// The construction rules are validated here, but the *content* binding between
/// `agent_type.contract.sandbox_policy` and `authored_policy` is established by
/// the validated catalog read that loaded both. This type records the policy
/// digest for identity; it does not re-open the catalog.
#[derive(Clone, Copy, Debug)]
pub struct SecurityRevision<'a> {
    agent_type: &'a AgentType,
    mode: SecurityContractMode,
    authored_policy: Option<&'a NormalizedSandboxPolicy>,
}

impl<'a> SecurityRevision<'a> {
    /// Build a validated view.
    ///
    /// `SECURITY_V1` requires both a declared policy reference and the loaded
    /// authored policy. `COARSE_V1` ignores any supplied policy: its frozen
    /// relation is reference equality, not the algebra.
    pub fn new(
        agent_type: &'a AgentType,
        mode: SecurityContractMode,
        authored_policy: Option<&'a NormalizedSandboxPolicy>,
    ) -> Result<Self, ContractError> {
        match mode {
            SecurityContractMode::SecurityV1 => {
                if agent_type.contract.sandbox_policy.is_none() {
                    return Err(ContractError::InvariantViolation(
                        "a SECURITY_V1 agent type is missing its authored sandbox policy".into(),
                    ));
                }
                let policy = authored_policy.ok_or_else(|| {
                    ContractError::InvariantViolation(
                        "a SECURITY_V1 agent type was presented without its authored policy".into(),
                    )
                })?;
                Ok(Self {
                    agent_type,
                    mode,
                    authored_policy: Some(policy),
                })
            }
            SecurityContractMode::CoarseV1 => Ok(Self {
                agent_type,
                mode,
                authored_policy: None,
            }),
        }
    }

    pub fn agent_type(&self) -> &'a AgentType {
        self.agent_type
    }

    pub fn mode(&self) -> SecurityContractMode {
        self.mode
    }

    pub fn authored_policy(&self) -> Option<&'a NormalizedSandboxPolicy> {
        self.authored_policy
    }

    pub fn is_security_v1(&self) -> bool {
        self.mode == SecurityContractMode::SecurityV1
    }
}

/// The Task-side security ceiling before a candidate is bound.
///
/// ```text
/// P_task = TaskAuthority(meet(P_generation, P_task_fine))
/// ```
///
/// This is exactly the policy admission checked the pinned AgentType against,
/// and it is the policy preselection MUST use: it is candidate-independent, so
/// one ceiling is compared against every candidate's authored policy. It is not
/// the complete execution authority — the bound AgentType and SpawnSource
/// ceilings still have to be folded in, which is what
/// [`EffectiveTaskAuthority`] does once a candidate tuple exists.
///
/// The type records the durable identities it was derived from so a caller
/// cannot silently substitute a different Task's policy, and it grants no
/// authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSecurityCeiling {
    task_id: TaskId,
    policy: NormalizedSandboxPolicy,
    policy_digest: String,
    binding_policy_digest: String,
    binding_derivation_digest: String,
    requirement_digest: String,
    required_workspace: WorkspaceMode,
    required_network: NetworkPolicy,
}

impl TaskSecurityCeiling {
    /// The only constructor. The binding must be supported by this build and
    /// must describe exactly its own policy.
    pub fn from_binding(
        task_id: TaskId,
        binding: &TaskSecurityBinding,
        requirement: &TaskAgentRequirement,
    ) -> Result<Self, ContractError> {
        let binding_policy_digest = validate_binding_policy(binding)?;
        let policy = task_coarse_authority(
            binding.policy(),
            requirement.hard.required_workspace,
            requirement.hard.required_network,
        )?;
        let policy_digest = sandbox_policy_content_digest(&policy);
        Ok(Self {
            task_id,
            policy,
            policy_digest,
            binding_policy_digest,
            binding_derivation_digest: security_derivation_content_digest(binding.derivation()),
            requirement_digest: task_agent_requirement_content_digest(requirement),
            required_workspace: requirement.hard.required_workspace,
            required_network: requirement.hard.required_network,
        })
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    /// The complete Task-side policy. This is what a candidate's authored
    /// policy must bound.
    pub fn policy(&self) -> &NormalizedSandboxPolicy {
        &self.policy
    }

    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    pub fn binding_policy_digest(&self) -> &str {
        &self.binding_policy_digest
    }

    pub fn binding_derivation_digest(&self) -> &str {
        &self.binding_derivation_digest
    }

    pub fn requirement_digest(&self) -> &str {
        &self.requirement_digest
    }

    /// The Task coarse mode the ceiling was derived from. A caller that presents
    /// a different requirement fails closed instead of comparing two Tasks.
    pub fn required_workspace(&self) -> WorkspaceMode {
        self.required_workspace
    }

    pub fn required_network(&self) -> NetworkPolicy {
        self.required_network
    }
}

/// A binding must describe exactly its own policy with a derivation this build
/// implements, or the durable row is not authority at all.
fn validate_binding_policy(binding: &TaskSecurityBinding) -> Result<String, ContractError> {
    binding.derivation().ensure_supported()?;
    let digest = sandbox_policy_content_digest(binding.policy());
    if binding.derivation().output_digest() != digest {
        return Err(ContractError::InvariantViolation(
            "task security derivation does not describe its own policy".into(),
        ));
    }
    Ok(digest)
}

/// The complete, typed authority of one `SECURITY_V1` Task for one exact
/// candidate tuple.
///
/// ```text
/// P_eff = TaskAuthority(meet(P_binding, P_agent, P_source))
/// ```
///
/// where `TaskAuthority` removes write/delete for `ReadOnly` and network for
/// `Disabled`. Because the Task's coarse upper bounds are applied last, a coarse
/// requirement can never be widened by a later term, and a fine term can still
/// erase a positive coarse requirement — the candidate stage MUST re-check the
/// modes against `policy()`, it MUST NOT reuse an admission-time verdict.
///
/// Every identity needed to re-prove the composition is recorded. The durable
/// binding policy itself is not copied: its content digest, derivation digest and
/// required-access digest are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveTaskAuthority {
    task_id: TaskId,
    generation_id: GenerationId,
    logical_agent_id: LogicalAgentId,
    agent_type_ref: AgentTypeRef,
    spawn_source_ref: SpawnSourceRef,
    enforcement_profile: EnforcementProfileRef,
    effective_policy: NormalizedSandboxPolicy,
    effective_policy_digest: String,
    binding_policy_digest: String,
    binding_derivation_digest: String,
    required_access_digest: String,
    requirement_digest: String,
    agent_policy_digest: String,
    source_policy_digest: Option<String>,
}

impl EffectiveTaskAuthority {
    /// The only constructor. It folds exactly the four authority terms and
    /// applies the Task's durable coarse ceiling last.
    ///
    /// A meet conflict is a candidate-local unsatisfiability
    /// (`SecurityUnenforceable`), never catalog corruption: the durable inputs
    /// were each valid; only their combination has no satisfying policy.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        generation_id: GenerationId,
        logical_agent_id: LogicalAgentId,
        binding: &TaskSecurityBinding,
        requirement: &TaskAgentRequirement,
        agent: &SecurityRevision<'_>,
        source: &SpawnSource,
        source_declaration: &SpawnSourceSecurityDeclaration,
        source_policy: Option<&NormalizedSandboxPolicy>,
    ) -> Result<Self, ContractError> {
        if !agent.is_security_v1() {
            return Err(ContractError::SecurityUnenforceable {
                reason: "a COARSE_V1 agent type cannot carry SECURITY_V1 task authority".into(),
            });
        }
        let agent_policy = agent.authored_policy().ok_or_else(|| {
            ContractError::InvariantViolation(
                "a SECURITY_V1 agent revision lost its authored policy".into(),
            )
        })?;

        // The source declaration owns the imported profile identity and the
        // authored ceiling identity. A present declaration policy must describe
        // exactly the policy bytes the caller read for that reference; an absent
        // one admits no source layer at all.
        match (source_declaration.source_policy(), source_policy) {
            (None, None) => {}
            (Some(input), Some(policy)) => {
                let actual = sandbox_policy_content_digest(policy);
                if input.policy_digest() != actual {
                    return Err(ContractError::EvidenceSubjectMismatch {
                        reason: "spawn source authored policy does not match its declaration"
                            .into(),
                    });
                }
            }
            _ => {
                return Err(ContractError::EvidenceSubjectMismatch {
                    reason: "spawn source authored policy presence disagrees with its declaration"
                        .into(),
                })
            }
        }

        // The binding must describe exactly its own policy, and its derivation
        // must be one this build implements.
        let binding_policy_digest = validate_binding_policy(binding)?;

        let agent_policy_digest = sandbox_policy_content_digest(agent_policy);
        let source_policy_digest = source_policy.map(sandbox_policy_content_digest);

        let bounded =
            meet_optional_layer(binding.policy(), Some(agent_policy)).map_err(|conflict| {
                ContractError::SecurityUnenforceable {
                    reason: format!("binding and AgentType policies do not meet: {conflict}"),
                }
            })?;
        let bounded = meet_optional_layer(&bounded, source_policy).map_err(|conflict| {
            ContractError::SecurityUnenforceable {
                reason: format!("source ceiling and candidate policies do not meet: {conflict}"),
            }
        })?;
        let effective_policy = task_coarse_authority(
            &bounded,
            requirement.hard.required_workspace,
            requirement.hard.required_network,
        )?;
        let effective_policy_digest = sandbox_policy_content_digest(&effective_policy);

        Ok(Self {
            task_id,
            generation_id,
            logical_agent_id,
            agent_type_ref: agent.agent_type().type_ref.clone(),
            spawn_source_ref: source.source_ref.clone(),
            enforcement_profile: source_declaration.enforcement_profile().clone(),
            effective_policy,
            effective_policy_digest,
            binding_policy_digest,
            binding_derivation_digest: security_derivation_content_digest(binding.derivation()),
            required_access_digest: binding.derivation().required_access_digest().to_string(),
            requirement_digest: task_agent_requirement_content_digest(requirement),
            agent_policy_digest,
            source_policy_digest,
        })
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn generation_id(&self) -> &GenerationId {
        &self.generation_id
    }

    pub fn logical_agent_id(&self) -> &LogicalAgentId {
        &self.logical_agent_id
    }

    pub fn agent_type_ref(&self) -> &AgentTypeRef {
        &self.agent_type_ref
    }

    pub fn spawn_source_ref(&self) -> &SpawnSourceRef {
        &self.spawn_source_ref
    }

    pub fn enforcement_profile(&self) -> &EnforcementProfileRef {
        &self.enforcement_profile
    }

    /// The complete effective policy. This is the only value an execution
    /// compiler may accept; `binding.policy()` is deliberately unreachable here.
    pub fn policy(&self) -> &NormalizedSandboxPolicy {
        &self.effective_policy
    }

    pub fn effective_policy_digest(&self) -> &str {
        &self.effective_policy_digest
    }

    pub fn binding_policy_digest(&self) -> &str {
        &self.binding_policy_digest
    }

    pub fn binding_derivation_digest(&self) -> &str {
        &self.binding_derivation_digest
    }

    pub fn required_access_digest(&self) -> &str {
        &self.required_access_digest
    }

    pub fn requirement_digest(&self) -> &str {
        &self.requirement_digest
    }

    pub fn agent_policy_digest(&self) -> &str {
        &self.agent_policy_digest
    }

    pub fn source_policy_digest(&self) -> Option<&str> {
        self.source_policy_digest.as_deref()
    }

    /// Re-check the Task's positive coarse modes against the complete policy.
    ///
    /// Admission checked the fine meet alone. A later intersection with the
    /// AgentType or SpawnSource ceiling can erase the only writable or reachable
    /// region, so every candidate stage MUST call this on the composed policy.
    pub fn validate_coarse_modes(
        &self,
        requirement: &TaskAgentRequirement,
    ) -> Result<(), ContractError> {
        crate::sandbox::validate_effective_task_execution_modes(
            requirement.hard.required_workspace,
            requirement.hard.required_network,
            &self.effective_policy,
        )
        .map_err(|conflict| ContractError::SecurityUnenforceable {
            reason: format!(
                "the complete effective policy no longer satisfies the Task coarse modes: {conflict}"
            ),
        })
    }

    /// The Task's coarse ceiling as a pure view, for tests and for callers that
    /// must distinguish `ReadOnly`/`Disabled` without re-reading the requirement.
    pub fn coarse_modes(requirement: &TaskAgentRequirement) -> (WorkspaceMode, NetworkPolicy) {
        (
            requirement.hard.required_workspace,
            requirement.hard.required_network,
        )
    }
}
