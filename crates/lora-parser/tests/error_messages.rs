//! Regression baseline for `ParseError` `Display` output.
//!
//! Wording in these messages reaches users (and bindings) verbatim
//! through `LoraError::message()`. Phases 8-9 of the error-handling
//! cleanup convert several `anyhow!("...")` sites elsewhere in the
//! tree into typed errors; this file pins the parser-side wording
//! so unrelated wording drift gets caught in CI.

use lora_parser::ParseError;

#[test]
fn message_variant_displays_with_span() {
    let err = ParseError::new("expected `MATCH`", 0, 5);
    assert_eq!(err.to_string(), "parse error at 0..5: expected `MATCH`");
}

#[test]
fn message_variant_with_offset_span() {
    let err = ParseError::new("expected identifier", 12, 17);
    assert_eq!(
        err.to_string(),
        "parse error at 12..17: expected identifier"
    );
}
