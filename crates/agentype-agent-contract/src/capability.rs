//! Capability vocabulary and its canonical catalog.
//!
//! A capability's semantics (matcher kind, security class) are defined **once**
//! by a catalog keyed on the exact `(capability_id, revision)`. An AgentType
//! references a capability; it never owns a private definition that could
//! redefine what a security class means. `ENFORCED` is imported evidence, not
//! an enum label.

use crate::error::ContractError;
use crate::ids::CapabilityId;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The five matcher shapes supported in M6-B v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatcherKind {
    /// Boolean presence: `false` means absent, `true` means present.
    /// `provided` satisfies `required` when `required` implies `provided`
    /// (`!required || provided`), so a present value also satisfies an absent
    /// requirement and the order is compositional.
    Bool,
    /// Required subset of a provided set (e.g. `tools ⊇ {git,ripgrep}`).
    Set,
    /// Enum lattice rank (e.g. `continuity >= logical`).
    Ordered,
    /// Numeric lower bound (e.g. `memory_mb >= 4096`).
    Quantity,
    /// Exact equality.
    Exact,
}

/// A validated non-negative, finite quantity.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Quantity(f64);

impl Quantity {
    pub fn new(value: f64) -> Result<Self, ContractError> {
        if value.is_finite() && value >= 0.0 {
            // Canonicalize negative zero so equal values cannot digest differently.
            Ok(Self(if value == 0.0 { 0.0 } else { value }))
        } else {
            Err(ContractError::InvalidNumber {
                field: "quantity".into(),
            })
        }
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// A capability value. The shape is determined by the capability definition.
#[derive(Clone, Debug, PartialEq)]
pub enum CapabilityValue {
    Bool(bool),
    Set(BTreeSet<String>),
    Ordered { class: String, rank: u32 },
    Quantity(Quantity),
    Exact(Value),
}

impl CapabilityValue {
    pub fn matcher_kind(&self) -> MatcherKind {
        match self {
            Self::Bool(_) => MatcherKind::Bool,
            Self::Set(_) => MatcherKind::Set,
            Self::Ordered { .. } => MatcherKind::Ordered,
            Self::Quantity(_) => MatcherKind::Quantity,
            Self::Exact(_) => MatcherKind::Exact,
        }
    }

    /// Whether the value denotes presence. `Bool(false)` is the algebraic
    /// bottom (absence); every other value is present. A canonical capability
    /// map MUST encode absence by omitting the entry, never by an explicit
    /// `Bool(false)`, so `get == None` and `Bool(false)` are the same state.
    pub fn is_present(&self) -> bool {
        !matches!(self, Self::Bool(false))
    }
}

/// Whether a capability participates in functional features or in a
/// security/authority proof (which then requires imported `ENFORCED` evidence).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecurityClass {
    Functional,
    Authority,
    Sandbox,
    Continuity,
}

/// Declaration metadata for a claim. This is *not* a proof: only imported
/// `ResolvedProvisioningEvidence` can satisfy a security requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assurance {
    Declared,
    Enforced,
}

impl SecurityClass {
    /// Whether this class requires imported enforcement evidence. This is
    /// independent of the refinement polarity.
    pub fn requires_evidence(self) -> bool {
        !matches!(self, Self::Functional)
    }
}

/// Refinement direction of a capability value. This is an independent,
/// catalog-owned property: it is NOT derived from the security class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapabilityPolarity {
    /// More value = more ability = wider authority (narrows by shrinking).
    Ability,
    /// More value = more restriction = narrower (narrows by growing).
    Restriction,
}

/// Exact `(capability_id, revision)` identity. Fields are private so an
/// unvalidated reference cannot be constructed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityRef {
    capability_id: CapabilityId,
    revision: u64,
}

impl CapabilityRef {
    pub fn new(capability_id: impl Into<String>, revision: u64) -> Result<Self, ContractError> {
        let capability_id = CapabilityId::from_string(capability_id);
        if capability_id.as_str().trim().is_empty() {
            return Err(ContractError::InvalidRef {
                reason: "capability id cannot be empty".into(),
            });
        }
        if revision == 0 {
            return Err(ContractError::InvalidRef {
                reason: "capability revision must be >= 1".into(),
            });
        }
        Ok(Self {
            capability_id,
            revision,
        })
    }

