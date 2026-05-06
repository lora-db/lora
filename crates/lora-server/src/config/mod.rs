//! Runtime configuration for `lora-server`.
//!
//! Resolves the bind address (`host` + `port`) from, in order of precedence:
//!
//! 1. CLI flags: `--host <HOST>`, `--port <PORT>` (also accepts `--host=<HOST>`).
//! 2. Environment variables: `LORA_SERVER_HOST`, `LORA_SERVER_PORT`.
//! 3. Built-in defaults: `127.0.0.1:4747`.
//!
//! The default HTTP port for the local LoraDB server is `4747` — short,
//! memorable, and outside the most common local development ports
//! (3000/4000/5000/8000/8080/8443/9000) and standard database ports
//! (Postgres 5432, Redis 6379, MongoDB 27017, Elasticsearch 9200, MySQL
//! 3306) so it does not collide with typical side projects.
//!
//! The parser also understands `--help` / `--version`, which return a
//! [`ConfigOutcome`] variant instead of a [`ServerConfig`] so the binary
//! can print and exit before booting the runtime.
//!
//! Layout:
//! - `errors` — [`ConfigError`] and its conversions.
//! - `env` — [`EnvInputs`], `resolve`, `resolve_from_process` and the
//!   per-flag parse helpers.
//! - `help` — `--help` and `--version` static text.
//! - `tests` — unit tests for the resolution logic.

mod env;
mod errors;
mod help;

#[cfg(test)]
mod tests;

use lora_database::SyncMode;

pub use env::{resolve, resolve_from_process, EnvInputs};
pub use errors::ConfigError;
pub use help::{help_text, version_text};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 4747;
pub const HOST_ENV: &str = "LORA_SERVER_HOST";
pub const PORT_ENV: &str = "LORA_SERVER_PORT";
pub const SNAPSHOT_PATH_ENV: &str = "LORA_SERVER_SNAPSHOT_PATH";
pub const WAL_DIR_ENV: &str = "LORA_SERVER_WAL_DIR";
pub const WAL_SYNC_MODE_ENV: &str = "LORA_SERVER_WAL_SYNC_MODE";

/// Default segment target for WAL-enabled deployments. Matches the
/// in-tree `WalConfig::enabled` constructor.
pub const DEFAULT_WAL_SEGMENT_TARGET_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// When set, the server mounts the `/admin/snapshot/{save,load}` routes
    /// and wires them to this path. `None` means the admin surface is
    /// disabled entirely — the default, so we never expose admin endpoints
    /// on a network-reachable process unless the operator asks for it.
    pub snapshot_path: Option<std::path::PathBuf>,
    /// When `Some`, the server restores the graph from this path at boot.
    /// Missing file at boot is treated as an empty graph (same as without
    /// `--restore-from`). Independent of `snapshot_path` so operators can
    /// restore from a read-only location and write back somewhere else.
    pub restore_from: Option<std::path::PathBuf>,
    /// When `Some`, the server attaches a WAL at this directory and
    /// brackets every query with begin/commit/abort. Also unlocks the
    /// `/admin/checkpoint`, `/admin/wal/status`, and
    /// `/admin/wal/truncate` admin routes (only when `snapshot_path`
    /// is also configured).
    pub wal_dir: Option<std::path::PathBuf>,
    /// Durability cadence for the WAL. Ignored when `wal_dir` is `None`.
    pub wal_sync_mode: SyncMode,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            snapshot_path: None,
            restore_from: None,
            wal_dir: None,
            wal_sync_mode: SyncMode::default(),
        }
    }
}

impl ServerConfig {
    pub fn bind_addr(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigOutcome {
    Run(ServerConfig),
    Help(String),
    Version(String),
}
