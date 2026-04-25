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

use std::fmt;
use std::num::ParseIntError;

use lora_database::SyncMode;

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
            wal_sync_mode: SyncMode::PerCommit,
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

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    UnknownArg(String),
    MissingValue(&'static str),
    EmptyValue(&'static str),
    InvalidPort { value: String, reason: String },
    InvalidSyncMode(String),
    UnexpectedPositional(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnknownArg(a) => write!(f, "unknown argument: {a}"),
            ConfigError::MissingValue(flag) => write!(f, "missing value for {flag}"),
            ConfigError::EmptyValue(flag) => write!(f, "{flag} value must not be empty"),
            ConfigError::InvalidPort { value, reason } => {
                write!(f, "invalid port '{value}': {reason}")
            }
            ConfigError::InvalidSyncMode(value) => {
                write!(
                    f,
                    "invalid --wal-sync-mode '{value}': expected per-commit, group, or none"
                )
            }
            ConfigError::UnexpectedPositional(a) => {
                write!(f, "unexpected positional argument: {a}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ParseIntError> for ConfigError {
    fn from(_: ParseIntError) -> Self {
        // Placeholder; we always build InvalidPort manually with the offending
        // string so the user sees what they typed.
        ConfigError::InvalidPort {
            value: String::new(),
            reason: "not a valid u16".into(),
        }
    }
}

pub fn help_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "lora-server {version} — HTTP server for the Lora in-memory graph database

USAGE:
    lora-server [OPTIONS]

OPTIONS:
        --host <HOST>              Bind address. Default: {DEFAULT_HOST} (or ${HOST_ENV} if set).
        --port <PORT>              TCP port.      Default: {DEFAULT_PORT} (or ${PORT_ENV} if set).
        --snapshot-path <PATH>     Enable the snapshot admin surface. Mounts
                                   POST /admin/snapshot/save and
                                   POST /admin/snapshot/load against this file.
                                   Also acts as the default target for
                                   POST /admin/checkpoint when --wal-dir is set.
                                   Also read from ${SNAPSHOT_PATH_ENV}.
        --restore-from <PATH>      Restore the graph from this snapshot at boot.
                                   Missing file is treated as empty. When
                                   --wal-dir is also set, the WAL is replayed
                                   on top of the snapshot.
        --wal-dir <DIR>            Attach a write-ahead log at this directory.
                                   Every mutating query is bracketed by
                                   begin/commit; a crashed process recovers
                                   committed writes on next boot. Read-only
                                   queries do not touch the WAL.
                                   Also enables the WAL admin routes
                                   (POST /admin/wal/status,
                                    POST /admin/wal/truncate,
                                    POST /admin/checkpoint) — independent of
                                   --snapshot-path. /admin/checkpoint requires
                                   `path` in the request body when no
                                   --snapshot-path default is configured.
                                   Also read from ${WAL_DIR_ENV}.
        --wal-sync-mode <MODE>     WAL durability cadence. One of:
                                   per-commit  fsync before each commit returns (default).
                                   group       buffer commits, fsync periodically.
                                   none        no fsync; rely on OS / external durability.
                                   Also read from ${WAL_SYNC_MODE_ENV}.
        --help                     Print this help and exit.
        --version                  Print version and exit.

ENVIRONMENT:
    {HOST_ENV}            Bind address (overridden by --host).
    {PORT_ENV}            TCP port      (overridden by --port).
    {SNAPSHOT_PATH_ENV}   Path used by --snapshot-path.
    {WAL_DIR_ENV}         Directory used by --wal-dir.
    {WAL_SYNC_MODE_ENV}   Mode used by --wal-sync-mode.

EXAMPLES:
    lora-server
    lora-server --host 0.0.0.0 --port 8080
    lora-server --snapshot-path /var/lib/lora/graph.bin
    lora-server --wal-dir /var/lib/lora/wal --snapshot-path /var/lib/lora/graph.bin \\
                --restore-from /var/lib/lora/graph.bin
"
    )
}

pub fn version_text() -> String {
    format!("lora-server {}", env!("CARGO_PKG_VERSION"))
}

/// Inputs to [`resolve`]. Wrapping the env values in a struct keeps the
/// caller-visible signature stable as new env-driven options are added.
#[derive(Debug, Default, Clone)]
pub struct EnvInputs {
    pub host: Option<String>,
    pub port: Option<String>,
    pub snapshot_path: Option<String>,
    pub wal_dir: Option<String>,
    pub wal_sync_mode: Option<String>,
}

/// Resolve a [`ConfigOutcome`] from CLI args and env values.
///
/// `args` includes the program name at position 0 (as produced by
/// [`std::env::args`]); it is skipped internally.
pub fn resolve<I>(args: I, env: EnvInputs) -> Result<ConfigOutcome, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let _program = iter.next();

    let mut cli_host: Option<String> = None;
    let mut cli_port: Option<String> = None;
    let mut cli_snapshot_path: Option<String> = None;
    let mut cli_restore_from: Option<String> = None;
    let mut cli_wal_dir: Option<String> = None;
    let mut cli_wal_sync_mode: Option<String> = None;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" => return Ok(ConfigOutcome::Help(help_text())),
            "--version" => return Ok(ConfigOutcome::Version(version_text())),
            "--host" => {
                let v = iter.next().ok_or(ConfigError::MissingValue("--host"))?;
                cli_host = Some(v);
            }
            "--port" => {
                let v = iter.next().ok_or(ConfigError::MissingValue("--port"))?;
                cli_port = Some(v);
            }
            "--snapshot-path" => {
                let v = iter
                    .next()
                    .ok_or(ConfigError::MissingValue("--snapshot-path"))?;
                cli_snapshot_path = Some(v);
            }
            "--restore-from" => {
                let v = iter
                    .next()
                    .ok_or(ConfigError::MissingValue("--restore-from"))?;
                cli_restore_from = Some(v);
            }
            "--wal-dir" => {
                let v = iter.next().ok_or(ConfigError::MissingValue("--wal-dir"))?;
                cli_wal_dir = Some(v);
            }
            "--wal-sync-mode" => {
                let v = iter
                    .next()
                    .ok_or(ConfigError::MissingValue("--wal-sync-mode"))?;
                cli_wal_sync_mode = Some(v);
            }
            s if s.starts_with("--host=") => {
                cli_host = Some(s["--host=".len()..].to_string());
            }
            s if s.starts_with("--port=") => {
                cli_port = Some(s["--port=".len()..].to_string());
            }
            s if s.starts_with("--snapshot-path=") => {
                cli_snapshot_path = Some(s["--snapshot-path=".len()..].to_string());
            }
            s if s.starts_with("--restore-from=") => {
                cli_restore_from = Some(s["--restore-from=".len()..].to_string());
            }
            s if s.starts_with("--wal-dir=") => {
                cli_wal_dir = Some(s["--wal-dir=".len()..].to_string());
            }
            s if s.starts_with("--wal-sync-mode=") => {
                cli_wal_sync_mode = Some(s["--wal-sync-mode=".len()..].to_string());
            }
            s if s.starts_with("--") => return Err(ConfigError::UnknownArg(arg)),
            _ => return Err(ConfigError::UnexpectedPositional(arg)),
        }
    }

    let host = cli_host
        .or(env.host)
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    if host.trim().is_empty() {
        return Err(ConfigError::EmptyValue("--host"));
    }

    let port = match cli_port.or(env.port) {
        Some(raw) => parse_port(&raw)?,
        None => DEFAULT_PORT,
    };

    let snapshot_path = cli_snapshot_path.or(env.snapshot_path).and_then(non_empty_path);
    let restore_from = cli_restore_from.and_then(non_empty_path);
    let wal_dir = cli_wal_dir.or(env.wal_dir).and_then(non_empty_path);
    let wal_sync_mode = match cli_wal_sync_mode.or(env.wal_sync_mode) {
        Some(raw) => parse_sync_mode(&raw)?,
        None => SyncMode::PerCommit,
    };

    Ok(ConfigOutcome::Run(ServerConfig {
        host,
        port,
        snapshot_path,
        restore_from,
        wal_dir,
        wal_sync_mode,
    }))
}

