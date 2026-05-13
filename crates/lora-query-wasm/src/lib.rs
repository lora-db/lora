//! WASM entry points for the `@loradb/lora-query` editor.
//!
//! We reuse `lora-parser` — the same pest-driven grammar that the engine
//! ships with — so syntax accepted by the editor is exactly the syntax
//! accepted by the database. Three functions are exposed:
//!
//! - [`parse`] — full parse, returning `{ ok, ast, errors }`.
//! - [`validate`] — lightweight check, returning only the diagnostics.
//! - [`format`] — normalize whitespace if the input parses, otherwise
//!   return the input unchanged so the editor never destroys partial work.

use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Serialize)]
struct Diagnostic {
    message: String,
    span: Span,
}

#[derive(Serialize)]
struct ParseResult {
    ok: bool,
    /// Debug-rendered AST. A future revision will switch to a structured
    /// JSON shape once the `lora-ast` crate grows `serde::Serialize`
    /// derives — the JS facade already treats this field as `unknown`.
    ast: Option<String>,
    errors: Vec<Diagnostic>,
}

fn diagnostic_from_parse_error(err: lora_parser::ParseError, source_len: usize) -> Diagnostic {
    match err {
        lora_parser::ParseError::Message { message, span } => Diagnostic {
            message,
            span: Span {
                start: span.start.min(source_len),
                end: span.end.min(source_len),
            },
        },
    }
}

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub fn parse(source: &str) -> Result<JsValue, JsValue> {
    let result = match lora_parser::parse_query(source) {
        Ok(doc) => ParseResult {
            ok: true,
            ast: Some(format!("{doc:#?}")),
            errors: Vec::new(),
        },
        Err(err) => ParseResult {
            ok: false,
            ast: None,
            errors: vec![diagnostic_from_parse_error(err, source.len())],
        },
    };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn validate(source: &str) -> Result<JsValue, JsValue> {
    let errors: Vec<Diagnostic> = match lora_parser::parse_query(source) {
        Ok(_) => Vec::new(),
        Err(err) => vec![diagnostic_from_parse_error(err, source.len())],
    };
    serde_wasm_bindgen::to_value(&errors).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn format(source: &str) -> String {
    if lora_parser::parse_query(source).is_err() {
        return source.to_owned();
    }
    normalize_whitespace(source)
}

/// Placeholder formatter.
///
/// A real pretty-printer would walk the [`lora_ast::Document`] and emit
/// canonical Cypher. Until that lands we apply a conservative lexical
/// pass that:
///   - trims trailing whitespace on every line,
///   - collapses runs of blank lines down to one,
///   - ensures exactly one space after every `,`.
/// The point is to give the editor a working `format()` hook today; the
/// AST-driven version will slot in behind the same signature.
fn normalize_whitespace(source: &str) -> String {
    let trimmed: Vec<&str> = source.lines().map(str::trim_end).collect();

    let mut out = String::with_capacity(source.len());
    let mut prev_blank = false;
    for line in trimmed {
        let is_blank = line.is_empty();
        if is_blank && prev_blank {
            continue;
        }
        prev_blank = is_blank;

        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            out.push(c);
            if c == ',' {
                while let Some(&next) = chars.peek() {
                    if next == ' ' || next == '\t' {
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek().is_some() {
                    out.push(' ');
                }
            }
        }
        out.push('\n');
    }

    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_normalises_commas_and_blank_lines() {
        let input = "MATCH (a,b,c)\n\n\nRETURN a,b   \n";
        let out = normalize_whitespace(input);
        assert_eq!(out, "MATCH (a, b, c)\n\nRETURN a, b\n");
    }

    #[test]
    fn format_returns_input_on_parse_error() {
        let bad = "MATCH (";
        assert_eq!(format(bad), bad);
    }
}
