//! Regression baseline for `ConfigError` `Display` output.
//!
//! These messages are surfaced by the CLI launcher and folded into
//! `LoraErrorCode::Config`. Pinning them catches wording drift in
//! CI before users see different errors for the same flag mistake.

use lora_server::ConfigError;

#[test]
fn unknown_arg() {
    let err = ConfigError::UnknownArg("--bogus".into());
    assert_eq!(err.to_string(), "unknown argument `--bogus`");
}

#[test]
fn missing_value() {
    let err = ConfigError::MissingValue("--port");
    assert_eq!(err.to_string(), "missing value for `--port`");
}

#[test]
fn empty_value() {
    let err = ConfigError::EmptyValue("--snapshot-path");
    assert_eq!(err.to_string(), "`--snapshot-path` value must not be empty");
}

#[test]
fn invalid_port() {
    let err = ConfigError::InvalidPort {
        value: "abc".into(),
        reason: "not a valid u16".into(),
    };
    assert_eq!(err.to_string(), "invalid port `abc`: not a valid u16");
}

#[test]
fn invalid_sync_mode() {
    let err = ConfigError::InvalidSyncMode("burst".into());
    assert_eq!(
        err.to_string(),
        "invalid `--wal-sync-mode` `burst`: expected `per-commit`, `group`, or `none`"
    );
}

#[test]
fn unexpected_positional() {
    let err = ConfigError::UnexpectedPositional("extra".into());
    assert_eq!(err.to_string(), "unexpected positional argument `extra`");
}
