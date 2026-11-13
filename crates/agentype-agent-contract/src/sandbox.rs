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
    CapabilityCatalog, CapabilityDefinition, CapabilityRef, CapabilityValue, FileOperationClass,
    NetworkEndpoint, NetworkTransport, PolicyOperation, MAX_NETWORK_ENDPOINTS,
};
use crate::error::ContractError;
use crate::records::{NetworkPolicy, SandboxPolicyRef, SecurityContract};
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

/// Maximum path depth of one selector. Deep enough for any workspace layout a
/// tool can address, shallow enough that the number of selector prefixes stays
/// bounded during meet and containment checks.
pub const MAX_PATH_COMPONENTS: usize = 32;

/// Maximum byte length of one path component. This is the common filesystem
/// name limit; it also bounds the canonical selector size.
pub const MAX_PATH_COMPONENT_LEN: usize = 255;

/// Maximum number of selectors in one normalized path set. Path normalization
/// removes covered selectors with a pairwise scan, so authored input is bounded
/// by [`PathSet::try_allowed`] and the decoder before that scan; the policy
/// boundary rechecks the normalized counts.
pub const MAX_PATH_SELECTORS: usize = 256;

/// Maximum number of selectors across the five path sets of one policy. The
/// per-set bound alone would still let one policy carry five times the work;
/// this aggregate bound keeps the whole authored policy finite.
pub const MAX_PATH_SELECTORS_PER_POLICY: usize = 512;

/// Maximum number of distinct members in one allowed-name set (`tools`,
/// `commands`, or allowed environment names). The network and path dimensions
/// are bounded independently; this keeps the remaining set dimensions from
/// accepting an unbounded authored collection.
pub const MAX_NAME_SET_MEMBERS: usize = 256;

/// Maximum number of entries in one policy's required-environment-values map.
pub const MAX_ENVIRONMENT_REQUIRED_VALUES: usize = 256;

/// Maximum number of entries in one policy's credential-grant or
/// resource-ceiling map.
pub const MAX_POLICY_MAP_ENTRIES: usize = 256;

/// Maximum number of members in one credential grant facet (`uses` or
/// `audience`). Delivery modes are a closed two-value domain.
pub const MAX_CREDENTIAL_FACET_MEMBERS: usize = 256;

/// Maximum canonical JSON size of one authored sandbox policy document.
///
/// The per-dimension bounds keep normalization finite, but their combination
/// (for example maximum-length path components in every selector) can still
/// describe a very large document. Publication and decode reject a document
/// above this byte budget before doing durable work with it. The limit is part
/// of the v1 input grammar, not of the canonical bytes of an accepted policy.
pub const MAX_SANDBOX_POLICY_BYTES: usize = 1_048_576;

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
        if components.len() > MAX_PATH_COMPONENTS {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "path selector has {} components; at most {MAX_PATH_COMPONENTS} are allowed",
                    components.len()
                ),
            });
        }
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
    if component.len() > MAX_PATH_COMPONENT_LEN {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "path component is {} bytes; at most {MAX_PATH_COMPONENT_LEN} are allowed",
                component.len()
            ),
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

    /// A finite allowed set bounded by [`MAX_PATH_SELECTORS`], normalized to a
    /// minimal antichain of prefixes.
    ///
    /// Prefer this fallible constructor at every boundary that accepts authored
    /// input: the count is checked *before* cover removal runs its pairwise scan.
    /// [`PathSet::allowed`] remains the infallible convenience for trusted,
    /// already-bounded callers.
    pub fn try_allowed<I>(selectors: I) -> Result<Self, ContractError>
    where
        I: IntoIterator<Item = PathSelector>,
    {
        let selectors: Vec<PathSelector> = selectors.into_iter().collect();
        if selectors.len() > MAX_PATH_SELECTORS {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "path set has {} selectors; at most {MAX_PATH_SELECTORS} are allowed",
                    selectors.len()
                ),
            });
        }
        Ok(Self::allowed(selectors))
    }

    /// A finite allowed set, normalized to a minimal antichain of prefixes.
    ///
    /// Cover removal is a pairwise scan. Authored input MUST go through
    /// [`PathSet::try_allowed`] or the decode/policy boundaries, which enforce
    /// [`MAX_PATH_SELECTORS`] before this scan; an oversized caller-built set is
    /// also rejected by [`NormalizedSandboxPolicy::from_parts`].
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
        if allowed.len() > MAX_NAME_SET_MEMBERS {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "allowed name set has {} members; at most {MAX_NAME_SET_MEMBERS} are allowed",
                    allowed.len()
                ),
            });
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

