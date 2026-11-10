//! B5.1 sandbox policy document and its normalized value form.
//!
//! The policy grammar is deliberately bounded (ADR-0013 §7): exact identifiers,
//! finite sets, anchored path-component trees, bounded numeric ceilings and
//! finite network rules. There is no regex, shell condition or script
//! evaluation, and no "needed to run" exemption.
//!
//! Every dimension states its absence of restriction explicitly as [`Any`]
//! (there is no `Option` field that different code paths could read
//! differently), and every finite allowed set is canonical: sorted, deduped and
//! prefix/subsumption-minimized. An **empty allowed set is legal** and means
//! deny-all in that dimension; it is never a conflict (ADR-0013 §6).
//!
//! The one dimension where absence is not `Top` is the credential dimension: an
//! omitted grant authorizes no credential use, so the credential map starts
//! empty rather than `Any` (ADR-0013 §6, ADR-0015).
//!
//! The type in this module is the normalized, canonical form. Construction
//! rejects malformed values and normalizes set-valued fields; `normalize` is
//! therefore idempotent by construction.
//!
//! [`Any`]: NameSet::Any

use crate::capability::NetworkEndpoint;
use crate::error::ContractError;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Canonical document schema for an authored sandbox policy.
///
/// A change that alters the canonical bytes of a logically equivalent policy
/// MUST bump this version so old rows are never reinterpreted.
pub const SANDBOX_POLICY_SCHEMA_VERSION: u32 = 1;

/// The root of an anchored logical path selector.
///
/// B5.1 intentionally knows only the committed logical workspace. Physical
/// paths, bind mounts and reparse points belong to enforcement, not to this
/// contract kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathRoot {
    Workspace,
}

impl PathRoot {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "WORKSPACE",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "WORKSPACE" => Some(Self::Workspace),
            _ => None,
        }
    }
}

/// An anchored path prefix: the selector covers the path itself and every
/// descendant. Components are exact names; wildcards are not part of v1.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathSelector {
    root: PathRoot,
    components: Vec<String>,
}

impl PathSelector {
    /// A selector under the committed logical workspace. An empty component
    /// list denotes the workspace root itself.
    pub fn workspace<I, S>(components: I) -> Result<Self, ContractError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let components = components
            .into_iter()
            .map(|component| {
                let component = component.into();
                validate_path_component(&component)?;
                Ok(component)
            })
            .collect::<Result<Vec<_>, ContractError>>()?;
        Ok(Self {
            root: PathRoot::Workspace,
            components,
        })
    }

    pub fn root(&self) -> PathRoot {
        self.root
    }

    pub fn components(&self) -> &[String] {
        &self.components
    }

    /// Whether this selector covers `other` (a prefix relation).
    pub fn covers(&self, other: &Self) -> bool {
        self.root == other.root
            && self.components.len() <= other.components.len()
            && self.components == other.components[..self.components.len()]
    }
}

fn validate_path_component(component: &str) -> Result<(), ContractError> {
    if component.is_empty() || component.trim() != component {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "path components must be non-empty and trimmed".into(),
        });
    }
    if component.contains('\0') || component.contains('/') || component.contains('\\') {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!("path component {component:?} must be a single exact name"),
        });
    }
    if component == "." || component == ".." {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!("path component {component:?} is not allowed"),
        });
    }
    if component.contains('*') || component.contains('?') {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "wildcards are not part of the v1 policy grammar".into(),
        });
    }
    Ok(())
}

/// An allowed path set. `Any` means "this layer adds no path restriction"; it
/// is not a grant of filesystem access beyond the other layers and the frozen
/// coarse contract. `Allowed(empty)` is an explicit deny-all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathSet {
    Any,
    Allowed(BTreeSet<PathSelector>),
}

impl PathSet {
    pub fn any() -> Self {
        Self::Any
    }

    /// A deny-all set. Legal, and distinct from [`PathSet::Any`].
    pub fn deny_all() -> Self {
        Self::Allowed(BTreeSet::new())
    }

    /// A finite allowed set, normalized to a minimal antichain of prefixes.
    pub fn allowed<I>(selectors: I) -> Self
    where
        I: IntoIterator<Item = PathSelector>,
    {
        let selectors: BTreeSet<PathSelector> = selectors.into_iter().collect();
        let all: Vec<PathSelector> = selectors.into_iter().collect();
        let kept = all
            .iter()
            .enumerate()
            .filter(|(index, selector)| {
                !all.iter()
                    .enumerate()
                    .any(|(other_index, other)| other_index != *index && other.covers(selector))
            })
            .map(|(_, selector)| selector.clone())
            .collect();
        Self::Allowed(kept)
    }

