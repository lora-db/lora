//! WAL surface for the database: the storage-agnostic admin trait,
//! the platform-portable `.loradb` archive backend, and the
//! per-query-scope guard used to bracket arm/commit/abort.

pub(crate) mod admin;
pub(crate) mod archive;
pub(crate) mod write_scope;

pub use admin::{WalAdmin, WalStatus};