/// The transport lattice as a two-bit mask: `TCP` and `UDP` are the atoms and
/// `Any` is their union. Collapsing `{TCP, UDP}` to `ANY` is therefore a plain
/// OR, not a separate normalization pass.
const TRANSPORT_TCP: u8 = 0b01;
const TRANSPORT_UDP: u8 = 0b10;
const TRANSPORT_ANY: u8 = TRANSPORT_TCP | TRANSPORT_UDP;

fn transport_mask(transport: NetworkTransport) -> u8 {
    match transport {
        NetworkTransport::Tcp => TRANSPORT_TCP,
        NetworkTransport::Udp => TRANSPORT_UDP,
        NetworkTransport::Any => TRANSPORT_ANY,
    }
}

fn transport_from_mask(mask: u8) -> Option<NetworkTransport> {
    match mask {
        TRANSPORT_TCP => Some(NetworkTransport::Tcp),
        TRANSPORT_UDP => Some(NetworkTransport::Udp),
        TRANSPORT_ANY => Some(NetworkTransport::Any),
        _ => None,
    }
}

/// Collapse one host's rules to the minimal set not covered by another rule of
/// the same host.
///
/// Ports form a flat domain with `None` (any port) above each exact port. An
/// any-port rule only removes an exact-port rule when its transport mask covers
/// that rule's whole mask, because rule removal is single-rule subsumption, not
/// set subtraction.
fn reduce_host_ports(ports: &mut BTreeMap<Option<u16>, u8>) {
    let Some(any_port) = ports.get(&None).copied() else {
        return;
    };
    if any_port == TRANSPORT_ANY {
        ports.clear();
        ports.insert(None, TRANSPORT_ANY);
        return;
    }
    ports.retain(|port, mask| {
        port.is_none() || *mask == TRANSPORT_ANY || (*mask & any_port) != *mask
    });
}

fn push_endpoint_rules(
    host: &str,
    ports: &BTreeMap<Option<u16>, u8>,
    out: &mut BTreeSet<NetworkEndpoint>,
) {
    for (port, mask) in ports {
        let Some(transport) = transport_from_mask(*mask) else {
            continue;
        };
        // The host and port already passed `NetworkEndpoint::new` when the
        // policy was decoded or authored, so this cannot fail. Skipping instead
        // of unwrapping keeps the infallible constructor total.
        if let Ok(endpoint) = NetworkEndpoint::new(transport, host.to_string(), *port) {
            out.insert(endpoint);
        }
    }
}

impl NetworkEndpointSet {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn deny_all() -> Self {
        Self::Allowed(BTreeSet::new())
    }

    /// A finite allowed set, normalized by merging an explicit `{TCP, UDP}` pair
    /// at the same host/port into the equivalent `ANY` selector, normalizing the
    /// explicit universal rule `ANY:*:*` to [`NetworkEndpointSet::Any`], and then
    /// removing endpoints covered by a broader rule.
    ///
    /// `Any` and `Allowed([ANY:*:*])` denote exactly the same v1 network
    /// universe (TCP/UDP × any host × any port), so they MUST not survive as two
    /// canonical spellings. Keeping both would make `project_network` and the
    /// policy order disagree about a semantically top policy.
    ///
    /// Normalization groups by host and port, so removal is `O(n log n)` rather
    /// than a global `O(n²)` pairwise scan. A rule can only be covered by a rule
    /// with the same host or the `*` host; this keeps a bounded `256 × 256` meet
    /// product cheap enough that the final 256-rule policy bound, not the
    /// intermediate normalization work, decides admissibility.
    pub fn allowed<I>(endpoints: I) -> Self
    where
        I: IntoIterator<Item = NetworkEndpoint>,
    {
        let mut by_host: BTreeMap<String, BTreeMap<Option<u16>, u8>> = BTreeMap::new();
        for endpoint in endpoints {
            let mask = by_host
                .entry(endpoint.host().to_string())
                .or_default()
                .entry(endpoint.port())
                .or_insert(0);
            *mask |= transport_mask(endpoint.transport());
        }
        for ports in by_host.values_mut() {
            reduce_host_ports(ports);
        }

        // `*` is the only cross-host wildcard: an exact-host rule can only be
        // covered by another rule of the same host or by a `*` rule.
        let wildcard = by_host.remove("*");
        let wildcard_any_port = wildcard
            .as_ref()
            .and_then(|ports| ports.get(&None).copied());
        for ports in by_host.values_mut() {
            if wildcard_any_port == Some(TRANSPORT_ANY) {
                ports.clear();
                continue;
            }
            ports.retain(|port, mask| {
                if wildcard_any_port.is_some_and(|w| w == *mask) {
                    return false;
                }
                match wildcard.as_ref().and_then(|rules| rules.get(port)) {
                    Some(w) => *w != TRANSPORT_ANY && *w != *mask,
                    None => true,
                }
            });
        }

        let mut all = BTreeSet::new();
        if let Some(wildcard) = wildcard.as_ref() {
            push_endpoint_rules("*", wildcard, &mut all);
        }
        for (host, ports) in &by_host {
            push_endpoint_rules(host, ports, &mut all);
        }

        // `ANY:*:*` is the whole v1 network operation domain, exactly like
        // `Any`. No broader finite rule exists because `*` and "any port" are
        // the only host/port wildcards.
        if all.iter().any(|endpoint| {
            endpoint.transport() == NetworkTransport::Any
                && endpoint.host() == "*"
                && endpoint.port().is_none()
        }) {
            return Self::Any;
        }
        Self::Allowed(all)
    }

