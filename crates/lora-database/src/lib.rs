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
mod durable_io;
mod error;
mod explain;
mod live_store;
mod named;
mod plan_cache;
mod snapshot;
mod stream;
mod transaction;
mod wal;

pub use database::{Database, GraphDirection, QueryRunner};
pub use error::{LoraError, LoraErrorCategory, LoraErrorCode};
pub use explain::{OperatorMetrics, PlanShape, ProfileMetrics, QueryPlan, QueryProfile};
pub use lora_compiler::{PlanTree, PlanTreeNode};
pub use named::{
    resolve_database_path, DatabaseName, DatabaseNameError, DatabaseOpenOptions,
    DEFAULT_DATABASE_MAX_BYTES,
};
pub use snapshot::{
    snapshot_credentials_from_json, snapshot_options_from_json, SnapshotAdmin, SnapshotByteFormat,
    SnapshotConfig,
};
pub use stream::QueryStream;
pub use transaction::{Transaction, TransactionError, TransactionMode};
pub use wal::{WalAdmin, WalStatus};

// Re-export the WAL configuration types so transports / operators can
// build a `Database::open_with_wal` argument without taking a direct
// `lora-wal` dependency.
pub use lora_wal::{SyncMode, WalConfig};

// Re-export the core execution types so callers don't need a direct
// dependency on `lora-executor`.
pub use lora_executor::{ExecuteOptions, LoraValue, QueryResult, ResultFormat, Row};

// Re-export the default in-memory backing store so callers only need to
// depend on `lora-database` for the happy path.
pub use lora_store::InMemoryGraph;

// Snapshot surface — re-exported so bindings/servers don't need a direct
// `lora-store` dependency just to name the meta / error types.
pub use lora_snapshot::{
    Compression, EncryptionKey, PasswordKdfParams, SnapshotCredentials, SnapshotEncryption,
    SnapshotInfo, SnapshotOptions, SnapshotPassword, DATABASE_SNAPSHOT_MAGIC,
};
pub use lora_store::{
    NodeId, NodeRecord, RelationshipId, RelationshipRecord, SnapshotError, SnapshotMeta,
};

// Standalone parsing entry point (does not require building a `Database`).
pub use lora_parser::parse_query;
