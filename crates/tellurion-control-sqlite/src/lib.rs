//! SQLite-backed persistence for Tellurion's dynamic control plane.
//!
//! The adapter opens short-lived configured connections and runs all blocking
//! SQLite work on Tokio's blocking pool. Mutations use `BEGIN IMMEDIATE`, so a
//! deployment has one writer at a time while readers continue under WAL.

mod schema;
mod store;

pub use store::SqliteControlStore;
