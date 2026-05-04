//! Regression baseline for `DatabaseNameError` `Display` output.
//!
//! Wording drifts here would change `LoraError::message()` for
//! every binding's "invalid database name" exception.

use lora_database::DatabaseNameError;

#[test]
fn empty() {
    assert_eq!(
        DatabaseNameError::Empty.to_string(),
        "database name must not be empty"
    );
}

#[test]
fn reserved() {
    let err = DatabaseNameError::Reserved("default".into());
    assert_eq!(err.to_string(), "database name `default` is reserved");
}

#[test]
fn absolute_path() {
    let err = DatabaseNameError::AbsolutePath("/etc/passwd".into());
    assert_eq!(
        err.to_string(),
        "invalid database name `/etc/passwd`: use a relative path under `database_dir`"
    );
}

#[test]
fn invalid_characters() {
    let err = DatabaseNameError::InvalidCharacters("..\\nope".into());
    assert_eq!(
        err.to_string(),
        "invalid database name `..\\nope`: use relative path components containing only letters, digits, `+`, `_`, `-`, with an optional `.loradb` suffix on the basename"
    );
}
