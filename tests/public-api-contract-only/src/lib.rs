//! Contract-only supported-surface fixture.
//!
//! This package proves the half of the imported-enforcement boundary that the
//! `agentype-public-api-boundary` fixture cannot: a host that depends on
//! `agentype-agent-contract` **alone** keeps the default feature set, so
//! `ImportedEnforcementProfile::import` / `from_parts` are not compiled in.
//!
//! The probes are fully typed calls, not bare function-item references. A bare
//! `let _ = ImportedEnforcementProfile::import;` fails with `E0283` ("type
//! annotations needed" for the `impl IntoIterator` argument) whether or not the
//! method exists, so it cannot distinguish "absent" from "generic". Naming every
//! argument type and pinning the expected error to `E0599` ("no such associated
//! item") makes the probe fail for the stated reason.
//!
//! This is a supported-surface convenience, not an unforgeable capability. Any
//! host that also embeds `agentype-runtime` gets the constructor through Cargo
//! feature unification; the load-bearing boundary is that no production API
//! accepts a caller-supplied profile and the authority transaction re-proves
//! imported facts. See `tests/public-api/src/lib.rs` for the runtime-side probe
//! that records the constructor as visible.

/// An imported enforcement profile is still a nameable, consumable type without
/// the producer feature: a contract-only consumer can read one it was handed.
///
/// ```no_run
/// use agentype_agent_contract::{EnforcementSubject, ImportedEnforcementProfile};
///
/// fn consume(profile: &ImportedEnforcementProfile) -> (String, bool) {
///     (
///         profile.reference().profile().to_string(),
///         profile.covers_subject(EnforcementSubject::ControlledPrimaryProcess),
///     )
/// }
/// ```
pub fn imported_enforcement_profile_is_consumable() {}

/// The supported contract-only surface has no way to mint imported enforcement
/// evidence: both constructors are behind `provisioning-producer`.
///
/// ```compile_fail,E0599
/// use agentype_agent_contract::{
///     EnforcementFeature, EnforcementProfileRef, EnforcementSubject, EnforcementSupport,
///     ImportedEnforcementProfile,
/// };
///
/// fn _no_profile_import() {
///     let _ = ImportedEnforcementProfile::import(
///         EnforcementProfileRef::new("contract-only", "contract-only", 1).unwrap(),
///         1,
///         EnforcementSupport::new([EnforcementFeature::WorkspaceWriteConfinement]),
///         [EnforcementSubject::ControlledPrimaryProcess],
///     );
/// }
/// ```
///
/// ```compile_fail,E0599
/// use agentype_agent_contract::{
///     EnforcementFeature, EnforcementProfileRef, EnforcementSubject, EnforcementSupport,
///     ImportedEnforcementProfile,
/// };
///
/// fn _no_profile_from_parts() {
///     let _ = ImportedEnforcementProfile::from_parts(
///         EnforcementProfileRef::new("contract-only", "contract-only", 1).unwrap(),
///         1,
///         EnforcementSupport::new([EnforcementFeature::WorkspaceWriteConfinement]),
///         [EnforcementSubject::ControlledPrimaryProcess],
///         "sha256:0000000000000000000000000000000000000000000000000000000000000000",
///     );
/// }
/// ```
pub fn imported_enforcement_constructors_are_absent() {}