    pub fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// A policy access dimension a security-class capability may project into.
///
/// The projection is a catalog-owned, versioned property (ADR-0013 §2). It is
/// never inferred from [`MatcherKind`], [`SecurityClass`], an identifier name or
/// a polarity, and a capability without an explicit projection contributes
/// nothing to a policy `RequiredAccess` floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolicyAccessDimension {
    FileOperationClass,
    ToolCommandSet,
    NetworkEndpoint,
    EnvironmentName,
}

impl PolicyAccessDimension {
    /// Stable canonical spelling. A change is a new canonical version.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FileOperationClass => "FILE_OPERATION_CLASS",
            Self::ToolCommandSet => "TOOL_COMMAND_SET",
            Self::NetworkEndpoint => "NETWORK_ENDPOINT",
            Self::EnvironmentName => "ENVIRONMENT_NAME",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "FILE_OPERATION_CLASS" => Some(Self::FileOperationClass),
            "TOOL_COMMAND_SET" => Some(Self::ToolCommandSet),
            "NETWORK_ENDPOINT" => Some(Self::NetworkEndpoint),
            "ENVIRONMENT_NAME" => Some(Self::EnvironmentName),
            _ => None,
        }
    }
}

/// A formal file operation class. This is a contract-kernel value; it is not a
/// filesystem containment proof by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileOperationClass {
    Read,
    Write,
    Execute,
    Delete,
}

impl FileOperationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "READ",
            Self::Write => "WRITE",
            Self::Execute => "EXECUTE",
            Self::Delete => "DELETE",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "READ" => Some(Self::Read),
            "WRITE" => Some(Self::Write),
            "EXECUTE" => Some(Self::Execute),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// Transport selector of a finite network endpoint rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NetworkTransport {
    Any,
    Tcp,
    Udp,
}

impl NetworkTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "ANY",
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ANY" => Some(Self::Any),
            "TCP" => Some(Self::Tcp),
            "UDP" => Some(Self::Udp),
            _ => None,
        }
    }
}

/// Longest exact network host token in bytes. A DNS name is bounded at 253
/// bytes, which also bounds every v1 IPv4/IPv6 textual literal.
pub const MAX_NETWORK_HOST_LEN: usize = 253;

/// Maximum number of distinct endpoint rules in one normalized policy set or
/// required-access set. The union-containment algorithm enumerates a finite
/// host/port partition, so an unbounded set would make admission cost
/// attacker-controlled. The bound applies to the *distinct* rules that survive
/// deduplication, which is exactly the input size of that algorithm.
pub const MAX_NETWORK_ENDPOINTS: usize = 256;

/// Validate the v1 exact-host or single-wildcard grammar.
///
/// `*` is the only wildcard token. Every other host is a bounded ASCII token:
/// 1 to [`MAX_NETWORK_HOST_LEN`] bytes from `[A-Za-z0-9._-]`, with at least one
/// alphanumeric byte. A token containing `:` MUST parse as an IPv6 literal, so a
/// `host:port` fragment is rejected and the port can only be a separate
/// selector. The accepted alphabet covers DNS names and textual IPv4 / IPv6
/// literals without admitting paths, URLs, quotes, whitespace, NUL or any other
/// control byte. Case is preserved; the exact host is an opaque authorization
/// token, not a DNS lookup key.
fn validate_network_host(host: &str) -> Result<(), ContractError> {
    if host == "*" {
        return Ok(());
    }
    if host.is_empty() || host.len() > MAX_NETWORK_HOST_LEN {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: format!(
                "network endpoint host must be 1..={MAX_NETWORK_HOST_LEN} bytes, got {}",
                host.len()
            ),
        });
    }
    if !host.is_ascii() {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "network endpoint host must be ASCII".into(),
        });
    }
    if host.trim() != host {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "network endpoint host must not have leading or trailing whitespace".into(),
        });
    }
    if host.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "network endpoint host must not contain NUL or control characters".into(),
        });
    }
    if host.contains('*') {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "the only valid network host wildcard is the exact token \"*\"".into(),
        });
    }
    if host.contains(':') {
        // `:` is reserved for textual IPv6 literals. A `host:port` string is a
        // URL fragment, not an exact host; the port is a separate selector.
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: format!(
                    "network endpoint host {host:?} contains ':' but is not a valid IPv6 literal"
                ),
            });
        }
    } else if !host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "network endpoint host contains a character outside [A-Za-z0-9._-]".into(),
        });
    }
    if !host.bytes().any(|byte| byte.is_ascii_alphanumeric()) {
        return Err(ContractError::InvalidSandboxPolicy {
            reason: "network endpoint host must contain at least one alphanumeric byte".into(),
        });
    }
    Ok(())
}

