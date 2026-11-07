//! Real source-config integration for [`JailedProcessAgentAdapter`].
//!
//! Core never interprets a `SourceConfig` body. This crate is the source-local
//! integration that *does*: it understands one strict, secret-free JSON grammar
//! and turns it into the opaque launch descriptor that the exact adapter
//! consumes during `ExecutionAdapter::start_execution`.
//!
//! Both `attest` and `prepare` are pure and read-only. They never create the
//! jail, spawn a process, or touch the filesystem: the only physical creation
//! point is the adapter's `start_execution`, under M5 and after Execution
//! commitment.
//!
//! # Grammar
//!
//! ```json
//! {
//!   "format": "agentype.workspace-jail.source-config/v1",
//!   "command": "C:\\absolute\\agent.exe",
//!   "args": ["--flag"],
//!   "jail_root": "C:\\absolute\\jail-base"
//! }
//! ```
//!
//! Unknown fields are rejected. There are deliberately no environment,
//! credential, secret, workspace-mode, or network fields: the adapter enforces
//! the workspace mode and network policy carried by the M5 start request, and
//! B.5 credentials are out of scope. A producer cannot smuggle an undeclared
//! capability through this grammar.
//!
//! # Digest binding
//!
//! The attested materialization digest covers the canonical descriptor **and**
//! the exact `(adapter_kind, adapter_binding_key)` it was prepared for. A
//! descriptor attested against one physical installation cannot be replayed
//! against another, even if the descriptor bytes are identical.

use agentype_adapter_api::AdapterDeadline;
use agentype_adapter_local_process::{
    JailLaunchDescriptor, JAIL_ADAPTER_KIND, JAIL_PROVISIONING_PROTOCOL,
};
use agentype_agent_contract::{canonical_json_body_digest, MaterializationDigest};
use agentype_runtime::{PreparedSource, ProvisioningResolutionError, SourceConfigIntegration};
use agentype_storage_sqlite::{ConfigMode, SourceConfigBodyView, SourceConfigRevision};
use serde_json::{json, Map, Value};

/// The only source-config `format` value this integration materializes.
pub const WORKSPACE_JAIL_CONFIG_FORMAT: &str = "agentype.workspace-jail.source-config/v1";

const FORMAT_FIELD: &str = "format";
const COMMAND_FIELD: &str = "command";
const ARGS_FIELD: &str = "args";
const ROOT_FIELD: &str = "jail_root";

/// Build the canonical `OpaqueJson` body a producer publishes.
///
/// Publication still rejects an invalid descriptor through
/// [`SourceConfigIntegration::attest`]; this helper only makes the supported
/// grammar explicit to operators and tests.
pub fn workspace_jail_config_body(
    command: impl Into<String>,
    args: Vec<String>,
    jail_root: impl Into<String>,
) -> Value {
    let mut map = Map::new();
    map.insert(
        FORMAT_FIELD.to_string(),
        Value::String(WORKSPACE_JAIL_CONFIG_FORMAT.to_string()),
    );
    map.insert(COMMAND_FIELD.to_string(), Value::String(command.into()));
    map.insert(
        ARGS_FIELD.to_string(),
        Value::Array(args.into_iter().map(Value::String).collect()),
    );
    map.insert(ROOT_FIELD.to_string(), Value::String(jail_root.into()));
    Value::Object(map)
}

/// Source-local integration for the workspace-jail adapter family.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkspaceJailSourceIntegration;

impl WorkspaceJailSourceIntegration {
    pub fn new() -> Self {
        Self
    }

    fn descriptor_for(
        &self,
        revision: &SourceConfigRevision,
        adapter_kind: &str,
    ) -> Result<Option<JailLaunchDescriptor>, ProvisioningResolutionError> {
        if adapter_kind != JAIL_ADAPTER_KIND {
            // This integration only owns the jailed-process grammar. Report a
            // candidate-local kind mismatch; composing it with another adapter
            // must not silently fall through.
            return Err(ProvisioningResolutionError::PolicyKindMismatch {
                policy_kind: adapter_kind.to_string(),
                imported_kind: JAIL_ADAPTER_KIND.to_string(),
            });
        }
        let payload = match revision.mode() {
            ConfigMode::OpaqueJson => match revision.body() {
                SourceConfigBodyView::OpaqueJson(value) => value,
                SourceConfigBodyView::ExternalRef(_) => return Ok(None),
            },
            ConfigMode::ExternalRef => return Ok(None),
        };
        match parse_body(payload) {
            Ok(descriptor) => Ok(Some(descriptor)),
            // A body outside this source's grammar is simply not materialized
            // here; another registered config/source may still be eligible.
            Err(()) => Ok(None),
        }
    }

    fn digest(
        descriptor: &JailLaunchDescriptor,
        adapter_kind: &str,
        adapter_binding_key: &str,
    ) -> Result<MaterializationDigest, ProvisioningResolutionError> {
        let canonical = json!({
            "protocol": JAIL_PROVISIONING_PROTOCOL,
            "adapter_kind": adapter_kind,
            "adapter_binding_key": adapter_binding_key,
            "command": descriptor.command(),
            "args": descriptor.args(),
            "jail_root": descriptor.jail_root().to_string_lossy(),
        });
        MaterializationDigest::new(canonical_json_body_digest(&canonical))
            .map_err(ProvisioningResolutionError::Evidence)
    }
}

