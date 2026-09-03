//! Provider-neutral Root wakeup transport.
//!
//! A RootBridge delivers durable outbox wakeups. It owns no Scheduler
//! execution authority, Kernel, storage handle, or Result body. `Ok` means
//! only that this bridge positively proved its own delivery criterion.

#![forbid(unsafe_code)]

use agentype_core::{ManualClock, OutboxEventId};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Index values forwarded on a wakeup. Only well-formed ID / ID-list fields
/// from the durable outbox payload become indexes; arbitrary payload is
/// never a notification body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootIndex {
    Id(String),
    Ids(Vec<String>),
}

/// Root-facing wakeup envelope. Not Result transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootWakeup {
    event_id: OutboxEventId,
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    indexes: BTreeMap<String, RootIndex>,
}

impl RootWakeup {
    /// Construct from durable outbox identity plus payload. Unknown
    /// non-index fields are dropped. Malformed `*_id` / `*_ids` values
    /// fail closed rather than being forwarded as a blob.
    pub fn from_outbox(
        event_id: OutboxEventId,
        event_type: impl Into<String>,
        aggregate_type: impl Into<String>,
        aggregate_id: impl Into<String>,
        payload: &Value,
    ) -> Result<Self, WakeupEnvelopeError> {
        let object = match payload {
            Value::Object(map) => map,
            other => {
                return Err(WakeupEnvelopeError::PayloadNotObject {
                    detail: format!("payload must be a JSON object, got {other}"),
                });
            }
        };
        Ok(Self {
            event_id,
            event_type: event_type.into(),
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.into(),
            indexes: extract_indexes(object)?,
        })
    }

    pub fn event_id(&self) -> &OutboxEventId {
        &self.event_id
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn aggregate_type(&self) -> &str {
        &self.aggregate_type
    }

    pub fn aggregate_id(&self) -> &str {
        &self.aggregate_id
    }

    pub fn indexes(&self) -> &BTreeMap<String, RootIndex> {
        &self.indexes
    }
}

fn extract_indexes(
    object: &Map<String, Value>,
) -> Result<BTreeMap<String, RootIndex>, WakeupEnvelopeError> {
    let mut indexes = BTreeMap::new();
    for (key, value) in object {
        if key.ends_with("_ids") {
            indexes.insert(key.clone(), RootIndex::Ids(parse_id_list(key, value)?));
        } else if key.ends_with("_id") {
            indexes.insert(key.clone(), RootIndex::Id(parse_id(key, value)?));
        }
        // Non-index fields are dropped, never forwarded.
    }
    Ok(indexes)
}

fn parse_id(key: &str, value: &Value) -> Result<String, WakeupEnvelopeError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        other => Err(WakeupEnvelopeError::MalformedIndex {
            field: key.to_string(),
            detail: format!("expected string, got {other}"),
        }),
    }
}

fn parse_id_list(key: &str, value: &Value) -> Result<Vec<String>, WakeupEnvelopeError> {
    let Value::Array(items) = value else {
        return Err(WakeupEnvelopeError::MalformedIndex {
            field: key.to_string(),
            detail: format!("expected string array, got {value}"),
        });
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(s) => out.push(s.clone()),
            other => {
                return Err(WakeupEnvelopeError::MalformedIndex {
                    field: key.to_string(),
                    detail: format!("expected string element, got {other}"),
                });
            }
        }
    }
    Ok(out)
}

/// Fail-closed envelope construction. These are durable/invariant faults
/// for the notifier, not ordinary RootBridge delivery errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeupEnvelopeError {
    PayloadNotObject { detail: String },
    MalformedIndex { field: String, detail: String },
}

impl std::fmt::Display for WakeupEnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadNotObject { detail } => {
                write!(f, "outbox payload is not a JSON object: {detail}")
            }
            Self::MalformedIndex { field, detail } => {
                write!(f, "malformed wakeup index {field}: {detail}")
            }
        }
    }
}

impl std::error::Error for WakeupEnvelopeError {}

/// Positive proof that this bridge completed its delivery criterion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeliveryReceipt {
    detail: Option<String>,
}

impl DeliveryReceipt {
    pub fn proven() -> Self {
        Self { detail: None }
    }

    pub fn proven_with(detail: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()),
        }
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// Mechanical delivery failure. Not a Scheduler `FailureClass`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootBridgeError {
    Unavailable(String),
    DeadlineExceeded(String),
    Protocol(String),
    Rejected(String),
    Other(String),
}

impl RootBridgeError {
    pub fn diagnostic(&self) -> &str {
        match self {
            Self::Unavailable(m)
            | Self::DeadlineExceeded(m)
            | Self::Protocol(m)
            | Self::Rejected(m)
            | Self::Other(m) => m,
        }
    }
}

