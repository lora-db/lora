//! Runtime configuration for `lora-server`.
//!
//! Resolves the bind address (`host` + `port`) from, in order of precedence:
//!
//! 1. CLI flags: `--host <HOST>`, `--port <PORT>` (also accepts `--host=<HOST>`).
//! 2. Environment variables: `LORA_SERVER_HOST`, `LORA_SERVER_PORT`.
//! 3. Built-in defaults: `127.0.0.1:4747`.
//!
//! The default port is picked to be short and memorable (a mirror of Neo4j's
//! `7474`) while avoiding collisions with common services: well-known
//! graph-DB ports (Neo4j 7474/7687, ArangoDB 8529, Dgraph 8080, JanusGraph
//! 8182), common web-dev ports (3000/4000/5000/8000/8080/8443/9000), and
//! standard databases (Postgres 5432, Redis 6379, MongoDB 27017,
//! Elasticsearch 9200, MySQL 3306).
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
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
        --host <HOST>    Bind address. Default: {DEFAULT_HOST} (or ${HOST_ENV} if set).
        --port <PORT>    TCP port.      Default: {DEFAULT_PORT} (or ${PORT_ENV} if set).
        --help           Print this help and exit.
        --version        Print version and exit.

ENVIRONMENT:
    {HOST_ENV}     Bind address (overridden by --host).
    {PORT_ENV}     TCP port      (overridden by --port).

EXAMPLES:
    lora-server
    lora-server --host 0.0.0.0 --port 8080
    {HOST_ENV}=0.0.0.0 {PORT_ENV}=8080 lora-server
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
) -> Result<ConfigOutcome, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let _program = iter.next();

    let mut cli_host: Option<String> = None;
    let mut cli_port: Option<String> = None;

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
            s if s.starts_with("--host=") => {
                cli_host = Some(s["--host=".len()..].to_string());
            }
            s if s.starts_with("--port=") => {
                cli_port = Some(s["--port=".len()..].to_string());
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

    Ok(ConfigOutcome::Run(ServerConfig { host, port }))
}

/// Resolve using the process environment and `std::env::args`.
pub fn resolve_from_process() -> Result<ConfigOutcome, ConfigError> {
    resolve(
        std::env::args(),
        std::env::var(HOST_ENV).ok(),
        std::env::var(PORT_ENV).ok(),
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
        let out = resolve(args(&[]), None, None).unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: DEFAULT_HOST.into(),
                port: DEFAULT_PORT,
            })
        );
    }

    #[test]
    fn env_vars_apply_without_cli() {
        let out = resolve(args(&[]), Some("0.0.0.0".into()), Some("9000".into())).unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: "0.0.0.0".into(),
                port: 9000,
            })
        );
    }

    #[test]
    fn cli_flags_override_env() {
        let out = resolve(
            args(&["--host", "10.0.0.1", "--port", "8080"]),
            Some("0.0.0.0".into()),
            Some("9000".into()),
        )
        .unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: "10.0.0.1".into(),
                port: 8080,
            })
        );
    }

    #[test]
    fn cli_equals_form_works() {
        let out = resolve(args(&["--host=::1", "--port=7000"]), None, None).unwrap();
        assert_eq!(
            out,
            ConfigOutcome::Run(ServerConfig {
                host: "::1".into(),
                port: 7000,
            })
        );
    }

    #[test]
    fn help_flag_returns_help_outcome() {
        match resolve(args(&["--help"]), None, None).unwrap() {
            ConfigOutcome::Help(s) => assert!(s.contains("USAGE")),
            other => panic!("expected Help, got {other:?}"),
        }
    }

    #[test]
    fn version_flag_returns_version_outcome() {
        match resolve(args(&["--version"]), None, None).unwrap() {
            ConfigOutcome::Version(s) => assert!(s.starts_with("lora-server ")),
            other => panic!("expected Version, got {other:?}"),
        }
    }

    #[test]
    fn invalid_port_is_rejected() {
        let err = resolve(args(&["--port", "notanumber"]), None, None).unwrap_err();
        match err {
            ConfigError::InvalidPort { value, .. } => assert_eq!(value, "notanumber"),
            other => panic!("expected InvalidPort, got {other:?}"),
        }
    }

    #[test]
    fn port_out_of_range_is_rejected() {
        let err = resolve(args(&["--port", "70000"]), None, None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPort { .. }));
    }

    #[test]
    fn missing_value_is_rejected() {
        let err = resolve(args(&["--host"]), None, None).unwrap_err();
        assert_eq!(err, ConfigError::MissingValue("--host"));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = resolve(args(&["--nope"]), None, None).unwrap_err();
        assert_eq!(err, ConfigError::UnknownArg("--nope".into()));
    }

    #[test]
    fn positional_is_rejected() {
        let err = resolve(args(&["something"]), None, None).unwrap_err();
        assert_eq!(err, ConfigError::UnexpectedPositional("something".into()));
    }

    #[test]
    fn ipv4_bind_addr_format() {
        let cfg = ServerConfig {
            host: "127.0.0.1".into(),
            port: 3000,
        };
        assert_eq!(cfg.bind_addr(), "127.0.0.1:3000");
    }

    #[test]
    fn ipv6_bind_addr_is_bracketed() {
        let cfg = ServerConfig {
            host: "::1".into(),
            port: 3000,
        };
        assert_eq!(cfg.bind_addr(), "[::1]:3000");
    }

    #[test]
    fn empty_host_rejected() {
        let err = resolve(args(&["--host", "   "]), None, None).unwrap_err();
        assert_eq!(err, ConfigError::EmptyValue("--host"));
    }
}
