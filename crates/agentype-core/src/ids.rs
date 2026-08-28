//! Correctness identities. These are opaque durable strings, not interchangeable.

use std::fmt;
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident, $prefix:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, Uuid::new_v4().simple()))
            }

            pub fn from_string(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

typed_id!(TaskId, "task");
typed_id!(BatchId, "batch");
typed_id!(AttemptId, "attempt");
typed_id!(LeaseId, "lease");
typed_id!(LogicalAgentId, "agent");
typed_id!(IncarnationId, "inc");
typed_id!(ExecutionId, "exec");
typed_id!(ResultId, "result");
typed_id!(EscalationId, "escalation");
typed_id!(CheckpointId, "checkpoint");
typed_id!(FailureId, "failure");
typed_id!(OutboxEventId, "event");
typed_id!(WorkstreamId, "workstream");
typed_id!(RequestId, "request");

/// Partition identity is the human-declared name (V0.1 topology).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartitionId(String);

impl PartitionId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PartitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for PartitionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Monotonic fencing token per Task. Claim increments it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LeaseEpoch(pub u64);

impl LeaseEpoch {
    pub fn initial() -> Self {
        Self(0)
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LeaseEpoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