/// A finite network endpoint rule: exact host (or `*` for any host), an optional
/// port and a transport selector. B5.1 does not implement regular expressions or
/// arbitrary matching languages.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NetworkEndpoint {
    transport: NetworkTransport,
    host: String,
    port: Option<u16>,
}

impl NetworkEndpoint {
    pub fn new(
        transport: NetworkTransport,
        host: impl Into<String>,
        port: Option<u16>,
    ) -> Result<Self, ContractError> {
        let host = host.into();
        validate_network_host(&host)?;
        if port == Some(0) {
            return Err(ContractError::InvalidSandboxPolicy {
                reason: "network endpoint port 0 is not a valid endpoint selector".into(),
            });
        }
        Ok(Self {
            transport,
            host,
            port,
        })
    }

    pub fn transport(&self) -> NetworkTransport {
        self.transport
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Whether this rule covers `other` (rule subsumption).
    ///
    /// A transport selector of [`NetworkTransport::Any`] covers both concrete
    /// transports, a host of `*` covers every host, and an omitted port covers
    /// every port. This is the only endpoint algebra B5.1 has: there is no
    /// regular-expression or arbitrary matching language.
    pub fn covers(&self, other: &Self) -> bool {
        let transport_covers =
            self.transport == NetworkTransport::Any || self.transport == other.transport;
        let host_covers = self.host == "*" || self.host == other.host;
        let port_covers = match (self.port, other.port) {
            (None, _) => true,
            (Some(left), Some(right)) => left == right,
            (Some(_), None) => false,
        };
        transport_covers && host_covers && port_covers
    }
}

/// The operation a capability member denotes in its declared dimension.
///
/// The variant MUST agree with [`PolicyProjection::dimension`]; a mismatch makes
/// the catalog entry invalid and admission MUST reject rather than drop it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyOperation {
    FileOperation(FileOperationClass),
    ToolCommand(String),
    NetworkEndpoint(NetworkEndpoint),
    EnvironmentName(String),
}

impl PolicyOperation {
    pub fn dimension(&self) -> PolicyAccessDimension {
        match self {
            Self::FileOperation(_) => PolicyAccessDimension::FileOperationClass,
            Self::ToolCommand(_) => PolicyAccessDimension::ToolCommandSet,
            Self::NetworkEndpoint(_) => PolicyAccessDimension::NetworkEndpoint,
            Self::EnvironmentName(_) => PolicyAccessDimension::EnvironmentName,
        }
    }
}

fn validate_operation_token(value: &str, what: &str) -> Result<(), ContractError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(ContractError::InvalidPolicyProjection {
            reason: format!("{what} must be a non-empty trimmed string"),
        });
    }
    Ok(())
}

/// An explicit, normative capability-to-policy projection.
///
/// Only a `Set` capability whose `polarity` is [`CapabilityPolarity::Ability`]
/// and whose `security_class` is `Authority`/`Sandbox` may carry one. Every
/// member of a projecting value MUST be present in `member_map`; an unmapped
/// member fails closed and is never silently dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyProjection {
    dimension: PolicyAccessDimension,
    member_map: BTreeMap<String, PolicyOperation>,
}

impl PolicyProjection {
    pub fn new(
        dimension: PolicyAccessDimension,
        member_map: BTreeMap<String, PolicyOperation>,
    ) -> Result<Self, ContractError> {
        for (member, operation) in &member_map {
            validate_operation_token(member, "policy member name")?;
            if operation.dimension() != dimension {
                return Err(ContractError::InvalidPolicyProjection {
                    reason: format!(
                        "policy member {member:?} maps to a {} operation, not {}",
                        operation.dimension().as_str(),
                        dimension.as_str()
                    ),
                });
            }
            match operation {
                PolicyOperation::ToolCommand(name) | PolicyOperation::EnvironmentName(name) => {
                    validate_operation_token(name, "policy operation name")?;
                }
                PolicyOperation::FileOperation(_) | PolicyOperation::NetworkEndpoint(_) => {}
            }
        }
        Ok(Self {
            dimension,
            member_map,
        })
    }

    pub fn dimension(&self) -> PolicyAccessDimension {
        self.dimension
    }

    pub fn member_map(&self) -> &BTreeMap<String, PolicyOperation> {
        &self.member_map
    }

    /// The operation for an exact member; `None` means the member is unmapped and
    /// projection MUST fail closed for the whole capability value.
    pub fn operation_for(&self, member: &str) -> Option<&PolicyOperation> {
        self.member_map.get(member)
    }
}

