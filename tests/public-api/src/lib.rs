//! External-consumer boundary fixture.
//!
//! This package is not part of the shipped product. It is a permanent
//! compile-time witness for one claim:
//!
//! > A supported production consumer depends on `agentype-runtime` and
//! > composes the daemon from `SchedulerDaemonBuilder` →
//! > `RunningSchedulerDaemon` → `SchedulerControl`. The runtime-mechanical
//! > Kernel and supervision surfaces are not reachable through that API, and
//! > the mechanical Scheduler handle is not nameable at all.
//!
//! Every claim is a rustdoc probe rather than a prose statement:
//!
//! * [`supported_composition`] is a `no_run` doctest: it must keep
//!   **compiling**, so the supported path cannot silently disappear.
//! * [`probes`] mirrors the boundary item by item. Each probe is a
//!   `compile_fail` doctest, so widening the production API turns this package
//!   red instead of quietly shipping.
//!
//! The negative probes run against this package's **default** feature set. CI
//! invokes this package as its own `cargo test` (see
//! `.github/workflows/ci.yml`) precisely because `--all-features` would switch
//! `test-support` on and defeat the point of the probe.
//!
//! # What this is not
//!
//! This is a **package trust boundary**, not a capability sandbox. A repository
//! consumer that deliberately depends on the non-published
//! `agentype-storage-sqlite` crate and requests its `runtime-internal` feature
//! can still reach the mechanical Kernel surface. The fixture keeps the
//! *supported* production shape honest; it does not make bypass impossible.

/// The supported production composition, as a `no_run` doctest.
///
/// It must keep compiling. If a rename, a narrowing, or a removal breaks the
/// supported path, this fails the fixture build rather than only a prose
/// document.
///
/// ```no_run
/// use agentype_root_bridge::{DeliveryReceipt, RootBridge, RootBridgeResult, RootWakeup};
/// use agentype_runtime::{
///     AdapterRegistry, ExecutionRegistry, NotifierBinding, NotifierConfig, NotifierRetryPolicy,
///     PhysicalObserverConfig, RuntimeTimingConfig, SchedulerDaemonBuilder, SqliteRuntimeConfig,
/// };
///
/// /// Minimal production root bridge: a host supplies one of these.
/// struct HostRootBridge;
///
/// impl RootBridge for HostRootBridge {
///     fn deliver(&self, _wakeup: &RootWakeup) -> RootBridgeResult<DeliveryReceipt> {
///         Ok(DeliveryReceipt::proven())
///     }
/// }
///
/// fn compose() -> Result<(), Box<dyn std::error::Error>> {
///     let store = SqliteRuntimeConfig::new("/var/lib/agentype/scheduler.sqlite", 10.0, 16_384)?;
///     // The observer config relation 0 < poll < freshness_limit < lease_seconds
///     // is validated at construction and at daemon startup.
///     let observer = PhysicalObserverConfig::new(0.05, 1.0, 64, 10.0)?;
///     let timing = RuntimeTimingConfig::new(1.0, 2.0, 10.0)?;
///     let execution_registry = ExecutionRegistry::new();
///     let adapters = AdapterRegistry::new();
///     let notifier = NotifierBinding::Enabled {
///         config: NotifierConfig::new(1.0, 8, NotifierRetryPolicy::new(0.1, 30.0)?)?,
///         bridge: std::sync::Arc::new(HostRootBridge),
///     };
///
///     let daemon = SchedulerDaemonBuilder::new(
///         store,
///         timing,
///         observer,
///         execution_registry,
///         adapters,
///         notifier,
///     )?
///     .start()?;
///
///     // The narrow host control surface: submit, cancel, read, and
///     // acknowledge consumed worker results. No claim, no renewal, no
///     // worker ACK/NACK, no quiescence override, no dispatcher.
///     let control = daemon.control();
///     let _schema_version = control.schema_version()?;
///     drop(control);
///
///     let _exit = daemon.join();
///     Ok(())
/// }
/// ```
pub fn supported_composition() {}

/// The supported result-backed ingress, as a `no_run` doctest.
///
/// A host submits a worker suggestion by selecting the intent its durable
/// Result already carries. The caller cannot pass a `RawWorkIntent`, so the
/// proposal's provenance cannot be forged.
///
/// ```no_run
/// use agentype_core::{GenerationId, ResultId};
/// use agentype_runtime::IntentIngress;
///
/// fn submit(ingress: &IntentIngress<'_>, generation: &GenerationId, result: &ResultId) {
///     let _proposal = ingress
///         .compile_from_result(generation, result, "audit-session-race", 1)
///         .expect("result-backed compile");
/// }
/// ```
pub fn result_carried_ingress() {}

