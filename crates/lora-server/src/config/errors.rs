//! [`ConfigError`] and its conversions.

use std::fmt;
use std::num::ParseIntError;

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
