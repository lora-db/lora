use std::fmt;
use std::path::{Path, PathBuf};

use lora_wal::{SyncMode, WalConfig};

/// Hard ceiling for one portable database archive/root.
///
/// The current WAL backend still stores segment files under this root, but
/// callers should treat the resolved `.lora` path as the database artifact.
/// The archive backend will use the same limit when it starts writing framed
/// compressed chunks directly into portable files.
pub const DEFAULT_DATABASE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Options for opening a named filesystem-backed database.
#[derive(Debug, Clone)]
pub struct DatabaseOpenOptions {
    pub database_dir: PathBuf,
    pub sync_mode: SyncMode,
    pub segment_target_bytes: u64,
    pub max_database_bytes: u64,
}

impl Default for DatabaseOpenOptions {
    fn default() -> Self {
        Self {
            database_dir: PathBuf::from("."),
            sync_mode: SyncMode::Group { interval_ms: 1_000 },
            segment_target_bytes: 8 * 1024 * 1024,
            max_database_bytes: DEFAULT_DATABASE_MAX_BYTES,
        }
    }
}

impl DatabaseOpenOptions {
    pub fn with_database_dir(mut self, database_dir: impl Into<PathBuf>) -> Self {
        self.database_dir = database_dir.into();
        self
    }

    pub fn wal_config_for(&self, name: &DatabaseName) -> WalConfig {
        WalConfig::Enabled {
            dir: self.database_path_for(name),
            sync_mode: self.sync_mode,
            segment_target_bytes: self.segment_target_bytes,
        }
    }

    pub fn database_path_for(&self, name: &DatabaseName) -> PathBuf {
        self.database_dir.join(format!("{}.lora", name.as_str()))
    }
}

/// Validated logical database name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DatabaseName(String);

impl DatabaseName {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, DatabaseNameError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(DatabaseNameError::Empty);
        }
        if value == "." || value == ".." {
            return Err(DatabaseNameError::Reserved(value.to_string()));
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(DatabaseNameError::InvalidCharacters(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DatabaseName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<&str> for DatabaseName {
    type Error = DatabaseNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseNameError {
    Empty,
    Reserved(String),
    InvalidCharacters(String),
}

impl fmt::Display for DatabaseNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "database name must not be empty"),
            Self::Reserved(name) => write!(f, "database name '{name}' is reserved"),
            Self::InvalidCharacters(name) => write!(
                f,
                "invalid database name '{name}': use only letters, digits, '_', '-', and '.'"
            ),
        }
    }
}

impl std::error::Error for DatabaseNameError {}

pub fn resolve_database_path(
    database_name: &str,
    database_dir: impl AsRef<Path>,
) -> Result<PathBuf, DatabaseNameError> {
    let name = DatabaseName::parse(database_name)?;
    Ok(database_dir
        .as_ref()
        .join(format!("{}.lora", name.as_str())))
}