    pub fn contains(&self, selector: &PathSelector) -> bool {
        match self {
            Self::Any => true,
            Self::Allowed(selectors) => selectors.iter().any(|allowed| allowed.covers(selector)),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    /// Whether this set is an explicit empty allowlist (deny-all).
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Allowed(selectors) if selectors.is_empty())
    }

    pub fn selectors(&self) -> Option<&BTreeSet<PathSelector>> {
        match self {
            Self::Any => None,
            Self::Allowed(selectors) => Some(selectors),
        }
    }
}

/// A finite allowed-name set (tools, commands, environment names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameSet {
    Any,
    Allowed(BTreeSet<String>),
}

impl NameSet {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn deny_all() -> Self {
        Self::Allowed(BTreeSet::new())
    }

    pub fn allowed<I, S>(names: I) -> Result<Self, ContractError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut allowed = BTreeSet::new();
        for name in names {
            let name = name.into();
            validate_name_token(&name, "allowed name")?;
            allowed.insert(name);
        }
        Ok(Self::Allowed(allowed))
    }

    pub fn contains(&self, name: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Allowed(names) => names.contains(name),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Allowed(names) if names.is_empty())
    }

    pub fn names(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::Any => None,
            Self::Allowed(names) => Some(names),
        }
    }
}

/// An allowed-endpoint set. `Allowed(empty)` denies all network egress while
/// remaining a valid policy, distinct from a structural conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkEndpointSet {
    Any,
    Allowed(BTreeSet<NetworkEndpoint>),
}

impl NetworkEndpointSet {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn deny_all() -> Self {
        Self::Allowed(BTreeSet::new())
    }

    /// A finite allowed set, normalized by removing endpoints covered by a
    /// broader rule.
    pub fn allowed<I>(endpoints: I) -> Self
    where
        I: IntoIterator<Item = NetworkEndpoint>,
    {
        let endpoints: BTreeSet<NetworkEndpoint> = endpoints.into_iter().collect();
        let all: Vec<NetworkEndpoint> = endpoints.into_iter().collect();
        let kept = all
            .iter()
            .enumerate()
            .filter(|(index, endpoint)| {
                !all.iter()
                    .enumerate()
                    .any(|(other_index, other)| other_index != *index && other.covers(endpoint))
            })
            .map(|(_, endpoint)| endpoint.clone())
            .collect();
        Self::Allowed(kept)
    }

    /// Whether some allowed rule covers `endpoint`.
    pub fn contains(&self, endpoint: &NetworkEndpoint) -> bool {
        match self {
            Self::Any => true,
            Self::Allowed(endpoints) => endpoints.iter().any(|rule| rule.covers(endpoint)),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Allowed(endpoints) if endpoints.is_empty())
    }

    pub fn endpoints(&self) -> Option<&BTreeSet<NetworkEndpoint>> {
        match self {
            Self::Any => None,
            Self::Allowed(endpoints) => Some(endpoints),
        }
    }
}

/// File access per operation class plus directory visibility.
///
/// Restrictions are expressed only as allowed regions; a hidden path is one
/// that no allowed selector covers. There is no separate negation that a later
/// meet could treat differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePolicy {
    read: PathSet,
    write: PathSet,
    execute: PathSet,
    delete: PathSet,
    visible: PathSet,
}

impl FilePolicy {
    pub fn new(
        read: PathSet,
        write: PathSet,
        execute: PathSet,
        delete: PathSet,
        visible: PathSet,
    ) -> Self {
        Self {
            read,
            write,
            execute,
            delete,
            visible,
        }
    }

    pub fn top() -> Self {
        Self::new(
            PathSet::any(),
            PathSet::any(),
            PathSet::any(),
            PathSet::any(),
            PathSet::any(),
        )
    }

    pub fn deny_all() -> Self {
        Self::new(
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
            PathSet::deny_all(),
        )
    }

    pub fn read(&self) -> &PathSet {
        &self.read
    }

    pub fn write(&self) -> &PathSet {
        &self.write
    }

    pub fn execute(&self) -> &PathSet {
        &self.execute
    }

