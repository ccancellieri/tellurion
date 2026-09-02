//! PostgreSQL-backed persistence for Tellurion's dynamic control plane.
//!
//! Correctness depends only on transactions, row locks, durable revisions,
//! and keyset polling. The adapter never executes `LISTEN`; every operation
//! may therefore use a different pooled or Pgpool-routed database session.

mod schema;
mod store;

pub use store::PostgresControlStore;