/// The supported M6-B.3 provisioning (operator) surface, as a `no_run` doctest.
///
/// It binds an existing LogicalAgent to an exact AgentType revision and reads
/// typed requirements / match results. It carries no semantic admission and no
/// M5 mechanical authority.
///
/// ```no_run
/// use agentype_agent_contract::AgentTypeRef;
/// use agentype_core::{LogicalAgentId, TaskId};
/// use agentype_runtime::ProvisioningAdmin;
///
/// fn use_admin(
///     admin: &ProvisioningAdmin<'_>,
///     agent: &LogicalAgentId,
///     task: &TaskId,
///     type_ref: &AgentTypeRef,
/// ) {
///     admin
///         .bind_logical_agent_type(agent, type_ref)
///         .expect("bind exact type");
///     let _ = admin.task_agent_requirement(task);
///     let _ = admin.logical_agent_type_binding(agent);
///     let _ = admin.match_existing_agents_for_task(task);
/// }
/// ```
pub fn provisioning_admin_surface() {}

/// Boundary probes: one `compile_fail` doctest per unreachable item.
///
/// The numbering is the boundary list this milestone was audited against.
///
/// ```compile_fail
/// // 1. The mechanical Scheduler handle is not nameable through the
/// //    supported surface: `agentype-runtime` does not re-export `Kernel`.
/// use agentype_runtime::Kernel;
/// ```
///
/// ```compile_fail
/// // 2. A second dispatcher cannot be constructed. `Dispatcher::new` is
/// //    crate-private; `Dispatcher::for_tests` exists only under
/// //    `test-support`.
/// use agentype_runtime::Dispatcher;
/// fn _no_second_dispatcher(dispatcher_new: fn()) {
///     let _ = agentype_runtime::Dispatcher::new;
/// }
/// ```
///
/// ```compile_fail
/// // 3. The supervision service cannot even be named: it is `pub(crate)`.
/// use agentype_runtime::supervision::SupervisionService;
/// ```
///
/// ```compile_fail
/// // 4. Periodic renewal is not callable. `SupervisionService::renew_due_now`
/// //    and `SupervisionRunner::start` are crate-private, and the runner is
/// //    surfaced only under `test-support`.
/// use agentype_runtime::SupervisionRunner;
/// ```
///
/// ```compile_fail
/// // 5. Raw recovery is not callable. `recover_runtime` and
/// //    `replay_persisted_terminal_consequence` are crate-private, and the
/// //    unlocked / notifier-less entry points exist only under
/// //    `test-support`.
/// use agentype_runtime::recover_runtime;
/// ```
///
/// ```compile_fail
/// // 6. The daemon hands out no mechanical handle.
/// use agentype_runtime::RunningSchedulerDaemon;
/// fn _no_kernel(daemon: &RunningSchedulerDaemon) {
///     let _ = daemon.kernel();
/// }
/// ```
///
/// ```compile_fail
/// // 7. Worker ACK/NACK is not reachable through the host control surface.
/// use agentype_core::{AttemptId, LeaseEpoch};
/// use agentype_runtime::SchedulerControl;
/// fn _no_worker_ack(control: &SchedulerControl<'_>, attempt: &AttemptId, epoch: LeaseEpoch) {
///     let _ = control.ack_success(attempt, epoch, None, &serde_json::Value::Null, None, true, false);
/// }
/// ```
///
/// ```compile_fail
/// // 8. The READY permit cannot be minted, so a second control loop cannot be
/// //    admitted.
/// use agentype_runtime::process_lock::ReadyPermit;
/// fn _no_ready_permit() {
///     let _ = ReadyPermit::mint();
/// }
/// ```
///
/// ```compile_fail
/// // 9. The runtime does not re-export the internal implementation crate, so
/// //    the mechanical storage surface has no path through it.
/// use agentype_runtime::agentype_storage_sqlite;
/// ```
///
/// ```compile_fail
/// // 10. Runtime workers cannot be created or started directly.
/// use agentype_runtime::ControlLoopRunner;
/// fn _no_direct_worker(service: ()) {
///     let _ = agentype_runtime::ControlLoopRunner::start;
///     let _ = service;
/// }
/// ```
///
/// ```compile_fail
/// // 11. The physical observer runner and service are not production API.
/// use agentype_runtime::observer::PhysicalObserverRunner;
/// fn _no_physical_observer(service: ()) {
///     let _ = agentype_runtime::observer::PhysicalObserverService::new;
///     let _ = service;
/// }
/// ```
///
/// ```compile_fail
/// // 12. The dispatch gate is not obtainable, so dispatch eligibility cannot
/// //     be driven from outside the Runtime.
/// use agentype_runtime::control::DispatchGate;
/// fn _no_gate() {
///     let _ = DispatchGate::closed();
/// }
/// ```
///
/// ```compile_fail
/// // 13. Worker ACK/NACK is not reachable through RootSemanticControl.
/// use agentype_core::{AttemptId, LeaseEpoch};
/// use agentype_runtime::RootSemanticControl;
/// fn _no_root_semantic_ack(control: &RootSemanticControl<'_>, attempt: &AttemptId, epoch: LeaseEpoch) {
///     let _ = control.ack_success(attempt, epoch, None, &serde_json::Value::Null, None, true, false);
/// }
/// ```
///
/// ```compile_fail
/// // 14. The proposal-only IntentIngress has no admission authority.
/// use agentype_core::ProposalId;
/// use agentype_runtime::IntentIngress;
/// fn _ingress_cannot_admit(ingress: &IntentIngress<'_>, proposal: &ProposalId) {
///     let _ = ingress.admit_proposal(proposal, 0, None);
/// }
/// ```
///
/// ```compile_fail
/// // 15. The proposal-only IntentIngress cannot reject a proposal.
/// use agentype_core::ProposalId;
/// use agentype_runtime::IntentIngress;
/// fn _ingress_cannot_reject(ingress: &IntentIngress<'_>, proposal: &ProposalId) {
///     let _ = ingress.reject_proposal(proposal, "no");
/// }
/// ```
///
/// ```compile_fail
/// // 16. The proposal-only IntentIngress cannot freeze a generation.
/// use agentype_core::GenerationId;
/// use agentype_runtime::IntentIngress;
/// fn _ingress_cannot_freeze(ingress: &IntentIngress<'_>, generation: &GenerationId) {
///     let _ = ingress.freeze_generation(generation, 0);
/// }
/// ```
///
/// ```compile_fail
/// // 17. The proposal-only IntentIngress cannot close a generation.
/// use agentype_core::GenerationId;
/// use agentype_runtime::IntentIngress;
/// fn _ingress_cannot_close(ingress: &IntentIngress<'_>, generation: &GenerationId) {
///     let _ = ingress.close_generation(generation, 0);
/// }
/// ```
///
/// ```compile_fail
/// // 18. Imported enforcement evidence has no constructor on the supported
/// //     default production surface; `for_tests` exists only under
/// //     `test-support`, so an ordinary consumer cannot mint a security proof.
/// use agentype_agent_contract::ResolvedProvisioningEvidence;
/// fn _no_evidence_mint() {
///     let _ = ResolvedProvisioningEvidence::for_tests;
/// }
/// ```
///
/// ```compile_fail
/// // 19. A validated SourceConfig revision cannot be forged: its fields are
/// //     private and it has no public constructor, so a workspace sibling can
/// //     only obtain one from the validated catalog read.
/// use agentype_storage_sqlite::SourceConfigRevision;
/// fn _no_revision_forge() {
///     let _ = SourceConfigRevision {
///         config: unreachable!(),
///         mode: unreachable!(),
///         locator: unreachable!(),
///         payload: unreachable!(),
///     };
/// }
/// ```
///
/// ```compile_fail
/// // 20. ProvisioningAdmin has no semantic admission authority.
/// use agentype_core::ProposalId;
/// use agentype_runtime::ProvisioningAdmin;
/// fn _admin_cannot_admit(admin: &ProvisioningAdmin<'_>, proposal: &ProposalId) {
///     let _ = admin.admit_proposal(proposal, 0, None);
/// }
/// ```
///
/// ```compile_fail
/// // 21. ProvisioningAdmin cannot freeze a generation.
/// use agentype_core::GenerationId;
/// use agentype_runtime::ProvisioningAdmin;
/// fn _admin_cannot_freeze(admin: &ProvisioningAdmin<'_>, generation: &GenerationId) {
///     let _ = admin.freeze_generation(generation, 0);
/// }
/// ```
///
/// ```compile_fail
/// // 22. ProvisioningAdmin has no mechanical claim authority.
/// use agentype_runtime::ProvisioningAdmin;
/// fn _admin_cannot_claim(admin: &ProvisioningAdmin<'_>) {
///     let _ = admin.claim_next_available();
/// }
/// ```
pub fn probes() {}