    /// Whether some single allowed rule covers `endpoint`.
    ///
    /// This is a rule-membership predicate, not the authority relation: authority
    /// is union containment, so `ANY:*:443` is granted by
    /// `TCP:*:443 ∪ UDP:*:*` even though `contains` returns `false` here. Use
    /// [`PolicyAccess::allows_endpoint`] or [`is_within`] for authorization.
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

/// Tool and command allowlists.
///
/// They do not grant two independent capabilities. ADR-0013 §7 defines one
/// abstract `ToolCommandSet`, whose members are the union of the two authored
/// allowlists. Normalization therefore stores the union in `tools` and leaves
/// `commands` deny-all, so `{tools: {git}, commands: {}}` and
/// `{tools: {}, commands: {git}}` are the same canonical policy. Keeping two
/// separate spellings would make `meet`, `is_within` and `PolicyAccess`
/// disagree about the same abstract operation. `Any` in either field means the
/// whole namespace, so normalization keeps both fields `Any`.
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
        if required_values.len() > MAX_ENVIRONMENT_REQUIRED_VALUES {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "required environment map has {} entries; at most {MAX_ENVIRONMENT_REQUIRED_VALUES} are allowed",
                    required_values.len()
                ),
            });
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
        if validated_uses.len() > MAX_CREDENTIAL_FACET_MEMBERS {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "credential use set has {} members; at most {MAX_CREDENTIAL_FACET_MEMBERS} are allowed",
                    validated_uses.len()
                ),
            });
        }
        if validated_audience.len() > MAX_CREDENTIAL_FACET_MEMBERS {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "credential audience set has {} members; at most {MAX_CREDENTIAL_FACET_MEMBERS} are allowed",
                    validated_audience.len()
                ),
            });
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
    // Bound the authored per-set and aggregate selector counts before the
    // downstream meet/containment normalization scans them again. The decode and
    // `PathSet::try_allowed` boundaries reject an oversized raw set before its
    // own pairwise cover removal.
    validate_path_sets(&parts.files)?;
    let environment = EnvironmentPolicy::new(
        normalize_name_set(&parts.environment.allowed_names)?,
        parts.environment.required_values.clone(),
    )?;
    validate_policy_map_entries(parts.credentials.len(), "credential grant")?;
    validate_policy_map_entries(parts.resources.len(), "resource ceiling")?;
    let mut credentials = BTreeMap::new();
    for (id, grant) in parts.credentials {
        if grant.id != id {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "credential grant identity does not match its map key".into(),
            });
        }
        let grant =
            CredentialGrant::new(id.clone(), grant.uses, grant.audience, grant.delivery_modes)?;
        // Every facet narrows, so an empty facet authorizes nothing. Dropping the
        // unusable grant makes the canonical form the unique identity of the
        // authorization semantics: an explicit no-authority grant and an absent
        // grant are the same policy.
        if grant.is_usable() {
            credentials.insert(id, grant);
        }
    }
    Ok(SandboxPolicyParts {
        files: FilePolicy::new(
            normalize_path_set(&parts.files.read),
            normalize_path_set(&parts.files.write),
            normalize_path_set(&parts.files.execute),
            normalize_path_set(&parts.files.delete),
            normalize_path_set(&parts.files.visible),
        ),
        tools: normalize_tool_policy(&parts.tools)?,
        network: match &parts.network {
            NetworkEndpointSet::Any => NetworkEndpointSet::Any,
            NetworkEndpointSet::Allowed(endpoints) => {
                // Bound the raw authored set before normalization's pairwise
                // cover removal, not only after it.
                validate_network_endpoint_count(endpoints.len())?;
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

/// Collapse the two authored allowlists into the one canonical spelling of the
/// v1 `ToolCommandSet`.
///
/// Both `Any` fields stay `Any` (the whole namespace). Otherwise the union is
/// stored in `tools` and `commands` is explicit deny-all. This is what makes
/// `{tools: {git}, commands: {}}`, `{tools: {}, commands: {git}}` and
/// `{tools: {git}, commands: {git}}` one canonical policy with one digest.
fn normalize_tool_policy(policy: &ToolPolicy) -> Result<ToolPolicy, ContractError> {
    match (policy.tools(), policy.commands()) {
        (NameSet::Any, _) | (_, NameSet::Any) => Ok(ToolPolicy::top()),
        (NameSet::Allowed(tools), NameSet::Allowed(commands)) => {
            let mut merged = tools.clone();
            merged.extend(commands.iter().cloned());
            Ok(ToolPolicy::new(
                NameSet::allowed(merged)?,
                NameSet::deny_all(),
            ))
        }
    }
}

fn validate_network_endpoint_count(count: usize) -> Result<(), ContractError> {
    if count > MAX_NETWORK_ENDPOINTS {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "network endpoint set has {count} rules; at most {MAX_NETWORK_ENDPOINTS} are allowed"
            ),
        });
    }
    Ok(())
}

fn validate_policy_map_entries(count: usize, what: &str) -> Result<(), ContractError> {
    if count > MAX_POLICY_MAP_ENTRIES {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "policy has {count} {what} entries; at most {MAX_POLICY_MAP_ENTRIES} are allowed"
            ),
        });
    }
    Ok(())
}

