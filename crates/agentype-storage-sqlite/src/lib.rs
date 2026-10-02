//! SQLite WAL authority for the Agentype M4 correctness kernel.
//!
//! Schema and transaction boundaries are derived from
//! docs/specs/v0.2/13-storage-and-transactions.md. This crate persists M6-A
//! Generation/proposal commitments and the M6-B.2 Agent Contract catalog, but
//! it MUST NOT introduce vendor/model semantics: AgentType, SpawnSource, and
//! SourceConfig content is stored as canonical M6-B JSON with Core-computed
//! content digests, and opaque SourceConfig bodies are never interpreted.
//!
//! # Internal implementation crate
//!
//! This crate is an **internal implementation crate** (`publish = false`).
//! The supported Agentype production API is `agentype-runtime`:
//! `SchedulerDaemonBuilder` → `RunningSchedulerDaemon` → `SchedulerControl`.
//!
//! Direct `Kernel` use is not a supported Agentype production API and does
//! not constitute a Scheduler runtime composition plane. `Kernel` exposes
//! claim, execution-commit, renewal, and recovery primitives that belong to
//! the Runtime's mechanical path; reaching them means depending on a
//! non-published implementation crate rather than on the supported surface.
//! `SchedulerDaemon` is the sole supported production composition root.

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

pub mod catalog;
pub mod frontier;
mod kernel;
mod schema;
mod store;
pub mod txutil;

pub use catalog::{AgentTypeStatus, ConfigMode, DurableAgentTypeLookup, SourceConfigBody};
pub use kernel::{
    CurrentAuthorityHint, ExecutionReconciliationSnapshot, ExecutionRoutingFacts, Kernel,
    LeaseSupervisionView, OutboxDeliveryCandidate, OutboxDeliverySnapshot, RunningAuthorityGrant,
    SupervisedRenewal,
};
pub use schema::{SCHEMA_SQL, SCHEMA_VERSION};
// Shared with `agentype-runtime`: the process lock resolves store identity
// from a filesystem handle while SQLite resolves the same string with its own
// special-filename rules, so both crates classify store paths with one table.
pub use store::{classify_store_path, StorePathKind, IMPLEMENTATION_LINE};
