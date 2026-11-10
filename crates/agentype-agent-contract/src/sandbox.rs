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

use crate::capability::{
    CapabilityDefinition, CapabilityValue, FileOperationClass, NetworkEndpoint, NetworkTransport,
    PolicyOperation,
};
use crate::error::ContractError;
use crate::records::{NetworkPolicy, SandboxPolicyRef};
use agentype_core::WorkspaceMode;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Canonical document schema for an authored sandbox policy.
///
/// A change that alters the canonical bytes of a logically equivalent policy
/// MUST bump this version so old rows are never reinterpreted.
pub const SANDBOX_POLICY_SCHEMA_VERSION: u32 = 1;

/// Canonical document schema for a `RequiredAccess` floor.
///
/// Required access is content-addressed separately so the Task binding can
/// record exactly which positive obligations admission checked.
pub const REQUIRED_ACCESS_SCHEMA_VERSION: u32 = 1;

/// The exact versions a durable policy row was written under.
///
/// A row's `(canonical_version, algebra_version)` pair is part of its identity.
/// Rows are decoded and re-derived only under the versions recorded with them,
/// and a pair this build does not implement fails closed instead of being
/// reinterpreted by the current algorithms (ADR-0013 §11). Golden vectors are
/// keyed by this pair for the same reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SandboxPolicyVersion {
    canonical_version: u32,
    algebra_version: u32,
}

impl SandboxPolicyVersion {
    /// The versions this build reads and writes.
    pub const CURRENT: Self = Self::new(
        SANDBOX_POLICY_SCHEMA_VERSION,
        SANDBOX_POLICY_MEET_ALGORITHM_VERSION,
    );

    pub const fn new(canonical_version: u32, algebra_version: u32) -> Self {
        Self {
            canonical_version,
            algebra_version,
        }
    }

    pub const fn canonical_version(self) -> u32 {
        self.canonical_version
    }

    pub const fn algebra_version(self) -> u32 {
        self.algebra_version
    }

    pub fn is_current(self) -> bool {
        self == Self::CURRENT
    }

    /// Reject a row recorded under a version this build does not implement.
    ///
    /// There is deliberately no fallback: evaluating an old row with a newer
    /// meet would be an implicit reinterpretation of committed authority.
    pub fn ensure_supported(self) -> Result<Self, ContractError> {
        if self.is_current() {
            Ok(self)
        } else {
            Err(ContractError::UnsupportedPolicyVersion {
                canonical_version: self.canonical_version,
                algebra_version: self.algebra_version,
            })
        }
    }
}

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

    /// A finite allowed set, normalized by merging an explicit `{TCP, UDP}` pair
    /// at the same host/port into the equivalent `ANY` selector and then removing
    /// endpoints covered by a broader rule.
    pub fn allowed<I>(endpoints: I) -> Self
    where
        I: IntoIterator<Item = NetworkEndpoint>,
    {
        let mut all: BTreeSet<NetworkEndpoint> = endpoints.into_iter().collect();
        // `ANY` is exactly the two-element transport domain in v1, so an
        // explicit TCP+UDP pair is the same rule and MUST NOT produce a second
        // canonical spelling.
        let keys: BTreeSet<(String, Option<u16>)> = all
            .iter()
            .map(|endpoint| (endpoint.host().to_string(), endpoint.port()))
            .collect();
        for (host, port) in keys {
            let tcp = NetworkEndpoint::new(NetworkTransport::Tcp, host.clone(), port);
            let udp = NetworkEndpoint::new(NetworkTransport::Udp, host.clone(), port);
            if let (Ok(tcp), Ok(udp)) = (tcp, udp) {
                if all.contains(&tcp) && all.contains(&udp) {
                    all.remove(&tcp);
                    all.remove(&udp);
                    all.insert(tcp.with_transport(NetworkTransport::Any));
                }
            }
        }
        let all: Vec<NetworkEndpoint> = all.into_iter().collect();
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

    /// Whether this grant can authorize any use at all. Every facet is a
    /// narrowing set, so an empty facet authorizes nothing rather than all.
    pub fn is_usable(&self) -> bool {
        !self.uses.is_empty() && !self.audience.is_empty() && !self.delivery_modes.is_empty()
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
        let grant =
            CredentialGrant::new(id.clone(), grant.uses, grant.audience, grant.delivery_modes)?;
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

// =============================================================================
// Policy algebra
// =============================================================================

/// Version of the normalization rules that produce [`NormalizedSandboxPolicy`].
///
/// Recorded separately from [`SANDBOX_POLICY_MEET_ALGORITHM_VERSION`]: a change
/// to canonicalization and a change to intersection are different events, and
/// old rows must stay readable under the versions they were written with.
pub const SANDBOX_POLICY_NORMALIZATION_VERSION: u32 = 1;

/// Version of the `meet`/ordering rules in this module.
///
/// A change that could produce different output bytes for the same inputs MUST
/// bump this and add golden vectors keyed by `(canonical_version, algebra_version)`.
pub const SANDBOX_POLICY_MEET_ALGORITHM_VERSION: u32 = 1;

/// A deterministic unsatisfiability result of two contracts (ADR-0013 §6).
///
/// This is reserved for required launch/structural constraints that cannot both
/// hold. An empty allowed set in a permission dimension is **not** a conflict:
/// it is a legal deny-all policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyConflict {
    reason: String,
}

impl PolicyConflict {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for PolicyConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sandbox policy conflict: {}", self.reason)
    }
}

