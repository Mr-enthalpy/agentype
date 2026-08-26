//! Closed persisted state machines. Storage converts these to/from TEXT.

use std::fmt;
use std::str::FromStr;

use crate::Error;

macro_rules! closed_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant)),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(stringify!($variant) => Ok(Self::$variant),)+
                    other => Err(Error::InvariantViolation(format!(
                        "unknown {} {other}",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

closed_enum!(TaskState {
    Blocked,
    Queued,
    Leased,
    Running,
    RetryWait,
    Suspended,
    Completed,
    Cancelled,
});

impl TaskState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Blocked => "BLOCKED",
            Self::Queued => "QUEUED",
            Self::Leased => "LEASED",
            Self::Running => "RUNNING",
            Self::RetryWait => "RETRY_WAIT",
            Self::Suspended => "SUSPENDED",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "BLOCKED" => Ok(Self::Blocked),
            "QUEUED" => Ok(Self::Queued),
            "LEASED" => Ok(Self::Leased),
            "RUNNING" => Ok(Self::Running),
            "RETRY_WAIT" => Ok(Self::RetryWait),
            "SUSPENDED" => Ok(Self::Suspended),
            "COMPLETED" => Ok(Self::Completed),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(Error::InvariantViolation(format!(
                "unknown TaskState {other}"
            ))),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

closed_enum!(AttemptState {
    Active,
    Succeeded,
    Failed,
    Expired,
    Cancelled,
});

impl AttemptState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Expired => "EXPIRED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "ACTIVE" => Ok(Self::Active),
            "SUCCEEDED" => Ok(Self::Succeeded),
            "FAILED" => Ok(Self::Failed),
            "EXPIRED" => Ok(Self::Expired),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(Error::InvariantViolation(format!(
                "unknown AttemptState {other}"
            ))),
        }
    }
}

closed_enum!(LeaseState {
    Active,
    Released,
    Expired,
    Revoked,
});

impl LeaseState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Released => "RELEASED",
            Self::Expired => "EXPIRED",
            Self::Revoked => "REVOKED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "ACTIVE" => Ok(Self::Active),
            "RELEASED" => Ok(Self::Released),
            "EXPIRED" => Ok(Self::Expired),
            "REVOKED" => Ok(Self::Revoked),
            other => Err(Error::InvariantViolation(format!(
                "unknown LeaseState {other}"
            ))),
        }
    }
}

closed_enum!(ExecutionState {
    Starting,
    Running,
    Succeeded,
    Failed,
    Lost,
    Unknown,
    Terminated,
});

impl ExecutionState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Lost => "LOST",
            Self::Unknown => "UNKNOWN",
            Self::Terminated => "TERMINATED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "STARTING" => Ok(Self::Starting),
            "RUNNING" => Ok(Self::Running),
            "SUCCEEDED" => Ok(Self::Succeeded),
            "FAILED" => Ok(Self::Failed),
            "LOST" => Ok(Self::Lost),
            "UNKNOWN" => Ok(Self::Unknown),
            "TERMINATED" => Ok(Self::Terminated),
            other => Err(Error::InvariantViolation(format!(
                "unknown ExecutionState {other}"
            ))),
        }
    }

    pub fn is_active_physical(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Unknown)
    }
}

closed_enum!(ResultState {
    Available,
    Acked,
});

impl ResultState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
            Self::Acked => "ACKED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "AVAILABLE" => Ok(Self::Available),
            "ACKED" => Ok(Self::Acked),
            other => Err(Error::InvariantViolation(format!(
                "unknown ResultState {other}"
            ))),
        }
    }
}

closed_enum!(BatchState {
    Open,
    Active,
    Suspended,
    Completed,
    Cancelled,
});

impl BatchState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Active => "ACTIVE",
            Self::Suspended => "SUSPENDED",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "OPEN" => Ok(Self::Open),
            "ACTIVE" => Ok(Self::Active),
            "SUSPENDED" => Ok(Self::Suspended),
            "COMPLETED" => Ok(Self::Completed),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(Error::InvariantViolation(format!(
                "unknown BatchState {other}"
            ))),
        }
    }
}

closed_enum!(LogicalAgentState {
    Initializing,
    Ready,
    Assigned,
    Reviving,
    Draining,
    Suspended,
    Retired,
});

impl LogicalAgentState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Initializing => "INITIALIZING",
            Self::Ready => "READY",
            Self::Assigned => "ASSIGNED",
            Self::Reviving => "REVIVING",
            Self::Draining => "DRAINING",
            Self::Suspended => "SUSPENDED",
            Self::Retired => "RETIRED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "INITIALIZING" => Ok(Self::Initializing),
            "READY" => Ok(Self::Ready),
            "ASSIGNED" => Ok(Self::Assigned),
            "REVIVING" => Ok(Self::Reviving),
            "DRAINING" => Ok(Self::Draining),
            "SUSPENDED" => Ok(Self::Suspended),
            "RETIRED" => Ok(Self::Retired),
            other => Err(Error::InvariantViolation(format!(
                "unknown LogicalAgentState {other}"
            ))),
        }
    }

    pub fn is_live_member(self) -> bool {
        matches!(
            self,
            Self::Initializing | Self::Ready | Self::Assigned | Self::Draining | Self::Reviving
        )
    }
}

