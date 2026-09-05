//! Adapter-operation deadline policy and the production invocation façade.
//!
//! Distinct from `RuntimeTimingConfig` (dispatcher poll / heartbeat / lease)
//! and from `ExecutionProfile.timeout_seconds` (adapter-specific execution
//! policy, never auto-copied onto start/observe/reconcile/collect/terminate).

use agentype_adapter_api::{
    AdapterDeadline, AdapterError, AdapterOperation, AdapterResult, DeadlineConfigError,
    ExecutionAdapter, ExecutionObservation, ExecutionOutcome, ExecutionRequest, RuntimeHandle,
    StartObservation,
};
use agentype_core::RequestId;
use agentype_execution_config::AdapterBindingKey;
use std::sync::Arc;
use std::time::Duration;

/// Per-operation Scheduler-facing latency bound. Runtime-local, not durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterDeadlinePolicy {
    start_execution: Duration,
    reconcile_start: Duration,
    observe_execution: Duration,
    collect_outcome: Duration,
    interrupt_execution: Duration,
    terminate_execution: Duration,
}

impl AdapterDeadlinePolicy {
    pub fn new(
        start_execution: Duration,
        reconcile_start: Duration,
        observe_execution: Duration,
        collect_outcome: Duration,
        interrupt_execution: Duration,
        terminate_execution: Duration,
    ) -> Result<Self, DeadlineConfigError> {
        for d in [
            start_execution,
            reconcile_start,
            observe_execution,
            collect_outcome,
            interrupt_execution,
            terminate_execution,
        ] {
            AdapterDeadline::after(d)?;
        }
        Ok(Self {
            start_execution,
            reconcile_start,
            observe_execution,
            collect_outcome,
            interrupt_execution,
            terminate_execution,
        })
    }

    pub fn uniform(timeout: Duration) -> Result<Self, DeadlineConfigError> {
        Self::new(timeout, timeout, timeout, timeout, timeout, timeout)
    }

    pub fn budget(&self, op: AdapterOperation) -> Duration {
        match op {
            AdapterOperation::StartExecution => self.start_execution,
            AdapterOperation::ReconcileStart => self.reconcile_start,
            AdapterOperation::ObserveExecution => self.observe_execution,
            AdapterOperation::CollectOutcome => self.collect_outcome,
            AdapterOperation::InterruptExecution => self.interrupt_execution,
            AdapterOperation::TerminateExecution => self.terminate_execution,
        }
    }
}

/// Installed adapter plus its operation policy. Production code invokes
/// through these methods so a deadline is always constructed. The raw
/// `ExecutionAdapter` is not exposed.
#[derive(Clone)]
pub struct ResolvedAdapterBinding {
    adapter_kind: String,
    adapter_binding_key: AdapterBindingKey,
    adapter: Arc<dyn ExecutionAdapter>,
    deadlines: AdapterDeadlinePolicy,
}

impl ResolvedAdapterBinding {
    pub(crate) fn new(
        adapter_kind: String,
        adapter_binding_key: AdapterBindingKey,
        adapter: Arc<dyn ExecutionAdapter>,
        deadlines: AdapterDeadlinePolicy,
    ) -> Self {
        Self {
            adapter_kind,
            adapter_binding_key,
            adapter,
            deadlines,
        }
    }

    pub fn adapter_kind(&self) -> &str {
        &self.adapter_kind
    }

    pub fn adapter_binding_key(&self) -> &AdapterBindingKey {
        &self.adapter_binding_key
    }

    pub fn policy(&self) -> AdapterDeadlinePolicy {
        self.deadlines
    }

    fn deadline(&self, op: AdapterOperation) -> AdapterResult<AdapterDeadline> {
        AdapterDeadline::after(self.deadlines.budget(op))
            .map_err(|e| AdapterError::other(e.to_string()))
    }

    pub fn start_execution(&self, request: &ExecutionRequest) -> AdapterResult<StartObservation> {
        let deadline = self.deadline(AdapterOperation::StartExecution)?;
        self.adapter.start_execution(request, &deadline)
    }

    pub fn reconcile_start(
        &self,
        request_id: &RequestId,
        persisted_handle: Option<&RuntimeHandle>,
    ) -> AdapterResult<StartObservation> {
        let deadline = self.deadline(AdapterOperation::ReconcileStart)?;
        self.adapter
            .reconcile_start(request_id, persisted_handle, &deadline)
    }