    pub fn delete(&self) -> &PathSet {
        &self.delete
    }

    pub fn visible(&self) -> &PathSet {
        &self.visible
    }
}

/// Tool and command allowlists. Both are finite name sets; `Any` means the
/// layer adds no restriction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPolicy {
    tools: NameSet,
    commands: NameSet,
}

impl ToolPolicy {
    pub fn new(tools: NameSet, commands: NameSet) -> Self {
        Self { tools, commands }
    }

    pub fn top() -> Self {
        Self::new(NameSet::any(), NameSet::any())
    }

    pub fn deny_all() -> Self {
        Self::new(NameSet::deny_all(), NameSet::deny_all())
    }

    pub fn tools(&self) -> &NameSet {
        &self.tools
    }

    pub fn commands(&self) -> &NameSet {
        &self.commands
    }

    /// A projected `ToolCommandSet` operation names either a tool or a command.
    pub fn allows(&self, name: &str) -> bool {
        self.tools.contains(name) || self.commands.contains(name)
    }
}

/// Environment allowlist plus required values.
///
/// Conflicting required values for the same name are a structural conflict in
/// `meet`; a required name outside the allowed set is rejected here so a policy
/// can never require a variable it simultaneously forbids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentPolicy {
    allowed_names: NameSet,
    required_values: BTreeMap<String, String>,
}

impl EnvironmentPolicy {
    pub fn new(
        allowed_names: NameSet,
        required_values: BTreeMap<String, String>,
    ) -> Result<Self, ContractError> {
        for (name, value) in &required_values {
            validate_name_token(name, "required environment name")?;
            validate_environment_value(value)?;
        }
        if let NameSet::Allowed(allowed) = &allowed_names {
            for name in required_values.keys() {
                if !allowed.contains(name) {
                    return Err(ContractError::InvalidSandboxPolicy {
                        reason: format!(
                            "required environment name {name:?} is not in the allowed name set"
                        ),
                    });
                }
            }
        }
        Ok(Self {
            allowed_names,
            required_values,
        })
    }

    pub fn top() -> Self {
        Self {
            allowed_names: NameSet::any(),
            required_values: BTreeMap::new(),
        }
    }

    pub fn deny_all() -> Self {
        Self {
            allowed_names: NameSet::deny_all(),
            required_values: BTreeMap::new(),
        }
    }

    pub fn allowed_names(&self) -> &NameSet {
        &self.allowed_names
    }

    pub fn required_values(&self) -> &BTreeMap<String, String> {
        &self.required_values
    }
}

fn validate_environment_value(value: &str) -> Result<(), ContractError> {
    if value.contains('\0') {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "environment values must not contain NUL".into(),
        });
    }
    if value.len() > 4096 {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "environment values are limited to 4096 bytes".into(),
        });
    }
    Ok(())
}

fn validate_name_token(value: &str, what: &str) -> Result<(), ContractError> {
    if value.is_empty() || value.trim() != value {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!("{what} must be non-empty and trimmed"),
        });
    }
    if value.len() > 256 || value.contains('\0') || value.contains('=') {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!("{what} {value:?} is not a bounded identifier"),
        });
    }
    Ok(())
}

/// A namespaced credential authorization identity.
///
/// A source-local `CredentialRef` string is not a global credential identity;
/// authorization is scoped by an explicit authority namespace plus the logical
/// reference (ADR-0013 §2). Neither part is derived from an adapter kind or
/// mutable provisioning identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialAuthorizationId {
    namespace: String,
    reference: String,
}

impl CredentialAuthorizationId {
    pub fn new(
        namespace: impl Into<String>,
        reference: impl Into<String>,
    ) -> Result<Self, ContractError> {
        let namespace = namespace.into();
        let reference = reference.into();
        validate_name_token(&namespace, "credential authority namespace")?;
        validate_name_token(&reference, "credential reference")?;
        Ok(Self {
            namespace,
            reference,
        })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }
}

/// The delivery mechanisms a v1 credential grant may use. v1 supports only a
/// trusted broker; transient raw delivery is an explicitly rejected fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialDeliveryMode {
    BrokeredAccess,
}

impl CredentialDeliveryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BrokeredAccess => "BROKERED_ACCESS",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "BROKERED_ACCESS" => Some(Self::BrokeredAccess),
            _ => None,
        }
    }
}