impl std::error::Error for PolicyConflict {}

fn conflict(reason: impl Into<String>) -> PolicyConflict {
    PolicyConflict::new(reason)
}

fn intersect_path_sets(left: &PathSet, right: &PathSet) -> PathSet {
    match (left, right) {
        (PathSet::Any, other) | (other, PathSet::Any) => other.clone(),
        (PathSet::Allowed(left), PathSet::Allowed(right)) => PathSet::allowed(
            left.iter()
                .flat_map(|a| right.iter().filter_map(|b| intersect_selectors(a, b)))
                .collect::<Vec<_>>(),
        ),
    }
}

fn intersect_selectors(left: &PathSelector, right: &PathSelector) -> Option<PathSelector> {
    if left.covers(right) {
        Some(right.clone())
    } else if right.covers(left) {
        Some(left.clone())
    } else {
        None
    }
}

fn intersect_name_sets(left: &NameSet, right: &NameSet) -> NameSet {
    match (left, right) {
        (NameSet::Any, other) | (other, NameSet::Any) => other.clone(),
        (NameSet::Allowed(left), NameSet::Allowed(right)) => {
            NameSet::Allowed(left.intersection(right).cloned().collect())
        }
    }
}

/// Intersection of two endpoint boxes. `None` means the boxes are disjoint.
fn intersect_endpoints(left: &NetworkEndpoint, right: &NetworkEndpoint) -> Option<NetworkEndpoint> {
    let transport = match (left.transport(), right.transport()) {
        (NetworkTransport::Any, other) => other,
        (other, NetworkTransport::Any) => other,
        (a, b) if a == b => a,
        _ => return None,
    };
    let host = if left.host() == "*" {
        right.host().to_string()
    } else if right.host() == "*" || left.host() == right.host() {
        left.host().to_string()
    } else {
        return None;
    };
    let port = match (left.port(), right.port()) {
        (None, other) => other,
        (other, None) => other,
        (Some(a), Some(b)) if a == b => Some(a),
        _ => return None,
    };
    NetworkEndpoint::new(transport, host, port).ok()
}

fn intersect_network_sets(
    left: &NetworkEndpointSet,
    right: &NetworkEndpointSet,
) -> NetworkEndpointSet {
    match (left, right) {
        (NetworkEndpointSet::Any, other) | (other, NetworkEndpointSet::Any) => other.clone(),
        (NetworkEndpointSet::Allowed(left), NetworkEndpointSet::Allowed(right)) => {
            NetworkEndpointSet::allowed(
                left.iter()
                    .flat_map(|a| right.iter().filter_map(|b| intersect_endpoints(a, b)))
                    .collect::<Vec<_>>(),
            )
        }
    }
}

fn meet_file_policies(left: &FilePolicy, right: &FilePolicy) -> FilePolicy {
    FilePolicy::new(
        intersect_path_sets(left.read(), right.read()),
        intersect_path_sets(left.write(), right.write()),
        intersect_path_sets(left.execute(), right.execute()),
        intersect_path_sets(left.delete(), right.delete()),
        intersect_path_sets(left.visible(), right.visible()),
    )
}

fn meet_environments(
    left: &EnvironmentPolicy,
    right: &EnvironmentPolicy,
) -> Result<EnvironmentPolicy, PolicyConflict> {
    let allowed = intersect_name_sets(left.allowed_names(), right.allowed_names());
    let mut required = left.required_values().clone();
    for (name, value) in right.required_values() {
        match required.get(name) {
            Some(existing) if existing != value => {
                return Err(conflict(format!(
                    "conflicting required environment values for {name:?}"
                )));
            }
            Some(_) => {}
            None => {
                required.insert(name.clone(), value.clone());
            }
        }
    }
    EnvironmentPolicy::new(allowed, required)
        .map_err(|error| PolicyConflict::new(error.to_string()))
}

