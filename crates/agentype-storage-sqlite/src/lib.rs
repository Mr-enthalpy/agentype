//! SQLite WAL authority for the Agentype M4 correctness kernel.
//!
//! Schema and transaction boundaries are derived from
//! docs/specs/v0.2/13-storage-and-transactions.md. This crate MUST NOT
//! introduce Generation, AgentType, or vendor semantics.

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

mod kernel;
mod schema;
mod store;
pub mod txutil;

pub use kernel::{
    CurrentAuthorityHint, ExecutionReconciliationSnapshot, Kernel, LeaseSupervisionView,
    OutboxDeliveryCandidate, OutboxDeliverySnapshot, RunningAuthorityGrant, SupervisedRenewal,
};
pub use schema::SCHEMA_VERSION;
pub use store::IMPLEMENTATION_LINE;