fn validate_required_access_counts(access: &RequiredAccess) -> Result<(), ContractError> {
    if access.tools_commands.len() > MAX_NAME_SET_MEMBERS {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "required tool or command set has {} members; at most {MAX_NAME_SET_MEMBERS} are allowed",
                access.tools_commands.len()
            ),
        });
    }
    if access.environment_names.len() > MAX_NAME_SET_MEMBERS {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "required environment name set has {} members; at most {MAX_NAME_SET_MEMBERS} are allowed",
                access.environment_names.len()
            ),
        });
    }
    if access.credentials.len() > MAX_POLICY_MAP_ENTRIES {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "required credential set has {} members; at most {MAX_POLICY_MAP_ENTRIES} are allowed",
                access.credentials.len()
            ),
        });
    }
    Ok(())
}

fn validate_path_sets(files: &FilePolicy) -> Result<(), ContractError> {
    let sets = [
        files.read(),
        files.write(),
        files.execute(),
        files.delete(),
        files.visible(),
    ];
    let mut total = 0usize;
    for set in sets {
        if let PathSet::Allowed(selectors) = set {
            if selectors.len() > MAX_PATH_SELECTORS {
                return Err(ContractError::InvalidSandboxPolicy {
                    reason: format!(
                        "path set has {} selectors; at most {MAX_PATH_SELECTORS} are allowed",
                        selectors.len()
                    ),
                });
            }
            total += selectors.len();
        }
    }
    if total > MAX_PATH_SELECTORS_PER_POLICY {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "policy has {total} path selectors; at most {MAX_PATH_SELECTORS_PER_POLICY} are allowed"
            ),
        });
    }
    Ok(())
}

fn validate_parts(parts: &SandboxPolicyParts) -> Result<(), ContractError> {
    // Post-normalization defense in depth; the raw-set bound above rejects a
    // wider authored policy before the pairwise cover scan.
    validate_path_sets(&parts.files)?;
    if let Some(endpoints) = parts.network.endpoints() {
        validate_network_endpoint_count(endpoints.len())?;
    }
    for set in [
        &parts.tools.tools,
        &parts.tools.commands,
        &parts.environment.allowed_names,
    ] {
        if let NameSet::Allowed(names) = set {
            if names.len() > MAX_NAME_SET_MEMBERS {
                return Err(ContractError::InvalidSandboxPolicy {
                    reason: format!(
                        "allowed name set has {} members; at most {MAX_NAME_SET_MEMBERS} are allowed",
                        names.len()
                    ),
                });
            }
        }
    }
    validate_policy_map_entries(parts.credentials.len(), "credential grant")?;
    validate_policy_map_entries(parts.resources.len(), "resource ceiling")?;
    if parts.environment.required_values.len() > MAX_ENVIRONMENT_REQUIRED_VALUES {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "required environment map has {} entries; at most {MAX_ENVIRONMENT_REQUIRED_VALUES} are allowed",
                parts.environment.required_values.len()
            ),
        });
    }
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
            // Each input is validated to at most `MAX_NETWORK_ENDPOINTS` rules, so
            // this product is deterministically bounded by 256 × 256 candidates.
            // `allowed` normalizes that product in `O(n log n)`, and
            // `from_parts` rejects a result that still exceeds the 256-rule
            // policy bound. No input can force a global quadratic normalization.
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
        tools: meet_tool_commands(left.tools(), right.tools()),
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

/// The `ToolCommandSet` a policy authorizes: the union of its `tools` and
/// `commands` allowlists. `None` is the whole namespace (`Any` in either
/// field); `Some` is the finite effective set.
fn effective_tool_commands(policy: &ToolPolicy) -> Option<BTreeSet<String>> {
    match (policy.tools(), policy.commands()) {
        (NameSet::Any, _) | (_, NameSet::Any) => None,
        (NameSet::Allowed(tools), NameSet::Allowed(commands)) => {
            Some(tools.union(commands).cloned().collect())
        }
    }
}

