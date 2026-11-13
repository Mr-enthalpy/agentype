//! M6-B.5 B5.2 storage conformance: the durable security-acquisition block.
//!
//! This row is a liveness diagnostic, not Task authority: one row per Task and
//! per indistinguishable condition, released only by an explicit operator
//! action or a configuration/profile change, and never reopened for the same
//! released condition.

use agentype_agent_contract::{
    AcquisitionBudget, AcquisitionCost, AffinityConstraint, AgentRequirementDraft, AgentType,
    AgentTypeContract, AgentTypeRef, AgentTypeSelector, Budget, CandidateDomain, ContinuityMode,
    EnvironmentPolicy, FilePolicy, GenerationPolicy, LifecycleMode, NetworkEndpoint,
    NetworkEndpointSet, NetworkPolicy, NetworkTransport, NormalizedSandboxPolicy, PathSet,
    SandboxPolicyParts, SandboxPolicyRef, SecurityAcquisitionBlock, SecurityAcquisitionBlockStatus,
    SecurityAcquisitionRelease, SecurityContract, SecurityContractMode, ToolPolicy,
    WorkingDirectory,
};
use agentype_core::{
    Clock, Error, GenerationId, InformationFunction, LogicalAgentId, ManualClock, PartitionSpec,
    RawWorkIntent, Retention, SemanticInputSet, TaskId, TaskSpec, WorkspaceMode,
};
use agentype_storage_sqlite::Kernel;
use rusqlite::Connection;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;
const BINDING_PROFILE_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

struct Db {
    dir: PathBuf,
    path: PathBuf,
}

impl Db {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("agentype-b5-block-{tag}-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            path: dir.join("scheduler.db"),
            dir,
        }
    }

    fn kernel(&self, capacity: i64) -> Kernel {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_000.0));
        let kernel = Kernel::open(&self.path, clock, 10.0, MAX_BYTES).unwrap();
        kernel
            .upsert_partition(&PartitionSpec::new(
                "general",
                capacity,
                Retention::Resident,
                "local",
                "default",
            ))
            .unwrap();
        kernel.reconcile_pool().unwrap();
        kernel
    }

    fn raw(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }

    fn row_count(&self) -> i64 {
        self.raw()
            .query_row(
                "SELECT COUNT(*) FROM security_acquisition_blocks",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn type_ref(id: &str, revision: u64) -> AgentTypeRef {
    AgentTypeRef::new(id, revision).unwrap()
}

fn policy_ref(id: &str, revision: u64) -> SandboxPolicyRef {
    SandboxPolicyRef::new(id, revision).unwrap()
}

fn endpoint() -> NetworkEndpointSet {
    NetworkEndpointSet::allowed([NetworkEndpoint::new(
        NetworkTransport::Tcp,
        "example.com",
        Some(443),
    )
    .unwrap()])
}

fn policy(read: PathSet, write: PathSet, network: NetworkEndpointSet) -> NormalizedSandboxPolicy {
    NormalizedSandboxPolicy::from_parts(SandboxPolicyParts {
        files: FilePolicy::new(
            read,
            write,
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        ),
        tools: ToolPolicy::top(),
        network,
        environment: EnvironmentPolicy::top(),
        credentials: BTreeMap::new(),
        resources: BTreeMap::new(),
        working_directory: WorkingDirectory::Any,
        attempt_isolation: false,
    })
    .unwrap()
}

fn generation_policy(reference: SandboxPolicyRef) -> GenerationPolicy {
    GenerationPolicy {
        allowed_information_functions: vec![InformationFunction::Expand],
        max_workspace: WorkspaceMode::ReadOnly,
        max_network: NetworkPolicy::Restricted,
        requires_attempt_isolation: false,
        min_continuity: ContinuityMode::None,
        sandbox_policy: Some(reference),
        budget_ceiling: Budget::new(100.0).unwrap(),
        allowed_affinity: None,
        anchor_constraint: None,
    }
}

fn agent_type(
    id: &str,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
    sandbox_policy: Option<SandboxPolicyRef>,
) -> AgentType {
    AgentType {
        type_ref: type_ref(id, 1),
        based_on: None,
        contract: AgentTypeContract {
            allowed_information_functions: vec![InformationFunction::Expand],
            required_capabilities: BTreeMap::new(),
            affinity: AffinityConstraint::Any,
            budget_ceiling: Budget::new(100.0).unwrap(),
            security: SecurityContract {
                workspace,
                network,
                requires_attempt_isolation: false,
            },
            lifecycle: [LifecycleMode::Resident].into_iter().collect(),
            continuity: ContinuityMode::Logical,
            sandbox_policy,
            anchor_constraint: None,
        },
    }
}

fn draft(pin: AgentTypeRef) -> AgentRequirementDraft {
    AgentRequirementDraft {
        required_type: AgentTypeSelector::Exact(pin),
        required_capabilities: BTreeMap::new(),
        required_network: NetworkPolicy::Restricted,
        required_attempt_isolation: false,
        sandbox_policy: None,
        required_anchor: None,
        budget: Budget::new(50.0).unwrap(),
    }
}

fn proposal(
    kernel: &Kernel,
    generation: &GenerationId,
    key: &str,
) -> agentype_core::ProposalRecord {
    let intent = RawWorkIntent {
        raw_intent_key: key.into(),
        objective: "security work".into(),
        information_function: InformationFunction::Expand,
        semantic_input_set: SemanticInputSet::new(),
        rationale: None,
        suggested_task_spec: Some(TaskSpec::new("audit", json!({})).partition("general")),
    };
    kernel
        .compile_root_intent(generation, intent, "session", 1)
        .unwrap()
}

/// Admit a real `SECURITY_V1` Task and return its id and generation.
fn security_task(kernel: &Kernel, tag: &str) -> (TaskId, GenerationId) {
    let generation_ref = policy_ref("generation", 1);
    kernel
        .publish_sandbox_policy(
            &generation_ref,
            &policy(PathSet::any(), PathSet::deny_all(), endpoint()),
        )
        .unwrap();
    let pin_ref = policy_ref("pinned", 1);
    kernel
        .publish_sandbox_policy(
            &pin_ref,
            &policy(PathSet::any(), PathSet::deny_all(), endpoint()),
        )
        .unwrap();
    let agent = agent_type(
        "pinned",
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        Some(pin_ref),
    );
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();

    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(generation_ref)),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = proposal(kernel, &generation.generation_id, tag);
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(agent.type_ref),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    (task_id, generation.generation_id)
}

/// A coarse Task: the typed path has no compiled candidate round, so it can
/// never own this diagnostic.
fn coarse_task(kernel: &Kernel, tag: &str) -> (TaskId, GenerationId) {
    let agent = agent_type(
        "plain",
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
        None,
    );
    kernel.publish_agent_type(&agent).unwrap();
    let generation = kernel.create_generation(json!({})).unwrap();
    let proposal = proposal(kernel, &generation.generation_id, tag);
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(agent.type_ref))
        .unwrap();
    (task_id, generation.generation_id)
}

