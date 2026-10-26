//! CLI / env resolution: turns argv + env into a [`ConfigOutcome`].

use lora_database::SyncMode;

use super::errors::ConfigError;
use super::help::{help_text, version_text};
use super::{
    ConfigOutcome, ServerConfig, ADMIN_PATH_ROOT_ENV, DEFAULT_HEADER_READ_TIMEOUT_MS, DEFAULT_HOST,
    DEFAULT_MAX_BODY_BYTES, DEFAULT_PORT, DEFAULT_QUERY_TIMEOUT_MS, HEADER_READ_TIMEOUT_MS_ENV,
    HOST_ENV, MAX_BODY_BYTES_ENV, MAX_CONCURRENT_QUERIES_ENV, PORT_ENV, QUERY_TIMEOUT_MS_ENV,
    SNAPSHOT_PATH_ENV, WAL_DIR_ENV, WAL_SYNC_MODE_ENV, WAL_TRUNCATE_CORRUPT_TAIL_ENV,
};

/// Inputs to [`resolve`]. Wrapping the env values in a struct keeps the
/// caller-visible signature stable as new env-driven options are added.
#[derive(Debug, Default, Clone)]
pub struct EnvInputs {
    pub host: Option<String>,
    pub port: Option<String>,
    pub snapshot_path: Option<String>,
    pub wal_dir: Option<String>,
    pub wal_sync_mode: Option<String>,
    pub query_timeout_ms: Option<String>,
    pub max_concurrent_queries: Option<String>,
    pub max_body_bytes: Option<String>,
    pub admin_path_root: Option<String>,
    pub header_read_timeout_ms: Option<String>,
    pub wal_truncate_corrupt_tail: Option<String>,
}

#[derive(Debug, Default)]
struct CliInputs {
    host: Option<String>,
    port: Option<String>,
    snapshot_path: Option<String>,
    restore_from: Option<String>,
    wal_dir: Option<String>,
    wal_sync_mode: Option<String>,
    query_timeout_ms: Option<String>,
    max_concurrent_queries: Option<String>,
    max_body_bytes: Option<String>,
    admin_path_root: Option<String>,
    header_read_timeout_ms: Option<String>,
    wal_truncate_corrupt_tail: Option<String>,
}