/// The canonical capability-document versions.
///
/// Version 1 is the frozen encoding with no projection; version 2 is the
/// projecting encoding. A reader MUST select the version recorded with the row
/// and MUST NOT reinterpret a v1 document as v2.
pub const CAPABILITY_DEFINITION_CANONICAL_VERSION_V1: u32 = 1;
pub const CAPABILITY_DEFINITION_CANONICAL_VERSION_V2: u32 = 2;

/// The canonical semantics of one exact capability revision.
///
/// `security_class` (does it need imported evidence?) and `polarity` (which
/// direction narrows?) are independent: a Sandbox-domain capability can still be
/// ability-shaped (e.g. an allowed-command set), and both facts are catalog
/// owned.
///
/// `policy_projection` is an additive, versioned catalog property introduced with
/// the B5.1 schema. A definition carrying one is encoded under the capability
/// canonical v2 document; a definition without one keeps the frozen v1 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityDefinition {
    pub matcher_kind: MatcherKind,
    pub security_class: SecurityClass,
    pub polarity: CapabilityPolarity,
    pub policy_projection: Option<PolicyProjection>,
}

impl CapabilityDefinition {
    /// The whole-definition invariant for a policy projection (ADR-0013 §2).
    ///
    /// The durable boundary re-runs this on read: a stored projection that does
    /// not satisfy it is catalog corruption, not a caller mistake.
    pub fn validate_projection(&self) -> Result<(), ContractError> {
        let Some(projection) = &self.policy_projection else {
            return Ok(());
        };
        if self.matcher_kind != MatcherKind::Set {
            return Err(ContractError::InvalidPolicyProjection {
                reason: "only a Set capability may carry a policy projection".into(),
            });
        }
        if self.polarity != CapabilityPolarity::Ability {
            return Err(ContractError::InvalidPolicyProjection {
                reason: "a Restriction never projects into RequiredAccess".into(),
            });
        }
        if !matches!(
            self.security_class,
            SecurityClass::Authority | SecurityClass::Sandbox
        ) {
            return Err(ContractError::InvalidPolicyProjection {
                reason: "only Authority/Sandbox capabilities may project".into(),
            });
        }
        // `PolicyProjection::new` already validated every member. Re-validate the
        // shape here so a hand-built value cannot bypass the constructor.
        PolicyProjection::new(projection.dimension, projection.member_map.clone())?;
        Ok(())
    }

    /// The canonical capability-document version this definition requires.
    ///
    /// Version 1 is the frozen encoding with no projection; version 2 is the
    /// projecting encoding. A reader MUST select the version recorded with the
    /// row and MUST NOT reinterpret a v1 document as v2.
    pub fn canonical_version(&self) -> u32 {
        if self.policy_projection.is_some() {
            2
        } else {
            1
        }
    }
}

/// Canonical, single-authority set of capability definitions.
#[derive(Clone, Debug, Default)]
pub struct CapabilityCatalog {
    definitions: BTreeMap<CapabilityRef, CapabilityDefinition>,
}

impl CapabilityCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Define a capability revision with no policy projection.
    ///
    /// Redefining a revision with different semantics is rejected, so a security
    /// class can never be redefined per AgentType.
    pub fn define(
        &mut self,
        reference: CapabilityRef,
        matcher_kind: MatcherKind,
        security_class: SecurityClass,
        polarity: CapabilityPolarity,
    ) -> Result<(), ContractError> {
        self.define_with_projection(reference, matcher_kind, security_class, polarity, None)
    }

    /// Define a capability revision with an optional explicit policy projection.
    ///
    /// The projection validity condition is checked before insertion; an invalid
    /// projection is caller error here and catalog corruption at the durable
    /// read boundary.
    pub fn define_with_projection(
        &mut self,
        reference: CapabilityRef,
        matcher_kind: MatcherKind,
        security_class: SecurityClass,
        polarity: CapabilityPolarity,
        policy_projection: Option<PolicyProjection>,
    ) -> Result<(), ContractError> {
        let definition = CapabilityDefinition {
            matcher_kind,
            security_class,
            polarity,
            policy_projection,
        };
        definition.validate_projection()?;
        match self.definitions.get(&reference) {
            Some(existing) if existing == &definition => Ok(()),
            Some(_) => Err(ContractError::CapabilityDefinitionConflict {
                capability: reference.capability_id().as_str().to_string(),
                revision: reference.revision(),
            }),
            None => {
                self.definitions.insert(reference, definition);
                Ok(())
            }
        }
    }

    pub fn get(&self, reference: &CapabilityRef) -> Option<&CapabilityDefinition> {
        self.definitions.get(reference)
    }

    pub fn contains(&self, reference: &CapabilityRef) -> bool {
        self.definitions.contains_key(reference)
    }
}

