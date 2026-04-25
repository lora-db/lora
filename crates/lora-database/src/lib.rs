//! In-memory Lora database — the database-facing orchestration layer.
//!
//! `lora-database` owns the parse → analyze → compile → execute pipeline
//! and exposes a single [`Database`] entry point that transports (HTTP,
//! benches, examples, embedded callers) can drive without knowing about the
//! underlying crates.
//!
//! # Quick start
//!
//! ```no_run
//! use lora_database::Database;
//!
//! let db = Database::in_memory();
//! db.execute("CREATE (:User {name: 'alice'})", None).unwrap();
//! ```

mod database;

pub use database::{Database, QueryRunner, SnapshotAdmin, WalAdmin, WalStatus};

// Re-export the WAL configuration types so transports / operators can
// build a `Database::open_with_wal` argument without taking a direct
// `lora-wal` dependency.
pub use lora_wal::{SyncMode, WalConfig};

// Re-export the core execution types so callers don't need a direct
// dependency on `lora-executor`.
pub use lora_executor::{ExecuteOptions, LoraValue, QueryResult, ResultFormat};

// Re-export the default in-memory backing store so callers only need to
// depend on `lora-database` for the happy path.
pub use lora_store::InMemoryGraph;

// Snapshot surface — re-exported so bindings/servers don't need a direct
// `lora-store` dependency just to name the meta / error types.
pub use lora_store::{SnapshotError, SnapshotMeta, Snapshotable};

// Standalone parsing entry point (does not require building a `Database`).
pub use lora_parser::parse_query;