/// An explicit positive credential grant. Every facet narrows; an empty facet
/// authorizes nothing rather than everything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialGrant {
    id: CredentialAuthorizationId,
    uses: BTreeSet<String>,
    audience: BTreeSet<String>,
    delivery_modes: BTreeSet<CredentialDeliveryMode>,
}

impl CredentialGrant {
    pub fn new(
        id: CredentialAuthorizationId,
        uses: impl IntoIterator<Item = String>,
        audience: impl IntoIterator<Item = String>,
        delivery_modes: impl IntoIterator<Item = CredentialDeliveryMode>,
    ) -> Result<Self, ContractError> {
        let mut validated_uses = BTreeSet::new();
        for value in uses {
            validate_name_token(&value, "credential use")?;
            validated_uses.insert(value);
        }
        let mut validated_audience = BTreeSet::new();
        for value in audience {
            validate_name_token(&value, "credential audience")?;
            validated_audience.insert(value);
        }
        Ok(Self {
            id,
            uses: validated_uses,
            audience: validated_audience,
            delivery_modes: delivery_modes.into_iter().collect(),
        })
    }

    pub fn id(&self) -> &CredentialAuthorizationId {
        &self.id
    }

    pub fn uses(&self) -> &BTreeSet<String> {
        &self.uses
    }

    pub fn audience(&self) -> &BTreeSet<String> {
        &self.audience
    }

    pub fn delivery_modes(&self) -> &BTreeSet<CredentialDeliveryMode> {
        &self.delivery_modes
    }
}

/// The scope a resource ceiling applies to. Scopes are not interchangeable: a
/// per-process ceiling and an execution-domain ceiling are different
/// guarantees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceScope {
    Process,
    ExecutionDomain,
}

impl ResourceScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Process => "PROCESS",
            Self::ExecutionDomain => "EXECUTION_DOMAIN",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "PROCESS" => Some(Self::Process),
            "EXECUTION_DOMAIN" => Some(Self::ExecutionDomain),
            _ => None,
        }
    }
}

/// The unit of a bounded numeric ceiling. A semantic token budget is not an OS
/// resource limit and is not represented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceUnit {
    Bytes,
    Count,
}

impl ResourceUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bytes => "BYTES",
            Self::Count => "COUNT",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "BYTES" => Some(Self::Bytes),
            "COUNT" => Some(Self::Count),
            _ => None,
        }
    }
}

/// A bounded resource ceiling. The unit and scope are part of the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceCeiling {
    scope: ResourceScope,
    unit: ResourceUnit,
    limit: u64,
}

impl ResourceCeiling {
    pub fn new(scope: ResourceScope, unit: ResourceUnit, limit: u64) -> Self {
        Self { scope, unit, limit }
    }

    pub fn scope(&self) -> ResourceScope {
        self.scope
    }

    pub fn unit(&self) -> ResourceUnit {
        self.unit
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// A validated resource-limit name (the ceiling's key).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceLimitName(String);

impl ResourceLimitName {
    pub fn new(name: impl Into<String>) -> Result<Self, ContractError> {
        let name = name.into();
        validate_name_token(&name, "resource limit name")?;
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The working-directory launch constraint. Two different required directories
/// cannot both hold; `meet` rejects instead of choosing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkingDirectory {
    Any,
    Required(PathSelector),
}

/// The raw, validated parts of a normalized policy. Use
/// [`NormalizedSandboxPolicy::from_parts`] to obtain the invariants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicyParts {
    pub files: FilePolicy,
    pub tools: ToolPolicy,
    pub network: NetworkEndpointSet,
    pub environment: EnvironmentPolicy,
    pub credentials: BTreeMap<CredentialAuthorizationId, CredentialGrant>,
    pub resources: BTreeMap<ResourceLimitName, ResourceCeiling>,
    pub working_directory: WorkingDirectory,
    pub attempt_isolation: bool,
}

impl Default for SandboxPolicyParts {
    fn default() -> Self {
        Self {
            files: FilePolicy::top(),
            tools: ToolPolicy::top(),
            network: NetworkEndpointSet::Any,
            environment: EnvironmentPolicy::top(),
            credentials: BTreeMap::new(),
            resources: BTreeMap::new(),
            working_directory: WorkingDirectory::Any,
            attempt_isolation: false,
        }
    }
}

/// A normalized, canonical sandbox policy.
///
/// All set-valued fields are canonical and every dimension states `Any`
/// explicitly. `meet`/ordering live in the policy algebra; this type owns the
/// document identity and its validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedSandboxPolicy {
    parts: SandboxPolicyParts,
}

impl NormalizedSandboxPolicy {
    /// The explicit top: no restriction at this layer in any dimension, no
    /// credential authorization, no resource ceiling and no isolation
    /// requirement.
    pub fn top() -> Self {
        Self {
            parts: SandboxPolicyParts::default(),
        }
    }

