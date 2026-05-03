//! Error helpers and stable error-code prefixes for the Node bindings.
//!
//! Every `NapiError` that crosses the JS boundary carries a `LORA_ERROR:`
//! or `INVALID_PARAMS:` prefix in its message so the JS wrapper can route
//! failures to the right error subclass (`LoraQueryError` vs
//! `InvalidParamsError`) without parsing free-form text.

pub(crate) const LORA_ERROR_CODE: &str = "LORA_ERROR";
pub(crate) const INVALID_PARAMS_CODE: &str = "INVALID_PARAMS";

pub(crate) fn format_error(err: &anyhow::Error) -> String {
    format!("{LORA_ERROR_CODE}: {err}")
}

pub(crate) fn closed_error_message() -> String {
    format!("{LORA_ERROR_CODE}: database is closed")
}