/// A source's (or config's) declaration about one exact capability revision.
///
/// `declaration_provenance_ref` is diagnostic metadata only. It MUST NOT
/// satisfy `SecurityClass::requires_evidence()`: only imported
/// `ResolvedProvisioningEvidence` does.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityClaim {
    pub reference: CapabilityRef,
    pub value: CapabilityValue,
    pub assurance: Assurance,
    pub declaration_provenance_ref: Option<String>,
}

/// Decide whether `provided` satisfies `required` under `matcher`.
pub fn value_satisfies(
    matcher: MatcherKind,
    required: &CapabilityValue,
    provided: &CapabilityValue,
) -> bool {
    match (matcher, required, provided) {
        (MatcherKind::Bool, CapabilityValue::Bool(r), CapabilityValue::Bool(p)) => !*r || *p,
        (MatcherKind::Set, CapabilityValue::Set(r), CapabilityValue::Set(p)) => r.is_subset(p),
        (
            MatcherKind::Ordered,
            CapabilityValue::Ordered {
                class: rc,
                rank: rr,
            },
            CapabilityValue::Ordered {
                class: pc,
                rank: pr,
            },
        ) => rc == pc && pr >= rr,
        (MatcherKind::Quantity, CapabilityValue::Quantity(r), CapabilityValue::Quantity(p)) => {
            p.get() >= r.get()
        }
        (MatcherKind::Exact, r, p) => r == p,
        _ => false,
    }
}

/// Whether `value` stays within a `ceiling` (used to keep config/source
/// declarations from exceeding a SpawnSource's provisionable envelope).
pub fn value_within(ceiling: &CapabilityValue, value: &CapabilityValue) -> bool {
    match (ceiling, value) {
        (CapabilityValue::Bool(a), CapabilityValue::Bool(b)) => !*b || *a,
        (CapabilityValue::Set(a), CapabilityValue::Set(b)) => b.is_subset(a),
        (
            CapabilityValue::Ordered {
                class: ac,
                rank: ar,
            },
            CapabilityValue::Ordered {
                class: bc,
                rank: br,
            },
        ) => ac == bc && br <= ar,
        (CapabilityValue::Quantity(a), CapabilityValue::Quantity(b)) => b.get() <= a.get(),
        (CapabilityValue::Exact(a), CapabilityValue::Exact(b)) => a == b,
        _ => false,
    }
}

/// Deterministic join of two *requirements* on the same exact capability
/// revision: the result is the stronger requirement, and `None` means the two
/// values are incompatible (e.g. disjoint `Exact` values or differing `Ordered`
/// classes).
///
/// This is an internal primitive shared by the B.1 restriction composition and
/// later folds. It is deliberately **not** exported: the public capability
/// intersection API is not frozen until B.5 (`D-SANDBOX-INTERSECTION`).
pub(crate) fn join_requirement_values(
    matcher: MatcherKind,
    a: &CapabilityValue,
    b: &CapabilityValue,
) -> Option<CapabilityValue> {
    match (matcher, a, b) {
        (MatcherKind::Bool, CapabilityValue::Bool(x), CapabilityValue::Bool(y)) => {
            Some(CapabilityValue::Bool(*x || *y))
        }
        (MatcherKind::Set, CapabilityValue::Set(x), CapabilityValue::Set(y)) => {
            Some(CapabilityValue::Set(x.union(y).cloned().collect()))
        }
        (
            MatcherKind::Ordered,
            CapabilityValue::Ordered {
                class: ac,
                rank: ar,
            },
            CapabilityValue::Ordered {
                class: bc,
                rank: br,
            },
        ) if ac == bc => Some(CapabilityValue::Ordered {
            class: ac.clone(),
            rank: (*ar).max(*br),
        }),
        (MatcherKind::Quantity, CapabilityValue::Quantity(x), CapabilityValue::Quantity(y)) => {
            Some(CapabilityValue::Quantity(if x.get() >= y.get() {
                *x
            } else {
                *y
            }))
        }
        (MatcherKind::Exact, x, y) if x == y => Some(x.clone()),
        _ => None,
    }
}

// NOTE: there is deliberately no `assurance_satisfies` helper. `Assurance` is
// declaration metadata only; no public API may express "a claim label satisfies
// a security class". Only imported `ResolvedProvisioningEvidence` does.