fn meet_credentials(
    left: &BTreeMap<CredentialAuthorizationId, CredentialGrant>,
    right: &BTreeMap<CredentialAuthorizationId, CredentialGrant>,
) -> Result<BTreeMap<CredentialAuthorizationId, CredentialGrant>, PolicyConflict> {
    let mut granted = BTreeMap::new();
    // The credential dimension has no `Any`: an omitted grant authorizes
    // nothing, so the meet keeps exactly the ids both ceilings grant.
    for (id, left_grant) in left {
        let Some(right_grant) = right.get(id) else {
            continue;
        };
        let grant = CredentialGrant::new(
            id.clone(),
            left_grant.uses().intersection(right_grant.uses()).cloned(),
            left_grant
                .audience()
                .intersection(right_grant.audience())
                .cloned(),
            left_grant
                .delivery_modes()
                .intersection(right_grant.delivery_modes())
                .copied(),
        )
        .map_err(|error| PolicyConflict::new(error.to_string()))?;
        granted.insert(id.clone(), grant);
    }
    Ok(granted)
}

fn meet_resources(
    left: &BTreeMap<ResourceLimitName, ResourceCeiling>,
    right: &BTreeMap<ResourceLimitName, ResourceCeiling>,
) -> Result<BTreeMap<ResourceLimitName, ResourceCeiling>, PolicyConflict> {
    let mut ceilings = left.clone();
    for (name, right_ceiling) in right {
        match ceilings.get(name) {
            None => {
                ceilings.insert(name.clone(), *right_ceiling);
            }
            Some(left_ceiling) => {
                if left_ceiling.scope() != right_ceiling.scope()
                    || left_ceiling.unit() != right_ceiling.unit()
                {
                    return Err(conflict(format!(
                        "resource ceiling {name:?} is declared with incompatible scope or unit"
                    )));
                }
                ceilings.insert(
                    name.clone(),
                    ResourceCeiling::new(
                        left_ceiling.scope(),
                        left_ceiling.unit(),
                        left_ceiling.limit().min(right_ceiling.limit()),
                    ),
                );
            }
        }
    }
    Ok(ceilings)
}

fn meet_working_directories(
    left: &WorkingDirectory,
    right: &WorkingDirectory,
) -> Result<WorkingDirectory, PolicyConflict> {
    match (left, right) {
        (WorkingDirectory::Any, other) | (other, WorkingDirectory::Any) => Ok(other.clone()),
        (WorkingDirectory::Required(left), WorkingDirectory::Required(right)) if left == right => {
            Ok(WorkingDirectory::Required(left.clone()))
        }
        _ => Err(conflict(
            "two different required working directories cannot both hold",
        )),
    }
}

/// Greatest lower bound of two normalized policies (ADR-0013 §6).
///
/// `Ok` is the greatest representable policy below both inputs; an empty
/// allowed set in a permission dimension is a legal result. `Err` is reserved
/// for structural/launch constraints that cannot both hold.
pub fn meet(
    left: &NormalizedSandboxPolicy,
    right: &NormalizedSandboxPolicy,
) -> Result<NormalizedSandboxPolicy, PolicyConflict> {
    let parts = SandboxPolicyParts {
        files: meet_file_policies(left.files(), right.files()),
        tools: ToolPolicy::new(
            intersect_name_sets(left.tools().tools(), right.tools().tools()),
            intersect_name_sets(left.tools().commands(), right.tools().commands()),
        ),
        network: intersect_network_sets(left.network(), right.network()),
        environment: meet_environments(left.environment(), right.environment())?,
        credentials: meet_credentials(left.credentials(), right.credentials())?,
        resources: meet_resources(left.resources(), right.resources())?,
        working_directory: meet_working_directories(
            left.working_directory(),
            right.working_directory(),
        )?,
        // An execution guarantee is a disjunction and no meet can clear it.
        attempt_isolation: left.attempt_isolation() || right.attempt_isolation(),
    };
    NormalizedSandboxPolicy::from_parts(parts)
        .map_err(|error| PolicyConflict::new(error.to_string()))
}

fn path_set_within(inner: &PathSet, outer: &PathSet) -> bool {
    match inner {
        PathSet::Any => outer.is_any(),
        PathSet::Allowed(inner) => match outer {
            PathSet::Any => true,
            PathSet::Allowed(outer) => inner
                .iter()
                .all(|selector| outer.iter().any(|allowed| allowed.covers(selector))),
        },
    }
}

fn name_set_within(inner: &NameSet, outer: &NameSet) -> bool {
    match inner {
        NameSet::Any => outer.is_any(),
        NameSet::Allowed(inner) => match outer {
            NameSet::Any => true,
            NameSet::Allowed(outer) => inner.is_subset(outer),
        },
    }
}

fn network_set_within(inner: &NetworkEndpointSet, outer: &NetworkEndpointSet) -> bool {
    match inner {
        NetworkEndpointSet::Any => outer.is_any(),
        NetworkEndpointSet::Allowed(inner) => match outer {
            NetworkEndpointSet::Any => true,
            NetworkEndpointSet::Allowed(outer) => inner
                .iter()
                .all(|endpoint| outer.iter().any(|allowed| allowed.covers(endpoint))),
        },
    }
}