fn tool_commands_within(inner: &ToolPolicy, outer: &ToolPolicy) -> bool {
    match (
        effective_tool_commands(inner),
        effective_tool_commands(outer),
    ) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(inner), Some(outer)) => inner.is_subset(&outer),
    }
}

/// Greatest lower bound of two `ToolCommandSet` policies over their shared
/// effective namespace, returned in canonical form.
fn meet_tool_commands(left: &ToolPolicy, right: &ToolPolicy) -> ToolPolicy {
    match (
        effective_tool_commands(left),
        effective_tool_commands(right),
    ) {
        (None, None) => ToolPolicy::top(),
        (None, Some(names)) | (Some(names), None) => {
            ToolPolicy::new(NameSet::Allowed(names), NameSet::deny_all())
        }
        (Some(left), Some(right)) => ToolPolicy::new(
            NameSet::Allowed(left.intersection(&right).cloned().collect()),
            NameSet::deny_all(),
        ),
    }
}

fn network_set_within(inner: &NetworkEndpointSet, outer: &NetworkEndpointSet) -> bool {
    match inner {
        NetworkEndpointSet::Any => outer.is_any(),
        NetworkEndpointSet::Allowed(inner) => match outer {
            NetworkEndpointSet::Any => true,
            NetworkEndpointSet::Allowed(outer) => network_allowed_union_within(inner, outer),
        },
    }
}

/// Host projection of the finite endpoint grammar. `*` is the only wildcard, so
/// an exact host and the single "every other host" atom partition the host
/// space without enumerating any concrete host twice. The exact atom indexes
/// the shared named-host table built by the containment check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkHostAtom {
    Exact(usize),
    Other,
}

/// Port projection of the finite endpoint grammar. `None` is the "any port"
/// wildcard; `Other` partitions every port not named by an exact rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkPortAtom {
    Exact(u16),
    Other,
}

/// Whether the union of `inner` rules is contained in the union of `outer`
/// rules.
///
/// A single inner rule may be covered by several outer rules together, e.g.
/// `ANY:*:443` is covered by `TCP:*:443 ∪ UDP:*:*`. The endpoint grammar is a
/// product of finite transport, host and port partitions, so we mark every atom
/// covered by the outer union once and require every atom an inner rule covers
/// to be marked. This is exact set containment, not "some single outer rule
/// covers the whole rule".
///
/// Marking outer coverage once makes the cost `O((|inner| + |outer|) · atoms)`
/// instead of `O(|inner| · |outer| · atoms)`. Each rule first projects to the
/// host and port atoms it covers, so an exact-port rule marks one port column
/// per covered host rather than rescanning every atom to reject it. Together
/// with [`MAX_NETWORK_ENDPOINTS`] this bounds the finite partition enumeration.
fn network_allowed_union_within(
    inner: &BTreeSet<NetworkEndpoint>,
    outer: &BTreeSet<NetworkEndpoint>,
) -> bool {
    let mut named_hosts = BTreeSet::new();
    let mut named_ports = BTreeSet::new();
    for endpoint in inner.iter().chain(outer.iter()) {
        if endpoint.host() != "*" {
            named_hosts.insert(endpoint.host().to_string());
        }
        if let Some(port) = endpoint.port() {
            named_ports.insert(port);
        }
    }
    let named_hosts: Vec<String> = named_hosts.into_iter().collect();
    let named_ports: Vec<u16> = named_ports.into_iter().collect();
    let mut host_atoms: Vec<NetworkHostAtom> =
        (0..named_hosts.len()).map(NetworkHostAtom::Exact).collect();
    host_atoms.push(NetworkHostAtom::Other);
    let mut port_atoms: Vec<NetworkPortAtom> = named_ports
        .iter()
        .copied()
        .map(NetworkPortAtom::Exact)
        .collect();
    port_atoms.push(NetworkPortAtom::Other);
    let transport_atoms = [NetworkTransport::Tcp, NetworkTransport::Udp];
    let atom_index = |transport: usize, host: usize, port: usize| {
        (transport * host_atoms.len() + host) * port_atoms.len() + port
    };
    let mut covered = vec![false; transport_atoms.len() * host_atoms.len() * port_atoms.len()];

    let rule_host_atoms = |rule: &NetworkEndpoint| {
        host_atoms
            .iter()
            .enumerate()
            .filter(|(_, host)| network_host_covers(rule.host(), **host, &named_hosts))
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    };
    let rule_port_atoms = |rule: &NetworkEndpoint| {
        port_atoms
            .iter()
            .enumerate()
            .filter(|(_, port)| network_port_covers(rule.port(), **port))
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    };

    for rule in outer {
        let hosts = rule_host_atoms(rule);
        let ports = rule_port_atoms(rule);
        for (transport_index, transport) in transport_atoms.iter().enumerate() {
            if !network_transport_covers(rule.transport(), *transport) {
                continue;
            }
            for &host_index in &hosts {
                for &port_index in &ports {
                    covered[atom_index(transport_index, host_index, port_index)] = true;
                }
            }
        }
    }

    inner.iter().all(|rule| {
        let hosts = rule_host_atoms(rule);
        let ports = rule_port_atoms(rule);
        transport_atoms
            .iter()
            .enumerate()
            .all(|(transport_index, transport)| {
                !network_transport_covers(rule.transport(), *transport)
                    || hosts.iter().all(|&host_index| {
                        ports.iter().all(|&port_index| {
                            covered[atom_index(transport_index, host_index, port_index)]
                        })
                    })
            })
    })
}