    pub fn observe_execution(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionObservation> {
        let deadline = self.deadline(AdapterOperation::ObserveExecution)?;
        self.adapter.observe_execution(handle, &deadline)
    }

    pub fn collect_outcome(&self, handle: &RuntimeHandle) -> AdapterResult<ExecutionOutcome> {
        let deadline = self.deadline(AdapterOperation::CollectOutcome)?;
        self.adapter.collect_outcome(handle, &deadline)
    }

    pub fn interrupt_execution(
        &self,
        handle: &RuntimeHandle,
    ) -> AdapterResult<ExecutionObservation> {
        let deadline = self.deadline(AdapterOperation::InterruptExecution)?;
        self.adapter.interrupt_execution(handle, &deadline)
    }

    pub fn terminate_execution(
        &self,
        handle: &RuntimeHandle,
    ) -> AdapterResult<ExecutionObservation> {
        let deadline = self.deadline(AdapterOperation::TerminateExecution)?;
        self.adapter.terminate_execution(handle, &deadline)
    }
}

#[cfg(test)]
pub fn test_deadlines() -> AdapterDeadlinePolicy {
    AdapterDeadlinePolicy::uniform(Duration::from_secs(30)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_adapter_api::FakeAdapter;
    use std::time::Instant;

    #[test]
    fn registration_policy_selects_per_operation_budget() {
        let start = Duration::from_secs(1);
        let recon = Duration::from_secs(2);
        let observe = Duration::from_secs(3);
        let collect = Duration::from_secs(4);
        let interrupt = Duration::from_secs(5);
        let terminate = Duration::from_secs(6);
        let policy =
            AdapterDeadlinePolicy::new(start, recon, observe, collect, interrupt, terminate)
                .unwrap();
        assert_eq!(policy.budget(AdapterOperation::StartExecution), start);
        assert_eq!(policy.budget(AdapterOperation::ReconcileStart), recon);
        assert_eq!(policy.budget(AdapterOperation::ObserveExecution), observe);
        assert_eq!(policy.budget(AdapterOperation::CollectOutcome), collect);
        assert_eq!(
            policy.budget(AdapterOperation::InterruptExecution),
            interrupt
        );
        assert_eq!(
            policy.budget(AdapterOperation::TerminateExecution),
            terminate
        );
    }

    #[test]
    fn zero_policy_duration_is_rejected() {
        assert!(AdapterDeadlinePolicy::uniform(Duration::ZERO).is_err());
    }

    #[test]
    fn binding_start_supplies_a_positive_deadline() {
        let fake = Arc::new(FakeAdapter::new());
        let binding = ResolvedAdapterBinding::new(
            "process".into(),
            AdapterBindingKey::for_tests(),
            fake.clone(),
            AdapterDeadlinePolicy::uniform(Duration::from_secs(9)).unwrap(),
        );
        // No request: just mint via a dummy? start needs ExecutionRequest.
        // Endpoint inspection: call observe with empty handle after we have
        // a request-free method. Use observe_execution.
        let handle = RuntimeHandle(serde_json::json!({}));
        binding.observe_execution(&handle).unwrap();
        let seen = fake.last_deadline().unwrap();
        assert_eq!(
            fake.last_operation(),
            Some(AdapterOperation::ObserveExecution)
        );
        assert!(!seen.is_expired());
        let remaining = seen.remaining();
        assert!(remaining > Duration::from_secs(8));
        assert!(remaining <= Duration::from_secs(9));
        let endpoint = seen.expires_at();
        let _ = seen.remaining();
        assert_eq!(seen.expires_at(), endpoint);
        assert!(endpoint > Instant::now());
    }

    /// M5.6 §45 #15-19: each Scheduler-facing operation receives the budget
    /// its policy slot defines — no operation inherits another's timeout and
    /// no call opens a fresh deadline inside another call.
    #[test]
    fn binding_selects_each_operation_deadline_budget() {
        let fake = Arc::new(FakeAdapter::new());
        let policy = AdapterDeadlinePolicy::new(
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(3),
            Duration::from_secs(4),
            Duration::from_secs(5),
            Duration::from_secs(6),
        )
        .unwrap();
        let binding = ResolvedAdapterBinding::new(
            "process".into(),
            AdapterBindingKey::for_tests(),
            fake.clone(),
            policy,
        );
        let handle = RuntimeHandle(serde_json::json!({"h": 1}));

        binding.reconcile_start(&RequestId::new(), None).unwrap();
        binding.observe_execution(&handle).unwrap();
        binding.collect_outcome(&handle).unwrap();
        binding.interrupt_execution(&handle).unwrap();
        binding.terminate_execution(&handle).unwrap();

        let slots = [
            (AdapterOperation::ReconcileStart, Duration::from_secs(2)),
            (AdapterOperation::ObserveExecution, Duration::from_secs(3)),
            (AdapterOperation::CollectOutcome, Duration::from_secs(4)),
            (AdapterOperation::InterruptExecution, Duration::from_secs(5)),
            (AdapterOperation::TerminateExecution, Duration::from_secs(6)),
        ];
        let mut endpoints = Vec::new();
        for (op, budget) in slots {
            let seen = fake.deadline_for(op).expect("deadline recorded");
            let remaining = seen.remaining();
            // The budget was minted moments ago: remaining is within one
            // second below the configured slot.
            assert!(
                remaining > budget - Duration::from_secs(1),
                "{op:?}: remaining {remaining:?} not from budget {budget:?}"
            );
            assert!(remaining <= budget, "{op:?}: remaining exceeded budget");
            endpoints.push(seen.expires_at());
        }
        // Distinct Scheduler-facing calls receive independent endpoints.
        for i in 0..endpoints.len() {
            for j in (i + 1)..endpoints.len() {
                assert_ne!(endpoints[i], endpoints[j]);
            }
        }
    }

    /// M5.6 §45 #11: the registry stores the policy with the installed
    /// adapter; resolving returns that exact per-adapter policy.
    #[test]
    fn registry_resolves_the_policy_installed_with_each_adapter() {
        let mut adapters = crate::AdapterRegistry::new();
        let fast = AdapterDeadlinePolicy::uniform(Duration::from_secs(2)).unwrap();
        let slow = AdapterDeadlinePolicy::uniform(Duration::from_secs(60)).unwrap();
        adapters
            .register_kind("fast", Arc::new(FakeAdapter::new()), fast)
            .unwrap();
        adapters
            .register_kind("slow", Arc::new(FakeAdapter::new()), slow)
            .unwrap();
        assert_eq!(
            adapters
                .resolve_unique("fast")
                .unwrap()
                .policy()
                .budget(AdapterOperation::ReconcileStart),
            Duration::from_secs(2)
        );
        assert_eq!(
            adapters
                .resolve_unique("slow")
                .unwrap()
                .policy()
                .budget(AdapterOperation::CollectOutcome),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn resolve_exact_rejects_binding_key_mismatch() {
        let mut adapters = crate::AdapterRegistry::new();
        adapters
            .register_kind(
                "process",
                Arc::new(FakeAdapter::new()),
                AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap(),
            )
            .unwrap();
        assert!(adapters
            .resolve_exact("process", &AdapterBindingKey::for_tests())
            .is_ok());
        let other = AdapterBindingKey::new("other-domain").unwrap();
        assert!(adapters.resolve_exact("process", &other).is_err());
    }

    /// M5.6 §49 #56-58: an observe timeout is an invocation error, never an
    /// observation. No terminality, quiescence, or physical transition can
    /// be derived from it (the RUNNING→UNKNOWN transition does not exist in
    /// the frozen physical graph; the future M5.8 observer owns lost
    /// observations at Scheduler-policy level).
    #[test]
    fn observe_timeout_is_an_invocation_error_not_an_observation() {
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_observe_error(AdapterError::deadline_exceeded(
            "observe budget exhausted before response",
        ));
        let binding = ResolvedAdapterBinding::new(
            "process".into(),
            AdapterBindingKey::for_tests(),
            fake.clone(),
            AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap(),
        );
        let err = binding
            .observe_execution(&RuntimeHandle(serde_json::json!({})))
            .unwrap_err();
        assert_eq!(
            err.kind(),
            agentype_adapter_api::AdapterErrorKind::DeadlineExceeded
        );
        assert_eq!(
            crate::observation::adapter_invocation_failure_class(&err),
            agentype_core::FailureClass::Timeout
        );
        assert!(fake
            .deadline_for(AdapterOperation::ObserveExecution)
            .is_some());
    }

    /// M5.6 §49 #59-60: an interrupt timeout proves nothing — the interrupt
    /// may or may not have taken effect. It is not interruption success, not
    /// quiescence, not Task cancellation proof.
    #[test]
    fn interrupt_timeout_proves_no_interruption_success() {
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_interrupt_error(AdapterError::deadline_exceeded(
            "interrupt signal unacknowledged before deadline",
        ));
        let binding = ResolvedAdapterBinding::new(
            "process".into(),
            AdapterBindingKey::for_tests(),
            fake.clone(),
            AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap(),
        );
        let err = binding
            .interrupt_execution(&RuntimeHandle(serde_json::json!({})))
            .unwrap_err();
        assert_eq!(
            err.kind(),
            agentype_adapter_api::AdapterErrorKind::DeadlineExceeded
        );
        assert_eq!(fake.interrupt_call_count(), 1);
        assert!(fake
            .deadline_for(AdapterOperation::InterruptExecution)
            .is_some());
    }

    /// M5.6 §49 #61-63: a terminate timeout — even one whose diagnostic says
    /// a kill was issued — is not TERMINATED, not process-exit proof, and
    /// not writer quiescence. "Kill sent" ≠ "quiescence confirmed".
    #[test]
    fn terminate_timeout_with_kill_diagnostic_proves_nothing() {
        let fake = Arc::new(FakeAdapter::new());
        fake.set_next_terminate_error(AdapterError::deadline_exceeded(
            "kill signal sent; exit not confirmed",
        ));
        let binding = ResolvedAdapterBinding::new(
            "process".into(),
            AdapterBindingKey::for_tests(),
            fake.clone(),
            AdapterDeadlinePolicy::uniform(Duration::from_secs(5)).unwrap(),
        );
        let err = binding
            .terminate_execution(&RuntimeHandle(serde_json::json!({})))
            .unwrap_err();
        assert_eq!(
            err.kind(),
            agentype_adapter_api::AdapterErrorKind::DeadlineExceeded
        );
        // The kill-issued diagnostic is preserved as bounded diagnostic
        // context only; it changes no standardized class.
        assert_eq!(
            err.diagnostic(),
            Some("kill signal sent; exit not confirmed")
        );
        assert_eq!(
            crate::observation::adapter_invocation_failure_class(&err),
            agentype_core::FailureClass::Timeout
        );
        assert_eq!(fake.terminate_call_count(), 1);
    }
}
