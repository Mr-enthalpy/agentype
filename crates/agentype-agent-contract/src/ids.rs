//! M6-B contract identities. Opaque durable strings, not interchangeable.

use std::fmt;
use uuid::Uuid;

macro_rules! contract_id {
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

contract_id!(AgentTypeId, "agent-type");
contract_id!(SpawnSourceId, "spawn-source");
contract_id!(SourceConfigId, "source-config");
contract_id!(CapabilityId, "capability");
contract_id!(AdapterPolicyId, "adapter-policy");
contract_id!(SandboxPolicyId, "sandbox-policy");