fn credentials_within(
    inner: &BTreeMap<CredentialAuthorizationId, CredentialGrant>,
    outer: &BTreeMap<CredentialAuthorizationId, CredentialGrant>,
) -> bool {
    inner.iter().all(|(id, grant)| {
        // An unusable grant authorizes nothing, so it cannot widen authority.
        if !grant.is_usable() {
            return true;
        }
        outer.get(id).is_some_and(|allowed| {
            grant.uses().is_subset(allowed.uses())
                && grant.audience().is_subset(allowed.audience())
                && grant.delivery_modes().is_subset(allowed.delivery_modes())
        })
    })
}

fn resources_within(
    inner: &BTreeMap<ResourceLimitName, ResourceCeiling>,
    outer: &BTreeMap<ResourceLimitName, ResourceCeiling>,
) -> bool {
    // `Any` (an omitted ceiling) adds no restriction, so every ceiling the
    // outer policy declares must be declared at least as strictly by the inner.
    outer.iter().all(|(name, allowed)| {
        inner.get(name).is_some_and(|ceiling| {
            ceiling.scope() == allowed.scope()
                && ceiling.unit() == allowed.unit()
                && ceiling.limit() <= allowed.limit()
        })
    })
}

fn working_directory_within(inner: &WorkingDirectory, outer: &WorkingDirectory) -> bool {
    match (inner, outer) {
        (_, WorkingDirectory::Any) => true,
        (WorkingDirectory::Required(inner), WorkingDirectory::Required(outer)) => inner == outer,
        (WorkingDirectory::Any, WorkingDirectory::Required(_)) => false,
    }
}

/// Whether `inner` is provably no wider than `outer` (`inner ⪯ outer`).
///
/// Attempt isolation is deliberately ignored: it is an execution guarantee, not
/// part of the authorization ordering, and a meet may only add it.
pub fn is_within(inner: &NormalizedSandboxPolicy, outer: &NormalizedSandboxPolicy) -> bool {
    path_set_within(inner.files().read(), outer.files().read())
        && path_set_within(inner.files().write(), outer.files().write())
        && path_set_within(inner.files().execute(), outer.files().execute())
        && path_set_within(inner.files().delete(), outer.files().delete())
        && path_set_within(inner.files().visible(), outer.files().visible())
        && name_set_within(inner.tools().tools(), outer.tools().tools())
        && name_set_within(inner.tools().commands(), outer.tools().commands())
        && network_set_within(inner.network(), outer.network())
        && name_set_within(
            inner.environment().allowed_names(),
            outer.environment().allowed_names(),
        )
        && outer
            .environment()
            .required_values()
            .iter()
            .all(|(name, value)| inner.environment().required_values().get(name) == Some(value))
        && credentials_within(inner.credentials(), outer.credentials())
        && resources_within(inner.resources(), outer.resources())
        && working_directory_within(inner.working_directory(), outer.working_directory())
}

/// Prove that a derived policy stays within a base policy, or fail closed.
pub fn prove_no_wider(
    derived: &NormalizedSandboxPolicy,
    base: &NormalizedSandboxPolicy,
) -> Result<(), PolicyConflict> {
    if is_within(derived, base) {
        Ok(())
    } else {
        Err(conflict(
            "derived policy is not provably within the base policy",
        ))
    }
}

// =============================================================================
// Required access and capability projection
// =============================================================================

/// The positive access floor of a Task in `SECURITY_V1`.
///
/// B5.1 deliberately has no path-level obligation (ADR-0013 §2): file access is
/// an operation-class floor, and path requirements would need a separate ADR.
/// Credential ids are usable authorization identities, never provider secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiredAccess {
    file_operations: BTreeSet<FileOperationClass>,
    tools_commands: BTreeSet<String>,
    network_endpoints: BTreeSet<NetworkEndpoint>,
    environment_names: BTreeSet<String>,
    credentials: BTreeSet<CredentialAuthorizationId>,
}

impl RequiredAccess {
    pub fn empty() -> Self {
        Self {
            file_operations: BTreeSet::new(),
            tools_commands: BTreeSet::new(),
            network_endpoints: BTreeSet::new(),
            environment_names: BTreeSet::new(),
            credentials: BTreeSet::new(),
        }
    }

    pub fn new(
        file_operations: impl IntoIterator<Item = FileOperationClass>,
        tools_commands: impl IntoIterator<Item = String>,
        network_endpoints: impl IntoIterator<Item = NetworkEndpoint>,
        environment_names: impl IntoIterator<Item = String>,
        credentials: impl IntoIterator<Item = CredentialAuthorizationId>,
    ) -> Result<Self, ContractError> {
        let mut tools = BTreeSet::new();
        for name in tools_commands {
            validate_name_token(&name, "required tool or command")?;
            tools.insert(name);
        }
        let mut environment = BTreeSet::new();
        for name in environment_names {
            validate_name_token(&name, "required environment name")?;
            environment.insert(name);
        }
        Ok(Self {
            file_operations: file_operations.into_iter().collect(),
            tools_commands: tools,
            network_endpoints: network_endpoints.into_iter().collect(),
            environment_names: environment,
            credentials: credentials.into_iter().collect(),
        })
    }

