//! SQLite WAL authority for the Agentype M4 correctness kernel.
//!
//! Schema and transaction boundaries are derived from
//! docs/specs/v0.2/13-storage-and-transactions.md. This crate MUST NOT
//! introduce Generation, AgentType, or vendor semantics.
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

mod kernel;
mod schema;
mod store;
pub mod txutil;

pub use kernel::{
    CurrentAuthorityHint, ExecutionReconciliationSnapshot, ExecutionRoutingFacts, Kernel,
    LeaseSupervisionView, OutboxDeliveryCandidate, OutboxDeliverySnapshot, RunningAuthorityGrant,
    SupervisedRenewal,
};
pub use schema::SCHEMA_VERSION;
pub use store::IMPLEMENTATION_LINE;