/// Resolve using the process environment and `std::env::args`.
pub fn resolve_from_process() -> Result<ConfigOutcome, ConfigError> {
    resolve(
        std::env::args(),
        EnvInputs {
            host: std::env::var(HOST_ENV).ok(),
            port: std::env::var(PORT_ENV).ok(),
            snapshot_path: std::env::var(SNAPSHOT_PATH_ENV).ok(),
            wal_dir: std::env::var(WAL_DIR_ENV).ok(),
            wal_sync_mode: std::env::var(WAL_SYNC_MODE_ENV).ok(),
        },
    )
}

fn non_empty_path(p: String) -> Option<std::path::PathBuf> {
    if p.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(p))
    }
}

fn parse_port(raw: &str) -> Result<u16, ConfigError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::EmptyValue("--port"));
    }
    trimmed
        .parse::<u16>()
        .map_err(|e| ConfigError::InvalidPort {
            value: raw.to_string(),
            reason: e.to_string(),
        })
}

fn parse_sync_mode(raw: &str) -> Result<SyncMode, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "per-commit" | "per_commit" | "percommit" => Ok(SyncMode::PerCommit),
        "group" => Ok(SyncMode::Group {
            // 50 ms cadence is short enough that a crash window is
            // bounded by the wallclock budget operators usually quote
            // ("at most ~50 ms of writes lost") and long enough that
            // the bg flusher does not tax disks under sustained load.
            interval_ms: 50,
        }),
        "none" | "off" => Ok(SyncMode::None),
        other => Err(ConfigError::InvalidSyncMode(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(xs: &[&str]) -> Vec<String> {
        std::iter::once("lora-server")
            .chain(xs.iter().copied())
            .map(String::from)
            .collect()
    }

    fn run_cfg(out: ConfigOutcome) -> ServerConfig {
        match out {
            ConfigOutcome::Run(c) => c,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn defaults_when_nothing_set() {
        let cfg = run_cfg(resolve(args(&[]), EnvInputs::default()).unwrap());
        assert_eq!(cfg, ServerConfig::default());
    }

    #[test]
    fn env_vars_apply_without_cli() {
        let cfg = run_cfg(
            resolve(
                args(&[]),
                EnvInputs {
                    host: Some("0.0.0.0".into()),
                    port: Some("9000".into()),
                    ..EnvInputs::default()
                },
            )
            .unwrap(),
        );
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 9000);
    }

    #[test]
    fn cli_flags_override_env() {
        let cfg = run_cfg(
            resolve(
                args(&["--host", "10.0.0.1", "--port", "8080"]),
                EnvInputs {
                    host: Some("0.0.0.0".into()),
                    port: Some("9000".into()),
                    ..EnvInputs::default()
                },
            )
            .unwrap(),
        );
        assert_eq!(cfg.host, "10.0.0.1");
        assert_eq!(cfg.port, 8080);
    }

    #[test]
    fn cli_equals_form_works() {
        let cfg = run_cfg(
            resolve(args(&["--host=::1", "--port=7000"]), EnvInputs::default()).unwrap(),
        );
        assert_eq!(cfg.host, "::1");
        assert_eq!(cfg.port, 7000);
    }

    #[test]
    fn snapshot_path_from_cli() {
        let cfg = run_cfg(
            resolve(
                args(&["--snapshot-path", "/tmp/snap.bin"]),
                EnvInputs::default(),
            )
            .unwrap(),
        );
        assert_eq!(
            cfg.snapshot_path,
            Some(std::path::PathBuf::from("/tmp/snap.bin"))
        );
    }

    #[test]
    fn snapshot_path_from_env() {
        let cfg = run_cfg(
            resolve(
                args(&[]),
                EnvInputs {
                    snapshot_path: Some("/var/lora/snap.bin".into()),
                    ..EnvInputs::default()
                },
            )
            .unwrap(),
        );
        assert_eq!(
            cfg.snapshot_path,
            Some(std::path::PathBuf::from("/var/lora/snap.bin"))
        );
    }

    #[test]
    fn cli_snapshot_path_overrides_env() {
        let cfg = run_cfg(
            resolve(
                args(&["--snapshot-path", "/cli/snap.bin"]),
                EnvInputs {
                    snapshot_path: Some("/env/snap.bin".into()),
                    ..EnvInputs::default()
                },
            )
            .unwrap(),
        );
        assert_eq!(
            cfg.snapshot_path,
            Some(std::path::PathBuf::from("/cli/snap.bin"))
        );
    }

    #[test]
    fn wal_dir_from_cli_and_env() {
        let cfg = run_cfg(
            resolve(args(&["--wal-dir", "/tmp/wal"]), EnvInputs::default()).unwrap(),
        );
        assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/tmp/wal")));

        let cfg = run_cfg(
            resolve(
                args(&[]),
                EnvInputs {
                    wal_dir: Some("/env/wal".into()),
                    ..EnvInputs::default()
                },
            )
            .unwrap(),
        );
        assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/env/wal")));

        // CLI overrides env.
        let cfg = run_cfg(
            resolve(
                args(&["--wal-dir=/cli/wal"]),
                EnvInputs {
                    wal_dir: Some("/env/wal".into()),
                    ..EnvInputs::default()
                },
            )
            .unwrap(),
        );
        assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/cli/wal")));
    }

    #[test]
    fn wal_sync_mode_parses_known_strings() {
        for (raw, expected) in [
            ("per-commit", SyncMode::PerCommit),
            ("PER_COMMIT", SyncMode::PerCommit),
            ("none", SyncMode::None),
            ("OFF", SyncMode::None),
        ] {
            let cfg = run_cfg(
                resolve(args(&["--wal-sync-mode", raw]), EnvInputs::default()).unwrap(),
            );
            assert_eq!(cfg.wal_sync_mode, expected, "raw={raw}");
        }

        // group parses to Group { .. } with v1 defaults.
        let cfg = run_cfg(
            resolve(args(&["--wal-sync-mode", "group"]), EnvInputs::default()).unwrap(),
        );
        assert!(matches!(cfg.wal_sync_mode, SyncMode::Group { .. }));
    }

    #[test]
    fn invalid_wal_sync_mode_rejected() {
        let err = resolve(
            args(&["--wal-sync-mode", "yolo"]),
            EnvInputs::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSyncMode(_)));
    }

    #[test]
    fn help_flag_returns_help_outcome() {
        match resolve(args(&["--help"]), EnvInputs::default()).unwrap() {
            ConfigOutcome::Help(s) => assert!(s.contains("USAGE")),
            other => panic!("expected Help, got {other:?}"),
        }
    }

    #[test]
    fn version_flag_returns_version_outcome() {
        match resolve(args(&["--version"]), EnvInputs::default()).unwrap() {
            ConfigOutcome::Version(s) => assert!(s.starts_with("lora-server ")),
            other => panic!("expected Version, got {other:?}"),
        }
    }

    #[test]
    fn invalid_port_is_rejected() {
        let err = resolve(args(&["--port", "notanumber"]), EnvInputs::default()).unwrap_err();
        match err {
            ConfigError::InvalidPort { value, .. } => assert_eq!(value, "notanumber"),
            other => panic!("expected InvalidPort, got {other:?}"),
        }
    }

    #[test]
    fn port_out_of_range_is_rejected() {
        let err = resolve(args(&["--port", "70000"]), EnvInputs::default()).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPort { .. }));
    }

    #[test]
    fn missing_value_is_rejected() {
        let err = resolve(args(&["--host"]), EnvInputs::default()).unwrap_err();
        assert_eq!(err, ConfigError::MissingValue("--host"));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = resolve(args(&["--nope"]), EnvInputs::default()).unwrap_err();
        assert_eq!(err, ConfigError::UnknownArg("--nope".into()));
    }

    #[test]
    fn positional_is_rejected() {
        let err = resolve(args(&["something"]), EnvInputs::default()).unwrap_err();
        assert_eq!(err, ConfigError::UnexpectedPositional("something".into()));
    }

    #[test]
    fn ipv4_bind_addr_format() {
        let cfg = ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
            ..ServerConfig::default()
        };
        assert_eq!(cfg.bind_addr(), "127.0.0.1:3000");
    }

    #[test]
    fn ipv6_bind_addr_is_bracketed() {
        let cfg = ServerConfig {
            host: "::1".into(),
            port: 3000,
            ..ServerConfig::default()
        };
        assert_eq!(cfg.bind_addr(), "[::1]:3000");
    }

    #[test]
    fn empty_host_rejected() {
        let err = resolve(args(&["--host", "   "]), EnvInputs::default()).unwrap_err();
        assert_eq!(err, ConfigError::EmptyValue("--host"));
    }
}