    /// Build a floor from capability-projected policy operations.
    pub fn from_operations<I>(operations: I) -> Result<Self, ContractError>
    where
        I: IntoIterator<Item = PolicyOperation>,
    {
        let mut access = Self::empty();
        for operation in operations {
            match operation {
                PolicyOperation::FileOperation(class) => {
                    access.file_operations.insert(class);
                }
                PolicyOperation::ToolCommand(name) => {
                    validate_name_token(&name, "required tool or command")?;
                    access.tools_commands.insert(name);
                }
                PolicyOperation::NetworkEndpoint(endpoint) => {
                    access.network_endpoints.insert(endpoint);
                }
                PolicyOperation::EnvironmentName(name) => {
                    validate_name_token(&name, "required environment name")?;
                    access.environment_names.insert(name);
                }
            }
        }
        Ok(access)
    }

    pub fn with_credentials<I>(mut self, credentials: I) -> Self
    where
        I: IntoIterator<Item = CredentialAuthorizationId>,
    {
        self.credentials.extend(credentials);
        self
    }

    pub fn file_operations(&self) -> &BTreeSet<FileOperationClass> {
        &self.file_operations
    }

    pub fn tools_commands(&self) -> &BTreeSet<String> {
        &self.tools_commands
    }

    pub fn network_endpoints(&self) -> &BTreeSet<NetworkEndpoint> {
        &self.network_endpoints
    }

    pub fn environment_names(&self) -> &BTreeSet<String> {
        &self.environment_names
    }

    pub fn credentials(&self) -> &BTreeSet<CredentialAuthorizationId> {
        &self.credentials
    }

    pub(crate) fn to_value(&self) -> Value {
        json!({
            "credentials": self
                .credentials
                .iter()
                .map(|id| json!({
                    "namespace": id.namespace,
                    "reference": id.reference,
                }))
                .collect::<Vec<_>>(),
            "environment_names": self.environment_names.iter().collect::<Vec<_>>(),
            "file_operations": self
                .file_operations
                .iter()
                .map(|class| class.as_str())
                .collect::<Vec<_>>(),
            "network_endpoints": self
                .network_endpoints
                .iter()
                .map(endpoint_value)
                .collect::<Vec<_>>(),
            "tools_commands": self.tools_commands.iter().collect::<Vec<_>>(),
        })
    }
}

/// The access a normalized policy actually permits.
///
/// It is a projection for obligation checking, not a second policy authority:
/// path regions, credential facets and resource ceilings stay in the policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyAccess {
    file_operations: BTreeSet<FileOperationClass>,
    tools: ToolPolicy,
    network: NetworkEndpointSet,
    environment: NameSet,
    credentials: BTreeSet<CredentialAuthorizationId>,
}

impl PolicyAccess {
    pub fn from_policy(policy: &NormalizedSandboxPolicy) -> Self {
        let files = policy.files();
        let mut file_operations = BTreeSet::new();
        for (class, set) in [
            (FileOperationClass::Read, files.read()),
            (FileOperationClass::Write, files.write()),
            (FileOperationClass::Execute, files.execute()),
            (FileOperationClass::Delete, files.delete()),
        ] {
            if !set.is_empty() {
                file_operations.insert(class);
            }
        }
        Self {
            file_operations,
            tools: policy.tools().clone(),
            network: policy.network().clone(),
            environment: policy.environment().allowed_names().clone(),
            // A grant with an empty facet authorizes nothing and MUST NOT
            // satisfy a credential obligation.
            credentials: policy
                .credentials()
                .iter()
                .filter(|(_, grant)| grant.is_usable())
                .map(|(id, _)| id.clone())
                .collect(),
        }
    }

    pub fn file_operations(&self) -> &BTreeSet<FileOperationClass> {
        &self.file_operations
    }

    pub fn allows_file_operation(&self, class: FileOperationClass) -> bool {
        self.file_operations.contains(&class)
    }

    pub fn allows_tool_or_command(&self, name: &str) -> bool {
        self.tools.allows(name)
    }

    pub fn allows_endpoint(&self, endpoint: &NetworkEndpoint) -> bool {
        self.network.contains(endpoint)
    }

    pub fn allows_environment_name(&self, name: &str) -> bool {
        self.environment.contains(name)
    }

    pub fn allows_credential(&self, id: &CredentialAuthorizationId) -> bool {
        self.credentials.contains(id)
    }
}