    /// Deny-all in every authorization dimension. Still a valid policy: an
    /// empty allowed set is a deny-all sub-policy, not a conflict.
    pub fn deny_all() -> Self {
        Self {
            parts: SandboxPolicyParts {
                files: FilePolicy::deny_all(),
                tools: ToolPolicy::deny_all(),
                network: NetworkEndpointSet::deny_all(),
                environment: EnvironmentPolicy::deny_all(),
                credentials: BTreeMap::new(),
                resources: BTreeMap::new(),
                working_directory: WorkingDirectory::Any,
                attempt_isolation: false,
            },
        }
    }

    pub fn from_parts(parts: SandboxPolicyParts) -> Result<Self, ContractError> {
        let parts = normalize_parts(parts)?;
        validate_parts(&parts)?;
        Ok(Self { parts })
    }

    pub fn parts(&self) -> &SandboxPolicyParts {
        &self.parts
    }

    pub fn into_parts(self) -> SandboxPolicyParts {
        self.parts
    }

    /// Rebuild the canonical form. Construction already normalizes every set,
    /// so this is idempotent; it re-runs validation to keep the invariant at
    /// any boundary that constructs from raw parts.
    pub fn normalize(&self) -> Result<Self, ContractError> {
        Self::from_parts(self.parts.clone())
    }

    pub fn files(&self) -> &FilePolicy {
        &self.parts.files
    }

    pub fn tools(&self) -> &ToolPolicy {
        &self.parts.tools
    }

    pub fn network(&self) -> &NetworkEndpointSet {
        &self.parts.network
    }

    pub fn environment(&self) -> &EnvironmentPolicy {
        &self.parts.environment
    }

    pub fn credentials(&self) -> &BTreeMap<CredentialAuthorizationId, CredentialGrant> {
        &self.parts.credentials
    }

    pub fn resources(&self) -> &BTreeMap<ResourceLimitName, ResourceCeiling> {
        &self.parts.resources
    }

    pub fn working_directory(&self) -> &WorkingDirectory {
        &self.parts.working_directory
    }

    pub fn attempt_isolation(&self) -> bool {
        self.parts.attempt_isolation
    }

    /// Canonical JSON value of the policy body (no envelope). The envelope is
    /// owned by [`crate::canonical`].
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "attempt_isolation": self.parts.attempt_isolation,
            "credentials": self
                .parts
                .credentials
                .iter()
                .map(|(id, grant)| credential_grant_value(id, grant))
                .collect::<Vec<_>>(),
            "environment": {
                "allowed_names": name_set_value(&self.parts.environment.allowed_names),
                "required_values": self.parts.environment.required_values,
            },
            "files": {
                "delete": path_set_value(&self.parts.files.delete),
                "execute": path_set_value(&self.parts.files.execute),
                "read": path_set_value(&self.parts.files.read),
                "visible": path_set_value(&self.parts.files.visible),
                "write": path_set_value(&self.parts.files.write),
            },
            "network": network_set_value(&self.parts.network),
            "resources": self
                .parts
                .resources
                .iter()
                .map(|(name, ceiling)| json!({
                    "limit": ceiling.limit,
                    "name": name.as_str(),
                    "scope": ceiling.scope.as_str(),
                    "unit": ceiling.unit.as_str(),
                }))
                .collect::<Vec<_>>(),
            "tools": {
                "commands": name_set_value(&self.parts.tools.commands),
                "tools": name_set_value(&self.parts.tools.tools),
            },
            "working_directory": match &self.parts.working_directory {
                WorkingDirectory::Any => json!({"kind": "ANY"}),
                WorkingDirectory::Required(path) => json!({
                    "kind": "REQUIRED",
                    "path": path_selector_value(path),
                }),
            },
        })
    }
}

