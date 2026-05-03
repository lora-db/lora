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

    let snapshot_path = cli_snapshot_path
        .or(env.snapshot_path)
        .and_then(non_empty_path);
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