/// The parts of a required-access floor that the effective policy does not
/// grant. This is an unsatisfied Task obligation, not a `PolicyConflict`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessDeficit {
    reasons: Vec<String>,
}

impl AccessDeficit {
    pub fn reasons(&self) -> &[String] {
        &self.reasons
    }
}

impl fmt::Display for AccessDeficit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "required access is not satisfied: {}",
            self.reasons.join("; ")
        )
    }
}

impl std::error::Error for AccessDeficit {}

/// Check `RequiredAccess ⊆ ProjectAccess(effective)` (ADR-0013 §5).
pub fn check_required_access(
    required: &RequiredAccess,
    access: &PolicyAccess,
) -> Result<(), AccessDeficit> {
    let mut reasons = Vec::new();
    for class in required.file_operations() {
        if !access.allows_file_operation(*class) {
            reasons.push(format!("file operation {}", class.as_str()));
        }
    }
    for name in required.tools_commands() {
        if !access.allows_tool_or_command(name) {
            reasons.push(format!("tool or command {name:?}"));
        }
    }
    for endpoint in required.network_endpoints() {
        if !access.allows_endpoint(endpoint) {
            reasons.push(format!(
                "network endpoint {}:{} over {}",
                endpoint.host(),
                endpoint
                    .port()
                    .map(|port| port.to_string())
                    .unwrap_or_else(|| "*".into()),
                endpoint.transport().as_str()
            ));
        }
    }
    for name in required.environment_names() {
        if !access.allows_environment_name(name) {
            reasons.push(format!("environment name {name:?}"));
        }
    }
    for id in required.credentials() {
        if !access.allows_credential(id) {
            reasons.push(format!("credential {}/{}", id.namespace(), id.reference()));
        }
    }
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(AccessDeficit { reasons })
    }
}

/// Project a capability value into its catalog-declared policy operations.
///
/// A `Set` capability with no projection contributes nothing. A projecting
/// capability MUST map every member of the value: an unmapped member fails the
/// whole projection closed and is never silently dropped (ADR-0013 §2).
pub fn project_capability_access(
    definition: &CapabilityDefinition,
    value: &CapabilityValue,
) -> Result<BTreeSet<PolicyOperation>, ContractError> {
    definition.validate_projection()?;
    let Some(projection) = &definition.policy_projection else {
        // A capability with no explicit mapping contributes nothing; it is
        // never guessed into a policy dimension.
        return Ok(BTreeSet::new());
    };
    let CapabilityValue::Set(members) = value else {
        return Err(ContractError::InvalidPolicyProjection {
            reason: "capability value shape does not match a Set definition".into(),
        });
    };
    let mut operations = BTreeSet::new();
    for member in members {
        let operation = projection.operation_for(member).ok_or_else(|| {
            ContractError::InvalidPolicyProjection {
                reason: format!("capability member {member:?} has no policy projection"),
            }
        })?;
        operations.insert(operation.clone());
    }
    Ok(operations)
}

// =============================================================================
// Mode and Task-local coarse/fine consistency
// =============================================================================

/// The explicit durable security mode selected at typed admission.
///
/// The mode is never inferred from `sandbox_policy.is_some()`: `SECURITY_V1`
/// with no authored Task policy is a legal combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecurityContractMode {
    CoarseV1,
    SecurityV1,
}

impl SecurityContractMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CoarseV1 => "COARSE_V1",
            Self::SecurityV1 => "SECURITY_V1",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "COARSE_V1" => Some(Self::CoarseV1),
            "SECURITY_V1" => Some(Self::SecurityV1),
            _ => None,
        }
    }
}

/// The imported enforcement protocol/profile identity a `SECURITY_V1` source
/// revision can import (ADR-0013 §11).
///
/// B5.1 records the identity only: compile semantics belong to B5.2 and the
/// proven coverage matrix to B5.4. The tuple is part of the immutable source
/// revision contract, so it is validated here and never guessed from an
/// adapter kind.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnforcementProfileRef {
    protocol: String,
    profile: String,
    version: u32,
}

impl EnforcementProfileRef {
    pub fn new(
        protocol: impl Into<String>,
        profile: impl Into<String>,
        version: u32,
    ) -> Result<Self, ContractError> {
        let protocol = protocol.into();
        let profile = profile.into();
        validate_name_token(&protocol, "enforcement protocol")?;
        validate_name_token(&profile, "enforcement profile")?;
        if version == 0 {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "enforcement profile version must be at least 1".into(),
            });
        }
        Ok(Self {
            protocol,
            profile,
            version,
        })
    }

    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

/// The immutable security declaration of one exact SpawnSource revision.
///
/// A source revision is `SECURITY_V1` exactly when this record exists; absence
/// keeps the frozen `COARSE_V1` contract. `source_policy` is the optional
/// authored `P_source` ceiling, and `None` means `P_source = Top` — it adds no
/// authored narrowing but still requires the complete full-path checks
/// (ADR-0013 §11). It is never a "skip eligibility" marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSourceSecurityDeclaration {
    source_policy: Option<PolicyInput>,
    enforcement_profile: EnforcementProfileRef,
}

