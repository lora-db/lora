//! CLI / env resolution: turns argv + env into a [`ConfigOutcome`].

use lora_database::SyncMode;

use super::errors::ConfigError;
use super::help::{help_text, version_text};
use super::{
    ConfigOutcome, ServerConfig, DEFAULT_HOST, DEFAULT_PORT, HOST_ENV, PORT_ENV, SNAPSHOT_PATH_ENV,
    WAL_DIR_ENV, WAL_SYNC_MODE_ENV,
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
}

#[derive(Debug, Default)]
struct CliInputs {
    host: Option<String>,
    port: Option<String>,
    snapshot_path: Option<String>,
    restore_from: Option<String>,
    wal_dir: Option<String>,
    wal_sync_mode: Option<String>,
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

    Ok(ConfigOutcome::Run(ServerConfig {
        host,
        port,
        snapshot_path,
        restore_from,
        wal_dir,
        wal_sync_mode,
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