impl CliInputs {
    fn set(&mut self, field: CliField, value: String) {
        match field {
            CliField::Host => self.host = Some(value),
            CliField::Port => self.port = Some(value),
            CliField::SnapshotPath => self.snapshot_path = Some(value),
            CliField::RestoreFrom => self.restore_from = Some(value),
            CliField::WalDir => self.wal_dir = Some(value),
            CliField::WalSyncMode => self.wal_sync_mode = Some(value),
            CliField::QueryTimeoutMs => self.query_timeout_ms = Some(value),
            CliField::MaxConcurrentQueries => self.max_concurrent_queries = Some(value),
            CliField::MaxBodyBytes => self.max_body_bytes = Some(value),
            CliField::AdminPathRoot => self.admin_path_root = Some(value),
            CliField::HeaderReadTimeoutMs => self.header_read_timeout_ms = Some(value),
            CliField::WalTruncateCorruptTail => self.wal_truncate_corrupt_tail = Some(value),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CliField {
    Host,
    Port,
    SnapshotPath,
    RestoreFrom,
    WalDir,
    WalSyncMode,
    QueryTimeoutMs,
    MaxConcurrentQueries,
    MaxBodyBytes,
    AdminPathRoot,
    HeaderReadTimeoutMs,
    /// A switch: given bare it means `true`; `--flag=<bool>` also works.
    WalTruncateCorruptTail,
}

impl CliField {
    fn is_switch(self) -> bool {
        matches!(self, CliField::WalTruncateCorruptTail)
    }
}

enum ParsedCli {
    Run(CliInputs),
    Immediate(ConfigOutcome),
}

/// Resolve a [`ConfigOutcome`] from CLI args and env values.
///
/// `args` includes the program name at position 0 (as produced by
/// [`std::env::args`]); it is skipped internally.
pub fn resolve<I>(args: I, env: EnvInputs) -> Result<ConfigOutcome, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let cli = match parse_cli_args(args)? {
        ParsedCli::Run(cli) => cli,
        ParsedCli::Immediate(outcome) => return Ok(outcome),
    };

    let host = cli
        .host
        .or(env.host)
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    if host.trim().is_empty() {
        return Err(ConfigError::EmptyValue("--host"));
    }

    let port = match cli.port.or(env.port) {
        Some(raw) => parse_port(&raw)?,
        None => DEFAULT_PORT,
    };

    let snapshot_path = cli
        .snapshot_path
        .or(env.snapshot_path)
        .and_then(non_empty_path);
    let restore_from = cli.restore_from.and_then(non_empty_path);
    let wal_dir = cli.wal_dir.or(env.wal_dir).and_then(non_empty_path);
    let wal_sync_mode = match cli.wal_sync_mode.or(env.wal_sync_mode) {
        Some(raw) => parse_sync_mode(&raw)?,
        None => SyncMode::default(),
    };
    let wal_truncate_corrupt_tail = match cli
        .wal_truncate_corrupt_tail
        .or(env.wal_truncate_corrupt_tail)
    {
        Some(raw) => parse_bool("--wal-truncate-corrupt-tail", &raw)?,
        None => false,
    };

    let query_timeout_ms = match cli.query_timeout_ms.or(env.query_timeout_ms) {
        Some(raw) => parse_number("--query-timeout-ms", &raw)?,
        None => DEFAULT_QUERY_TIMEOUT_MS,
    };
    let max_concurrent_queries = match cli.max_concurrent_queries.or(env.max_concurrent_queries) {
        Some(raw) => parse_number("--max-concurrent-queries", &raw)?,
        None => crate::app::default_max_concurrent_queries(),
    };
    let max_body_bytes = match cli.max_body_bytes.or(env.max_body_bytes) {
        Some(raw) => parse_number("--max-body-bytes", &raw)?,
        None => DEFAULT_MAX_BODY_BYTES,
    };
    let admin_path_root = cli
        .admin_path_root
        .or(env.admin_path_root)
        .and_then(non_empty_path);
    let header_read_timeout_ms = match cli.header_read_timeout_ms.or(env.header_read_timeout_ms) {
        Some(raw) => parse_number("--header-read-timeout-ms", &raw)?,
        None => DEFAULT_HEADER_READ_TIMEOUT_MS,
    };

    Ok(ConfigOutcome::Run(ServerConfig {
        host,
        port,
        snapshot_path,
        restore_from,
        wal_dir,
        wal_sync_mode,
        wal_truncate_corrupt_tail,
        query_timeout_ms,
        max_concurrent_queries,
        max_body_bytes,
        header_read_timeout_ms,
        admin_path_root,
    }))
}

fn parse_cli_args<I>(args: I) -> Result<ParsedCli, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let _program = iter.next();

    let mut cli = CliInputs::default();

    while let Some(arg) = iter.next() {
        if arg == "--help" {
            return Ok(ParsedCli::Immediate(ConfigOutcome::Help(help_text())));
        }
        if arg == "--version" {
            return Ok(ParsedCli::Immediate(ConfigOutcome::Version(version_text())));
        }

        if let Some((field, flag)) = cli_field_for_flag(&arg) {
            if field.is_switch() {
                cli.set(field, "true".to_string());
                continue;
            }
            let value = iter.next().ok_or(ConfigError::MissingValue(flag))?;
            cli.set(field, value);
            continue;
        }

        if let Some((field, value)) = cli_field_for_equals(&arg) {
            cli.set(field, value.to_string());
            continue;
        }

        if arg.starts_with("--") {
            return Err(ConfigError::UnknownArg(arg));
        }
        return Err(ConfigError::UnexpectedPositional(arg));
    }

    Ok(ParsedCli::Run(cli))
}

fn cli_field_for_flag(arg: &str) -> Option<(CliField, &'static str)> {
    match arg {
        "--host" => Some((CliField::Host, "--host")),
        "--port" => Some((CliField::Port, "--port")),
        "--snapshot-path" => Some((CliField::SnapshotPath, "--snapshot-path")),
        "--restore-from" => Some((CliField::RestoreFrom, "--restore-from")),
        "--wal-dir" => Some((CliField::WalDir, "--wal-dir")),
        "--wal-sync-mode" => Some((CliField::WalSyncMode, "--wal-sync-mode")),
        "--query-timeout-ms" => Some((CliField::QueryTimeoutMs, "--query-timeout-ms")),
        "--max-concurrent-queries" => {
            Some((CliField::MaxConcurrentQueries, "--max-concurrent-queries"))
        }
        "--max-body-bytes" => Some((CliField::MaxBodyBytes, "--max-body-bytes")),
        "--admin-path-root" => Some((CliField::AdminPathRoot, "--admin-path-root")),
        "--header-read-timeout-ms" => {
            Some((CliField::HeaderReadTimeoutMs, "--header-read-timeout-ms"))
        }
        "--wal-truncate-corrupt-tail" => Some((
            CliField::WalTruncateCorruptTail,
            "--wal-truncate-corrupt-tail",
        )),
        _ => None,
    }
}

fn cli_field_for_equals(arg: &str) -> Option<(CliField, &str)> {
    let (flag, value) = arg.split_once('=')?;
    let (field, _) = cli_field_for_flag(flag)?;
    Some((field, value))
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
            query_timeout_ms: std::env::var(QUERY_TIMEOUT_MS_ENV).ok(),
            max_concurrent_queries: std::env::var(MAX_CONCURRENT_QUERIES_ENV).ok(),
            max_body_bytes: std::env::var(MAX_BODY_BYTES_ENV).ok(),
            admin_path_root: std::env::var(ADMIN_PATH_ROOT_ENV).ok(),
            header_read_timeout_ms: std::env::var(HEADER_READ_TIMEOUT_MS_ENV).ok(),
            wal_truncate_corrupt_tail: std::env::var(WAL_TRUNCATE_CORRUPT_TAIL_ENV).ok(),
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

fn parse_number<T>(flag: &'static str, raw: &str) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::EmptyValue(flag));
    }
    trimmed
        .parse::<T>()
        .map_err(|e| ConfigError::InvalidNumber {
            flag,
            value: raw.to_string(),
            reason: e.to_string(),
        })
}

fn parse_bool(flag: &'static str, raw: &str) -> Result<bool, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        "" => Err(ConfigError::EmptyValue(flag)),
        _ => Err(ConfigError::InvalidBool {
            flag,
            value: raw.to_string(),
        }),
    }
}

fn parse_sync_mode(raw: &str) -> Result<SyncMode, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "group-sync" | "group_sync" | "groupsync" => Ok(SyncMode::GroupSync {
            // 50 ms cadence is short enough that a crash window is
            // bounded by the wallclock budget operators usually quote
            // ("at most ~50 ms of writes lost") and long enough that
            // the bg flusher does not tax disks under sustained load.
            interval_ms: 50,
        }),
        other => Err(ConfigError::InvalidSyncMode(other.to_string())),
    }
}