impl SpawnSourceSecurityDeclaration {
    pub fn new(
        source_policy: Option<PolicyInput>,
        enforcement_profile: EnforcementProfileRef,
    ) -> Self {
        Self {
            source_policy,
            enforcement_profile,
        }
    }

    pub fn without_policy(enforcement_profile: EnforcementProfileRef) -> Self {
        Self::new(None, enforcement_profile)
    }

    pub fn with_policy(
        source_policy: PolicyInput,
        enforcement_profile: EnforcementProfileRef,
    ) -> Self {
        Self::new(Some(source_policy), enforcement_profile)
    }

    pub fn source_policy(&self) -> Option<&PolicyInput> {
        self.source_policy.as_ref()
    }

    pub fn enforcement_profile(&self) -> &EnforcementProfileRef {
        &self.enforcement_profile
    }

    /// A source revision with a declaration is always `SECURITY_V1`; the mode
    /// is a property of the record, not of `source_policy.is_some()`.
    pub fn mode(&self) -> SecurityContractMode {
        SecurityContractMode::SecurityV1
    }
}

/// Prove that a Task's coarse fields and its (optional) fine policy agree
/// before any cross-object meet or normalization (ADR-0013 §3).
///
/// `None` means "this Task authored no additional fine restriction" and is
/// distinguishable from an explicit top policy. A contradiction is rejected
/// here even when a stricter ceiling would later hide it.
pub fn validate_task_local_coarse_fine(
    workspace: WorkspaceMode,
    network: NetworkPolicy,
    policy: Option<&NormalizedSandboxPolicy>,
) -> Result<(), PolicyConflict> {
    let Some(policy) = policy else {
        return Ok(());
    };
    match workspace {
        WorkspaceMode::ReadOnly => {
            if !policy.files().write().is_empty() {
                return Err(conflict(
                    "task workspace is read-only but its policy allows writing",
                ));
            }
        }
        WorkspaceMode::Write => {
            if policy.files().write().is_empty() {
                return Err(conflict(
                    "task requires writing but its policy allows no write region",
                ));
            }
        }
    }
    if network == NetworkPolicy::Disabled && !policy.network().is_empty() {
        return Err(conflict(
            "task network is disabled but its policy allows network endpoints",
        ));
    }
    Ok(())
}

// =============================================================================
// Derivation identity
// =============================================================================

/// One authored or derived policy input to an intersection.
///
/// The digest identifies canonical content; the optional authored reference is
/// provenance. Two different sources can therefore share content without
/// losing their respective derivation inputs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyInput {
    reference: Option<SandboxPolicyRef>,
    policy_digest: String,
}

impl PolicyInput {
    pub fn of(reference: Option<SandboxPolicyRef>, policy: &NormalizedSandboxPolicy) -> Self {
        Self {
            reference,
            policy_digest: crate::canonical::sandbox_policy_content_digest(policy),
        }
    }

    pub fn authored(reference: SandboxPolicyRef, policy: &NormalizedSandboxPolicy) -> Self {
        Self::of(Some(reference), policy)
    }

    pub fn derived(policy: &NormalizedSandboxPolicy) -> Self {
        Self::of(None, policy)
    }

    pub fn from_parts(
        reference: Option<SandboxPolicyRef>,
        policy_digest: impl Into<String>,
    ) -> Self {
        Self {
            reference,
            policy_digest: policy_digest.into(),
        }
    }

    pub fn reference(&self) -> Option<&SandboxPolicyRef> {
        self.reference.as_ref()
    }

    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }
}

/// The immutable identity of one policy derivation (ADR-0013 §4).
///
/// Normalization and intersection versions are recorded separately, and the
/// output and required-access digests bind the exact content this derivation
/// produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityDerivation {
    inputs: Vec<PolicyInput>,
    normalization_version: u32,
    meet_algorithm_version: u32,
    output_digest: String,
    required_access_digest: String,
}