fn network_transport_covers(rule: NetworkTransport, atom: NetworkTransport) -> bool {
    rule == NetworkTransport::Any || rule == atom
}

fn network_host_covers(rule: &str, atom: NetworkHostAtom, named_hosts: &[String]) -> bool {
    match atom {
        NetworkHostAtom::Exact(index) => rule == "*" || rule == named_hosts[index].as_str(),
        NetworkHostAtom::Other => rule == "*",
    }
}

fn network_port_covers(rule: Option<u16>, atom: NetworkPortAtom) -> bool {
    match (rule, atom) {
        (None, _) => true,
        (Some(rule), NetworkPortAtom::Exact(port)) => rule == port,
        (Some(_), NetworkPortAtom::Other) => false,
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
        && tool_commands_within(inner.tools(), outer.tools())
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
        let network_endpoints: BTreeSet<NetworkEndpoint> = network_endpoints.into_iter().collect();
        validate_network_endpoint_count(network_endpoints.len())?;
        let access = Self {
            file_operations: file_operations.into_iter().collect(),
            tools_commands: tools,
            network_endpoints,
            environment_names: environment,
            credentials: credentials.into_iter().collect(),
        };
        validate_required_access_counts(&access)?;
        Ok(access)
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
        validate_network_endpoint_count(access.network_endpoints.len())?;
        validate_required_access_counts(&access)?;
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

    /// Whether the effective network set grants this exact required endpoint.
    ///
    /// A required endpoint is a singleton rule and MUST be checked against the
    /// union of all effective rules, not against one covering rule. Otherwise
    /// ordering and requirement satisfaction disagree: `ANY:*:443` is within
    /// `TCP:*:443 ∪ UDP:*:*`, but no single outer rule covers it.
    pub fn allows_endpoint(&self, endpoint: &NetworkEndpoint) -> bool {
        let required = NetworkEndpointSet::Allowed(BTreeSet::from([endpoint.clone()]));
        network_set_within(&required, &self.network)
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
    // Required endpoints are a finite set: one union-containment check decides
    // every member at once. The per-endpoint scan runs only on the rejecting
    // path, to name the endpoints that are actually missing. Calling
    // `allows_endpoint` for each member would rebuild the required singleton and
    // re-enumerate the same partition up to 256 times for one admission.
    let required_network = required.network_endpoints();
    if !required_network.is_empty()
        && !network_set_within(
            &NetworkEndpointSet::Allowed(required_network.clone()),
            &access.network,
        )
    {
        for endpoint in required_network {
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

/// The coarse workspace projection of an authored policy (ADR-0013 §3).
///
/// `Write` is projected when either a write region or a delete region survives;
/// `ReadOnly` means the policy authorizes no content or directory-entry
/// modification. This is the projection the coarse `SecurityContract` of an
/// AgentType must equal and that a Generation ceiling must match, never a
/// separate authorization grant.
pub fn project_workspace(policy: &NormalizedSandboxPolicy) -> WorkspaceMode {
    if policy.files().write().is_empty() && policy.files().delete().is_empty() {
        WorkspaceMode::ReadOnly
    } else {
        WorkspaceMode::Write
    }
}

/// The coarse network projection of an authored policy (ADR-0013 §3).
///
/// An empty finite allowlist is `Disabled`, an unrestricted set is `Enabled`,
/// and every other finite allowlist is `Restricted`.
pub fn project_network(policy: &NormalizedSandboxPolicy) -> NetworkPolicy {
    match policy.network() {
        NetworkEndpointSet::Any => NetworkPolicy::Enabled,
        NetworkEndpointSet::Allowed(endpoints) if endpoints.is_empty() => NetworkPolicy::Disabled,
        NetworkEndpointSet::Allowed(_) => NetworkPolicy::Restricted,
    }
}

/// The coarse AgentType security contract an authored policy projects to.
///
/// Equality with the AgentType's `SecurityContract` is the §3 relation; attempt
/// isolation is carried as an execution guarantee, not as an allowed set.
pub fn project_security_contract(policy: &NormalizedSandboxPolicy) -> SecurityContract {
    SecurityContract {
        workspace: project_workspace(policy),
        network: project_network(policy),
        requires_attempt_isolation: policy.attempt_isolation(),
    }
}

/// Prove that a `SECURITY_V1` AgentType's coarse `SecurityContract` is exactly
/// the projection of its authored policy (ADR-0013 §3).
pub fn validate_agent_type_coarse_fine(
    contract: &SecurityContract,
    policy: &NormalizedSandboxPolicy,
) -> Result<(), PolicyConflict> {
    if project_security_contract(policy) != *contract {
        return Err(conflict(
            "agent type security contract does not equal its authored policy projection",
        ));
    }
    Ok(())
}

/// Prove that a `SECURITY_V1` Generation ceiling is exactly the projection of
/// its authored policy (ADR-0013 §3).
///
/// A Generation is a maximum-authority ceiling, so both coarse modes must equal
/// the policy projection rather than merely bound it. The isolation bit has one
/// authority, `requires_attempt_isolation`: a generation that requires isolation
/// must author it, and a policy that authors isolation must declare the
/// guarantee. Anything else would leave `P_eff.attempt_isolation = true` with no
/// requirement that asks a committed execution to request isolation.
pub fn validate_generation_coarse_fine(
    max_workspace: WorkspaceMode,
    max_network: NetworkPolicy,
    requires_attempt_isolation: bool,
    policy: &NormalizedSandboxPolicy,
) -> Result<(), PolicyConflict> {
    if project_workspace(policy) != max_workspace {
        return Err(conflict(
            "generation workspace ceiling does not equal its authored policy projection",
        ));
    }
    if project_network(policy) != max_network {
        return Err(conflict(
            "generation network ceiling does not equal its authored policy projection",
        ));
    }
    match (requires_attempt_isolation, policy.attempt_isolation()) {
        (true, false) => Err(conflict(
            "generation requires attempt isolation but its authored policy does not entail it",
        )),
        (false, true) => Err(conflict(
            "generation authored policy requires attempt isolation but the generation does not declare the guarantee",
        )),
        _ => Ok(()),
    }
}

/// Prove that a Task's coarse fields and its optional fine policy agree before
/// any cross-object meet or normalization (ADR-0013 §3).
///
/// `None` means "this Task authored no additional fine restriction" and is
/// distinguishable from an explicit top policy. A contradiction is rejected
/// here even when a stricter ceiling would later hide it, so a coarse field can
/// never be silently contradicted by the Task's own authored policy.
pub fn validate_task_local_coarse_fine(
    workspace: WorkspaceMode,
    network: NetworkPolicy,
    policy: Option<&NormalizedSandboxPolicy>,
) -> Result<(), PolicyConflict> {
    let Some(policy) = policy else {
        return Ok(());
    };
    if project_workspace(policy) != workspace {
        return Err(conflict(match workspace {
            WorkspaceMode::ReadOnly => "task workspace is read-only but its policy allows writing",
            WorkspaceMode::Write => "task requires writing but its policy allows no write region",
        }));
    }
    let projected = project_network(policy);
    match network {
        NetworkPolicy::Disabled if projected != NetworkPolicy::Disabled => {
            return Err(conflict(
                "task network is disabled but its policy allows network endpoints",
            ));
        }
        NetworkPolicy::Restricted if projected == NetworkPolicy::Disabled => {
            return Err(conflict(
                "task network is restricted but its policy allows no network endpoint",
            ));
        }
        NetworkPolicy::Enabled if projected != NetworkPolicy::Enabled => {
            return Err(conflict(
                "task network is enabled but its policy does not allow every endpoint",
            ));
        }
        _ => {}
    }
    Ok(())
}

/// Prove that a Task's coarse execution modes remain satisfiable **after** the
/// Task/Generation policy meet (ADR-0013 §5).
///
/// `fold_generation_policy_security_v1` compares coarse levels before the meet,
/// but an intersection can erase a positive requirement even when both layers
/// are individually valid: `write {src} ∩ write {tests} = ∅` is a legal
/// deny-all write set, not a `PolicyConflict`. A Task whose durable
/// `required_workspace` is `Write` therefore MUST be rejected at admission if
/// the admitted policy has no writable region. The same applies to the positive
/// network modes: `Restricted` needs at least one endpoint and `Enabled` needs
/// every endpoint.
///
/// `ReadOnly` and `Disabled` are upper bounds already carried by the durable
/// coarse requirement and add no positive check here.
pub fn validate_effective_task_execution_modes(
    workspace: WorkspaceMode,
    network: NetworkPolicy,
    policy: &NormalizedSandboxPolicy,
) -> Result<(), PolicyConflict> {
    if workspace == WorkspaceMode::Write && policy.files().write().is_empty() {
        return Err(conflict(
            "task requires writing but the admitted policy allows no write region",
        ));
    }
    match network {
        NetworkPolicy::Restricted if project_network(policy) == NetworkPolicy::Disabled => {
            return Err(conflict(
                "task network is restricted but the admitted policy allows no network endpoint",
            ));
        }
        NetworkPolicy::Enabled if project_network(policy) != NetworkPolicy::Enabled => {
            return Err(conflict(
                "task network is enabled but the admitted policy does not allow every endpoint",
            ));
        }
        _ => {}
    }
    Ok(())
}

/// The Task's coarse-constrained authority for admission checks (ADR-0013 §3,
/// §5).
///
/// `P_task` is the coarse workspace/network term intersected with the optional
/// fine policy, so the authority admission may rely on is
/// `meet(P_generation, P_task)`. A `ReadOnly` Task removes every write and delete
/// region and a `Disabled` Task removes every endpoint, even when the fine layer
/// still shows them; the fine-derived meet remains the durable
/// `TaskSecurityBinding` identity.
///
/// Checking the fine layer alone is wrong in both directions. It accepts a
/// `ReadOnly` Task whose required capability projects to `Write` or `Delete`,
/// and it rejects a legal `ReadOnly` Task because unrelated Generation write
/// paths make the fine layer look wider than a read-only AgentType.
///
/// This is a pure narrowing: it only replaces the constrained dimensions, so the
/// credential, tool, environment and resource grants of the meet are preserved.
pub fn task_coarse_authority(
    policy: &NormalizedSandboxPolicy,
    workspace: WorkspaceMode,
    network: NetworkPolicy,
) -> Result<NormalizedSandboxPolicy, ContractError> {
    let mut parts = policy.parts().clone();
    if workspace == WorkspaceMode::ReadOnly {
        let files = parts.files;
        parts.files = FilePolicy::new(
            files.read().clone(),
            PathSet::deny_all(),
            files.execute().clone(),
            PathSet::deny_all(),
            files.visible().clone(),
        );
    }
    if network == NetworkPolicy::Disabled {
        parts.network = NetworkEndpointSet::deny_all();
    }
    NormalizedSandboxPolicy::from_parts(parts)
}

/// Meet an optional authored layer onto a base policy.
///
/// `None` means the object authored no additional fine layer; it adds no
/// restriction and MUST NOT be represented as an explicit top policy, because
/// an explicit top would erase the base's credential grants through the
/// not-`Top` credential dimension. `Some(layer)` delegates to [`meet`].
pub fn meet_optional_layer(
    base: &NormalizedSandboxPolicy,
    layer: Option<&NormalizedSandboxPolicy>,
) -> Result<NormalizedSandboxPolicy, PolicyConflict> {
    match layer {
        None => Ok(base.clone()),
        Some(layer) => meet(base, layer),
    }
}

/// Prove that every capability claim an AgentType requires is allowed by the
/// same AgentType's authored policy (ADR-0013 §3, §6).
///
/// A formally mapped capability and its policy dimension express the same
/// restriction and therefore have exactly one authoritative value. A claim
/// whose projected operations the authored policy does not allow is an invalid
/// `SECURITY_V1` AgentType, not a candidate-local mismatch.
pub fn validate_capability_claims_against_policy(
    claims: &BTreeMap<CapabilityRef, CapabilityValue>,
    catalog: &CapabilityCatalog,
    policy: &NormalizedSandboxPolicy,
) -> Result<(), ContractError> {
    let access = PolicyAccess::from_policy(policy);
    for (reference, value) in claims {
        let definition =
            catalog
                .get(reference)
                .ok_or_else(|| ContractError::CapabilityMismatch {
                    capability: reference.capability_id().as_str().to_string(),
                })?;
        let operations = project_capability_access(definition, value)?;
        let required = RequiredAccess::from_operations(operations)?;
        check_required_access(&required, &access).map_err(|deficit| {
            ContractError::InvalidPolicyProjection {
                reason: format!(
                    "agent type capability {}@{} is not allowed by its authored policy: {deficit}",
                    reference.capability_id().as_str(),
                    reference.revision()
                ),
            }
        })?;
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
///
/// **The bound policy is a fine-policy derivation, not the Task's complete
/// authority.** When the Task authors no fine layer, the effective policy can
/// still contain authority granted only by the Generation ceiling — for example
/// a `ReadOnly` Task under a `Write` Generation retains the Generation's write
/// paths in this policy. The Task's coarse `required_workspace` /
/// `required_network` fields remain an independent durable requirement, and any
/// later execution-plan compilation MUST compose and verify both. A consumer
/// MUST NOT compile a plan from this policy alone and treat it as the complete
/// effective Task permission.
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
