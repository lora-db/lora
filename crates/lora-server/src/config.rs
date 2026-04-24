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

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 4747;
pub const HOST_ENV: &str = "LORA_SERVER_HOST";
pub const PORT_ENV: &str = "LORA_SERVER_PORT";
pub const SNAPSHOT_PATH_ENV: &str = "LORA_SERVER_SNAPSHOT_PATH";

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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            snapshot_path: None,
            restore_from: None,
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
        --host <HOST>             Bind address. Default: {DEFAULT_HOST} (or ${HOST_ENV} if set).
        --port <PORT>             TCP port.      Default: {DEFAULT_PORT} (or ${PORT_ENV} if set).
        --snapshot-path <PATH>    Enable the admin surface. Mounts
                                  POST /admin/snapshot/save and
                                  POST /admin/snapshot/load against this file.
                                  Also read from ${SNAPSHOT_PATH_ENV}.
        --restore-from <PATH>     Restore the graph from this snapshot at boot.
                                  Missing file is treated as empty.
        --help                    Print this help and exit.
        --version                 Print version and exit.

ENVIRONMENT:
    {HOST_ENV}     Bind address (overridden by --host).
    {PORT_ENV}     TCP port      (overridden by --port).
    {SNAPSHOT_PATH_ENV}  Path used by --snapshot-path.

EXAMPLES:
    lora-server
    lora-server --host 0.0.0.0 --port 8080
    lora-server --snapshot-path /var/lib/lora/graph.bin
"
    )
}

pub fn version_text() -> String {
    format!("lora-server {}", env!("CARGO_PKG_VERSION"))
}

/// Resolve a [`ConfigOutcome`] from CLI args and explicit env values.
///
/// `args` includes the program name at position 0 (as produced by
/// [`std::env::args`]); it is skipped internally.
pub fn resolve<I>(
    args: I,
    env_host: Option<String>,
    env_port: Option<String>,
    env_snapshot_path: Option<String>,
) -> Result<ConfigOutcome, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let _program = iter.next();

    let mut cli_host: Option<String> = None;
    let mut cli_port: Option<String> = None;
    let mut cli_snapshot_path: Option<String> = None;
    let mut cli_restore_from: Option<String> = None;

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
            s if s.starts_with("--") => return Err(ConfigError::UnknownArg(arg)),
            _ => return Err(ConfigError::UnexpectedPositional(arg)),
        }
    }

    let host = cli_host
        .or(env_host)
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    if host.trim().is_empty() {
        return Err(ConfigError::EmptyValue("--host"));
    }

    let port = match cli_port.or(env_port) {
        Some(raw) => parse_port(&raw)?,
        None => DEFAULT_PORT,
    };

    let snapshot_path = cli_snapshot_path.or(env_snapshot_path).and_then(|p| {
        if p.trim().is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(p))
        }
    });

    let restore_from = cli_restore_from.and_then(|p| {
        if p.trim().is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(p))
        }
    });

    Ok(ConfigOutcome::Run(ServerConfig {
        host,
        port,
        snapshot_path,
        restore_from,
    }))
}