impl SecurityDerivation {
    pub fn new(
        inputs: impl IntoIterator<Item = PolicyInput>,
        output: &NormalizedSandboxPolicy,
        required_access: &RequiredAccess,
    ) -> Result<Self, ContractError> {
        let mut inputs: Vec<PolicyInput> = inputs.into_iter().collect();
        inputs.sort();
        inputs.dedup();
        if inputs.is_empty() {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "a security derivation needs at least one input policy".into(),
            });
        }
        Ok(Self {
            inputs,
            normalization_version: SANDBOX_POLICY_NORMALIZATION_VERSION,
            meet_algorithm_version: SANDBOX_POLICY_MEET_ALGORITHM_VERSION,
            output_digest: crate::canonical::sandbox_policy_content_digest(output),
            required_access_digest: crate::canonical::required_access_content_digest(
                required_access,
            ),
        })
    }

    pub fn from_parts(
        inputs: impl IntoIterator<Item = PolicyInput>,
        normalization_version: u32,
        meet_algorithm_version: u32,
        output_digest: impl Into<String>,
        required_access_digest: impl Into<String>,
    ) -> Self {
        Self {
            inputs: inputs.into_iter().collect(),
            normalization_version,
            meet_algorithm_version,
            output_digest: output_digest.into(),
            required_access_digest: required_access_digest.into(),
        }
    }

    pub fn inputs(&self) -> &[PolicyInput] {
        &self.inputs
    }

    pub fn normalization_version(&self) -> u32 {
        self.normalization_version
    }

    pub fn meet_algorithm_version(&self) -> u32 {
        self.meet_algorithm_version
    }

    pub fn output_digest(&self) -> &str {
        &self.output_digest
    }

    pub fn required_access_digest(&self) -> &str {
        &self.required_access_digest
    }

    /// Reject a derivation recorded under normalization or meet versions this
    /// build does not implement.
    ///
    /// The two versions are checked separately (ADR-0013 §4): a row is never
    /// reinterpreted by a newer normalizer or a newer meet.
    pub fn ensure_supported(&self) -> Result<(), ContractError> {
        if self.normalization_version == SANDBOX_POLICY_NORMALIZATION_VERSION
            && self.meet_algorithm_version == SANDBOX_POLICY_MEET_ALGORITHM_VERSION
        {
            Ok(())
        } else {
            Err(ContractError::UnsupportedPolicyDerivationVersion {
                normalization_version: self.normalization_version,
                meet_algorithm_version: self.meet_algorithm_version,
            })
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        json!({
            "inputs": self
                .inputs
                .iter()
                .map(|input| json!({
                    "policy_digest": input.policy_digest,
                    "reference": input.reference.as_ref().map(|reference| json!({
                        "revision": reference.revision(),
                        "sandbox_policy_id": reference.id().as_str(),
                    })),
                }))
                .collect::<Vec<_>>(),
            "meet_algorithm_version": self.meet_algorithm_version,
            "normalization_version": self.normalization_version,
            "output_digest": self.output_digest,
            "required_access_digest": self.required_access_digest,
        })
    }
}

// =============================================================================
// Task security binding
// =============================================================================

/// A positive `R_min` component admission deliberately deferred to candidate
/// matching (ADR-0013 §5).
///
/// Recording a component as pending is not a satisfaction claim. Only
/// `Unsatisfied` rejects admission, and a pending component MUST be discharged
/// before Execution commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PendingObligation {
    RequiredCapabilities,
    AttemptIsolation,
    PlacementConstraints,
}

impl PendingObligation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequiredCapabilities => "REQUIRED_CAPABILITIES",
            Self::AttemptIsolation => "ATTEMPT_ISOLATION",
            Self::PlacementConstraints => "PLACEMENT_CONSTRAINTS",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "REQUIRED_CAPABILITIES" => Some(Self::RequiredCapabilities),
            "ATTEMPT_ISOLATION" => Some(Self::AttemptIsolation),
            "PLACEMENT_CONSTRAINTS" => Some(Self::PlacementConstraints),
            _ => None,
        }
    }
}

/// The immutable `TaskSecurityBinding` of one `SECURITY_V1` Task (ADR-0013 §4,
/// §11).
///
/// It binds the admitted `P_generation ∩ P_task` bytes to their derivation
/// identity and to the deferred `R_min` components. It is never a second
/// authored policy and carries no independent mode: the parent
/// `task_agent_requirements.security_contract_mode` remains the sole owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSecurityBinding {
    policy: NormalizedSandboxPolicy,
    derivation: SecurityDerivation,
    pending_obligations: BTreeSet<PendingObligation>,
}

impl TaskSecurityBinding {
    pub fn new(
        policy: NormalizedSandboxPolicy,
        derivation: SecurityDerivation,
        pending_obligations: impl IntoIterator<Item = PendingObligation>,
    ) -> Result<Self, ContractError> {
        if derivation.output_digest() != crate::canonical::sandbox_policy_content_digest(&policy) {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "task security binding derivation does not describe its admitted policy"
                    .into(),
            });
        }
        Ok(Self {
            policy,
            derivation,
            pending_obligations: pending_obligations.into_iter().collect(),
        })
    }

    pub fn policy(&self) -> &NormalizedSandboxPolicy {
        &self.policy
    }

    pub fn derivation(&self) -> &SecurityDerivation {
        &self.derivation
    }

    pub fn pending_obligations(&self) -> &BTreeSet<PendingObligation> {
        &self.pending_obligations
    }
}
