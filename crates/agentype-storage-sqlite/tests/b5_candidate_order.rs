//! M6-B.5 B5.2 storage conformance: mode-aware existing-agent preselection.
//!
//! A `SECURITY_V1` Task is ranked over the proven policy order, not policy
//! reference equality, and `COARSE_V1` candidates are excluded from it. A
//! `COARSE_V1` Task keeps the frozen relation byte-for-byte.

use agentype_agent_contract::{
    AdapterPolicyRef, AffinityConstraint, AgentRequirementDraft, AgentType, AgentTypeContract,
    AgentTypeRef, AgentTypeSelector, Budget, ContinuityMode, EnforcementProfileRef,
    EnvironmentPolicy, FilePolicy, GenerationPolicy, LifecycleMode, NetworkEndpoint,
    NetworkEndpointSet, NetworkPolicy, NetworkTransport, NormalizedSandboxPolicy, PathSelector,
    PathSet, SandboxPolicyParts, SandboxPolicyRef, SecurityContract, SecurityContractMode,
    ToolPolicy, WorkingDirectory,
};
use agentype_core::{
    Clock, GenerationId, InformationFunction, ManualClock, PartitionSpec, RawWorkIntent, Retention,
    SemanticInputSet, TaskSpec, WorkspaceMode,
};
use agentype_storage_sqlite::Kernel;
use rusqlite::Connection;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BYTES: usize = 16_384;

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
        let dir = std::env::temp_dir().join(format!("agentype-b5-order-{tag}-{}", nanos()));
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

    fn ready_agents(&self) -> Vec<String> {
        let conn = Connection::open(&self.path).unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM logical_agents WHERE partition_name='general' AND state='READY' ORDER BY id")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
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

fn workspace(components: &[&str]) -> PathSet {
    PathSet::allowed([PathSelector::workspace(components.iter().copied()).unwrap()])
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

fn publish_secure(
    kernel: &Kernel,
    id: &str,
    reference: SandboxPolicyRef,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
) -> AgentTypeRef {
    let agent = agent_type(id, workspace, network, Some(reference));
    kernel
        .publish_agent_type_with_security(&agent, SecurityContractMode::SecurityV1)
        .unwrap();
    agent.type_ref
}

fn publish_coarse(kernel: &Kernel, id: &str) -> AgentTypeRef {
    let agent = agent_type(id, WorkspaceMode::ReadOnly, NetworkPolicy::Restricted, None);
    kernel.publish_agent_type(&agent).unwrap();
    agent.type_ref
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

fn agent_id_for(kernel: &Kernel, db: &Db, pin: &AgentTypeRef, index: usize) -> String {
    let agents = db.ready_agents();
    let agent = agentype_core::LogicalAgentId::from_string(agents[index].clone());
    kernel.bind_logical_agent_type(&agent, pin).unwrap();
    agents[index].clone()
}

#[test]
fn security_task_ranks_by_policy_order_and_excludes_coarse_candidates() {
    let db = Db::new("security-order");
    let kernel = db.kernel(4);

    // Generation ceiling: read anything, write nothing, one endpoint.
    let generation_ref = policy_ref("generation", 1);
    kernel
        .publish_sandbox_policy(
            &generation_ref,
            &policy(PathSet::any(), PathSet::deny_all(), endpoint()),
        )
        .unwrap();
    // The pinned type bounds more than the Task needs: it may write `src` and
    // reach any endpoint.
    let pinned_ref = policy_ref("pinned", 1);
    kernel
        .publish_sandbox_policy(
            &pinned_ref,
            &policy(
                PathSet::any(),
                workspace(&["src"]),
                NetworkEndpointSet::any(),
            ),
        )
        .unwrap();

    let pinned = publish_secure(
        &kernel,
        "pinned",
        pinned_ref,
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );

    // Candidate A: exactly the Task ceiling under a *different* reference. This
    // is the B5.2 case reference equality used to drop.
    let narrow_ref = policy_ref("narrow", 1);
    kernel
        .publish_sandbox_policy(
            &narrow_ref,
            &policy(PathSet::any(), PathSet::deny_all(), endpoint()),
        )
        .unwrap();
    let narrow = publish_secure(
        &kernel,
        "narrow",
        narrow_ref,
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );

    // Candidate B: broader than the pin.
    let broad_ref = policy_ref("broad", 1);
    kernel
        .publish_sandbox_policy(
            &broad_ref,
            &policy(PathSet::any(), PathSet::any(), NetworkEndpointSet::any()),
        )
        .unwrap();
    let broad = publish_secure(
        &kernel,
        "broad",
        broad_ref,
        WorkspaceMode::Write,
        NetworkPolicy::Enabled,
    );

    // Candidate C: cannot bound the Task's `read anything` floor.
    let narrow_read_ref = policy_ref("narrow-read", 1);
    kernel
        .publish_sandbox_policy(
            &narrow_read_ref,
            &policy(workspace(&["sandbox"]), PathSet::deny_all(), endpoint()),
        )
        .unwrap();
    let narrow_read = publish_secure(
        &kernel,
        "narrow-read",
        narrow_read_ref,
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );

    // Candidate D: COARSE_V1. It must never enter a SECURITY_V1 candidate set.
    let coarse = publish_coarse(&kernel, "coarse");

    let agent_narrow = agent_id_for(&kernel, &db, &narrow, 0);
    let agent_broad = agent_id_for(&kernel, &db, &broad, 1);
    let agent_narrow_read = agent_id_for(&kernel, &db, &narrow_read, 2);
    let agent_coarse = agent_id_for(&kernel, &db, &coarse, 3);

    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(generation_ref)),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = proposal(&kernel, &generation.generation_id, "order");
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pinned),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    let matched = kernel.match_existing_agents_for_task(&task_id).unwrap();
    let ids: Vec<&str> = matched
        .iter()
        .map(|candidate| candidate.logical_agent_id.as_str())
        .collect();
    assert_eq!(ids, vec![agent_narrow.as_str(), agent_broad.as_str()]);
    assert!(!ids.contains(&agent_narrow_read.as_str()));
    assert!(!ids.contains(&agent_coarse.as_str()));
}

#[test]
fn coarse_task_keeps_the_frozen_reference_relation() {
    let db = Db::new("coarse-order");
    let kernel = db.kernel(2);
    let pinned = publish_coarse(&kernel, "plain-a");
    let other = publish_coarse(&kernel, "plain-b");

    let agent_a = agent_id_for(&kernel, &db, &pinned, 0);
    let agent_b = agent_id_for(&kernel, &db, &other, 1);

    let generation = kernel.create_generation(json!({})).unwrap();
    let proposal = proposal(&kernel, &generation.generation_id, "coarse");
    let task_id = kernel
        .admit_typed_proposal(&proposal.proposal_id, 0, None, draft(pinned))
        .unwrap();
    assert_eq!(
        kernel.get_task_security_mode(&task_id).unwrap(),
        SecurityContractMode::CoarseV1
    );

    let matched = kernel.match_existing_agents_for_task(&task_id).unwrap();
    let ids: Vec<&str> = matched
        .iter()
        .map(|candidate| candidate.logical_agent_id.as_str())
        .collect();
    // The exact pin ranks first; the other compatible COARSE type remains a
    // candidate, exactly as before B5.2.
    assert_eq!(ids, vec![agent_a.as_str(), agent_b.as_str()]);
}

#[test]
fn a_security_task_without_a_binding_is_never_silently_coarsened() {
    let db = Db::new("missing-binding");
    let kernel = db.kernel(1);
    let generation_ref = policy_ref("generation", 1);
    kernel
        .publish_sandbox_policy(
            &generation_ref,
            &policy(PathSet::any(), PathSet::deny_all(), endpoint()),
        )
        .unwrap();
    let pinned = publish_secure(
        &kernel,
        "pinned",
        generation_ref.clone(),
        WorkspaceMode::ReadOnly,
        NetworkPolicy::Restricted,
    );

    let generation = kernel
        .create_generation_with_security_mode(
            json!({}),
            Some(generation_policy(generation_ref)),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();
    let proposal = proposal(&kernel, &generation.generation_id, "missing");
    let task_id = kernel
        .admit_typed_proposal_with_security_mode(
            &proposal.proposal_id,
            0,
            None,
            draft(pinned),
            SecurityContractMode::SecurityV1,
        )
        .unwrap();

    // Simulate a bypassing writer that removed the binding while the requirement
    // still claims SECURITY_V1.
    {
        let conn = Connection::open(&db.path).unwrap();
        conn.execute("PRAGMA foreign_keys=OFF", []).unwrap();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='trigger'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        for name in names {
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}"))
                .unwrap();
        }
        conn.execute(
            "DELETE FROM task_security_bindings WHERE task_id=?1",
            [task_id.as_str()],
        )
        .unwrap();
    }

    let error = kernel.match_existing_agents_for_task(&task_id).unwrap_err();
    assert!(matches!(error, agentype_core::Error::InvariantViolation(_)));
}

// Keep `AdapterPolicyRef` and `EnforcementProfileRef` imported: the storage
// fixtures deliberately use the same vocabulary as the contract layer.
#[allow(dead_code)]
fn _refs() -> (AdapterPolicyRef, EnforcementProfileRef) {
    (
        AdapterPolicyRef::new("adapter", 1).unwrap(),
        EnforcementProfileRef::new("enforcement", "profile", 1).unwrap(),
    )
}