/// Resolve using the process environment and `std::env::args`.
pub fn resolve_from_process() -> Result<ConfigOutcome, ConfigError> {
    resolve(
        std::env::args(),
        std::env::var(HOST_ENV).ok(),
        std::env::var(PORT_ENV).ok(),
        std::env::var(SNAPSHOT_PATH_ENV).ok(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(xs: &[&str]) -> Vec<String> {
        std::iter::once("lora-server")
            .chain(xs.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn defaults_when_nothing_set() {
        let out = resolve(args(&[]), None, None, None).unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: DEFAULT_HOST.into(),
                port: DEFAULT_PORT,
                snapshot_path: None,
                restore_from: None,
            })
        );
    }

    #[test]
    fn env_vars_apply_without_cli() {
        let out = resolve(args(&[]), Some("0.0.0.0".into()), Some("9000".into()), None).unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: "0.0.0.0".into(),
                port: 9000,
                snapshot_path: None,
                restore_from: None,
            })
        );
    }

    #[test]
    fn cli_flags_override_env() {
        let out = resolve(
            args(&["--host", "10.0.0.1", "--port", "8080"]),
            Some("0.0.0.0".into()),
            Some("9000".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: "10.0.0.1".into(),
                port: 8080,
                snapshot_path: None,
                restore_from: None,
            })
        );
    }

    #[test]
    fn cli_equals_form_works() {
        let out = resolve(args(&["--host=::1", "--port=7000"]), None, None, None).unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: "::1".into(),
                port: 7000,
                snapshot_path: None,
                restore_from: None,
            })
        );
    }

    #[test]
    fn snapshot_path_from_cli() {
        let out = resolve(
            args(&["--snapshot-path", "/tmp/snap.bin"]),
            None,
            None,
            None,
        )
        .unwrap();
        let ConfigOutcome::Run(cfg) = out else {
            panic!("expected Run");
        };
        assert_eq!(
            cfg.snapshot_path,
            Some(std::path::PathBuf::from("/tmp/snap.bin"))
        );
    }

    #[test]
    fn snapshot_path_from_env() {
        let out = resolve(args(&[]), None, None, Some("/var/lora/snap.bin".into())).unwrap();
        let ConfigOutcome::Run(cfg) = out else {
            panic!("expected Run");
        };
        assert_eq!(
            cfg.snapshot_path,
            Some(std::path::PathBuf::from("/var/lora/snap.bin"))
        );
    }

    #[test]
    fn cli_snapshot_path_overrides_env() {
        let out = resolve(
            args(&["--snapshot-path", "/cli/snap.bin"]),
            None,
            None,
            Some("/env/snap.bin".into()),
        )
        .unwrap();
        let ConfigOutcome::Run(cfg) = out else {
            panic!("expected Run");
        };
        assert_eq!(
            cfg.snapshot_path,
            Some(std::path::PathBuf::from("/cli/snap.bin"))
        );
    }

    #[test]
    fn help_flag_returns_help_outcome() {
        match resolve(args(&["--help"]), None, None, None).unwrap() {
            ConfigOutcome::Help(s) => assert!(s.contains("USAGE")),
            other => panic!("expected Help, got {other:?}"),
        }
    }

    #[test]
    fn version_flag_returns_version_outcome() {
        match resolve(args(&["--version"]), None, None, None).unwrap() {
            ConfigOutcome::Version(s) => assert!(s.starts_with("lora-server ")),
            other => panic!("expected Version, got {other:?}"),
        }
    }

    #[test]
    fn invalid_port_is_rejected() {
        let err = resolve(args(&["--port", "notanumber"]), None, None, None).unwrap_err();
        match err {
            ConfigError::InvalidPort { value, .. } => assert_eq!(value, "notanumber"),
            other => panic!("expected InvalidPort, got {other:?}"),
        }
    }

    #[test]
    fn port_out_of_range_is_rejected() {
        let err = resolve(args(&["--port", "70000"]), None, None, None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPort { .. }));
    }

    #[test]
    fn missing_value_is_rejected() {
        let err = resolve(args(&["--host"]), None, None, None).unwrap_err();
        assert_eq!(err, ConfigError::MissingValue("--host"));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = resolve(args(&["--nope"]), None, None, None).unwrap_err();
        assert_eq!(err, ConfigError::UnknownArg("--nope".into()));
    }

    #[test]
    fn positional_is_rejected() {
        let err = resolve(args(&["something"]), None, None, None).unwrap_err();
        assert_eq!(err, ConfigError::UnexpectedPositional("something".into()));
    }

    #[test]
    fn ipv4_bind_addr_format() {
        let cfg = ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
            snapshot_path: None,
            restore_from: None,
        };
        assert_eq!(cfg.bind_addr(), "127.0.0.1:3000");
    }

    #[test]
    fn ipv6_bind_addr_is_bracketed() {
        let cfg = ServerConfig {
            host: "::1".into(),
            port: 3000,
            snapshot_path: None,
            restore_from: None,
        };
        assert_eq!(cfg.bind_addr(), "[::1]:3000");
    }

    #[test]
    fn empty_host_rejected() {
        let err = resolve(args(&["--host", "   "]), None, None, None).unwrap_err();
        assert_eq!(err, ConfigError::EmptyValue("--host"));
    }
}