fn defer_block(
    task_id: &TaskId,
    generation_id: &GenerationId,
    agents: &[&str],
    observed: usize,
) -> SecurityAcquisitionBlock {
    let budget = AcquisitionBudget::new(1, 1).unwrap();
    let exhaustion = budget
        .require_whole_round(AcquisitionCost::new(observed, observed))
        .unwrap_err();
    let domain = CandidateDomain::from_ids(
        agents
            .iter()
            .map(|agent| LogicalAgentId::from_string(*agent)),
    );
    SecurityAcquisitionBlock::from_budget_defer(
        task_id.clone(),
        generation_id.clone(),
        domain,
        BINDING_PROFILE_DIGEST,
        exhaustion,
        1_000.0,
    )
    .unwrap()
}

#[test]
fn one_condition_is_one_durable_diagnostic() {
    let db = Db::new("one-condition");
    let kernel = db.kernel(1);
    let (task_id, generation_id) = security_task(&kernel, "one-condition");

    assert!(kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .is_none());

    let block = defer_block(&task_id, &generation_id, &["agent-a", "agent-b"], 2);
    kernel.record_security_acquisition_block(&block).unwrap();
    let stored = kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored, block);
    assert!(stored.is_blocked());
    assert_eq!(stored.candidate_count(), 2);
    assert_eq!(db.row_count(), 1);

    // The same indistinguishable condition is a no-op: no second row and no
    // rewritten timestamp.
    let same = defer_block(&task_id, &generation_id, &["agent-b", "agent-a"], 2);
    kernel.record_security_acquisition_block(&same).unwrap();
    assert_eq!(db.row_count(), 1);
    assert_eq!(
        kernel
            .get_security_acquisition_block(&task_id)
            .unwrap()
            .unwrap(),
        block
    );

    // A different candidate domain is a new condition: the diagnostic fields are
    // replaced in place.
    let newer = defer_block(&task_id, &generation_id, &["agent-a"], 5);
    kernel.record_security_acquisition_block(&newer).unwrap();
    assert_eq!(db.row_count(), 1);
    let replaced = kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(replaced.candidate_count(), 1);
    assert_eq!(replaced.cost(), AcquisitionCost::new(5, 5));
    assert!(replaced.is_blocked());
}