impl SourceConfigIntegration for WorkspaceJailSourceIntegration {
    fn protocol(&self) -> &str {
        JAIL_PROVISIONING_PROTOCOL
    }

    fn attest(
        &self,
        revision: &SourceConfigRevision,
        adapter_kind: &str,
        adapter_binding_key: &str,
        deadline: &AdapterDeadline,
    ) -> Result<Option<MaterializationDigest>, ProvisioningResolutionError> {
        if deadline.is_expired() {
            return Err(ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                "deadline expired before workspace-jail attestation".into(),
            ));
        }
        let Some(descriptor) = self.descriptor_for(revision, adapter_kind)? else {
            return Ok(None);
        };
        if adapter_binding_key.trim().is_empty() {
            return Ok(None);
        }
        if deadline.is_expired() {
            return Err(ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                "deadline expired after workspace-jail attestation".into(),
            ));
        }
        Ok(Some(Self::digest(
            &descriptor,
            adapter_kind,
            adapter_binding_key,
        )?))
    }

    fn prepare(
        &self,
        revision: &SourceConfigRevision,
        adapter_kind: &str,
        adapter_binding_key: &str,
        deadline: &AdapterDeadline,
    ) -> Result<PreparedSource, ProvisioningResolutionError> {
        if deadline.is_expired() {
            return Err(ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                "deadline expired before workspace-jail preparation".into(),
            ));
        }
        let descriptor = self
            .descriptor_for(revision, adapter_kind)?
            .ok_or_else(|| {
                ProvisioningResolutionError::ExternalReferenceNotAttested(
                    "workspace-jail config is not materializable by this integration".into(),
                )
            })?;
        if adapter_binding_key.trim().is_empty() {
            return Err(ProvisioningResolutionError::ExactBindingUnresolved(
                "workspace-jail preparation requires an exact binding key".into(),
            ));
        }
        let digest = Self::digest(&descriptor, adapter_kind, adapter_binding_key)?;
        if deadline.is_expired() {
            return Err(ProvisioningResolutionError::ProvisioningDeadlineExceeded(
                "deadline expired after workspace-jail preparation".into(),
            ));
        }
        Ok(PreparedSource {
            digest: digest.as_str().to_string(),
            descriptor: descriptor.encode(),
        })
    }
}

fn parse_body(payload: &Value) -> Result<JailLaunchDescriptor, ()> {
    let object = payload.as_object().ok_or(())?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            FORMAT_FIELD | COMMAND_FIELD | ARGS_FIELD | ROOT_FIELD
        ) {
            return Err(());
        }
    }
    let format = object.get(FORMAT_FIELD).and_then(Value::as_str).ok_or(())?;
    if format != WORKSPACE_JAIL_CONFIG_FORMAT {
        return Err(());
    }
    let command = object
        .get(COMMAND_FIELD)
        .and_then(Value::as_str)
        .ok_or(())?;
    let args = object
        .get(ARGS_FIELD)
        .and_then(Value::as_array)
        .ok_or(())?
        .iter()
        .map(|value| value.as_str().map(str::to_string).ok_or(()))
        .collect::<Result<Vec<_>, ()>>()?;
    let jail_root = object.get(ROOT_FIELD).and_then(Value::as_str).ok_or(())?;
    JailLaunchDescriptor::new(command, args, jail_root).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentype_storage_sqlite::SourceConfigRevision;

    // `SourceConfigRevision` has no public constructor by design. These unit
    // tests cover the pure grammar helper; attest/prepare are exercised through
    // real durable revisions in the crate's integration tests.
    #[test]
    fn config_helper_is_strict_and_round_trips() {
        let body = workspace_jail_config_body(
            if cfg!(windows) {
                "C:\\abs\\agent.exe"
            } else {
                "/abs/agent"
            },
            vec!["--flag".into()],
            if cfg!(windows) {
                "C:\\abs\\jail"
            } else {
                "/abs/jail"
            },
        );
        assert!(parse_body(&body).is_ok());

        let mut unknown = body.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("env".into(), json!({}));
        assert!(parse_body(&unknown).is_err());

        let mut wrong_format = body.clone();
        wrong_format
            .as_object_mut()
            .unwrap()
            .insert("format".into(), json!("agentype.workspace-jail/v1"));
        assert!(parse_body(&wrong_format).is_err());

        let mut relative = body;
        relative
            .as_object_mut()
            .unwrap()
            .insert("command".into(), json!("relative.exe"));
        assert!(parse_body(&relative).is_err());
    }

    #[test]
    fn protocol_matches_the_adapter_grammar() {
        assert_eq!(
            WorkspaceJailSourceIntegration::new().protocol(),
            JAIL_PROVISIONING_PROTOCOL
        );
    }

    #[allow(dead_code)]
    fn _revision_is_unforgeable(_: &SourceConfigRevision) {}
}