impl std::fmt::Display for RootBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "root bridge unavailable: {m}"),
            Self::DeadlineExceeded(m) => write!(f, "root bridge deadline exceeded: {m}"),
            Self::Protocol(m) => write!(f, "root bridge protocol: {m}"),
            Self::Rejected(m) => write!(f, "root bridge rejected: {m}"),
            Self::Other(m) => write!(f, "root bridge error: {m}"),
        }
    }
}

impl std::error::Error for RootBridgeError {}

pub type RootBridgeResult<T> = Result<T, RootBridgeError>;

/// Bounded wakeup side channel. Implementations MUST return within their
/// configured delivery budget. The notifier retries at-least-once; Result
/// transport remains in Scheduler storage.
pub trait RootBridge: Send + Sync {
    fn deliver(&self, wakeup: &RootWakeup) -> RootBridgeResult<DeliveryReceipt>;
}

/// Scripted next outcome for [`RecordingRootBridge`].
#[derive(Clone, Debug)]
enum ScriptedOutcome {
    Ok,
    Err(RootBridgeError),
    Panic(&'static str),
}

struct RecordingState {
    log: Vec<RootWakeup>,
    scripted: VecDeque<ScriptedOutcome>,
    hold: bool,
    in_flight: usize,
    clock: Option<Arc<ManualClock>>,
    advance_on_deliver: f64,
}

/// Deterministic fake: records every `deliver` by `event_id`, injects
/// success/error/panic, can block, and can advance a [`ManualClock`] during
/// the call so completion-time backoff is testable.
#[derive(Clone)]
pub struct RecordingRootBridge {
    state: Arc<Mutex<RecordingState>>,
    signal: Arc<Condvar>,
}

impl Default for RecordingRootBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingRootBridge {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState {
                log: Vec::new(),
                scripted: VecDeque::new(),
                hold: false,
                in_flight: 0,
                clock: None,
                advance_on_deliver: 0.0,
            })),
            signal: Arc::new(Condvar::new()),
        }
    }

    pub fn with_clock(clock: Arc<ManualClock>) -> Self {
        let bridge = Self::new();
        bridge.state.lock().expect("recording bridge").clock = Some(clock);
        bridge
    }

    /// Advance the attached clock by `dt` seconds *during* each `deliver`
    /// (after the call is observed, before it returns). Tests use this to
    /// prove backoff is anchored at completion, not call start.
    pub fn set_advance_on_deliver(&self, dt: f64) {
        self.state
            .lock()
            .expect("recording bridge")
            .advance_on_deliver = dt;
    }

    pub fn script_ok(&self) {
        self.state
            .lock()
            .expect("recording bridge")
            .scripted
            .push_back(ScriptedOutcome::Ok);
    }

    pub fn script_err(&self, err: RootBridgeError) {
        self.state
            .lock()
            .expect("recording bridge")
            .scripted
            .push_back(ScriptedOutcome::Err(err));
    }

    pub fn script_panic(&self, message: &'static str) {
        self.state
            .lock()
            .expect("recording bridge")
            .scripted
            .push_back(ScriptedOutcome::Panic(message));
    }

    /// Hold the next (and subsequent) `deliver` calls until [`release`].
    pub fn hold(&self) {
        self.state.lock().expect("recording bridge").hold = true;
    }

    pub fn release(&self) {
        let mut g = self.state.lock().expect("recording bridge");
        g.hold = false;
        self.signal.notify_all();
    }

    pub fn wait_until_in_flight(&self) {
        let mut g = self.state.lock().expect("recording bridge");
        while g.in_flight == 0 {
            g = self.signal.wait(g).expect("recording bridge");
        }
    }

    pub fn wait_until_in_flight_timeout(&self, timeout: Duration) -> bool {
        let mut g = self.state.lock().expect("recording bridge");
        let deadline = std::time::Instant::now() + timeout;
        while g.in_flight == 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, timed_out) = self
                .signal
                .wait_timeout(g, remaining)
                .expect("recording bridge");
            g = next;
            if timed_out.timed_out() && g.in_flight == 0 {
                return false;
            }
        }
        true
    }

    pub fn in_flight(&self) -> usize {
        self.state.lock().expect("recording bridge").in_flight
    }

    pub fn deliveries(&self) -> Vec<RootWakeup> {
        self.state.lock().expect("recording bridge").log.clone()
    }

    pub fn deliver_count(&self) -> usize {
        self.state.lock().expect("recording bridge").log.len()
    }
}