#[test]
fn a_released_condition_is_never_reopened() {
    let db = Db::new("release");
    let kernel = db.kernel(1);
    let (task_id, generation_id) = security_task(&kernel, "release");

    let block = defer_block(&task_id, &generation_id, &["agent-a", "agent-b"], 2);
    kernel.record_security_acquisition_block(&block).unwrap();

    assert!(kernel
        .release_security_acquisition_block(
            &task_id,
            SecurityAcquisitionRelease::OperatorAction,
            1_010.0
        )
        .unwrap());
    let released = kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(released.status(), SecurityAcquisitionBlockStatus::Released);
    assert_eq!(released.released_at(), Some(1_010.0));
    assert_eq!(
        released.release_reason(),
        Some(SecurityAcquisitionRelease::OperatorAction)
    );

    // Release is idempotent, and a different reason cannot overwrite the first.
    assert!(!kernel
        .release_security_acquisition_block(
            &task_id,
            SecurityAcquisitionRelease::ConfigurationChanged,
            1_020.0
        )
        .unwrap());
    assert_eq!(
        kernel
            .get_security_acquisition_block(&task_id)
            .unwrap()
            .unwrap()
            .release_reason(),
        Some(SecurityAcquisitionRelease::OperatorAction)
    );

    // Re-observing the same released condition MUST NOT resurrect the block.
    kernel
        .record_security_acquisition_block(&defer_block(
            &task_id,
            &generation_id,
            &["agent-a", "agent-b"],
            2,
        ))
        .unwrap();
    let still = kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(still.status(), SecurityAcquisitionBlockStatus::Released);
    assert_eq!(still.released_at(), Some(1_010.0));

    // A genuinely different condition is a new diagnostic and starts BLOCKED.
    let newer = defer_block(&task_id, &generation_id, &["agent-c"], 3);
    kernel.record_security_acquisition_block(&newer).unwrap();
    let reblocked = kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap();
    assert!(reblocked.is_blocked());
    assert_eq!(reblocked.released_at(), None);
    assert_eq!(reblocked.release_reason(), None);
    assert_eq!(reblocked.candidate_count(), 1);
}

#[test]
fn a_block_names_only_its_own_task_generation_and_mode() {
    let db = Db::new("identity");
    let kernel = db.kernel(2);
    let (task_id, generation_id) = security_task(&kernel, "identity");

    // A different Generation is not this Task's durable dispatch generation.
    let foreign_generation = GenerationId::from_string("generation-other");
    let error = kernel
        .record_security_acquisition_block(&defer_block(
            &task_id,
            &foreign_generation,
            &["agent-a"],
            2,
        ))
        .unwrap_err();
    assert!(matches!(error, Error::InvalidAuthority(_)));

    // The real generation is accepted.
    kernel
        .record_security_acquisition_block(&defer_block(&task_id, &generation_id, &["agent-a"], 2))
        .unwrap();

    // The coarse typed path has no candidate-compilation round at all, so a
    // block for it is refused by the durable parent rule.
    let (coarse_task, coarse_generation) = coarse_task(&kernel, "identity-coarse");
    let error = kernel
        .record_security_acquisition_block(&defer_block(
            &coarse_task,
            &coarse_generation,
            &["agent-a"],
            2,
        ))
        .unwrap_err();
    assert!(matches!(error, Error::Conflict(_)));

    // An unreleased task with no diagnostic has nothing to release.
    assert!(!kernel
        .release_security_acquisition_block(
            &coarse_task,
            SecurityAcquisitionRelease::ConfigurationChanged,
            1_100.0
        )
        .unwrap());
}

#[test]
fn a_release_timestamp_must_be_a_real_instant() {
    let db = Db::new("timestamp");
    let kernel = db.kernel(1);
    let (task_id, generation_id) = security_task(&kernel, "timestamp");
    kernel
        .record_security_acquisition_block(&defer_block(&task_id, &generation_id, &["agent-a"], 2))
        .unwrap();

    assert!(kernel
        .release_security_acquisition_block(
            &task_id,
            SecurityAcquisitionRelease::OperatorAction,
            f64::NAN,
        )
        .is_err());
    assert!(kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap()
        .is_blocked());
}

#[test]
fn the_durable_row_cannot_be_deleted_or_reopened_by_a_raw_writer() {
    let db = Db::new("raw");
    let kernel = db.kernel(1);
    let (task_id, generation_id) = security_task(&kernel, "raw");
    kernel
        .record_security_acquisition_block(&defer_block(&task_id, &generation_id, &["agent-a"], 2))
        .unwrap();
    kernel
        .release_security_acquisition_block(
            &task_id,
            SecurityAcquisitionRelease::ConfigurationChanged,
            1_010.0,
        )
        .unwrap();

    let raw = db.raw();
    // Diagnostics are released, never deleted.
    assert!(raw
        .execute(
            "DELETE FROM security_acquisition_blocks WHERE task_id=?1",
            [task_id.as_str()]
        )
        .is_err());
    // The same released condition cannot be reopened by rewriting the row.
    assert!(raw
        .execute(
            "UPDATE security_acquisition_blocks
             SET status='BLOCKED', released_at=NULL, release_reason=NULL
             WHERE task_id=?1",
            [task_id.as_str()]
        )
        .is_err());

    // Both attempts left the durable row intact and released.
    let stored = kernel
        .get_security_acquisition_block(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status(), SecurityAcquisitionBlockStatus::Released);
    assert_eq!(db.row_count(), 1);
}