closed_enum!(IncarnationState {
    Starting,
    Warm,
    Cold,
    Lost,
    Terminated,
});

impl IncarnationState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Warm => "WARM",
            Self::Cold => "COLD",
            Self::Lost => "LOST",
            Self::Terminated => "TERMINATED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "STARTING" => Ok(Self::Starting),
            "WARM" => Ok(Self::Warm),
            "COLD" => Ok(Self::Cold),
            "LOST" => Ok(Self::Lost),
            "TERMINATED" => Ok(Self::Terminated),
            other => Err(Error::InvariantViolation(format!(
                "unknown IncarnationState {other}"
            ))),
        }
    }

    pub fn is_live_presence(self) -> bool {
        matches!(self, Self::Starting | Self::Warm | Self::Cold)
    }
}

closed_enum!(EscalationState {
    Open,
    Resolved,
    Cancelled,
});

impl EscalationState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Resolved => "RESOLVED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "OPEN" => Ok(Self::Open),
            "RESOLVED" => Ok(Self::Resolved),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(Error::InvariantViolation(format!(
                "unknown EscalationState {other}"
            ))),
        }
    }
}

closed_enum!(OutboxState {
    Pending,
    Delivered,
    Acked,
});

impl OutboxState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Delivered => "DELIVERED",
            Self::Acked => "ACKED",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "PENDING" => Ok(Self::Pending),
            "DELIVERED" => Ok(Self::Delivered),
            "ACKED" => Ok(Self::Acked),
            other => Err(Error::InvariantViolation(format!(
                "unknown OutboxState {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureClass {
    TransientExternal,
    Timeout,
    ExecutionLost,
    StartFailure,
    ResourceUnavailable,
    PermissionFailure,
    InvalidResult,
    AdapterProtocolFailure,
    Unknown,
    WriterQuiescenceUnknown,
}

impl FailureClass {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::TransientExternal => "TRANSIENT_EXTERNAL",
            Self::Timeout => "TIMEOUT",
            Self::ExecutionLost => "EXECUTION_LOST",
            Self::StartFailure => "START_FAILURE",
            Self::ResourceUnavailable => "RESOURCE_UNAVAILABLE",
            Self::PermissionFailure => "PERMISSION_FAILURE",
            Self::InvalidResult => "INVALID_RESULT",
            Self::AdapterProtocolFailure => "ADAPTER_PROTOCOL_FAILURE",
            Self::Unknown => "UNKNOWN",
            Self::WriterQuiescenceUnknown => "WRITER_QUIESCENCE_UNKNOWN",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "TRANSIENT_EXTERNAL" => Ok(Self::TransientExternal),
            "TIMEOUT" => Ok(Self::Timeout),
            "EXECUTION_LOST" => Ok(Self::ExecutionLost),
            "START_FAILURE" => Ok(Self::StartFailure),
            "RESOURCE_UNAVAILABLE" => Ok(Self::ResourceUnavailable),
            "PERMISSION_FAILURE" => Ok(Self::PermissionFailure),
            "INVALID_RESULT" => Ok(Self::InvalidResult),
            "ADAPTER_PROTOCOL_FAILURE" => Ok(Self::AdapterProtocolFailure),
            "UNKNOWN" => Ok(Self::Unknown),
            "WRITER_QUIESCENCE_UNKNOWN" => Ok(Self::WriterQuiescenceUnknown),
            other => Err(Error::InvariantViolation(format!(
                "unknown FailureClass {other}"
            ))),
        }
    }

    pub fn is_mechanical(self) -> bool {
        !matches!(self, Self::WriterQuiescenceUnknown)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceMode {
    ReadOnly,
    Write,
}

impl WorkspaceMode {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Write => "write",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "read_only" => Ok(Self::ReadOnly),
            "write" => Ok(Self::Write),
            other => Err(Error::InvariantViolation(format!(
                "unknown WorkspaceMode {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Retention {
    Resident,
    Ephemeral,
}

impl Retention {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Ephemeral => "ephemeral",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "resident" => Ok(Self::Resident),
            "ephemeral" => Ok(Self::Ephemeral),
            other => Err(Error::InvariantViolation(format!(
                "unknown Retention {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContinuityPreference {
    Required,
    Preferred,
    None,
}

impl ContinuityPreference {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Preferred => "preferred",
            Self::None => "none",
        }
    }

    pub fn parse_sql(s: &str) -> Result<Self, Error> {
        match s {
            "required" => Ok(Self::Required),
            "preferred" => Ok(Self::Preferred),
            "none" => Ok(Self::None),
            other => Err(Error::InvariantViolation(format!(
                "unknown ContinuityPreference {other}"
            ))),
        }
    }
}
