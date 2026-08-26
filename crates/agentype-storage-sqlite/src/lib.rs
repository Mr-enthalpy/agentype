//! SQLite WAL authority for the Agentype M4 correctness kernel.
//!
//! Schema and transaction boundaries are derived from
//! docs/specs/v0.2/13-storage-and-transactions.md. This crate MUST NOT
//! introduce Generation, AgentType, or vendor semantics.

mod kernel;
mod schema;
mod store;
mod txutil;

pub use kernel::Kernel;
pub use schema::SCHEMA_VERSION;