fn normalize_parts(parts: SandboxPolicyParts) -> Result<SandboxPolicyParts, ContractError> {
    let environment = EnvironmentPolicy::new(
        normalize_name_set(&parts.environment.allowed_names)?,
        parts.environment.required_values.clone(),
    )?;
    let mut credentials = BTreeMap::new();
    for (id, grant) in parts.credentials {
        if grant.id != id {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "credential grant identity does not match its map key".into(),
            });
        }
        let grant = CredentialGrant::new(
            id.clone(),
            grant.uses,
            grant.audience,
            grant.delivery_modes,
        )?;
        credentials.insert(id, grant);
    }
    Ok(SandboxPolicyParts {
        files: FilePolicy::new(
            normalize_path_set(&parts.files.read),
            normalize_path_set(&parts.files.write),
            normalize_path_set(&parts.files.execute),
            normalize_path_set(&parts.files.delete),
            normalize_path_set(&parts.files.visible),
        ),
        tools: ToolPolicy::new(
            normalize_name_set(&parts.tools.tools)?,
            normalize_name_set(&parts.tools.commands)?,
        ),
        network: match &parts.network {
            NetworkEndpointSet::Any => NetworkEndpointSet::Any,
            NetworkEndpointSet::Allowed(endpoints) => {
                NetworkEndpointSet::allowed(endpoints.iter().cloned())
            }
        },
        environment,
        credentials,
        resources: parts.resources,
        working_directory: parts.working_directory,
        attempt_isolation: parts.attempt_isolation,
    })
}

fn normalize_path_set(set: &PathSet) -> PathSet {
    match set {
        PathSet::Any => PathSet::Any,
        PathSet::Allowed(selectors) => PathSet::allowed(selectors.iter().cloned()),
    }
}

fn normalize_name_set(set: &NameSet) -> Result<NameSet, ContractError> {
    match set {
        NameSet::Any => Ok(NameSet::Any),
        NameSet::Allowed(names) => NameSet::allowed(names.iter().cloned()),
    }
}

fn validate_parts(parts: &SandboxPolicyParts) -> Result<(), ContractError> {
    if let NameSet::Allowed(allowed) = &parts.environment.allowed_names {
        for name in parts.environment.required_values.keys() {
            if !allowed.contains(name) {
                return Err(ContractError::InvalidSandboxPolicy {
                    reason: format!(
                        "required environment name {name:?} is not in the allowed name set"
                    ),
                });
            }
        }
    }
    for value in parts.environment.required_values.values() {
        validate_environment_value(value)?;
    }
    for (id, grant) in &parts.credentials {
        if grant.id != *id {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "credential grant identity does not match its map key".into(),
            });
        }
    }
    Ok(())
}

fn path_set_value(set: &PathSet) -> Value {
    match set {
        PathSet::Any => json!({"kind": "ANY"}),
        PathSet::Allowed(selectors) => json!({
            "kind": "ALLOWED",
            "selectors": selectors
                .iter()
                .map(path_selector_value)
                .collect::<Vec<_>>(),
        }),
    }
}

fn path_selector_value(selector: &PathSelector) -> Value {
    json!({
        "components": selector.components,
        "root": selector.root.as_str(),
    })
}

fn name_set_value(set: &NameSet) -> Value {
    match set {
        NameSet::Any => json!({"kind": "ANY"}),
        NameSet::Allowed(names) => json!({
            "kind": "ALLOWED",
            "names": names.iter().collect::<Vec<_>>(),
        }),
    }
}

fn network_set_value(set: &NetworkEndpointSet) -> Value {
    match set {
        NetworkEndpointSet::Any => json!({"kind": "ANY"}),
        NetworkEndpointSet::Allowed(endpoints) => json!({
            "kind": "ALLOWED",
            "endpoints": endpoints
                .iter()
                .map(endpoint_value)
                .collect::<Vec<_>>(),
        }),
    }
}

pub(crate) fn endpoint_value(endpoint: &NetworkEndpoint) -> Value {
    json!({
        "host": endpoint.host(),
        "port": endpoint.port(),
        "transport": endpoint.transport().as_str(),
    })
}

fn credential_grant_value(id: &CredentialAuthorizationId, grant: &CredentialGrant) -> Value {
    json!({
        "audience": grant.audience.iter().collect::<Vec<_>>(),
        "delivery_modes": grant
            .delivery_modes
            .iter()
            .map(|mode| mode.as_str())
            .collect::<Vec<_>>(),
        "id": {
            "namespace": id.namespace,
            "reference": id.reference,
        },
        "uses": grant.uses.iter().collect::<Vec<_>>(),
    })
}