impl RootBridge for RecordingRootBridge {
    fn deliver(&self, wakeup: &RootWakeup) -> RootBridgeResult<DeliveryReceipt> {
        let mut g = self.state.lock().expect("recording bridge");
        g.in_flight += 1;
        self.signal.notify_all();
        while g.hold {
            g = self.signal.wait(g).expect("recording bridge");
        }
        if g.advance_on_deliver != 0.0 {
            if let Some(clock) = g.clock.clone() {
                clock.advance(g.advance_on_deliver);
            }
        }
        g.log.push(wakeup.clone());
        let outcome = g.scripted.pop_front().unwrap_or(ScriptedOutcome::Ok);
        g.in_flight -= 1;
        self.signal.notify_all();
        drop(g);
        match outcome {
            ScriptedOutcome::Ok => Ok(DeliveryReceipt::proven()),
            ScriptedOutcome::Err(err) => Err(err),
            ScriptedOutcome::Panic(message) => panic!("{message}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_core::Clock;
    use serde_json::json;

    fn wakeup_from(payload: Value) -> Result<RootWakeup, WakeupEnvelopeError> {
        RootWakeup::from_outbox(
            OutboxEventId::from_string("event_1"),
            "BATCH_RESULTS_READY",
            "batch",
            "batch-123",
            &payload,
        )
    }

    #[test]
    fn wakeup_preserves_identity_and_well_formed_indexes() {
        let wakeup = wakeup_from(json!({
            "batch_id": "batch-123",
            "task_ids": ["t1", "t2"],
            "result_body": {"secret": "nope"},
            "message": "hello"
        }))
        .unwrap();
        assert_eq!(wakeup.event_id().as_str(), "event_1");
        assert_eq!(wakeup.event_type(), "BATCH_RESULTS_READY");
        assert_eq!(wakeup.aggregate_type(), "batch");
        assert_eq!(wakeup.aggregate_id(), "batch-123");
        assert_eq!(
            wakeup.indexes().get("batch_id"),
            Some(&RootIndex::Id("batch-123".into()))
        );
        assert_eq!(
            wakeup.indexes().get("task_ids"),
            Some(&RootIndex::Ids(vec!["t1".into(), "t2".into()]))
        );
        assert!(!wakeup.indexes().contains_key("result_body"));
        assert!(!wakeup.indexes().contains_key("message"));
    }

    #[test]
    fn ids_suffix_is_checked_before_id_suffix() {
        let wakeup = wakeup_from(json!({"agent_ids": ["a", "b"]})).unwrap();
        assert_eq!(
            wakeup.indexes().get("agent_ids"),
            Some(&RootIndex::Ids(vec!["a".into(), "b".into()]))
        );
    }

    #[test]
    fn malformed_id_fails_closed() {
        let err = wakeup_from(json!({"batch_id": 1})).unwrap_err();
        assert!(matches!(err, WakeupEnvelopeError::MalformedIndex { .. }));
    }

    #[test]
    fn malformed_ids_fails_closed() {
        let err = wakeup_from(json!({"task_ids": "t1"})).unwrap_err();
        assert!(matches!(err, WakeupEnvelopeError::MalformedIndex { .. }));
        let err = wakeup_from(json!({"task_ids": [1]})).unwrap_err();
        assert!(matches!(err, WakeupEnvelopeError::MalformedIndex { .. }));
    }

    #[test]
    fn non_object_payload_fails_closed() {
        let err = wakeup_from(json!(["not", "an", "object"])).unwrap_err();
        assert!(matches!(err, WakeupEnvelopeError::PayloadNotObject { .. }));
    }

    #[test]
    fn recording_bridge_ok_is_positive_proof() {
        let bridge = RecordingRootBridge::new();
        let wakeup = wakeup_from(json!({"batch_id": "b"})).unwrap();
        assert!(bridge.deliver(&wakeup).is_ok());
        assert_eq!(bridge.deliver_count(), 1);
        assert_eq!(bridge.deliveries()[0].event_id().as_str(), "event_1");
    }

    #[test]
    fn recording_bridge_error_is_not_delivered() {
        let bridge = RecordingRootBridge::new();
        bridge.script_err(RootBridgeError::Unavailable("down".into()));
        let wakeup = wakeup_from(json!({"batch_id": "b"})).unwrap();
        let err = bridge.deliver(&wakeup).unwrap_err();
        assert!(matches!(err, RootBridgeError::Unavailable(_)));
        assert_eq!(bridge.deliver_count(), 1);
    }

    #[test]
    fn recording_bridge_advances_clock_during_deliver() {
        let clock = Arc::new(ManualClock::new(100.0));
        let bridge = RecordingRootBridge::with_clock(clock.clone());
        bridge.set_advance_on_deliver(50.0);
        let wakeup = wakeup_from(json!({"batch_id": "b"})).unwrap();
        let start = clock.now();
        bridge.deliver(&wakeup).unwrap();
        assert!((clock.now() - start - 50.0).abs() < 0.001);
    }
}
