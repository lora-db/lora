//! WASM entry points for the `@loradb/lora-query` editor.
//!
//! We reuse `lora-parser` — the same pest-driven grammar that the engine
//! ships with — so syntax accepted by the editor is exactly the syntax
//! accepted by the database. Four functions are exposed:
//!
//! - [`parse`] — full parse, returning `{ ok, ast, errors }`.
//! - [`validate`] — lightweight check, returning only the diagnostics.
//! - [`format`] — pretty-print: normalise whitespace + uppercase Cypher
//!   keywords (outside strings / comments).
//! - [`highlight`] — walk the AST and emit `{ start, end, kind }` spans
//!   for the editor to colour: variables, labels, rel types, function
//!   names, parameters, literals, property keys.
//!
//! Diagnostics carry both a human summary and pest's full positional
//! report (the `--> L:C` block) so the editor can show a rich tooltip
//! pointing at the failure site.
//!
//! When [`parse_query`] returns an error the original pest message is
//! re-parsed here to recover line / column / expected-rule info — the
//! upstream wrapper currently collapses the span to `0..len`, so doing
//! it locally avoids touching `lora-parser`.

use serde::Serialize;
use wasm_bindgen::prelude::*;

use lora_ast::{
    Document, Expr, Match, MultiPartQuery, Pattern, PatternElement, PatternElementChain,
    PatternPart, ProjectionBody, ProjectionItem, Query, QueryPart, ReadingClause, RegularQuery,
    RelationshipDetail, SinglePartQuery, SingleQuery, Statement, UpdatingClause, Variable, With,
};

// ─────────────────────────────────────────────────────────────────────
// Public surface
// ─────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Serialize)]
struct Diagnostic {
    /// Severity for the lint marker. Syntax errors are `Error`,
    /// semantic checks are typically `Warning`, helpful hints `Info`.
    severity: Severity,
    /// Short, human-readable summary — suitable for an inline lint
    /// tooltip in the editor.
    message: String,
    /// The full pest report — caret indicator, snippet, and the
    /// `= expected …` footer included. Preserve it verbatim so callers
    /// can render the canonical positional message:
    /// ```text
    ///  --> 2:16
    ///   |
    /// 2 | WHERE a.name = 'Joos
    ///   |                ^---
    ///   |
    ///   = expected unary_expression
    /// ```
    details: String,
    /// 1-based line number of the error position.
    line: usize,
    /// 1-based column number of the error position.
    column: usize,
    /// Rule names pest was hoping to see (e.g. `["unary_expression"]`).
    expected: Vec<String>,
    /// Short concrete code snippets that would be valid in the failing
    /// position. Editors can render these as "Try one of:" hints.
    examples: Vec<String>,
    /// Byte offsets into the original source.
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

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
enum HighlightKind {
    Variable,
    Parameter,
    Label,
    RelType,
    PropertyKey,
    FunctionName,
    Namespace,
    StringLiteral,
    NumberLiteral,
    BoolLiteral,
    NullLiteral,
    Keyword,
}

#[derive(Serialize)]
struct HighlightSpan {
    start: usize,
    end: usize,
    kind: HighlightKind,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Outline {
    /// Variables introduced anywhere in the query, each tagged with the
    /// first source position they appear at. The editor uses
    /// `decl_start` to decide whether a completion candidate is in
    /// scope at the current cursor.
    variables: Vec<OutlineVariable>,
    /// Parameter names — i.e. `$foo` references.
    parameters: Vec<String>,
    /// Distinct labels seen in node patterns.
    labels: Vec<String>,
    /// Distinct relationship types seen in relationship patterns.
    rel_types: Vec<String>,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum VariableKind {
    /// Bound by a node pattern (`(n:Label)`).
    Node,
    /// Bound by a relationship pattern (`-[r:TYPE]->`).
    Relationship,
    /// Bound by `UNWIND ... AS x`, `WITH … AS y`, `RETURN … AS z`,
    /// or `SET x = expr`. Could be a primitive, list, map, ...
    Scalar,
    /// Bound by `MATCH p = (...)-...` (path-pattern binding).
    Pattern,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutlineVariable {
    name: String,
    /// Where the variable was first declared.
    decl_start: usize,
    decl_end: usize,
    /// First label / rel-type observed on the binding, if any.
    /// `(alice:Person)` gives `alice -> "Person"`;
    /// `-[r:KNOWS]->` gives `r -> "KNOWS"`.
    label: Option<String>,
    /// Which kind of binding introduced this variable. Lets the
    /// completion popup decide whether `var.property` should call
    /// `getPropertyKeys` with `kind:"node"` or `kind:"relationship"`.
    kind: VariableKind,
    /// When the variable was introduced by an `AS` alias (WITH /
    /// RETURN / UNWIND), this is the name of the source variable it
    /// was projected from — if the source was itself a simple
    /// variable reference. Lets completion follow aliases.
    alias_of: Option<String>,
}

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

// ─────────────────────────────────────────────────────────────────────
// Multi-statement support
// ─────────────────────────────────────────────────────────────────────
//
// `lora_parser::parse_query` only consumes a single statement, but the
// editor frequently hosts multi-statement scripts separated by `;`. We
// split the input on top-level `;` here so every WASM endpoint
// (`validate`, `analyse`, `outline`, `highlight`, `parse`, `format`)
// transparently handles whole documents. Offsets, line numbers, and
// pest's `--> L:C` reports are translated to whole-doc coordinates
// before returning.

/// One top-level statement carved out of the source.
struct StatementSlice {
    /// Byte offset of the slice in the original source (after leading
    /// whitespace has been trimmed).
    start: usize,
    /// Byte offset just past the last non-whitespace byte (before the
    /// terminating `;`, if any).
    end: usize,
    /// Whether the slice was followed by a `;` in the original source.
    /// `format` uses this to decide which separator to emit.
    had_terminator: bool,
}

/// Split `source` on top-level `;` that lie outside strings, comments,
/// and balanced delimiters. The trailing remainder (no `;`) is emitted
/// as its own slice. Leading + trailing whitespace inside a slice is
/// trimmed off so the parser doesn't waste cycles re-rejecting
/// whitespace.
fn split_statements(source: &str) -> Vec<StatementSlice> {
    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut out: Vec<StatementSlice> = Vec::new();
    if n == 0 {
        return out;
    }

    #[derive(PartialEq)]
    enum State {
        Normal,
        Single,
        Double,
        Back,
        Line,
        Block,
    }

    let mut state = State::Normal;
    let mut depth: i32 = 0;
    let mut seg_start: usize = 0;

    let push =
        |seg_start: usize, term_pos: usize, had_terminator: bool, out: &mut Vec<StatementSlice>| {
            let start = trim_start(source, seg_start, term_pos);
            let end = trim_end(source, seg_start, term_pos);
            if end > start {
                out.push(StatementSlice {
                    start,
                    end,
                    had_terminator,
                });
            }
        };

    let mut i: usize = 0;
    while i < n {
        let c = bytes[i];
        match state {
            State::Normal => {
                if c == b'\'' {
                    state = State::Single;
                } else if c == b'"' {
                    state = State::Double;
                } else if c == b'`' {
                    state = State::Back;
                } else if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
                    state = State::Line;
                    i += 1;
                } else if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                    state = State::Block;
                    i += 1;
                } else if c == b'(' || c == b'[' || c == b'{' {
                    depth += 1;
                } else if c == b')' || c == b']' || c == b'}' {
                    depth = (depth - 1).max(0);
                } else if c == b';' && depth == 0 {
                    push(seg_start, i, true, &mut out);
                    seg_start = i + 1;
                }
            }
            State::Single => {
                if c == b'\\' && i + 1 < n {
                    i += 1;
                } else if c == b'\'' {
                    state = State::Normal;
                }
            }
            State::Double => {
                if c == b'\\' && i + 1 < n {
                    i += 1;
                } else if c == b'"' {
                    state = State::Normal;
                }
            }
            State::Back => {
                if c == b'`' {
                    state = State::Normal;
                }
            }
            State::Line => {
                if c == b'\n' {
                    state = State::Normal;
                }
            }
            State::Block => {
                if c == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                    state = State::Normal;
                    i += 1;
                }
            }
        }
        i += 1;
    }
    // Trailing remainder. Always attempt to emit so an unterminated
    // string or comment can still surface its own diagnostic at the
    // open token.
    push(seg_start, n, false, &mut out);
    out
}

fn trim_start(source: &str, from: usize, to: usize) -> usize {
    // ASCII-only whitespace: anything multi-byte (e.g. NBSP `U+00A0`,
    // bytes `C2 A0`) is treated as content. Using `.is_whitespace()`
    // on a single byte would happily walk into a continuation byte
    // and land mid-char, which would panic the next time we sliced.
    let bytes = source.as_bytes();
    let mut i = from;
    while i < to && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}
fn trim_end(source: &str, from: usize, to: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = to;
    while i > from && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i
}

/// Count newlines in `source[0..byte_offset]`. Lines are 0-indexed in
/// the return — add 1 when translating to pest's 1-based line column.
fn line_offset_at(source: &str, byte_offset: usize) -> usize {
    let bytes = source.as_bytes();
    let end = byte_offset.min(bytes.len());
    let mut count = 0;
    for &b in &bytes[..end] {
        if b == b'\n' {
            count += 1;
        }
    }
    count
}

/// Build a byte-offset → UTF-16 code-unit offset map for `source`.
///
/// JS strings are UTF-16 indexed; Rust `&str` is UTF-8 byte indexed.
/// Returning Rust byte offsets through wasm-bindgen gives JS-side code
/// the wrong positions whenever the document contains a multi-byte
/// character (e.g. the em dash `—`). We build a translation table
/// once per WASM call, then map every offset we emit so downstream
/// consumers (CodeMirror, the editor's lint gutter) line up against
/// the document the way the user sees it.
///
/// The returned vector has length `source.len() + 1` — index by byte
/// offset, the value is the matching UTF-16 code unit offset. Indices
/// that fall on the inner bytes of a multi-byte character map to the
/// UTF-16 position of the start of that character.
fn build_utf16_offset_map(source: &str) -> Vec<u32> {
    let mut out = Vec::with_capacity(source.len() + 1);
    let mut utf16: u32 = 0;
    for (byte_idx, ch) in source.char_indices() {
        while out.len() <= byte_idx {
            out.push(utf16);
        }
        utf16 += ch.len_utf16() as u32;
    }
    while out.len() <= source.len() {
        out.push(utf16);
    }
    out
}

#[inline]
fn js_offset(map: &[u32], byte_offset: usize) -> usize {
    let idx = byte_offset.min(map.len().saturating_sub(1));
    map[idx] as usize
}

/// Rewrite the byte-offset fields of a Diagnostic to UTF-16 code-unit
/// offsets so JS consumers can use them directly as document positions.
fn diagnostic_offsets_to_js(d: &mut Diagnostic, map: &[u32]) {
    d.span.start = js_offset(map, d.span.start);
    d.span.end = js_offset(map, d.span.end);
}

fn fold_range_offsets_to_js(r: &mut FoldRange, map: &[u32]) {
    r.start = js_offset(map, r.start);
    r.end = js_offset(map, r.end);
}

fn highlight_offsets_to_js(s: &mut HighlightSpan, map: &[u32]) {
    s.start = js_offset(map, s.start);
    s.end = js_offset(map, s.end);
}

fn outline_offsets_to_js(o: &mut Outline, map: &[u32]) {
    for v in &mut o.variables {
        v.decl_start = js_offset(map, v.decl_start);
        v.decl_end = js_offset(map, v.decl_end);
    }
}

/// Translate a per-slice diagnostic into whole-doc coordinates. Rewrites
/// the byte span, the 1-based `line`, and the pest positional anchors
/// inside `details` (`--> L:C` and the `L | content` snippet line).
fn translate_diagnostic_in_place(d: &mut Diagnostic, source: &str, slice_start: usize) {
    let line_delta = line_offset_at(source, slice_start);
    d.line += line_delta;
    d.span.start += slice_start;
    d.span.end += slice_start;
    if line_delta == 0 {
        return;
    }
    // Rewrite each line in `details`:
    //   - `  --> L:C\n`           → bump L by line_delta
    //   - `L | <content>\n`       → bump leading L
    let mut out = String::with_capacity(d.details.len());
    for raw in d.details.split('\n') {
        out.push_str(&rewrite_pest_line(raw, line_delta));
        out.push('\n');
    }
    if !d.details.ends_with('\n') {
        out.pop();
    }
    d.details = out;
}

fn rewrite_pest_line(line: &str, line_delta: usize) -> String {
    // Anchor: `  --> L:C` (allow leading whitespace).
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("--> ") {
        let lead_len = line.len() - trimmed.len();
        let mut iter = rest.splitn(2, ':');
        if let (Some(l), Some(rest_col)) = (iter.next(), iter.next()) {
            if let Ok(line_n) = l.trim().parse::<usize>() {
                let new_line = line_n + line_delta;
                return format!("{}--> {new_line}:{rest_col}", &line[..lead_len]);
            }
        }
    }
    // Snippet line: ` L | <content>` — allow leading whitespace + digits.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let num_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > num_start && i < bytes.len() {
        // Allow optional whitespace then `|`.
        let mut j = i;
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'|' {
            if let Ok(n) = line[num_start..i].parse::<usize>() {
                let new_n = n + line_delta;
                return format!("{}{}{}", &line[..num_start], new_n, &line[i..]);
            }
        }
    }
    line.to_owned()
}

#[wasm_bindgen]
pub fn parse(source: &str) -> Result<JsValue, JsValue> {
    // `parse` returns a debug-printed AST string. For multi-statement
    // input the result is a `\n;\n`-joined concatenation of each
    // slice's AST when every slice parsed, otherwise None + the
    // union of errors (with translated offsets).
    let slices = split_statements(source);
    let mut asts: Vec<String> = Vec::new();
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut any_failed = false;
    if slices.is_empty() {
        let result = ParseResult {
            ok: true,
            ast: Some(String::new()),
            errors,
        };
        return serde_wasm_bindgen::to_value(&result)
            .map_err(|e| JsValue::from_str(&e.to_string()));
    }
    let utf16_map = build_utf16_offset_map(source);
    for slice in &slices {
        let slice_src = &source[slice.start..slice.end];
        match lora_parser::parse_query(slice_src) {
            Ok(doc) => asts.push(format!("{doc:#?}")),
            Err(err) => {
                any_failed = true;
                let mut d = diagnostic_from_parse_error(err, slice_src);
                translate_diagnostic_in_place(&mut d, source, slice.start);
                diagnostic_offsets_to_js(&mut d, &utf16_map);
                errors.push(d);
            }
        }
    }
    let result = ParseResult {
        ok: !any_failed,
        ast: if any_failed {
            None
        } else {
            Some(asts.join("\n;\n"))
        },
        errors,
    };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn validate(source: &str) -> Result<JsValue, JsValue> {
    let utf16_map = build_utf16_offset_map(source);
    let mut errors: Vec<Diagnostic> = Vec::new();
    for slice in split_statements(source) {
        let slice_src = &source[slice.start..slice.end];
        if let Err(err) = lora_parser::parse_query(slice_src) {
            let mut d = diagnostic_from_parse_error(err, slice_src);
            translate_diagnostic_in_place(&mut d, source, slice.start);
            diagnostic_offsets_to_js(&mut d, &utf16_map);
            errors.push(d);
        }
    }
    serde_wasm_bindgen::to_value(&errors).map_err(|e| JsValue::from_str(&e.to_string()))
}

// ─────────────────────────────────────────────────────────────────────
// Builtin function registry — sourced from `lora-builtins-meta` so the
// editor stays in lockstep with the analyzer / executor.
// ─────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinInfo {
    /// Canonical name as registered in `BUILTIN_SPECS`, e.g.
    /// `"string.upper"`, `"vector.distance"`. Aggregates are reported
    /// in their lower-case form (`"count"`, `"collect"`).
    name: String,
    /// Minimum arity (inclusive).
    min_args: usize,
    /// Maximum arity (inclusive). `None` denotes variadic.
    max_args: Option<usize>,
    /// True for entries that come from `AggregateFunction` rather than
    /// `BUILTIN_SPECS`. Lets the editor surface them differently (e.g.
    /// suggest `count` at the top level but not inside WHERE).
    is_aggregate: bool,
    /// Argument slot indices that expect an enum literal (`'L2'`,
    /// `'L1'`, …) — `vector.distance` and `vector.norm` use this.
    accepts_enum_at: Vec<usize>,
    /// Argument slot indices that expect a type literal (`INT`,
    /// `FLOAT`, …) — `cast.to` / `cast.try` / `type.is` use this.
    accepts_type_at: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinAliasInfo {
    /// Alternate name the user may type (`"date"`, `"tolower"`).
    alias: String,
    /// Resolved canonical name in `BUILTIN_SPECS`.
    canonical: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinsResult {
    /// Every namespaced builtin from `BUILTIN_SPECS` plus all
    /// aggregates, merged into one list so the editor can fan them out
    /// by namespace prefix.
    functions: Vec<BuiltinInfo>,
    /// Compatibility aliases from `BUILTIN_ALIASES` (e.g.
    /// `tolower → string.lower`, `date → temporal.now`).
    aliases: Vec<BuiltinAliasInfo>,
}

/// Snapshot the static builtin registry for the JS side. Returns
/// `{ functions, aliases }` — the editor turns this into autocomplete
/// + signature-hint metadata at load time. Pure data, no parsing
/// involved; safe to call repeatedly (JS caches the result).
#[wasm_bindgen]
pub fn builtins() -> Result<JsValue, JsValue> {
    use lora_builtins_meta::{AggregateFunction, BUILTIN_ALIASES, BUILTIN_SPECS};

    let mut functions: Vec<BuiltinInfo> = BUILTIN_SPECS
        .iter()
        .map(|spec| BuiltinInfo {
            name: spec.name.to_owned(),
            min_args: spec.arity.min,
            max_args: spec.arity.max,
            is_aggregate: false,
            accepts_enum_at: spec.enum_arg_slots.to_vec(),
            accepts_type_at: spec.type_arg_slots.to_vec(),
        })
        .collect();

    // Aggregates aren't in BUILTIN_SPECS — they live in a separate enum.
    // Enumerate by parsing the lowercase form back; the canonical list
    // is short and stable.
    for name in [
        "count",
        "sum",
        "avg",
        "min",
        "max",
        "collect",
        "stdev",
        "stdevp",
        "percentilecont",
        "percentiledisc",
    ] {
        // Use parse() so we never drift if a future aggregate is added
        // without updating this list; unknown names are skipped.
        if let Some(agg) = AggregateFunction::parse(name) {
            functions.push(BuiltinInfo {
                name: agg.name().to_owned(),
                min_args: agg.arity().min,
                max_args: agg.arity().max,
                is_aggregate: true,
                accepts_enum_at: Vec::new(),
                accepts_type_at: Vec::new(),
            });
        }
    }

    let aliases: Vec<BuiltinAliasInfo> = BUILTIN_ALIASES
        .iter()
        .map(|a| BuiltinAliasInfo {
            alias: a.alias.to_owned(),
            canonical: a.canonical.to_owned(),
        })
        .collect();

    let result = BuiltinsResult { functions, aliases };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn format(source: &str) -> String {
    // Reformat each top-level statement and rejoin with `;\n\n`. The
    // trailing `;` is preserved when the original ended with one, so
    // round-trips don't accidentally change semantics. Slices that
    // fail to parse are kept verbatim so partial work isn't destroyed.
    let slices = split_statements(source);
    if slices.is_empty() {
        return source.to_owned();
    }
    if slices.len() == 1 && !slices[0].had_terminator {
        // Single-statement input — preserve the legacy "return source
        // on parse failure" behavior exactly.
        let slice_src = &source[slices[0].start..slices[0].end];
        if lora_parser::parse_query(slice_src).is_err() {
            return source.to_owned();
        }
        return prettify(slice_src);
    }
    let total = slices.len();
    let any_trailing = slices.iter().any(|s| s.had_terminator);
    let mut out = String::with_capacity(source.len() + 16);
    for (idx, slice) in slices.iter().enumerate() {
        let slice_src = &source[slice.start..slice.end];
        let formatted = if lora_parser::parse_query(slice_src).is_ok() {
            prettify(slice_src)
        } else {
            slice_src.to_owned()
        };
        let trimmed = formatted.trim_end_matches('\n');
        out.push_str(trimmed);
        let is_last = idx + 1 == total;
        if !is_last {
            out.push_str(";\n\n");
        } else if any_trailing && slice.had_terminator {
            out.push_str(";\n");
        } else {
            out.push('\n');
        }
    }
    out
}

/// AST-driven highlight pass. Returns an array of `{ start, end, kind }`
/// spans for the editor to mark with CSS classes. For multi-statement
/// scripts the spans cover every statement that parses; statements
/// that fail to parse contribute nothing (the editor's StreamLanguage
/// covers keyword colouring as a baseline).
#[wasm_bindgen]
pub fn highlight(source: &str) -> Result<JsValue, JsValue> {
    let utf16_map = build_utf16_offset_map(source);
    let mut all_spans: Vec<HighlightSpan> = Vec::new();
    for slice in split_statements(source) {
        let slice_src = &source[slice.start..slice.end];
        let mut spans = if let Ok(doc) = lora_parser::parse_query(slice_src) {
            collect_highlights(&doc, slice_src)
        } else {
            // Slice doesn't parse — likely a multi-statement chunk
            // without `;` separators, or a query mid-edit. Fall back to
            // a lexical scan so labels / rel-types / properties /
            // parameters / literals still get coloured. The AST path
            // remains authoritative once the slice becomes parseable.
            collect_highlights_lexical(slice_src)
        };
        for s in &mut spans {
            s.start += slice.start;
            s.end += slice.start;
            highlight_offsets_to_js(s, &utf16_map);
        }
        all_spans.extend(spans);
    }
    serde_wasm_bindgen::to_value(&all_spans).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Lightweight scope summary used by the autocomplete popup: which
/// variables / parameters / labels / rel-types appear anywhere in the
/// document, plus where each variable was first bound. For
/// multi-statement scripts the outlines from every parseable slice are
/// merged with global offsets; variable / parameter / label /
/// rel-type names are deduped (first occurrence wins, preserving the
/// declaration site).
#[wasm_bindgen]
pub fn outline(source: &str) -> Result<JsValue, JsValue> {
    let utf16_map = build_utf16_offset_map(source);
    let mut merged = Outline::default();
    let mut seen_vars: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_params: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_labels: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_rels: std::collections::HashSet<String> = std::collections::HashSet::new();
    for slice in split_statements(source) {
        let slice_src = &source[slice.start..slice.end];
        let Ok(doc) = lora_parser::parse_query(slice_src) else {
            continue;
        };
        let local = collect_outline(&doc, slice_src);
        for mut v in local.variables {
            if seen_vars.insert(v.name.clone()) {
                v.decl_start += slice.start;
                v.decl_end += slice.start;
                merged.variables.push(v);
            }
        }
        for p in local.parameters {
            if seen_params.insert(p.clone()) {
                merged.parameters.push(p);
            }
        }
        for l in local.labels {
            if seen_labels.insert(l.clone()) {
                merged.labels.push(l);
            }
        }
        for r in local.rel_types {
            if seen_rels.insert(r.clone()) {
                merged.rel_types.push(r);
            }
        }
    }
    outline_offsets_to_js(&mut merged, &utf16_map);
    serde_wasm_bindgen::to_value(&merged).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FoldRange {
    start: usize,
    end: usize,
    /// Soft category for the folded region — useful if the host wants
    /// to label collapsed regions differently (`pattern`, `properties`,
    /// `projection`, `case`, ...).
    kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Analysis {
    /// Semantic diagnostics — undeclared variables, unknown labels /
    /// rel types (against the host-provided lists), unused bindings.
    /// Syntax errors are *not* duplicated here; use [`validate`] for
    /// those.
    diagnostics: Vec<Diagnostic>,
    /// Suggested fold ranges, sorted by `start`.
    fold_ranges: Vec<FoldRange>,
}

/// Host-provided context to drive semantic checks. Each field is
/// optional — pass empty arrays when you don't have schema info.
#[derive(serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AnalyseConfig {
    /// Known node labels. Unknown labels produce a warning when
    /// `strict_labels` is also true.
    labels: Vec<String>,
    rel_types: Vec<String>,
    /// When true, raise a warning for labels / rel-types that aren't in
    /// the provided lists. Default false so an empty list doesn't flood
    /// the editor with warnings.
    strict_labels: bool,
    strict_rel_types: bool,
}

/// Run a second-pass analysis on the source. The host should pass any
/// known schema info in `config` — labels / rel-types / strict flags
/// — and we'll surface mismatches as warnings the editor can render
/// alongside the syntax linter's errors.
///
/// Multi-statement scripts are analysed per top-level slice:
/// diagnostics + fold ranges are emitted with whole-doc offsets, and
/// each slice contributes one extra `kind: "query"` fold range so the
/// chevron on the slice's first line collapses the full statement.
/// Slices that don't parse contribute no semantic diagnostics — the
/// syntactic error comes from [`validate`] — but the surrounding
/// clean slices still get their RETURN / undeclared-variable checks.
#[wasm_bindgen]
pub fn analyse(source: &str, config: JsValue) -> Result<JsValue, JsValue> {
    let cfg: AnalyseConfig = if config.is_undefined() || config.is_null() {
        AnalyseConfig::default()
    } else {
        serde_wasm_bindgen::from_value(config).unwrap_or_default()
    };
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut fold_ranges: Vec<FoldRange> = Vec::new();
    let slices = split_statements(source);
    let multi = slices.len() > 1;
    let utf16_map = build_utf16_offset_map(source);
    for slice in &slices {
        let slice_src = &source[slice.start..slice.end];
        // Emit a `kind: "query"` fold range for every slice when the doc
        // holds more than one statement — this gives each statement a
        // chevron on its first line regardless of whether the slice
        // parses cleanly. Single-statement docs skip this so the
        // `query`-level fold doesn't compete with the natural clause
        // folds emitted below.
        if multi {
            fold_ranges.push(FoldRange {
                start: slice.start,
                end: slice.end,
                kind: "query".to_owned(),
            });
        }
        let Ok(doc) = lora_parser::parse_query(slice_src) else {
            continue;
        };
        let outline = collect_outline(&doc, slice_src);
        let mut local_diags: Vec<Diagnostic> = Vec::new();
        check_undeclared_uses(&outline, slice_src, &mut local_diags);
        check_schema(&outline, &cfg, &mut local_diags);
        check_unused_bindings(&outline, slice_src, &mut local_diags);
        check_unknown_functions(&doc, slice_src, &mut local_diags);
        for mut d in local_diags {
            translate_diagnostic_in_place(&mut d, source, slice.start);
            diagnostics.push(d);
        }
        for r in collect_fold_ranges(&doc, slice_src) {
            fold_ranges.push(FoldRange {
                start: r.start + slice.start,
                end: r.end + slice.start,
                kind: r.kind,
            });
        }
    }
    fold_ranges.sort_by_key(|r| r.start);
    for d in &mut diagnostics {
        diagnostic_offsets_to_js(d, &utf16_map);
    }
    for r in &mut fold_ranges {
        fold_range_offsets_to_js(r, &utf16_map);
    }
    let analysis = Analysis {
        diagnostics,
        fold_ranges,
    };
    serde_wasm_bindgen::to_value(&analysis).map_err(|e| JsValue::from_str(&e.to_string()))
}

// ─────────────────────────────────────────────────────────────────────
// Diagnostic helpers
// ─────────────────────────────────────────────────────────────────────

fn diagnostic_from_parse_error(err: lora_parser::ParseError, source: &str) -> Diagnostic {
    let pest_message = match err {
        lora_parser::ParseError::Message { message, .. } => message,
    };

    let (line, column) = parse_line_col(&pest_message).unwrap_or((1, 1));
    let (start, end) = line_col_to_span(source, line, column);
    let expected = parse_expected(&pest_message);
    let summary = diagnose_summary(&expected, source, start);
    let what_we_wanted = if expected.is_empty() {
        format!("Parse error at line {line}, column {column}.")
    } else {
        format!(
            "Expected {} at line {line}, column {column}.",
            humanize_expected(&expected),
        )
    };
    let message = match summary {
        Some(s) => format!("{s}\n{what_we_wanted}"),
        None => what_we_wanted,
    };

    let examples = examples_for(&expected);
    Diagnostic {
        severity: Severity::Error,
        message,
        details: pest_message,
        line,
        column,
        expected,
        examples,
        span: Span { start, end },
    }
}

/// Map expected rule names to short, concrete code snippets the user
/// could drop in. Returns at most six examples — order matters: the
/// first item is the most idiomatic.
fn examples_for(rules: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if !seen.iter().any(|e| e == s) && seen.len() < 6 {
            seen.push(s.to_owned());
        }
    };
    for rule in rules {
        match rule.as_str() {
            "node_pattern" => {
                push("(n)");
                push("(n:Label)");
                push("(n:Label {key: 'value'})");
            }
            "relationship_pattern" | "pattern_element_chain" => {
                push("-[:KNOWS]->");
                push("-[r:KNOWS]->");
                push("-[r:KNOWS*1..3]->");
            }
            "shortest_path_pattern" => {
                push("shortestPath((a)-[*]-(b))");
            }
            "node_label_set" | "rel_type_name" | "rel_types" => {
                push(":Person");
                push(":KNOWS|FOLLOWS");
            }
            "properties" | "map_literal" => {
                push("{name: 'Alice'}");
                push("{since: 2024, active: TRUE}");
            }
            "string_literal" => {
                push("'Alice'");
                push("\"Alice\"");
            }
            "integer_literal" | "decimal_literal" => {
                push("42");
                push("3.14");
            }
            "boolean_literal" => {
                push("TRUE");
                push("FALSE");
            }
            "null_literal" => {
                push("NULL");
            }
            "parameter" => {
                push("$name");
            }
            "list_literal" => {
                push("[1, 2, 3]");
                push("['a', 'b']");
            }
            "function_invocation" | "function_name" => {
                push("count(*)");
                push("string.upper(n.name)");
                push("math.abs(x)");
            }
            "case_expression" => {
                push("CASE x WHEN 1 THEN 'one' ELSE 'other' END");
            }
            "where_clause" => {
                push("WHERE n.name = 'Alice'");
            }
            "return_clause" => {
                push("RETURN n");
                push("RETURN n.name AS name");
            }
            "with_clause" => {
                push("WITH n, count(*) AS c");
            }
            "match_clause" => {
                push("MATCH (n:Person) RETURN n");
            }
            "order_clause" => {
                push("ORDER BY n.name DESC");
            }
            "limit_clause" => {
                push("LIMIT 10");
            }
            "skip_clause" => {
                push("SKIP 5");
            }
            "expression" | "unary_expression" | "atom" => {
                push("n.name");
                push("42");
                push("'hello'");
            }
            "variable" | "symbolic_name" => {
                push("n");
                push("person");
            }
            _ => {}
        }
    }
    seen
}

/// Find the `--> L:C` line in a pest error message and return `(L, C)`.
fn parse_line_col(message: &str) -> Option<(usize, usize)> {
    let marker = "--> ";
    let idx = message.find(marker)?;
    let rest = &message[idx + marker.len()..];
    let coord = rest
        .split(|c: char| c == '\n' || c.is_whitespace())
        .next()?;
    let (line_str, col_str) = coord.split_once(':')?;
    Some((line_str.parse().ok()?, col_str.parse().ok()?))
}

/// Parse the `= expected foo, bar, or baz` footer.
fn parse_expected(message: &str) -> Vec<String> {
    let needle = "= expected ";
    let Some(idx) = message.find(needle) else {
        return Vec::new();
    };
    let rest = &message[idx + needle.len()..];
    let line = rest.lines().next().unwrap_or(rest);
    line.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty() && *s != "or")
        .map(str::to_owned)
        .collect()
}

fn humanize_expected(rules: &[String]) -> String {
    let parts: Vec<String> = rules.iter().map(|r| friendly_rule(r)).collect();
    match parts.as_slice() {
        [] => "more input".to_owned(),
        [one] => one.clone(),
        [a, b] => format!("{a} or {b}"),
        _ => {
            let (last, init) = parts.split_last().unwrap();
            format!("{}, or {last}", init.join(", "))
        }
    }
}

fn friendly_rule(rule: &str) -> String {
    match rule {
        // ── Patterns ─────────────────────────────────────────────────
        "node_pattern" => "a node pattern (e.g. `(n)` or `(n:Label)`)".into(),
        "relationship_pattern" => "a relationship pattern (e.g. `-[r:TYPE]->`)".into(),
        "pattern" => "a graph pattern".into(),
        "pattern_part" => "a pattern part".into(),
        "pattern_element" => "a pattern element".into(),
        "pattern_element_chain" => "a relationship chain (e.g. `-[r]->(b)`)".into(),
        "shortest_path_pattern" => "a `shortestPath(...)` or `allShortestPaths(...)` call".into(),
        "node_label_set" => "one or more `:Label`s".into(),
        "node_labels" => "one or more `:Label`s on the node".into(),
        "rel_type_name" => "a relationship type (e.g. `KNOWS`)".into(),
        "rel_types" => "one or more relationship types (e.g. `:KNOWS|FOLLOWS`)".into(),
        "range_literal" => "a length range (e.g. `*1..3`)".into(),

        // ── Names ────────────────────────────────────────────────────
        "symbolic_name" => "a name".into(),
        "schema_name" => "a label or relationship-type name".into(),
        "variable" => "a variable name".into(),
        "property_key_name" => "a property name".into(),
        "function_name" => "a function name (e.g. `count`, `string.upper`)".into(),
        "procedure_name" => "a procedure name (e.g. `db.indexes`)".into(),
        "namespace" => "a namespace prefix (e.g. `math.`, `string.`)".into(),

        // ── Expressions ──────────────────────────────────────────────
        "expression" => "an expression".into(),
        "unary_expression" => "an expression (value, variable, or function call)".into(),
        "atom" => "a value".into(),
        "literal" => "a literal value".into(),
        "list_literal" => "a list literal (e.g. `[1, 2, 3]`)".into(),
        "map_literal" => "a map literal (e.g. `{name: 'a'}`)".into(),
        "parenthesized_expression" => "a parenthesised expression".into(),
        "case_expression" => "a `CASE ... END` expression".into(),
        "function_invocation" => "a function call (e.g. `count(n)`)".into(),

        // ── Properties / maps ────────────────────────────────────────
        "properties" => "a properties map (e.g. `{name: 'Alice'}`)".into(),
        "map_entry" => "a `key: value` entry".into(),

        // ── Literals ─────────────────────────────────────────────────
        "string_literal" => "a quoted string (e.g. `'Alice'` or `\"Alice\"`)".into(),
        "integer_literal" => "an integer".into(),
        "decimal_literal" => "a number".into(),
        "boolean_literal" => "`TRUE` or `FALSE`".into(),
        "null_literal" => "`NULL`".into(),
        "parameter" => "a parameter (e.g. `$name`)".into(),

        // ── Clauses ──────────────────────────────────────────────────
        "match_clause" => "a `MATCH ...` clause".into(),
        "where_clause" => "a `WHERE` filter".into(),
        "return_clause" => "a `RETURN ...` clause".into(),
        "with_clause" => "a `WITH ...` projection".into(),
        "create_clause" => "a `CREATE ...` clause".into(),
        "merge_clause" => "a `MERGE ...` clause".into(),
        "delete_clause" => "a `DELETE ...` clause".into(),
        "set_clause" => "a `SET ...` clause".into(),
        "remove_clause" => "a `REMOVE ...` clause".into(),
        "unwind_clause" => "an `UNWIND ... AS ...` clause".into(),
        "call_clause" => "a `CALL ...` clause".into(),
        "yield_clause" => "a `YIELD ...` clause".into(),
        "order_clause" => "an `ORDER BY ...` clause".into(),
        "limit_clause" => "a `LIMIT n` clause".into(),
        "skip_clause" => "a `SKIP n` clause".into(),
        "projection_item" => "a projection (expression or `*`)".into(),
        "projection_body" => "one or more projections".into(),

        // ── Misc ─────────────────────────────────────────────────────
        "EOI" => "end of input".into(),
        "COMMENT" => "a comment".into(),
        "WHITESPACE" => "whitespace".into(),
        other => other.replace('_', " "),
    }
}

/// Generate a high-level, plain-English summary of what likely went
/// wrong, prepended to the structured message. We look for distinctive
/// rule sets in `expected` to recognise common situations.
fn diagnose_summary(expected: &[String], source: &str, byte_pos: usize) -> Option<String> {
    let near = &source[..byte_pos.min(source.len())];
    let after = &source[byte_pos.min(source.len())..];

    if expected.iter().any(|r| r == "EOI") {
        return Some(
            "Extra content after the query — every clause may already be complete.".into(),
        );
    }
    if expected
        .iter()
        .any(|r| r == "node_pattern" || r == "properties" || r == "node_label_set")
        && near.trim_end().ends_with('(')
    {
        return Some(
            "An open `(` is missing the node it describes. Try `(n)`, `(n:Label)`, or `(n {key: value})`.".into(),
        );
    }
    if expected.iter().any(|r| r == "relationship_pattern") && near.trim_end().ends_with('-') {
        return Some(
            "A relationship pattern was started but never closed. Try `-[r:TYPE]->` or `-->`."
                .into(),
        );
    }
    if expected
        .iter()
        .any(|r| r == "schema_name" || r == "rel_type_name")
        && near.trim_end().ends_with(':')
    {
        return Some(
            "After `:`, name the label or relationship type (e.g. `:Person`, `:KNOWS`).".into(),
        );
    }
    if expected
        .iter()
        .any(|r| r == "expression" || r == "unary_expression")
    {
        if after.starts_with('\'') || after.starts_with('"') {
            return Some(
                "Looks like an unterminated string literal — the quote was never closed.".into(),
            );
        }
        if near
            .chars()
            .rev()
            .find(|c| !c.is_whitespace())
            .is_some_and(|c| matches!(c, '=' | '<' | '>' | '+' | '-' | '*' | '/' | '%' | ',' | '('))
        {
            return Some("Missing a value on the right-hand side of an operator.".into());
        }
    }
    None
}

/// Convert a 1-based (line, column) coordinate into byte offsets.
fn line_col_to_span(source: &str, line: usize, column: usize) -> (usize, usize) {
    let mut offset = 0usize;
    let mut current_line = 1usize;
    for ch in source.chars() {
        if current_line == line {
            break;
        }
        offset += ch.len_utf8();
        if ch == '\n' {
            current_line += 1;
        }
    }

    let line_start = offset;
    for (col_count, ch) in (1usize..).zip(source[line_start..].chars()) {
        if col_count >= column {
            break;
        }
        if ch == '\n' {
            break;
        }
        offset += ch.len_utf8();
    }

    let start = offset.min(source.len());
    let end = (start + next_char_len(source, start)).min(source.len());
    (start, end.max(start))
}

fn next_char_len(source: &str, at: usize) -> usize {
    source[at..].chars().next().map(char::len_utf8).unwrap_or(1)
}

// ─────────────────────────────────────────────────────────────────────
// Pretty-printer
// ─────────────────────────────────────────────────────────────────────

const KEYWORDS: &[&str] = &[
    "MATCH",
    "OPTIONAL",
    "WHERE",
    "RETURN",
    "WITH",
    "CREATE",
    "MERGE",
    "DELETE",
    "DETACH",
    "SET",
    "REMOVE",
    "UNWIND",
    "ORDER",
    "BY",
    "ASC",
    "DESC",
    "LIMIT",
    "SKIP",
    "AS",
    "AND",
    "OR",
    "XOR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "TRUE",
    "FALSE",
    "CALL",
    "YIELD",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "DISTINCT",
    "EXISTS",
    "CONSTRAINT",
    "INDEX",
    "SHOW",
    "DROP",
    "UNIQUE",
    "FOR",
    "ON",
    "REQUIRE",
];

/// Append the UTF-8 character starting at byte offset `i` in `src` to
/// `out`. Returns the number of bytes consumed (1 for ASCII, 2-4 for
/// multi-byte sequences).
///
/// Use this instead of `out.push(bytes[i] as char)` — that cast
/// re-interprets a single UTF-8 byte as a Latin-1 code point, which
/// then re-encodes as two bytes when pushed to a `String`, corrupting
/// every non-ASCII character (e.g. an em-dash `—` turns into mojibake).
fn append_utf8_char(out: &mut String, src: &str, i: usize) -> usize {
    let bytes = src.as_bytes();
    let b = bytes[i];
    if b < 0x80 {
        out.push(b as char);
        return 1;
    }
    // Leading-byte → expected UTF-8 sequence length.
    let len = if b < 0xC0 {
        1 // stray continuation byte; copy verbatim
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    };
    let end = (i + len).min(bytes.len());
    out.push_str(&src[i..end]);
    end - i
}

fn prettify(source: &str) -> String {
    // 1. lexical tidy (commas, trailing whitespace, blank lines)
    // 2. uppercase keywords (string-aware)
    // 3. fold AND/OR continuation lines back onto their predicate so
    //    step 7 sees a single-line WHERE body it can re-format.
    // 4. collapse multi-line content inside (), [], {}, and CASE..END
    //    onto a single logical line. This canonicalises hand-formatted
    //    input so the downstream splitters see one consistent shape.
    // 5. break each top-level clause onto its own line
    // 6. reformat projection lists (RETURN/WITH ≥ 3 items split, else inline)
    // 7. split chained WHERE predicates (AND/OR ≥ 1 connective)
    // 8. expand large CREATE/MERGE/SET property maps onto one key per line
    // 9. split CASE..WHEN..THEN..ELSE..END across lines for readability
    // 10. indent CALL { ... } subquery bodies one level deeper
    let s1 = normalize_whitespace(source);
    let s2 = uppercase_keywords(&s1);
    let s3 = join_logical_continuations(&s2);
    let s4 = collapse_multiline_blocks(&s3);
    let s5 = reflow_clauses(&s4);
    let s6 = split_long_projections(&s5);
    let s7 = split_long_where(&s6);
    let s8 = split_long_property_maps(&s7);
    let s9 = split_case_expressions(&s8);
    indent_call_subqueries(&s9)
}

/// Join lines whose first non-whitespace token is `AND` or `OR`
/// (uppercased) onto the previous line. Lets later passes treat
/// chained predicates as a single-line WHERE body, regardless of
/// whether the user wrote them inline or split across lines.
fn join_logical_continuations(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut iter = source.lines().peekable();
    let mut first = true;
    while let Some(line) = iter.next() {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(line);
        while let Some(&next) = iter.peek() {
            let trimmed = next.trim_start();
            let is_cont = trimmed.starts_with("AND ")
                || trimmed.starts_with("OR ")
                || trimmed == "AND"
                || trimmed == "OR";
            if !is_cont {
                break;
            }
            iter.next();
            out.push(' ');
            out.push_str(trimmed);
        }
    }
    if source.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Reformat every `RETURN` / `WITH` projection. The pass collects the
/// body across any continuation lines (a leading `RETURN` or `WITH`
/// followed by lines that aren't themselves clause starters), then
/// emits 3+-item bodies one-per-line and ≤2-item bodies inline.
///
/// Multi-line input (already split by a previous prettify or by hand)
/// is normalised the same way as inline input, so the pass is
/// idempotent.
fn split_long_projections(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];

        let (kw, after_kw) = if let Some(rest) = trimmed.strip_prefix("RETURN ") {
            ("RETURN", rest)
        } else if let Some(rest) = trimmed.strip_prefix("WITH ") {
            ("WITH", rest)
        } else if trimmed == "RETURN" || trimmed == "WITH" {
            (
                if trimmed == "RETURN" {
                    "RETURN"
                } else {
                    "WITH"
                },
                "",
            )
        } else {
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        };

        // Collect the body across continuation lines. A continuation
        // is any non-blank line that doesn't start with another clause
        // keyword — that lets us absorb hand-split bodies like
        //   RETURN a,
        //          b,
        //          c
        // into one logical body before re-emitting.
        let mut body = String::new();
        body.push_str(after_kw);
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            let next_trim = next.trim_start();
            if next_trim.is_empty() {
                break;
            }
            if starts_with_clause_keyword(next_trim) {
                break;
            }
            if !body.is_empty() && !body.ends_with(' ') {
                body.push(' ');
            }
            body.push_str(next_trim);
            j += 1;
        }

        let body = body.trim().trim_end_matches(',').trim_end();

        // Pull off a leading DISTINCT, if any.
        let (lead_label, body) = if let Some(rest) = body.strip_prefix("DISTINCT ") {
            (format!("{kw} DISTINCT"), rest)
        } else {
            (kw.to_owned(), body)
        };

        let parts: Vec<&str> = split_top_level_commas(body)
            .into_iter()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();

        if parts.is_empty() {
            out.push_str(indent);
            out.push_str(&lead_label);
            out.push('\n');
        } else if !should_split_projection(&parts) {
            out.push_str(indent);
            out.push_str(&lead_label);
            out.push(' ');
            out.push_str(&parts.join(", "));
            out.push('\n');
        } else {
            let item_indent = format!("{indent}  ");
            out.push_str(indent);
            out.push_str(&lead_label);
            out.push('\n');
            for (idx, item) in parts.iter().enumerate() {
                out.push_str(&item_indent);
                out.push_str(item);
                if idx + 1 < parts.len() {
                    out.push(',');
                }
                out.push('\n');
            }
        }

        i = j;
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Decide whether a projection body is "complex enough" to deserve a
/// multi-line layout. Always split when there are 3+ items. For
/// 2-item bodies, split only when at least one item carries an alias
/// (`expr AS name`) or contains a function call — those are the cases
/// where the visual weight of each item rewards being on its own line.
fn should_split_projection(parts: &[&str]) -> bool {
    if parts.len() >= 3 {
        return true;
    }
    if parts.len() < 2 {
        return false;
    }
    parts.iter().any(|p| p.contains(" AS ") || p.contains('('))
}

/// Whether `trimmed` (a left-trimmed line) starts with one of the
/// known Cypher clause-starter keywords, followed by a word boundary.
fn starts_with_clause_keyword(trimmed: &str) -> bool {
    let bytes = trimmed.as_bytes();
    for kw in CLAUSE_STARTERS {
        if bytes.len() < kw.len() {
            continue;
        }
        // Compare on raw bytes — see match_clause_starter for the
        // motivation: slicing `trimmed[..kw.len()]` would panic when
        // the input has a multi-byte UTF-8 char straddling that index.
        if !bytes[..kw.len()].eq_ignore_ascii_case(kw.as_bytes()) {
            continue;
        }
        let after = bytes.get(kw.len()).copied();
        let is_boundary = match after {
            None => true,
            Some(b) => !(b.is_ascii_alphanumeric() || b == b'_'),
        };
        if is_boundary {
            return true;
        }
    }
    false
}

/// Split `s` at every top-level (depth-0) comma, respecting balanced
/// `()` / `[]` / `{}` and quoted strings. Trailing whitespace on each
/// item is preserved as-is — the caller decides how to format.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut state: u8 = 0; // 0 normal, 1 single-q, 2 double-q, 3 backtick
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                } else if c == b'"' {
                    state = 2;
                } else if c == b'`' {
                    state = 3;
                } else if c == b'(' || c == b'[' || c == b'{' {
                    depth += 1;
                } else if c == b')' || c == b']' || c == b'}' {
                    depth = (depth - 1).max(0);
                } else if c == b',' && depth == 0 {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                } else if c == b'\'' {
                    state = 0;
                }
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                } else if c == b'"' {
                    state = 0;
                }
            }
            3 if c == b'`' => {
                state = 0;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&s[start..]);
    out
}

/// One predicate fragment in a chained WHERE — `connector` is the
/// keyword that joins it to the previous fragment (`""` for the head).
struct LogicalPart<'a> {
    connector: &'static str,
    text: &'a str,
}

/// Split a WHERE body at every top-level `AND` / `OR` keyword. Inside
/// parens/brackets/braces, strings, or backticks the split is suppressed,
/// so `WHERE (a AND b) OR c` yields two parts, not three.
fn split_top_level_logical(s: &str) -> Vec<LogicalPart<'_>> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut state: u8 = 0; // 0 normal, 1 single-q, 2 double-q, 3 backtick
    let mut parts: Vec<LogicalPart<'_>> = Vec::new();
    let mut connector: &'static str = "";
    let mut head_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                match c {
                    b'\'' => {
                        state = 1;
                        i += 1;
                        continue;
                    }
                    b'"' => {
                        state = 2;
                        i += 1;
                        continue;
                    }
                    b'`' => {
                        state = 3;
                        i += 1;
                        continue;
                    }
                    b'(' | b'[' | b'{' => {
                        depth += 1;
                        i += 1;
                        continue;
                    }
                    b')' | b']' | b'}' => {
                        depth = (depth - 1).max(0);
                        i += 1;
                        continue;
                    }
                    _ => {}
                }

                if depth == 0 && matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                    let ws_start = i;
                    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                        i += 1;
                    }
                    let kw_len = if bytes.get(i..i + 3) == Some(b"AND") {
                        3
                    } else if bytes.get(i..i + 2) == Some(b"OR") {
                        2
                    } else {
                        0
                    };
                    if kw_len > 0 {
                        let after = bytes.get(i + kw_len).copied();
                        let is_boundary = match after {
                            None => true,
                            Some(b) => !(b.is_ascii_alphanumeric() || b == b'_'),
                        };
                        if is_boundary {
                            parts.push(LogicalPart {
                                connector,
                                text: &s[head_start..ws_start],
                            });
                            connector = if kw_len == 3 { "AND" } else { "OR" };
                            i += kw_len;
                            while i < bytes.len()
                                && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r')
                            {
                                i += 1;
                            }
                            head_start = i;
                            continue;
                        }
                    }
                    continue;
                }
                i += 1;
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    state = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                if c == b'`' {
                    state = 0;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    parts.push(LogicalPart {
        connector,
        text: &s[head_start..],
    });
    parts
}

/// When a WHERE clause body holds 2+ top-level predicates joined by
/// `AND` / `OR`, put each one on its own line indented two spaces
/// under the WHERE. Single-predicate WHERE clauses stay inline.
fn split_long_where(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];

        let Some(body) = trimmed.strip_prefix("WHERE ") else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        let parts = split_top_level_logical(body);
        if parts.len() < 2 {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let cont_indent = format!("{indent}  ");
        out.push_str(indent);
        out.push_str("WHERE ");
        out.push_str(parts[0].text.trim());
        out.push('\n');
        for part in &parts[1..] {
            out.push_str(&cont_indent);
            out.push_str(part.connector);
            out.push(' ');
            out.push_str(part.text.trim());
            out.push('\n');
        }
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Collapse multi-line content inside any open `(`, `[`, `{`, or
/// `CASE...END` region onto a single logical line. Newlines + runs of
/// whitespace become a single space; strings and comments are
/// preserved verbatim. Content outside these regions (clause-level
/// newlines, blank lines between clauses) is left alone.
///
/// Running this before [`reflow_clauses`] gives the downstream
/// splitters (`split_long_projections`, `split_long_property_maps`,
/// `split_case_expressions`) a canonical "everything on one line"
/// shape regardless of how the user hand-formatted the source. The
/// CALL subquery body intentionally gets collapsed too — the clauses
/// inside are re-broken onto their own lines by `reflow_clauses`
/// because brace depth is not tracked there.
fn collapse_multiline_blocks(source: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        Str(u8),
        Line,
        Block,
    }

    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut state = State::Normal;
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut case_depth = 0i32;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            State::Str(q) => {
                if c == b'\\' && i + 1 < bytes.len() {
                    out.push('\\');
                    i += 1;
                    i += append_utf8_char(&mut out, source, i);
                    continue;
                }
                let consumed = append_utf8_char(&mut out, source, i);
                if c == q {
                    state = State::Normal;
                }
                i += consumed;
                continue;
            }
            State::Line => {
                let consumed = append_utf8_char(&mut out, source, i);
                if c == b'\n' {
                    state = State::Normal;
                }
                i += consumed;
                continue;
            }
            State::Block => {
                if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str("*/");
                    state = State::Normal;
                    i += 2;
                    continue;
                }
                i += append_utf8_char(&mut out, source, i);
                continue;
            }
            State::Normal => {}
        }

        if c == b'\'' || c == b'"' || c == b'`' {
            out.push(c as char);
            state = State::Str(c);
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            out.push_str("//");
            state = State::Line;
            i += 2;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out.push_str("/*");
            state = State::Block;
            i += 2;
            continue;
        }
        if c == b'(' {
            paren += 1;
            out.push('(');
            i += 1;
            continue;
        }
        if c == b')' {
            paren = (paren - 1).max(0);
            out.push(')');
            i += 1;
            continue;
        }
        if c == b'[' {
            bracket += 1;
            out.push('[');
            i += 1;
            continue;
        }
        if c == b']' {
            bracket = (bracket - 1).max(0);
            out.push(']');
            i += 1;
            continue;
        }
        if c == b'{' {
            brace += 1;
            out.push('{');
            i += 1;
            continue;
        }
        if c == b'}' {
            brace = (brace - 1).max(0);
            out.push('}');
            i += 1;
            continue;
        }

        // Track CASE / END so multi-line CASE bodies outside any
        // bracket nesting also get collapsed.
        let prev_is_boundary = i == 0
            || !{
                let p = bytes[i - 1];
                p.is_ascii_alphanumeric() || p == b'_'
            };
        if c < 0x80
            && prev_is_boundary
            && bytes.get(i..i + 4) == Some(b"CASE")
            && !is_word_char(bytes.get(i + 4).copied())
        {
            case_depth += 1;
            out.push_str("CASE");
            i += 4;
            continue;
        }
        if c < 0x80
            && prev_is_boundary
            && bytes.get(i..i + 3) == Some(b"END")
            && !is_word_char(bytes.get(i + 3).copied())
        {
            case_depth = (case_depth - 1).max(0);
            out.push_str("END");
            i += 3;
            continue;
        }

        // Whitespace handling: when we're inside any nesting, collapse
        // runs of whitespace (including newlines) into a single space.
        // Outside nesting, preserve verbatim so clause-level newlines /
        // blank lines survive for `reflow_clauses` to consume.
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            let inside = paren > 0 || bracket > 0 || brace > 0 || case_depth > 0;
            if inside {
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                out.push(' ');
                continue;
            }
            i += append_utf8_char(&mut out, source, i);
            continue;
        }

        i += append_utf8_char(&mut out, source, i);
    }
    out
}

/// Expand large property maps on `CREATE` / `MERGE` / `SET` / `REMOVE`
/// / `ON CREATE SET` / `ON MATCH SET` / `DELETE` lines onto one key per
/// line. A map is "large" when it has 3+ entries, when its inline body
/// is longer than 60 characters, or when it contains a `CASE`
/// expression. The closing `}` returns to the line's base indent;
/// anything after the map (e.g. the `)` of an enclosing node pattern,
/// or `]->...` of a relationship pattern) stays on that same line.
fn split_long_property_maps(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 32);
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];

        if !is_write_clause_line(trimmed) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        match split_maps_on_line(line, indent) {
            Some(rewritten) => out.push_str(&rewritten),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Whether `trimmed` (already left-trimmed) begins a write clause whose
/// inline property maps are eligible for expansion.
fn is_write_clause_line(trimmed: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "CREATE ",
        "MERGE ",
        "SET ",
        "REMOVE ",
        "DELETE ",
        "DETACH DELETE ",
        "ON CREATE SET ",
        "ON MATCH SET ",
    ];
    PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

/// Rewrite a single line, expanding every eligible `{ ... }` property
/// map. Returns `Some(rewritten)` if at least one map was split, `None`
/// when the line was already canonical (so the caller can copy it
/// verbatim without paying an allocation).
fn split_maps_on_line(line: &str, indent: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len() + 32);
    let inner_indent = format!("{indent}  ");
    let mut state: u8 = 0; // 0 normal, 1 single-q, 2 double-q, 3 backtick
    let mut changed = false;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    out.push('\'');
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    out.push('"');
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    out.push('`');
                    i += 1;
                    continue;
                }
                if c == b'{' {
                    if let Some(end_idx) = find_matching_brace(bytes, i) {
                        let body = &line[i + 1..end_idx];
                        let parts = split_top_level_commas(body);
                        let non_empty: Vec<&str> = parts
                            .iter()
                            .map(|p| p.trim())
                            .filter(|p| !p.is_empty())
                            .collect();
                        let body_trim = body.trim();
                        let should_split = non_empty.len() >= 3
                            || body_trim.len() > 60
                            || contains_top_level_case(body);
                        if should_split && !non_empty.is_empty() {
                            out.push('{');
                            out.push('\n');
                            for (idx, part) in non_empty.iter().enumerate() {
                                out.push_str(&inner_indent);
                                out.push_str(part);
                                if idx + 1 < non_empty.len() {
                                    out.push(',');
                                }
                                out.push('\n');
                            }
                            out.push_str(indent);
                            out.push('}');
                            changed = true;
                            i = end_idx + 1;
                            continue;
                        }
                    }
                    out.push('{');
                    i += 1;
                    continue;
                }
                i += append_utf8_char(&mut out, line, i);
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    out.push('\\');
                    i += 1;
                    i += append_utf8_char(&mut out, line, i);
                    continue;
                }
                let consumed = append_utf8_char(&mut out, line, i);
                if c == b'\'' {
                    state = 0;
                }
                i += consumed;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    out.push('\\');
                    i += 1;
                    i += append_utf8_char(&mut out, line, i);
                    continue;
                }
                let consumed = append_utf8_char(&mut out, line, i);
                if c == b'"' {
                    state = 0;
                }
                i += consumed;
            }
            3 => {
                let consumed = append_utf8_char(&mut out, line, i);
                if c == b'`' {
                    state = 0;
                }
                i += consumed;
            }
            _ => {
                i += 1;
            }
        }
    }
    if changed {
        Some(out)
    } else {
        None
    }
}

/// Find the byte offset of the `}` that closes the `{` at `start`.
/// Respects string and backtick state, and nested `{`. Returns `None`
/// when the input is malformed (unclosed brace).
fn find_matching_brace(bytes: &[u8], start: usize) -> Option<usize> {
    debug_assert_eq!(bytes.get(start), Some(&b'{'));
    let mut state: u8 = 0;
    let mut depth = 0i32;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    i += 1;
                    continue;
                }
                if c == b'{' {
                    depth += 1;
                    i += 1;
                    continue;
                }
                if c == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                    i += 1;
                    continue;
                }
                i += 1;
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    state = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                if c == b'`' {
                    state = 0;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

/// Does this map body contain a CASE expression at the top level (not
/// inside a string)? Used to force a multi-line split — CASEs are
/// always more readable broken out.
fn contains_top_level_case(body: &str) -> bool {
    let bytes = body.as_bytes();
    let mut state: u8 = 0;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    i += 1;
                    continue;
                }
                let prev_is_boundary = i == 0
                    || !{
                        let p = bytes[i - 1];
                        p.is_ascii_alphanumeric() || p == b'_'
                    };
                if prev_is_boundary
                    && bytes.get(i..i + 4) == Some(b"CASE")
                    && !is_word_char(bytes.get(i + 4).copied())
                {
                    return true;
                }
                i += 1;
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    state = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                if c == b'`' {
                    state = 0;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    false
}

/// Re-emit every inline `CASE ... END` expression across multiple
/// lines: CASE stays where it is (with its optional scrutinee), each
/// `WHEN ...` / `ELSE ...` segment goes on its own line indented two
/// spaces further, and the closing `END` returns to CASE's indent.
/// Anything trailing the `END` on the original line (e.g.
/// `END AS bracket,`) is preserved on the END line.
///
/// Nested CASEs work because `find_matching_end` counts depth.
fn split_case_expressions(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 32);
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        let base_indent = &line[..indent_len];
        let inner_indent = format!("{base_indent}  ");

        if !line_has_top_level_case(line) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let bytes = line.as_bytes();
        let mut buf = String::with_capacity(line.len() + 16);
        let mut state: u8 = 0;
        let mut depth = 0i32;
        let mut i = 0usize;

        while i < bytes.len() {
            let c = bytes[i];
            match state {
                0 => {
                    if c == b'\'' {
                        state = 1;
                        buf.push('\'');
                        i += 1;
                        continue;
                    }
                    if c == b'"' {
                        state = 2;
                        buf.push('"');
                        i += 1;
                        continue;
                    }
                    if c == b'`' {
                        state = 3;
                        buf.push('`');
                        i += 1;
                        continue;
                    }
                    if matches!(c, b'(' | b'[' | b'{') {
                        depth += 1;
                        buf.push(c as char);
                        i += 1;
                        continue;
                    }
                    if matches!(c, b')' | b']' | b'}') {
                        depth = (depth - 1).max(0);
                        buf.push(c as char);
                        i += 1;
                        continue;
                    }

                    if c < 0x80
                        && depth == 0
                        && is_word_boundary(bytes, i)
                        && bytes.get(i..i + 4) == Some(b"CASE")
                        && !is_word_char(bytes.get(i + 4).copied())
                    {
                        if let Some(end_idx) = find_matching_end(bytes, i + 4) {
                            // Body sits between CASE and END.
                            let body = &line[i + 4..end_idx];
                            let segments = split_case_body(body);
                            let head = segments.first().map(|s| s.trim()).unwrap_or("");
                            buf.push_str("CASE");
                            if !head.is_empty() {
                                buf.push(' ');
                                buf.push_str(head);
                            }
                            for seg in segments.iter().skip(1) {
                                buf.push('\n');
                                buf.push_str(&inner_indent);
                                buf.push_str(seg.trim());
                            }
                            buf.push('\n');
                            buf.push_str(base_indent);
                            buf.push_str("END");
                            i = end_idx + 3;
                            continue;
                        }
                    }

                    i += append_utf8_char(&mut buf, line, i);
                }
                1 => {
                    if c == b'\\' && i + 1 < bytes.len() {
                        buf.push('\\');
                        i += 1;
                        i += append_utf8_char(&mut buf, line, i);
                        continue;
                    }
                    let consumed = append_utf8_char(&mut buf, line, i);
                    if c == b'\'' {
                        state = 0;
                    }
                    i += consumed;
                }
                2 => {
                    if c == b'\\' && i + 1 < bytes.len() {
                        buf.push('\\');
                        i += 1;
                        i += append_utf8_char(&mut buf, line, i);
                        continue;
                    }
                    let consumed = append_utf8_char(&mut buf, line, i);
                    if c == b'"' {
                        state = 0;
                    }
                    i += consumed;
                }
                3 => {
                    let consumed = append_utf8_char(&mut buf, line, i);
                    if c == b'`' {
                        state = 0;
                    }
                    i += consumed;
                }
                _ => {
                    i += 1;
                }
            }
        }

        out.push_str(&buf);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Cheap word-boundary lookup on raw byte input.
fn is_word_char(c: Option<u8>) -> bool {
    matches!(
        c,
        Some(b'_') | Some(b'0'..=b'9') | Some(b'a'..=b'z') | Some(b'A'..=b'Z')
    )
}
fn is_word_boundary(bytes: &[u8], i: usize) -> bool {
    i == 0 || !is_word_char(Some(bytes[i - 1]))
}

/// Quick check: does this line contain at least one CASE outside any
/// string/comment? Used to bypass the expensive splitter pass.
fn line_has_top_level_case(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut state: u8 = 0;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    i += 1;
                    continue;
                }
                if is_word_boundary(bytes, i)
                    && bytes.get(i..i + 4) == Some(b"CASE")
                    && !is_word_char(bytes.get(i + 4).copied())
                {
                    return true;
                }
                i += 1;
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    state = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                if c == b'`' {
                    state = 0;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    false
}

/// Find the offset of the matching `END` for a CASE whose body starts
/// at `start`. Tracks nested CASE / END pairs; respects string and
/// bracket state. Returns `None` if the line lacks a matching END.
fn find_matching_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut case_depth = 1i32;
    let mut state: u8 = 0;
    let mut i = start;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    i += 1;
                    continue;
                }
                if matches!(c, b'(' | b'[' | b'{') {
                    depth += 1;
                    i += 1;
                    continue;
                }
                if matches!(c, b')' | b']' | b'}') {
                    depth = (depth - 1).max(0);
                    i += 1;
                    continue;
                }
                if depth == 0 && is_word_boundary(bytes, i) {
                    if bytes.get(i..i + 4) == Some(b"CASE")
                        && !is_word_char(bytes.get(i + 4).copied())
                    {
                        case_depth += 1;
                        i += 4;
                        continue;
                    }
                    if bytes.get(i..i + 3) == Some(b"END")
                        && !is_word_char(bytes.get(i + 3).copied())
                    {
                        case_depth -= 1;
                        if case_depth == 0 {
                            return Some(i);
                        }
                        i += 3;
                        continue;
                    }
                }
                i += 1;
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    state = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                if c == b'`' {
                    state = 0;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

/// Split a CASE body (the slice between CASE and END) at every
/// top-level WHEN / ELSE keyword. The first element is the optional
/// scrutinee expression that may appear before the first WHEN/ELSE.
fn split_case_body(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut case_depth = 0i32;
    let mut state: u8 = 0;
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    i += 1;
                    continue;
                }
                if matches!(c, b'(' | b'[' | b'{') {
                    depth += 1;
                    i += 1;
                    continue;
                }
                if matches!(c, b')' | b']' | b'}') {
                    depth = (depth - 1).max(0);
                    i += 1;
                    continue;
                }
                if depth == 0 && is_word_boundary(bytes, i) {
                    if bytes.get(i..i + 4) == Some(b"CASE")
                        && !is_word_char(bytes.get(i + 4).copied())
                    {
                        case_depth += 1;
                        i += 4;
                        continue;
                    }
                    if bytes.get(i..i + 3) == Some(b"END")
                        && !is_word_char(bytes.get(i + 3).copied())
                    {
                        case_depth -= 1;
                        i += 3;
                        continue;
                    }
                    if case_depth == 0 {
                        if bytes.get(i..i + 4) == Some(b"WHEN")
                            && !is_word_char(bytes.get(i + 4).copied())
                        {
                            parts.push(&body[start..i]);
                            start = i;
                            i += 4;
                            continue;
                        }
                        if bytes.get(i..i + 4) == Some(b"ELSE")
                            && !is_word_char(bytes.get(i + 4).copied())
                        {
                            parts.push(&body[start..i]);
                            start = i;
                            i += 4;
                            continue;
                        }
                    }
                }
                i += 1;
            }
            1 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    state = 0;
                }
                i += 1;
            }
            2 => {
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                if c == b'`' {
                    state = 0;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    parts.push(&body[start..]);
    parts
}

/// Indent the body of every `CALL { ... }` subquery one extra level
/// (two spaces). Tracks brace depth with string/comment awareness so
/// braces inside strings or property maps don't trigger changes.
///
/// The `{` of a CALL block ends its line and the body starts on the
/// next; the matching `}` is moved to its own line at the outer
/// indent. Nested CALL blocks compound: each level adds two spaces.
fn indent_call_subqueries(source: &str) -> String {
    #[derive(Clone, Copy)]
    enum SState {
        Normal,
        Str(u8),
        Line,
        Block,
    }

    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len() + 16);
    let mut state = SState::Normal;
    let mut depth: usize = 0;
    let mut brace_depth: i32 = 0;
    let mut scope_stack: Vec<i32> = Vec::new();
    let mut pending_call = false;
    let mut at_line_start = true;
    let mut i = 0usize;

    let push_indent = |out: &mut String, depth: usize| {
        for _ in 0..depth {
            out.push_str("  ");
        }
    };

    while i < bytes.len() {
        let c = bytes[i];

        // Inside string / line-comment / block-comment we just copy.
        match state {
            SState::Str(q) => {
                if c == b'\\' && i + 1 < bytes.len() {
                    out.push('\\');
                    i += 1;
                    i += append_utf8_char(&mut out, source, i);
                    continue;
                }
                let consumed = append_utf8_char(&mut out, source, i);
                if c == q {
                    state = SState::Normal;
                }
                i += consumed;
                continue;
            }
            SState::Line => {
                let consumed = append_utf8_char(&mut out, source, i);
                if c == b'\n' {
                    state = SState::Normal;
                    at_line_start = true;
                }
                i += consumed;
                continue;
            }
            SState::Block => {
                if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str("*/");
                    state = SState::Normal;
                    i += 2;
                    continue;
                }
                let consumed = append_utf8_char(&mut out, source, i);
                if c == b'\n' {
                    at_line_start = true;
                }
                i += consumed;
                continue;
            }
            SState::Normal => {}
        }

        // Lazy indent at the start of every non-empty line.
        if at_line_start && c != b'\n' {
            push_indent(&mut out, depth);
            at_line_start = false;
        }

        if c == b'\n' {
            out.push('\n');
            at_line_start = true;
            pending_call = false;
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            out.push(c as char);
            state = SState::Str(c);
            pending_call = false;
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            out.push_str("//");
            state = SState::Line;
            i += 2;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out.push_str("/*");
            state = SState::Block;
            i += 2;
            continue;
        }

        // Word-bounded match for "CALL". Only fires for ASCII bytes;
        // continuation bytes (>= 0x80) are not word characters, so the
        // multi-byte UTF-8 boundary case is handled implicitly.
        let prev_is_boundary = i == 0
            || !{
                let p = bytes[i - 1];
                p.is_ascii_alphanumeric() || p == b'_'
            };
        if c < 0x80
            && prev_is_boundary
            && bytes.get(i..i + 4) == Some(b"CALL")
            && bytes
                .get(i + 4)
                .is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'))
        {
            out.push_str("CALL");
            pending_call = true;
            i += 4;
            continue;
        }

        if c == b'{' {
            brace_depth += 1;
            if pending_call {
                scope_stack.push(brace_depth);
                depth += 1;
                pending_call = false;
                out.push('{');
                // Strip any trailing spaces on this `{` line so the
                // body genuinely starts on the next line.
                let mut j = i + 1;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                // If the next significant char isn't already a newline,
                // inject one so the body starts cleanly.
                if j < bytes.len() && bytes[j] != b'\n' {
                    out.push('\n');
                    at_line_start = true;
                }
                i = j;
                continue;
            }
            out.push('{');
            i += 1;
            continue;
        }

        if c == b'}' {
            if scope_stack.last() == Some(&brace_depth) {
                scope_stack.pop();
                depth = depth.saturating_sub(1);
                // Move `}` to its own line at the outer indent.
                while out.ends_with(' ') || out.ends_with('\t') {
                    out.pop();
                }
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                push_indent(&mut out, depth);
                out.push('}');
                at_line_start = false;
                brace_depth = (brace_depth - 1).max(0);
                pending_call = false;
                i += 1;
                continue;
            }
            out.push('}');
            brace_depth = (brace_depth - 1).max(0);
            i += 1;
            continue;
        }

        if c != b' ' && c != b'\t' {
            pending_call = false;
        }
        i += append_utf8_char(&mut out, source, i);
    }
    out
}

/// Clause-starters that should always sit at the beginning of a line.
/// Multi-word entries must be ordered longest-first so a greedy match
/// picks "ON CREATE SET" before "ON CREATE" before "ON", and
/// "OPTIONAL MATCH" before "MATCH".
const CLAUSE_STARTERS: &[&str] = &[
    "ON CREATE SET",
    "ON MATCH SET",
    "OPTIONAL MATCH",
    "DETACH DELETE",
    "ORDER BY",
    "UNION ALL",
    "ON CREATE",
    "ON MATCH",
    "MATCH",
    "WHERE",
    "WITH",
    "RETURN",
    "CREATE",
    "MERGE",
    "DELETE",
    "SET",
    "REMOVE",
    "UNWIND",
    "CALL",
    "YIELD",
    "UNION",
    "LIMIT",
    "SKIP",
];

/// Indentation applied to continuation clauses, making them visually
/// attach to the clause they modify (WHERE under MATCH, ON CREATE SET
/// under MERGE, etc.).
const CONTINUATION_CLAUSES: &[&str] = &[
    "WHERE",
    "ORDER BY",
    "LIMIT",
    "SKIP",
    "ON CREATE SET",
    "ON MATCH SET",
    "ON CREATE",
    "ON MATCH",
];

/// Scan the (already-uppercased, whitespace-normalised) source and put
/// every clause starter at column 0 of its own line. Skips quoted
/// strings and `//` / `/* ... */` comments so we never inject newlines
/// inside literal content.
fn reflow_clauses(source: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        Str(char),
        Line,
        Block,
    }

    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len() + 16);
    let mut state = State::Normal;
    let mut i = 0;
    // Track bracket/paren nesting so we never re-flow a clause keyword
    // that appears inside a list comprehension (`[g IN xs WHERE ... | g]`),
    // a list predicate (`ALL(x IN xs WHERE p(x))`), or a function call
    // argument. Brace depth is deliberately *not* tracked: CALL { ... }
    // and EXISTS { ... } subqueries contain real clauses and we want
    // them re-flowed.
    let mut bracket_depth: i32 = 0;
    let mut paren_depth: i32 = 0;

    while i < bytes.len() {
        let b = bytes[i];

        match state {
            State::Str(q) => {
                if b == b'\\' && i + 1 < bytes.len() {
                    out.push('\\');
                    i += 1;
                    i += append_utf8_char(&mut out, source, i);
                    continue;
                }
                let consumed = append_utf8_char(&mut out, source, i);
                if b < 0x80 && (b as char) == q {
                    state = State::Normal;
                }
                i += consumed;
                continue;
            }
            State::Line => {
                let consumed = append_utf8_char(&mut out, source, i);
                if b == b'\n' {
                    state = State::Normal;
                }
                i += consumed;
                continue;
            }
            State::Block => {
                if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str("*/");
                    i += 2;
                    state = State::Normal;
                    continue;
                }
                i += append_utf8_char(&mut out, source, i);
                continue;
            }
            State::Normal => {}
        }

        if b == b'\'' || b == b'"' || b == b'`' {
            out.push(b as char);
            state = State::Str(b as char);
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            out.push_str("//");
            i += 2;
            state = State::Line;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out.push_str("/*");
            i += 2;
            state = State::Block;
            continue;
        }

        if b == b'[' {
            bracket_depth += 1;
            out.push('[');
            i += 1;
            continue;
        }
        if b == b']' {
            bracket_depth = (bracket_depth - 1).max(0);
            out.push(']');
            i += 1;
            continue;
        }
        if b == b'(' {
            paren_depth += 1;
            out.push('(');
            i += 1;
            continue;
        }
        if b == b')' {
            paren_depth = (paren_depth - 1).max(0);
            out.push(')');
            i += 1;
            continue;
        }

        // Try to match a clause starter at this position. We only do so
        // when the previous char is a word boundary, and only for ASCII
        // bytes (clause keywords are all ASCII).
        let prev_is_boundary = match i {
            0 => true,
            _ => {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'_')
            }
        };
        if b < 0x80 && prev_is_boundary && bracket_depth == 0 && paren_depth == 0 {
            if let Some(kw) = match_clause_starter(source, i) {
                // Trim trailing whitespace from `out`. If after that we
                // already end on a newline (or `out` is empty), we're
                // effectively at column 0 — don't inject another `\n`.
                while out.ends_with(' ') || out.ends_with('\t') {
                    out.pop();
                }
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                if CONTINUATION_CLAUSES.contains(&kw) {
                    out.push_str("  ");
                }
                out.push_str(kw);
                i += kw.len();
                // Collapse any run of spaces between the clause keyword
                // and the next token down to a single space so chained
                // prettify() calls don't accumulate whitespace.
                let mut ate_space = false;
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                    ate_space = true;
                }
                if ate_space && i < bytes.len() && bytes[i] != b'\n' {
                    out.push(' ');
                }
                continue;
            }
        }

        i += append_utf8_char(&mut out, source, i);
    }

    // Collapse any blank-line runs we may have produced.
    let mut result = String::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        prev_blank = is_blank;
        result.push_str(line);
        result.push('\n');
    }
    while result.ends_with("\n\n") {
        result.pop();
    }
    result
}

/// Longest-match for a clause starter at byte offset `start`. The
/// starter must be followed by a word boundary so we don't match inside
/// identifiers.
fn match_clause_starter(source: &str, start: usize) -> Option<&'static str> {
    let rest_bytes = &source.as_bytes()[start..];
    for kw in CLAUSE_STARTERS {
        if rest_bytes.len() < kw.len() {
            continue;
        }
        // Compare on raw bytes so we don't trip over multi-byte UTF-8
        // characters at `kw.len()` (slicing the `&str` there would
        // panic). All clause keywords are ASCII, so the bytes-level
        // case-insensitive compare is sufficient.
        if !rest_bytes[..kw.len()].eq_ignore_ascii_case(kw.as_bytes()) {
            continue;
        }
        let after = rest_bytes.get(kw.len()).copied();
        match after {
            None => return Some(kw),
            Some(c) if c.is_ascii_alphanumeric() || c == b'_' => continue,
            Some(_) => return Some(kw),
        }
    }
    None
}

/// Lexical pass that:
///   - trims trailing whitespace on every line,
///   - collapses runs of blank lines down to one,
///   - ensures exactly one space after every `,`,
///   - collapses runs of internal spaces/tabs to a single space
///     (preserving leading indent), so visually-aligned input like
///     `ON MATCH  SET` matches our multi-word clause starters.
///
/// Whitespace inside strings, line comments, and block comments is
/// left untouched, so literal content and `// rule of three   ...`
/// banners survive.
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

        // Preserve leading indent verbatim. Only normalise from the
        // first non-whitespace character onwards.
        let body_start = line
            .find(|c: char| c != ' ' && c != '\t')
            .unwrap_or(line.len());
        out.push_str(&line[..body_start]);

        let bytes = line.as_bytes();
        let mut i = body_start;
        // State machine: Normal (collapse multi-space), in-string,
        // in-line-comment, in-block-comment. Comments / strings copy
        // verbatim, normal collapses runs of ASCII whitespace.
        #[derive(Clone, Copy)]
        enum WState {
            Normal,
            Str(u8),
            Line,
            Block,
        }
        let mut state = WState::Normal;

        while i < bytes.len() {
            let c = bytes[i];
            match state {
                WState::Str(q) => {
                    if c == b'\\' && i + 1 < bytes.len() {
                        out.push('\\');
                        i += 1;
                        i += append_utf8_char(&mut out, line, i);
                        continue;
                    }
                    let consumed = append_utf8_char(&mut out, line, i);
                    if c == q {
                        state = WState::Normal;
                    }
                    i += consumed;
                }
                WState::Line => {
                    i += append_utf8_char(&mut out, line, i);
                }
                WState::Block => {
                    if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        out.push_str("*/");
                        state = WState::Normal;
                        i += 2;
                        continue;
                    }
                    i += append_utf8_char(&mut out, line, i);
                }
                WState::Normal => {
                    if c == b'\'' || c == b'"' || c == b'`' {
                        out.push(c as char);
                        state = WState::Str(c);
                        i += 1;
                        continue;
                    }
                    if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        out.push_str("//");
                        state = WState::Line;
                        i += 2;
                        continue;
                    }
                    if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                        out.push_str("/*");
                        state = WState::Block;
                        i += 2;
                        continue;
                    }
                    if c == b' ' || c == b'\t' {
                        // Swallow any extra whitespace; we emit at
                        // most one space here.
                        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                            i += 1;
                        }
                        // Trailing whitespace on the line was already
                        // stripped by trim_end above, so we always
                        // have a non-whitespace follower here.
                        out.push(' ');
                        continue;
                    }
                    if c == b',' {
                        out.push(',');
                        i += 1;
                        // Eat any trailing whitespace after `,`; emit
                        // a single space if more content follows.
                        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                            i += 1;
                        }
                        if i < bytes.len() {
                            out.push(' ');
                        }
                        continue;
                    }
                    i += append_utf8_char(&mut out, line, i);
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

/// State-machine pass that uppercases known Cypher keywords while
/// leaving identifiers, string literals, and comments untouched.
fn uppercase_keywords(source: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        Str(char),
        Line,
        Block,
    }

    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut state = State::Normal;

    while let Some(c) = chars.next() {
        match state {
            State::Normal => {
                if c == '\'' || c == '"' || c == '`' {
                    out.push(c);
                    state = State::Str(c);
                } else if c == '/' && chars.peek() == Some(&'/') {
                    out.push(c);
                    out.push(chars.next().unwrap());
                    state = State::Line;
                } else if c == '/' && chars.peek() == Some(&'*') {
                    out.push(c);
                    out.push(chars.next().unwrap());
                    state = State::Block;
                } else if c.is_ascii_alphabetic() || c == '_' {
                    let mut word = String::new();
                    word.push(c);
                    while let Some(&nc) = chars.peek() {
                        if nc.is_ascii_alphanumeric() || nc == '_' {
                            word.push(nc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let upper = word.to_ascii_uppercase();
                    if KEYWORDS.contains(&upper.as_str()) {
                        out.push_str(&upper);
                    } else {
                        out.push_str(&word);
                    }
                } else {
                    out.push(c);
                }
            }
            State::Str(q) => {
                out.push(c);
                if c == '\\' {
                    if let Some(&next) = chars.peek() {
                        out.push(next);
                        chars.next();
                    }
                } else if c == q {
                    state = State::Normal;
                }
            }
            State::Line => {
                out.push(c);
                if c == '\n' {
                    state = State::Normal;
                }
            }
            State::Block => {
                out.push(c);
                if c == '*' && chars.peek() == Some(&'/') {
                    out.push(chars.next().unwrap());
                    state = State::Normal;
                }
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// AST-driven highlight collection
// ─────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────
// Outline collection (variables / labels / rel types / parameters)
// ─────────────────────────────────────────────────────────────────────

fn collect_outline(doc: &Document, source: &str) -> Outline {
    let mut acc = OutlineAcc::default();
    walk_statement_outline(&doc.statement, source, &mut acc);
    acc.into_outline()
}

#[derive(Default)]
struct OutlineAcc {
    variables: Vec<OutlineVariable>,
    seen_vars: std::collections::HashSet<String>,
    parameters: Vec<String>,
    seen_params: std::collections::HashSet<String>,
    labels: Vec<String>,
    seen_labels: std::collections::HashSet<String>,
    rel_types: Vec<String>,
    seen_rel_types: std::collections::HashSet<String>,
}

impl OutlineAcc {
    fn add_variable(
        &mut self,
        v: &Variable,
        label: Option<&str>,
        kind: VariableKind,
        alias_of: Option<&str>,
    ) {
        if self.seen_vars.insert(v.name.clone()) {
            self.variables.push(OutlineVariable {
                name: v.name.clone(),
                decl_start: v.span.start,
                decl_end: v.span.end,
                label: label.map(str::to_owned),
                kind,
                alias_of: alias_of.map(str::to_owned),
            });
        }
    }
    fn add_parameter(&mut self, name: &str) {
        if self.seen_params.insert(name.to_owned()) {
            self.parameters.push(name.to_owned());
        }
    }
    fn add_label(&mut self, name: &str) {
        if self.seen_labels.insert(name.to_owned()) {
            self.labels.push(name.to_owned());
        }
    }
    fn add_rel_type(&mut self, name: &str) {
        if self.seen_rel_types.insert(name.to_owned()) {
            self.rel_types.push(name.to_owned());
        }
    }
    fn into_outline(self) -> Outline {
        Outline {
            variables: self.variables,
            parameters: self.parameters,
            labels: self.labels,
            rel_types: self.rel_types,
        }
    }
}

fn walk_statement_outline(stmt: &Statement, source: &str, acc: &mut OutlineAcc) {
    if let Statement::Query(Query::Regular(rq)) = stmt {
        walk_single_outline(&rq.head, source, acc);
        for u in &rq.unions {
            walk_single_outline(&u.query, source, acc);
        }
    }
}

fn walk_single_outline(q: &SingleQuery, source: &str, acc: &mut OutlineAcc) {
    match q {
        SingleQuery::SinglePart(part) => walk_single_part_outline(part, source, acc),
        SingleQuery::MultiPart(mp) => {
            for p in &mp.parts {
                for rc in &p.reading_clauses {
                    walk_reading_outline(rc, source, acc);
                }
                for uc in &p.updating_clauses {
                    walk_updating_outline(uc, source, acc);
                }
                walk_projection_outline(&p.with_clause.body, source, acc);
                if let Some(filter) = &p.with_clause.where_ {
                    walk_expr_outline(filter, source, acc);
                }
            }
            walk_single_part_outline(&mp.tail, source, acc);
        }
    }
}

fn walk_single_part_outline(part: &SinglePartQuery, source: &str, acc: &mut OutlineAcc) {
    for rc in &part.reading_clauses {
        walk_reading_outline(rc, source, acc);
    }
    for uc in &part.updating_clauses {
        walk_updating_outline(uc, source, acc);
    }
    if let Some(ret) = &part.return_clause {
        walk_projection_outline(&ret.body, source, acc);
    }
}

fn walk_projection_outline(body: &ProjectionBody, source: &str, acc: &mut OutlineAcc) {
    for item in &body.items {
        if let ProjectionItem::Expr { expr, alias, .. } = item {
            walk_expr_outline(expr, source, acc);
            if let Some(a) = alias {
                // If the projection is just `<source>` (a bare variable
                // reference), record the alias as pointing at it so
                // completion can resolve `x.` back through `n AS x`.
                let alias_source = match expr {
                    Expr::Variable(v) => Some(v.name.as_str()),
                    _ => None,
                };
                let inferred_label = alias_source.and_then(|name| {
                    acc.variables
                        .iter()
                        .find(|vv| vv.name == name)
                        .and_then(|vv| vv.label.clone())
                });
                acc.add_variable(
                    a,
                    inferred_label.as_deref(),
                    VariableKind::Scalar,
                    alias_source,
                );
            }
        }
    }
    for sort in &body.order {
        walk_expr_outline(&sort.expr, source, acc);
    }
    if let Some(skip) = &body.skip {
        walk_expr_outline(skip, source, acc);
    }
    if let Some(limit) = &body.limit {
        walk_expr_outline(limit, source, acc);
    }
}

fn walk_reading_outline(rc: &ReadingClause, source: &str, acc: &mut OutlineAcc) {
    match rc {
        ReadingClause::Match(m) => {
            walk_pattern_outline(&m.pattern, source, acc);
            if let Some(filter) = &m.where_ {
                walk_expr_outline(filter, source, acc);
            }
        }
        ReadingClause::Unwind(u) => {
            walk_expr_outline(&u.expr, source, acc);
            acc.add_variable(&u.alias, None, VariableKind::Scalar, None);
        }
        ReadingClause::InQueryCall(_) => {}
        ReadingClause::CallSubquery(c) => {
            walk_single_outline(&c.body.head, source, acc);
            for u in &c.body.unions {
                walk_single_outline(&u.query, source, acc);
            }
        }
    }
}

fn walk_updating_outline(uc: &UpdatingClause, source: &str, acc: &mut OutlineAcc) {
    match uc {
        UpdatingClause::Create(c) => walk_pattern_outline(&c.pattern, source, acc),
        UpdatingClause::Merge(m) => {
            walk_pattern_part_outline(&m.pattern_part, source, acc);
            // ON CREATE / ON MATCH SET actions introduce real bindings
            // and reference parameters; walking them here keeps the
            // outline in sync with the highlighter.
            for action in &m.actions {
                walk_set_outline(&action.set, source, acc);
            }
        }
        UpdatingClause::Delete(d) => {
            for e in &d.expressions {
                walk_expr_outline(e, source, acc);
            }
        }
        UpdatingClause::Set(s) => walk_set_outline(s, source, acc),
        UpdatingClause::Remove(r) => {
            for item in &r.items {
                match item {
                    lora_ast::RemoveItem::Property { expr, .. } => {
                        walk_expr_outline(expr, source, acc);
                    }
                    lora_ast::RemoveItem::Labels {
                        variable, labels, ..
                    } => {
                        acc.add_variable(variable, None, VariableKind::Node, None);
                        for l in labels {
                            acc.add_label(l);
                        }
                    }
                }
            }
        }
        UpdatingClause::Foreach(f) => {
            walk_expr_outline(&f.list, source, acc);
            for body in &f.body {
                walk_updating_outline(body, source, acc);
            }
        }
    }
}

fn walk_set_outline(s: &lora_ast::Set, source: &str, acc: &mut OutlineAcc) {
    for item in &s.items {
        match item {
            lora_ast::SetItem::SetProperty { target, value, .. } => {
                walk_expr_outline(target, source, acc);
                walk_expr_outline(value, source, acc);
            }
            lora_ast::SetItem::SetVariable {
                variable, value, ..
            }
            | lora_ast::SetItem::MutateVariable {
                variable, value, ..
            } => {
                acc.add_variable(variable, None, VariableKind::Scalar, None);
                walk_expr_outline(value, source, acc);
            }
            lora_ast::SetItem::SetLabels {
                variable, labels, ..
            } => {
                let first = labels.first().map(String::as_str);
                acc.add_variable(variable, first, VariableKind::Node, None);
                for l in labels {
                    acc.add_label(l);
                }
            }
        }
    }
}

fn walk_pattern_outline(p: &Pattern, source: &str, acc: &mut OutlineAcc) {
    for part in &p.parts {
        walk_pattern_part_outline(part, source, acc);
    }
}

fn walk_pattern_part_outline(part: &PatternPart, source: &str, acc: &mut OutlineAcc) {
    if let Some(v) = &part.binding {
        acc.add_variable(v, None, VariableKind::Pattern, None);
    }
    walk_pattern_element_outline(&part.element, source, acc);
}

#[allow(clippy::only_used_in_recursion)]
fn walk_pattern_element_outline(el: &PatternElement, source: &str, acc: &mut OutlineAcc) {
    match el {
        PatternElement::NodeChain { head, chain, .. } => {
            walk_node_outline(head, acc);
            for link in chain {
                walk_rel_outline(&link.relationship, acc);
                walk_node_outline(&link.node, acc);
            }
        }
        PatternElement::Parenthesized(inner, _) => walk_pattern_element_outline(inner, source, acc),
        PatternElement::ShortestPath { element, .. } => {
            walk_pattern_element_outline(element, source, acc)
        }
    }
}

fn walk_node_outline(np: &lora_ast::NodePattern, acc: &mut OutlineAcc) {
    let first_label = np.labels.iter().flat_map(|g| g.iter()).next().cloned();
    if let Some(v) = &np.variable {
        acc.add_variable(v, first_label.as_deref(), VariableKind::Node, None);
    }
    for group in &np.labels {
        for l in group {
            acc.add_label(l);
        }
    }
}

fn walk_rel_outline(rp: &lora_ast::RelationshipPattern, acc: &mut OutlineAcc) {
    if let Some(detail) = &rp.detail {
        let first_type = detail.types.first().map(String::as_str);
        if let Some(v) = &detail.variable {
            acc.add_variable(v, first_type, VariableKind::Relationship, None);
        }
        for t in &detail.types {
            acc.add_rel_type(t);
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
fn walk_expr_outline(expr: &Expr, source: &str, acc: &mut OutlineAcc) {
    match expr {
        // Expr::Variable here is a *reference* to a binding, not a new
        // declaration — only call sites that declare variables (List
        // comprehensions, Reduce, etc.) tag a `kind`. References stay
        // out of the outline; they're already collected at declaration.
        Expr::Variable(_) => {}
        Expr::Parameter(name, _) => acc.add_parameter(name),
        Expr::List(items, _) => {
            for it in items {
                walk_expr_outline(it, source, acc);
            }
        }
        Expr::Map(entries, _) => {
            for (_, v) in entries {
                walk_expr_outline(v, source, acc);
            }
        }
        Expr::Property { expr, .. } => walk_expr_outline(expr, source, acc),
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr_outline(lhs, source, acc);
            walk_expr_outline(rhs, source, acc);
        }
        Expr::Unary { expr, .. } => walk_expr_outline(expr, source, acc),
        Expr::FunctionCall { args, .. } => {
            for a in args {
                walk_expr_outline(a, source, acc);
            }
        }
        Expr::TypeCast { expr, .. } => walk_expr_outline(expr, source, acc),
        Expr::Case {
            input,
            alternatives,
            else_expr,
            ..
        } => {
            if let Some(i) = input {
                walk_expr_outline(i, source, acc);
            }
            for (cond, val) in alternatives {
                walk_expr_outline(cond, source, acc);
                walk_expr_outline(val, source, acc);
            }
            if let Some(e) = else_expr {
                walk_expr_outline(e, source, acc);
            }
        }
        Expr::ListPredicate {
            variable,
            list,
            predicate,
            ..
        } => {
            acc.add_variable(variable, None, VariableKind::Scalar, None);
            walk_expr_outline(list, source, acc);
            walk_expr_outline(predicate, source, acc);
        }
        Expr::ListComprehension {
            variable,
            list,
            filter,
            map_expr,
            ..
        } => {
            acc.add_variable(variable, None, VariableKind::Scalar, None);
            walk_expr_outline(list, source, acc);
            if let Some(f) = filter {
                walk_expr_outline(f, source, acc);
            }
            if let Some(m) = map_expr {
                walk_expr_outline(m, source, acc);
            }
        }
        Expr::Reduce {
            accumulator,
            init,
            variable,
            list,
            expr,
            ..
        } => {
            acc.add_variable(accumulator, None, VariableKind::Scalar, None);
            acc.add_variable(variable, None, VariableKind::Scalar, None);
            walk_expr_outline(init, source, acc);
            walk_expr_outline(list, source, acc);
            walk_expr_outline(expr, source, acc);
        }
        _ => {}
    }
}

fn collect_highlights(doc: &Document, source: &str) -> Vec<HighlightSpan> {
    let mut out: Vec<HighlightSpan> = Vec::new();
    visit_statement(&doc.statement, source, &mut out);
    out.sort_by_key(|h| (h.start, h.end));
    out
}

/// Lexical highlight fallback for slices that don't parse cleanly.
///
/// When the user is mid-typing — or, more commonly, when several queries
/// share one source slice because they're separated by blank lines and
/// no `;` — the AST-driven `collect_highlights` returns nothing and the
/// editor loses all semantic colour. This scanner emits the same
/// {Label, RelType, PropertyKey, FunctionName, Namespace, Parameter,
/// Variable, StringLiteral, NumberLiteral, BoolLiteral, NullLiteral}
/// span kinds using only lexical cues — bracket context, neighbour
/// punctuation, the well-known keyword set — so colour stays consistent
/// with the AST output and survives partial / multi-statement input.
fn collect_highlights_lexical(source: &str) -> Vec<HighlightSpan> {
    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut out: Vec<HighlightSpan> = Vec::new();

    #[derive(Clone, Copy, PartialEq)]
    enum Lex {
        Normal,
        Single,
        Double,
        Back,
        LineComment,
        BlockComment,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Delim {
        Paren,   // (
        Bracket, // [
        Brace,   // {
    }

    let mut state = Lex::Normal;
    let mut stack: Vec<Delim> = Vec::new();
    // Pending classification for the *next* identifier we see.
    //
    // The scanner walks left-to-right; some token classifications depend
    // on the immediately preceding punctuation (`$foo`, `.bar`, `:Label`,
    // first identifier after `(` / `[`). We stash the intended kind here
    // when we see the trigger, and read+clear it when we land on the
    // identifier.
    let mut next_ident: Option<HighlightKind> = None;
    // True when the next identifier in a `{ … }` map literal should be
    // treated as a property key (i.e. it's followed by `:`). We can't
    // know that without lookahead, so we tentatively mark *all*
    // identifiers inside a brace as PropertyKey and then drop the mark
    // if the next non-whitespace char isn't `:`. To keep this scanner
    // single-pass, we emit speculatively and let the layered AST output
    // (when it parses later) overwrite as needed.
    let mut at_string_start: usize = 0;
    let mut i = 0;

    let push = |out: &mut Vec<HighlightSpan>, start: usize, end: usize, kind: HighlightKind| {
        if start < end {
            out.push(HighlightSpan { start, end, kind });
        }
    };

    while i < n {
        let c = bytes[i];
        match state {
            Lex::Single => {
                if c == b'\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if c == b'\'' {
                    push(
                        &mut out,
                        at_string_start,
                        i + 1,
                        HighlightKind::StringLiteral,
                    );
                    state = Lex::Normal;
                }
                i += 1;
                continue;
            }
            Lex::Double => {
                if c == b'\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    push(
                        &mut out,
                        at_string_start,
                        i + 1,
                        HighlightKind::StringLiteral,
                    );
                    state = Lex::Normal;
                }
                i += 1;
                continue;
            }
            Lex::Back => {
                if c == b'`' {
                    state = Lex::Normal;
                }
                i += 1;
                continue;
            }
            Lex::LineComment => {
                if c == b'\n' {
                    state = Lex::Normal;
                }
                i += 1;
                continue;
            }
            Lex::BlockComment => {
                if c == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                    state = Lex::Normal;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            Lex::Normal => {}
        }

        // Normal state
        if c == b'\'' {
            at_string_start = i;
            state = Lex::Single;
            i += 1;
            continue;
        }
        if c == b'"' {
            at_string_start = i;
            state = Lex::Double;
            i += 1;
            continue;
        }
        if c == b'`' {
            state = Lex::Back;
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
            state = Lex::LineComment;
            i += 2;
            continue;
        }
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            state = Lex::BlockComment;
            i += 2;
            continue;
        }
        if c == b'(' {
            stack.push(Delim::Paren);
            // First identifier inside a `(` (before any `:`) is a
            // pattern variable binding. Mark the next ident accordingly,
            // unless the next ident is preceded by `:` (label) — handled
            // below when we encounter the colon.
            next_ident = Some(HighlightKind::Variable);
            i += 1;
            continue;
        }
        if c == b'[' {
            stack.push(Delim::Bracket);
            next_ident = Some(HighlightKind::Variable);
            i += 1;
            continue;
        }
        if c == b'{' {
            stack.push(Delim::Brace);
            // Map / properties block — first identifier is a property
            // key (`{name: 'Alice'}`). Subsequent keys after a comma
            // will be re-classified when we hit the comma.
            next_ident = Some(HighlightKind::PropertyKey);
            i += 1;
            continue;
        }
        if c == b')' || c == b']' || c == b'}' {
            stack.pop();
            next_ident = None;
            i += 1;
            continue;
        }
        if c == b',' {
            // Inside a `{ … }`, the identifier after a comma is another
            // property key. Outside braces, identifiers after `,` are
            // regular references — leave them unclassified.
            if matches!(stack.last(), Some(Delim::Brace)) {
                next_ident = Some(HighlightKind::PropertyKey);
            } else {
                next_ident = None;
            }
            i += 1;
            continue;
        }
        if c == b':' {
            // Inside `(...)` → next identifier is a Label.
            // Inside `[...]` → next identifier is a RelType.
            // Inside `{...}` → the colon separates key from value;
            // leave next_ident alone (we already classified the key).
            if let Some(top) = stack.last() {
                match top {
                    Delim::Paren => next_ident = Some(HighlightKind::Label),
                    Delim::Bracket => next_ident = Some(HighlightKind::RelType),
                    Delim::Brace => next_ident = None,
                }
            }
            i += 1;
            continue;
        }
        if c == b'$' {
            // Parameter — the immediately-following identifier is the
            // parameter name. The whole `$name` span is highlighted as
            // Parameter, so we record the start and read forward.
            let start = i;
            let mut j = i + 1;
            while j < n && (is_ident_byte(bytes[j])) {
                j += 1;
            }
            if j > i + 1 {
                push(&mut out, start, j, HighlightKind::Parameter);
            }
            i = j;
            next_ident = None;
            continue;
        }
        if c == b'.' {
            // `.<ident>` — property access (`n.name`) or namespace
            // member call (`math.abs(`). Peek past the identifier: if
            // a `(` follows, the member is a function; otherwise the
            // member is a property key. The AST highlight remains
            // authoritative when the slice parses.
            let start = i + 1;
            let mut j = start;
            while j < n && is_ident_byte(bytes[j]) {
                j += 1;
            }
            if j > start {
                let mut peek = j;
                while peek < n && (bytes[peek] == b' ' || bytes[peek] == b'\t') {
                    peek += 1;
                }
                let kind = if peek < n && bytes[peek] == b'(' {
                    HighlightKind::FunctionName
                } else {
                    HighlightKind::PropertyKey
                };
                push(&mut out, start, j, kind);
            }
            i = j;
            next_ident = None;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            // Exponent
            if i < n && (bytes[i] == b'e' || bytes[i] == b'E') {
                i += 1;
                if i < n && (bytes[i] == b'+' || bytes[i] == b'-') {
                    i += 1;
                }
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            push(&mut out, start, i, HighlightKind::NumberLiteral);
            next_ident = None;
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < n && is_ident_byte(bytes[i]) {
                i += 1;
            }
            let end = i;
            let word = &source[start..end];
            let upper = word.to_ascii_uppercase();

            // Peek past whitespace for the next punctuation character —
            // we need to know whether this identifier is `name(`,
            // `name.`, or something else, to pick a sensible kind.
            let mut peek = i;
            while peek < n && (bytes[peek] == b' ' || bytes[peek] == b'\t') {
                peek += 1;
            }
            let next_byte = if peek < n { Some(bytes[peek]) } else { None };

            // Resolve `<ident>.<member>(`: caller is a namespace,
            // member is a function. We need to know this BEFORE the
            // reserved-word check because the rust analyser's reserved
            // set deliberately includes the known namespace identifiers
            // (`math`, `string`, `list`, …) for the undeclared-uses
            // check.
            let mut namespace_call = false;
            if next_byte == Some(b'.') {
                let after = peek + 1;
                let mut k = after;
                while k < n && is_ident_byte(bytes[k]) {
                    k += 1;
                }
                let mut after_member = k;
                while after_member < n
                    && (bytes[after_member] == b' ' || bytes[after_member] == b'\t')
                {
                    after_member += 1;
                }
                if k > after && after_member < n && bytes[after_member] == b'(' {
                    namespace_call = true;
                }
            }

            let kind = if is_clause_keyword(word) {
                // MATCH / MERGE / SET / LIMIT / … — always Keyword,
                // even when followed by `(` (a `MATCH (n)` is NOT a
                // function call). Bool / null literals get their own
                // distinct kinds.
                if upper == "TRUE" || upper == "FALSE" {
                    Some(HighlightKind::BoolLiteral)
                } else if upper == "NULL" {
                    Some(HighlightKind::NullLiteral)
                } else {
                    Some(HighlightKind::Keyword)
                }
            } else if next_byte == Some(b'(') {
                // Function call — wins for aggregate-style reserved
                // words (`count`, `sum`, `avg`, …) that aren't clause
                // keywords.
                Some(HighlightKind::FunctionName)
            } else if namespace_call {
                Some(HighlightKind::Namespace)
            } else if is_reserved_word(word) {
                if upper == "TRUE" || upper == "FALSE" {
                    Some(HighlightKind::BoolLiteral)
                } else if upper == "NULL" {
                    Some(HighlightKind::NullLiteral)
                } else {
                    // Other reserved words that aren't clause keywords
                    // (the analyser's reserved set is broader than the
                    // editor's clause set) — colour them as keywords
                    // too so the user sees them as such.
                    Some(HighlightKind::Keyword)
                }
            } else {
                // Either a `<var>.<prop>` head (the `.` branch above
                // colours the prop side) or a free reference that
                // inherits its kind from the surrounding bracket
                // context (`(varname)` binding, etc).
                next_ident.take()
            };
            if let Some(k) = kind {
                push(&mut out, start, end, k);
            }
            next_ident = None;
            continue;
        }
        // Any other character clears the next-ident pending state when
        // it's an operator or symbol — but keep it across whitespace.
        if c != b' ' && c != b'\t' && c != b'\n' && c != b'\r' {
            next_ident = None;
        }
        i += 1;
    }
    out.sort_by_key(|h| (h.start, h.end));
    out
}

#[inline]
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Cypher clause / control keywords. We treat these as `Keyword` spans
/// in the lexical fallback so the colour matches the synchronous
/// StreamLanguage tokenizer. Words like `count` / `sum` / `avg` are
/// deliberately excluded — when followed by `(` they should highlight
/// as functions, not keywords.
fn is_clause_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "MATCH"
            | "OPTIONAL"
            | "WHERE"
            | "RETURN"
            | "WITH"
            | "CREATE"
            | "MERGE"
            | "DELETE"
            | "DETACH"
            | "SET"
            | "REMOVE"
            | "UNWIND"
            | "FOREACH"
            | "ORDER"
            | "BY"
            | "ASC"
            | "ASCENDING"
            | "DESC"
            | "DESCENDING"
            | "LIMIT"
            | "SKIP"
            | "USING"
            | "UNION"
            | "ALL"
            | "AS"
            | "DISTINCT"
            | "CALL"
            | "YIELD"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "EXISTS"
            | "CONSTRAINT"
            | "INDEX"
            | "SHOW"
            | "DROP"
            | "UNIQUE"
            | "FOR"
            | "ON"
            | "REQUIRE"
            | "ASSERT"
            | "AND"
            | "OR"
            | "XOR"
            | "NOT"
            | "IN"
            | "IS"
            | "STARTS"
            | "ENDS"
            | "CONTAINS"
            | "PROFILE"
            | "EXPLAIN"
    )
}

fn push(out: &mut Vec<HighlightSpan>, start: usize, end: usize, kind: HighlightKind) {
    if start < end {
        out.push(HighlightSpan { start, end, kind });
    }
}

fn push_variable(out: &mut Vec<HighlightSpan>, v: &Variable) {
    push(out, v.span.start, v.span.end, HighlightKind::Variable);
}

fn visit_statement(stmt: &Statement, source: &str, out: &mut Vec<HighlightSpan>) {
    match stmt {
        Statement::Query(q) => visit_query(q, source, out),
        Statement::Schema(_) => {
            // DDL statements (CREATE INDEX / CREATE CONSTRAINT / SHOW
            // INDEXES / DROP CONSTRAINT / …) store their identifiers as
            // bare strings in the AST without source spans, so the
            // typed visitor can't precisely re-locate `n`, `Person`,
            // `email`, etc. in the source. Drop down to the lexical
            // scanner — it walks the slice character-by-character and
            // produces the same span kinds (Variable, Label,
            // PropertyKey, …) the rest of the highlighter emits.
            out.extend(collect_highlights_lexical(source));
        }
    }
}

fn visit_query(q: &Query, source: &str, out: &mut Vec<HighlightSpan>) {
    match q {
        Query::Regular(rq) => visit_regular(rq, source, out),
        Query::StandaloneCall(_) => {}
    }
}

fn visit_regular(rq: &RegularQuery, source: &str, out: &mut Vec<HighlightSpan>) {
    visit_single(&rq.head, source, out);
    for union in &rq.unions {
        visit_single(&union.query, source, out);
    }
}

fn visit_single(q: &SingleQuery, source: &str, out: &mut Vec<HighlightSpan>) {
    match q {
        SingleQuery::SinglePart(part) => visit_single_part(part, source, out),
        SingleQuery::MultiPart(mp) => visit_multi_part(mp, source, out),
    }
}

fn visit_single_part(part: &SinglePartQuery, source: &str, out: &mut Vec<HighlightSpan>) {
    for rc in &part.reading_clauses {
        visit_reading(rc, source, out);
    }
    for uc in &part.updating_clauses {
        visit_updating(uc, source, out);
    }
    if let Some(ret) = &part.return_clause {
        visit_projection(&ret.body, source, out);
    }
}

fn visit_multi_part(mp: &MultiPartQuery, source: &str, out: &mut Vec<HighlightSpan>) {
    for qp in &mp.parts {
        visit_query_part(qp, source, out);
    }
    visit_single_part(&mp.tail, source, out);
}

fn visit_query_part(qp: &QueryPart, source: &str, out: &mut Vec<HighlightSpan>) {
    for rc in &qp.reading_clauses {
        visit_reading(rc, source, out);
    }
    for uc in &qp.updating_clauses {
        visit_updating(uc, source, out);
    }
    visit_with(&qp.with_clause, source, out);
}

fn visit_with(w: &With, source: &str, out: &mut Vec<HighlightSpan>) {
    visit_projection(&w.body, source, out);
    if let Some(filter) = &w.where_ {
        visit_expr(filter, source, out);
    }
}

fn visit_projection(body: &ProjectionBody, source: &str, out: &mut Vec<HighlightSpan>) {
    for item in &body.items {
        if let ProjectionItem::Expr { expr, alias, .. } = item {
            visit_expr(expr, source, out);
            if let Some(a) = alias {
                push_variable(out, a);
            }
        }
    }
    for sort in &body.order {
        visit_expr(&sort.expr, source, out);
    }
    if let Some(skip) = &body.skip {
        visit_expr(skip, source, out);
    }
    if let Some(limit) = &body.limit {
        visit_expr(limit, source, out);
    }
}

fn visit_reading(rc: &ReadingClause, source: &str, out: &mut Vec<HighlightSpan>) {
    match rc {
        ReadingClause::Match(m) => visit_match(m, source, out),
        ReadingClause::Unwind(u) => {
            visit_expr(&u.expr, source, out);
            push_variable(out, &u.alias);
        }
        ReadingClause::InQueryCall(_) => {}
        ReadingClause::CallSubquery(c) => visit_regular(&c.body, source, out),
    }
}

fn visit_updating(uc: &UpdatingClause, source: &str, out: &mut Vec<HighlightSpan>) {
    match uc {
        UpdatingClause::Create(c) => visit_pattern(&c.pattern, source, out),
        UpdatingClause::Merge(m) => {
            visit_pattern_part(&m.pattern_part, source, out);
            // ON CREATE / ON MATCH SET clauses carry their own SET
            // items — without visiting them we'd lose colouring on
            // every function call / parameter / variable / property
            // inside a `MERGE ... ON CREATE SET …` action. Same shape
            // as the top-level Set arm below.
            for action in &m.actions {
                visit_set(&action.set, source, out);
            }
        }
        UpdatingClause::Delete(d) => {
            for e in &d.expressions {
                visit_expr(e, source, out);
            }
        }
        UpdatingClause::Set(s) => visit_set(s, source, out),
        UpdatingClause::Remove(r) => {
            for item in &r.items {
                match item {
                    lora_ast::RemoveItem::Property { expr, .. } => visit_expr(expr, source, out),
                    lora_ast::RemoveItem::Labels { variable, .. } => push_variable(out, variable),
                }
            }
        }
        UpdatingClause::Foreach(f) => {
            push_variable(out, &f.variable);
            visit_expr(&f.list, source, out);
            for body in &f.body {
                visit_updating(body, source, out);
            }
        }
    }
}

fn visit_set(s: &lora_ast::Set, source: &str, out: &mut Vec<HighlightSpan>) {
    for item in &s.items {
        match item {
            lora_ast::SetItem::SetProperty { target, value, .. } => {
                visit_expr(target, source, out);
                visit_expr(value, source, out);
            }
            lora_ast::SetItem::SetVariable {
                variable, value, ..
            }
            | lora_ast::SetItem::MutateVariable {
                variable, value, ..
            } => {
                push_variable(out, variable);
                visit_expr(value, source, out);
            }
            lora_ast::SetItem::SetLabels { variable, .. } => {
                push_variable(out, variable);
            }
        }
    }
}

fn visit_match(m: &Match, source: &str, out: &mut Vec<HighlightSpan>) {
    visit_pattern(&m.pattern, source, out);
    if let Some(filter) = &m.where_ {
        visit_expr(filter, source, out);
    }
}

fn visit_pattern(p: &Pattern, source: &str, out: &mut Vec<HighlightSpan>) {
    for part in &p.parts {
        visit_pattern_part(part, source, out);
    }
}

fn visit_pattern_part(part: &PatternPart, source: &str, out: &mut Vec<HighlightSpan>) {
    if let Some(v) = &part.binding {
        push_variable(out, v);
    }
    visit_pattern_element(&part.element, source, out);
}

fn visit_pattern_element(el: &PatternElement, source: &str, out: &mut Vec<HighlightSpan>) {
    match el {
        PatternElement::NodeChain { head, chain, .. } => {
            visit_node_pattern(head, source, out);
            for link in chain {
                visit_chain_link(link, source, out);
            }
        }
        PatternElement::Parenthesized(inner, _) => {
            visit_pattern_element(inner, source, out);
        }
        PatternElement::ShortestPath { element, .. } => {
            visit_pattern_element(element, source, out);
        }
    }
}

fn visit_chain_link(link: &PatternElementChain, source: &str, out: &mut Vec<HighlightSpan>) {
    visit_rel_pattern(&link.relationship, source, out);
    visit_node_pattern(&link.node, source, out);
}

fn visit_node_pattern(np: &lora_ast::NodePattern, source: &str, out: &mut Vec<HighlightSpan>) {
    if let Some(v) = &np.variable {
        push_variable(out, v);
    }
    // Labels and property keys have no per-token span — recover them by
    // scanning the slice of source covered by the node pattern.
    let slice = &source[np.span.start..np.span.end];
    let base = np.span.start;
    mark_labels(slice, base, &mut |s, e| {
        push(out, s, e, HighlightKind::Label)
    });
    if let Some(props) = &np.properties {
        visit_expr(props, source, out);
        mark_property_keys(slice, base, &mut |s, e| {
            push(out, s, e, HighlightKind::PropertyKey)
        });
    }
}

fn visit_rel_pattern(
    rp: &lora_ast::RelationshipPattern,
    source: &str,
    out: &mut Vec<HighlightSpan>,
) {
    if let Some(detail) = &rp.detail {
        visit_rel_detail(detail, source, out);
    }
}

fn visit_rel_detail(rd: &RelationshipDetail, source: &str, out: &mut Vec<HighlightSpan>) {
    if let Some(v) = &rd.variable {
        push_variable(out, v);
    }
    let slice = &source[rd.span.start..rd.span.end];
    let base = rd.span.start;
    mark_labels(slice, base, &mut |s, e| {
        push(out, s, e, HighlightKind::RelType)
    });
    if let Some(props) = &rd.properties {
        visit_expr(props, source, out);
        mark_property_keys(slice, base, &mut |s, e| {
            push(out, s, e, HighlightKind::PropertyKey)
        });
    }
}

fn visit_expr(expr: &Expr, source: &str, out: &mut Vec<HighlightSpan>) {
    match expr {
        Expr::Variable(v) => push_variable(out, v),
        Expr::Integer(_, span) | Expr::Float(_, span) => {
            push(out, span.start, span.end, HighlightKind::NumberLiteral);
        }
        Expr::String(_, span) => {
            push(out, span.start, span.end, HighlightKind::StringLiteral);
        }
        Expr::Bool(_, span) => {
            push(out, span.start, span.end, HighlightKind::BoolLiteral);
        }
        Expr::Null(span) => {
            push(out, span.start, span.end, HighlightKind::NullLiteral);
        }
        Expr::Parameter(_, span) => {
            push(out, span.start, span.end, HighlightKind::Parameter);
        }
        Expr::List(items, _) => {
            for it in items {
                visit_expr(it, source, out);
            }
        }
        Expr::Map(entries, span) => {
            mark_property_keys(&source[span.start..span.end], span.start, &mut |s, e| {
                push(out, s, e, HighlightKind::PropertyKey)
            });
            for (_, v) in entries {
                visit_expr(v, source, out);
            }
        }
        Expr::Property { expr, span, .. } => {
            visit_expr(expr, source, out);
            // The key sits between the last `.` and `span.end`.
            let slice = &source[span.start..span.end];
            if let Some(dot) = slice.rfind('.') {
                let key_start = span.start + dot + 1;
                push(out, key_start, span.end, HighlightKind::PropertyKey);
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            visit_expr(lhs, source, out);
            visit_expr(rhs, source, out);
        }
        Expr::Unary { expr, .. } => visit_expr(expr, source, out),
        Expr::FunctionCall {
            name, args, span, ..
        } => {
            mark_function_name(source, name, *span, out);
            for a in args {
                visit_expr(a, source, out);
            }
        }
        Expr::TypeCast { expr, .. } => visit_expr(expr, source, out),
        Expr::Case {
            input,
            alternatives,
            else_expr,
            ..
        } => {
            if let Some(i) = input {
                visit_expr(i, source, out);
            }
            for (cond, val) in alternatives {
                visit_expr(cond, source, out);
                visit_expr(val, source, out);
            }
            if let Some(e) = else_expr {
                visit_expr(e, source, out);
            }
        }
        Expr::ListPredicate {
            variable,
            list,
            predicate,
            ..
        } => {
            push_variable(out, variable);
            visit_expr(list, source, out);
            visit_expr(predicate, source, out);
        }
        Expr::ListComprehension {
            variable,
            list,
            filter,
            map_expr,
            ..
        } => {
            push_variable(out, variable);
            visit_expr(list, source, out);
            if let Some(f) = filter {
                visit_expr(f, source, out);
            }
            if let Some(m) = map_expr {
                visit_expr(m, source, out);
            }
        }
        Expr::Reduce {
            accumulator,
            init,
            variable,
            list,
            expr,
            ..
        } => {
            push_variable(out, accumulator);
            push_variable(out, variable);
            visit_expr(init, source, out);
            visit_expr(list, source, out);
            visit_expr(expr, source, out);
        }
        _ => {}
    }
}

/// Mark the function-name (and namespace prefix, when present) of a
/// `Foo.bar(args)` call. The AST's call span can be quite a bit wider
/// than the call itself (it may swallow surrounding tokens), so we
/// walk backwards from the first `(` to find the actual identifier.
fn mark_function_name(
    source: &str,
    name: &[String],
    span: lora_ast::Span,
    out: &mut Vec<HighlightSpan>,
) {
    if name.is_empty() {
        return;
    }
    let slice = &source[span.start..span.end];
    let Some(paren_rel) = slice.find('(') else {
        return;
    };
    let bytes = slice.as_bytes();

    // Step back from `(` over whitespace.
    let mut end_of_ident = paren_rel;
    while end_of_ident > 0 && bytes[end_of_ident - 1].is_ascii_whitespace() {
        end_of_ident -= 1;
    }
    if end_of_ident == 0 {
        return;
    }

    // Identifier characters [A-Za-z0-9_] back from end_of_ident.
    let mut start_of_ident = end_of_ident;
    while start_of_ident > 0 {
        let c = bytes[start_of_ident - 1];
        if c.is_ascii_alphanumeric() || c == b'_' {
            start_of_ident -= 1;
        } else {
            break;
        }
    }
    if start_of_ident == end_of_ident {
        return;
    }

    push(
        out,
        span.start + start_of_ident,
        span.start + end_of_ident,
        HighlightKind::FunctionName,
    );

    // Namespace prefix: if a `.` directly precedes the identifier,
    // sweep further back over `[A-Za-z0-9_.]` to capture the full path.
    if name.len() > 1 && start_of_ident > 0 && bytes[start_of_ident - 1] == b'.' {
        let mut ns_start = start_of_ident - 1;
        while ns_start > 0 {
            let c = bytes[ns_start - 1];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
                ns_start -= 1;
            } else {
                break;
            }
        }
        push(
            out,
            span.start + ns_start,
            span.start + start_of_ident - 1,
            HighlightKind::Namespace,
        );
    }
}

/// Within a slice, find every `:Identifier` sequence and emit its
/// identifier as a span anchored at `base`. Skips identifiers that are
/// preceded by `::` (cast operator) and identifiers that sit inside a
/// `{ … }` property map — those colons are map-entry separators
/// (`{email: row.email}`), not label markers, and treating them as
/// labels would mislabel any identifier on the right-hand side of a
/// property's value (`row` would otherwise show as a node label).
/// String literals are skipped too so a `':Foo'` inside a value
/// doesn't trigger.
fn mark_labels(slice: &str, base: usize, emit: &mut dyn FnMut(usize, usize)) {
    let bytes = slice.as_bytes();
    let mut depth_braces: i32 = 0;
    let mut in_string: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            in_string = Some(c);
            i += 1;
            continue;
        }
        if c == b'{' {
            depth_braces += 1;
            i += 1;
            continue;
        }
        if c == b'}' {
            depth_braces -= 1;
            i += 1;
            continue;
        }
        if c == b':' && depth_braces == 0 {
            // skip `::`
            if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                i += 2;
                continue;
            }
            // skip whitespace
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > start {
                emit(base + start, base + j);
            }
            i = j;
            continue;
        }
        i += 1;
    }
}

/// Mark `key:` style property keys inside a map literal slice.
fn mark_property_keys(slice: &str, base: usize, emit: &mut dyn FnMut(usize, usize)) {
    let bytes = slice.as_bytes();
    let mut depth_braces: i32 = 0;
    let mut depth_parens: i32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth_braces += 1,
            b'}' => depth_braces -= 1,
            b'(' => depth_parens += 1,
            b')' => depth_parens -= 1,
            _ => {}
        }
        if depth_braces > 0 && bytes[i].is_ascii_alphabetic() {
            let start = i;
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            // peek next non-whitespace
            let mut k = j;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b':' {
                emit(base + start, base + j);
            }
            i = j;
        } else {
            i += 1;
        }
        let _ = depth_parens; // currently unused, kept for future relational guards
    }
}

// ─────────────────────────────────────────────────────────────────────
// Semantic checks
// ─────────────────────────────────────────────────────────────────────

fn make_diag(severity: Severity, message: String, start: usize, end: usize) -> Diagnostic {
    Diagnostic {
        severity,
        message,
        details: String::new(),
        line: 0,
        column: 0,
        expected: Vec::new(),
        examples: Vec::new(),
        span: Span { start, end },
    }
}

/// Walk the source for every occurrence of `\bword\b` outside strings
/// and comments. Returns byte offsets into `source`.
fn scan_word_uses(source: &str, word: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let word_bytes = word.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state: u8 = 0; // 0 normal, 1 single-q, 2 double-q, 3 backtick, 4 line-comment, 5 block-comment
    while i < bytes.len() {
        let c = bytes[i];
        match state {
            0 => {
                if c == b'\'' {
                    state = 1;
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    state = 2;
                    i += 1;
                    continue;
                }
                if c == b'`' {
                    state = 3;
                    i += 1;
                    continue;
                }
                if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    state = 4;
                    i += 2;
                    continue;
                }
                if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    state = 5;
                    i += 2;
                    continue;
                }
                if c == word_bytes[0]
                    && i + word_bytes.len() <= bytes.len()
                    && &bytes[i..i + word_bytes.len()] == word_bytes
                {
                    let before_ok = i == 0 || {
                        let p = bytes[i - 1];
                        !(p.is_ascii_alphanumeric() || p == b'_')
                    };
                    let after_idx = i + word_bytes.len();
                    let after_ok = after_idx == bytes.len() || {
                        let n = bytes[after_idx];
                        !(n.is_ascii_alphanumeric() || n == b'_')
                    };
                    if before_ok && after_ok {
                        out.push((i, after_idx));
                        i = after_idx;
                        continue;
                    }
                }
                i += 1;
            }
            1..=3 => {
                let quote = if state == 1 {
                    b'\''
                } else if state == 2 {
                    b'"'
                } else {
                    b'`'
                };
                if c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == quote {
                    state = 0;
                }
                i += 1;
            }
            4 => {
                if c == b'\n' {
                    state = 0;
                }
                i += 1;
            }
            5 => {
                if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    state = 0;
                    i += 2;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Find variable references in the source that don't correspond to any
/// declared binding in the outline. Useful for catching typos like
/// `RETURN nm` after `MATCH (n)`.
fn check_undeclared_uses(outline: &Outline, source: &str, out: &mut Vec<Diagnostic>) {
    let known: std::collections::HashSet<&str> =
        outline.variables.iter().map(|v| v.name.as_str()).collect();

    // We don't have a full identifier-use inventory yet (would require
    // a deeper visit). Heuristic for now: scan the source for known
    // keywords / function names / typed builtins, and flag any *new*
    // identifier that appears in a RETURN / WITH projection but isn't
    // declared. Phase 1: identify candidates from the source text by
    // looking at the post-RETURN / post-WITH slice.
    //
    // Empty `known` set is a strong signal the query has zero
    // declarations — bail out so we don't spam warnings for queries
    // like standalone calls.
    if known.is_empty() {
        return;
    }

    for keyword in ["RETURN", "WITH"] {
        for (kw_start, kw_end) in scan_word_uses(source, keyword) {
            // Walk forward from `kw_end` to the next clause boundary or
            // end-of-input, splitting on top-level commas (depth 0).
            let mut depth = 0i32;
            let mut tok_start: Option<usize> = None;
            let mut state: u8 = 0;
            let bytes = source.as_bytes();
            let mut i = kw_end;
            while i < bytes.len() {
                let c = bytes[i];
                match state {
                    0 => {
                        let is_ident_char = c.is_ascii_alphabetic()
                            || c == b'_'
                            || (tok_start.is_some() && (c.is_ascii_alphanumeric() || c == b'.'));

                        // ALWAYS close an open token before reacting to
                        // a non-identifier character — strings, parens,
                        // commas, even quote starts. Otherwise tokens
                        // bleed across delimiters (e.g. `count(r)` got
                        // emitted as one bogus identifier).
                        if !is_ident_char {
                            if let Some(start) = tok_start.take() {
                                // Skip function calls — `count(`, `string.upper(`, etc.
                                if !followed_by_open_paren(source, i) {
                                    let raw = &source[start..i];
                                    emit_unknown(raw, start, i, &known, out);
                                }
                            }
                        }

                        if c == b'\'' {
                            state = 1;
                        } else if c == b'"' {
                            state = 2;
                        } else if c == b'(' || c == b'[' || c == b'{' {
                            depth += 1;
                        } else if c == b')' || c == b']' || c == b'}' {
                            depth = (depth - 1).max(0);
                        } else if depth == 0 {
                            // Detect end of projection list: another
                            // clause keyword starts.
                            if (c == b'\n' || c == b' ') && tok_start.is_none() {
                                if let Some(kw) = match_clause_starter(source, i + 1) {
                                    let _ = kw;
                                    break;
                                }
                            }
                            // Function-call detection: skip the
                            // identifier when it's immediately followed
                            // by `(` (after optional whitespace).
                            // We handled the boundary above, but we
                            // also want to avoid flagging an
                            // identifier *just before* the `(`. Track
                            // the most recent emitted token in
                            // `emit_unknown` via the helper below.
                            if (c.is_ascii_alphabetic() || c == b'_') && tok_start.is_none() {
                                tok_start = Some(i);
                            }
                        }
                    }
                    1 => {
                        if c == b'\\' && i + 1 < bytes.len() {
                            i += 1;
                        } else if c == b'\'' {
                            state = 0;
                        }
                    }
                    2 => {
                        if c == b'\\' && i + 1 < bytes.len() {
                            i += 1;
                        } else if c == b'"' {
                            state = 0;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if let Some(start) = tok_start {
                if !followed_by_open_paren(source, i) {
                    let raw = &source[start..i];
                    emit_unknown(raw, start, i, &known, out);
                }
            }
            let _ = kw_start;
        }
    }
}

fn emit_unknown(
    raw: &str,
    start: usize,
    end: usize,
    known: &std::collections::HashSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    // Bail on dotted access like `friend.name` — only the head matters.
    let head = raw.split('.').next().unwrap_or(raw);
    if head.is_empty()
        || !head
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return;
    }
    // Keywords / known functions / namespaces — these are not variables.
    if is_reserved_word(head) {
        return;
    }
    if known.contains(head) {
        return;
    }
    out.push(make_diag(
        Severity::Warning,
        format!("`{head}` is not declared anywhere earlier in the query."),
        start,
        end - (raw.len() - head.len()),
    ));
}

/// Look ahead past whitespace in `source` starting at `from` and return
/// true when the next non-whitespace character is `(` — i.e. the
/// preceding identifier is a function call, not a variable reference.
fn followed_by_open_paren(source: &str, from: usize) -> bool {
    let bytes = source.as_bytes();
    let mut i = from;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i < bytes.len() && bytes[i] == b'('
}

fn is_reserved_word(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "MATCH"
            | "OPTIONAL"
            | "WHERE"
            | "RETURN"
            | "WITH"
            | "CREATE"
            | "MERGE"
            | "DELETE"
            | "DETACH"
            | "SET"
            | "REMOVE"
            | "UNWIND"
            | "ORDER"
            | "BY"
            | "ASC"
            | "DESC"
            | "LIMIT"
            | "SKIP"
            | "AS"
            | "AND"
            | "OR"
            | "XOR"
            | "NOT"
            | "IN"
            | "IS"
            | "NULL"
            | "TRUE"
            | "FALSE"
            | "CALL"
            | "YIELD"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "DISTINCT"
            | "EXISTS"
            | "CONSTRAINT"
            | "INDEX"
            | "SHOW"
            | "DROP"
            | "UNIQUE"
            | "FOR"
            | "ON"
            | "REQUIRE"
            | "COUNT"
            | "SUM"
            | "AVG"
            | "MIN"
            | "MAX"
            | "SIZE"
            | "KEYS"
            | "REVERSE"
            | "ID"
            | "MATH"
            | "STRING"
            | "LIST"
            | "MAP"
            | "BYTES"
            | "BITS"
            | "JSON"
            | "UUID"
            | "CAST"
            | "TYPE"
            | "TEMPORAL"
            | "GEO"
            | "VECTOR"
            | "CRYPTO"
    )
}

fn check_schema(outline: &Outline, cfg: &AnalyseConfig, out: &mut Vec<Diagnostic>) {
    if cfg.strict_labels && !cfg.labels.is_empty() {
        let known: std::collections::HashSet<&str> =
            cfg.labels.iter().map(String::as_str).collect();
        for l in &outline.labels {
            if !known.contains(l.as_str()) {
                out.push(make_diag(
                    Severity::Warning,
                    format!("Label `:{l}` is not in the known schema."),
                    0,
                    0,
                ));
            }
        }
    }
    if cfg.strict_rel_types && !cfg.rel_types.is_empty() {
        let known: std::collections::HashSet<&str> =
            cfg.rel_types.iter().map(String::as_str).collect();
        for t in &outline.rel_types {
            if !known.contains(t.as_str()) {
                out.push(make_diag(
                    Severity::Warning,
                    format!("Relationship type `:{t}` is not in the known schema."),
                    0,
                    0,
                ));
            }
        }
    }
}

fn check_unused_bindings(outline: &Outline, source: &str, out: &mut Vec<Diagnostic>) {
    for v in &outline.variables {
        let uses = scan_word_uses(source, &v.name);
        if uses.len() <= 1 {
            // Declared but never referenced anywhere else.
            out.push(make_diag(
                Severity::Info,
                format!("`{}` is declared but never used.", v.name),
                v.decl_start,
                v.decl_end,
            ));
        }
    }
}

/// Flag function calls that reference a name absent from
/// `BUILTIN_SPECS`, `BUILTIN_ALIASES`, and `AggregateFunction`. The
/// engine would reject these at compile time anyway; surfacing the
/// diagnostic in the editor lets the user catch the typo before
/// running.
fn check_unknown_functions(doc: &Document, _source: &str, out: &mut Vec<Diagnostic>) {
    let mut calls: Vec<(String, lora_ast::Span)> = Vec::new();
    walk_function_calls(&doc.statement, &mut calls);
    for (name, span) in calls {
        if is_known_function(&name) {
            continue;
        }
        out.push(make_diag(
            Severity::Warning,
            format!("Unknown function `{name}`."),
            span.start,
            span.end,
        ));
    }
}

fn is_known_function(name: &str) -> bool {
    lora_builtins_meta::resolve_function(name).is_some()
}

fn walk_function_calls(stmt: &Statement, out: &mut Vec<(String, lora_ast::Span)>) {
    if let Statement::Query(Query::Regular(rq)) = stmt {
        walk_fn_single(&rq.head, out);
        for u in &rq.unions {
            walk_fn_single(&u.query, out);
        }
    }
}

fn walk_fn_single(q: &SingleQuery, out: &mut Vec<(String, lora_ast::Span)>) {
    match q {
        SingleQuery::SinglePart(part) => walk_fn_single_part(part, out),
        SingleQuery::MultiPart(mp) => {
            for p in &mp.parts {
                for rc in &p.reading_clauses {
                    walk_fn_reading(rc, out);
                }
                for uc in &p.updating_clauses {
                    walk_fn_updating(uc, out);
                }
                for item in &p.with_clause.body.items {
                    if let ProjectionItem::Expr { expr, .. } = item {
                        walk_fn_expr(expr, out);
                    }
                }
                if let Some(filter) = &p.with_clause.where_ {
                    walk_fn_expr(filter, out);
                }
            }
            walk_fn_single_part(&mp.tail, out);
        }
    }
}

fn walk_fn_single_part(part: &SinglePartQuery, out: &mut Vec<(String, lora_ast::Span)>) {
    for rc in &part.reading_clauses {
        walk_fn_reading(rc, out);
    }
    for uc in &part.updating_clauses {
        walk_fn_updating(uc, out);
    }
    if let Some(ret) = &part.return_clause {
        for item in &ret.body.items {
            if let ProjectionItem::Expr { expr, .. } = item {
                walk_fn_expr(expr, out);
            }
        }
    }
}

fn walk_fn_reading(rc: &ReadingClause, out: &mut Vec<(String, lora_ast::Span)>) {
    match rc {
        ReadingClause::Match(m) => {
            if let Some(filter) = &m.where_ {
                walk_fn_expr(filter, out);
            }
        }
        ReadingClause::Unwind(u) => walk_fn_expr(&u.expr, out),
        ReadingClause::InQueryCall(_) => {}
        ReadingClause::CallSubquery(c) => {
            walk_fn_single(&c.body.head, out);
            for u in &c.body.unions {
                walk_fn_single(&u.query, out);
            }
        }
    }
}

fn walk_fn_updating(uc: &UpdatingClause, out: &mut Vec<(String, lora_ast::Span)>) {
    match uc {
        UpdatingClause::Create(_) | UpdatingClause::Merge(_) => {}
        UpdatingClause::Delete(d) => {
            for e in &d.expressions {
                walk_fn_expr(e, out);
            }
        }
        UpdatingClause::Set(s) => {
            for item in &s.items {
                match item {
                    lora_ast::SetItem::SetProperty { target, value, .. } => {
                        walk_fn_expr(target, out);
                        walk_fn_expr(value, out);
                    }
                    lora_ast::SetItem::SetVariable { value, .. }
                    | lora_ast::SetItem::MutateVariable { value, .. } => walk_fn_expr(value, out),
                    lora_ast::SetItem::SetLabels { .. } => {}
                }
            }
        }
        UpdatingClause::Remove(_) => {}
        UpdatingClause::Foreach(f) => {
            walk_fn_expr(&f.list, out);
            for body in &f.body {
                walk_fn_updating(body, out);
            }
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
fn walk_fn_expr(expr: &Expr, out: &mut Vec<(String, lora_ast::Span)>) {
    match expr {
        Expr::FunctionCall {
            name, args, span, ..
        } => {
            let joined = name.join(".");
            out.push((joined, *span));
            for a in args {
                walk_fn_expr(a, out);
            }
        }
        Expr::List(items, _) => {
            for it in items {
                walk_fn_expr(it, out);
            }
        }
        Expr::Map(entries, _) => {
            for (_, v) in entries {
                walk_fn_expr(v, out);
            }
        }
        Expr::Property { expr, .. } => walk_fn_expr(expr, out),
        Expr::Binary { lhs, rhs, .. } => {
            walk_fn_expr(lhs, out);
            walk_fn_expr(rhs, out);
        }
        Expr::Unary { expr, .. } => walk_fn_expr(expr, out),
        Expr::TypeCast { expr, .. } => walk_fn_expr(expr, out),
        Expr::Case {
            input,
            alternatives,
            else_expr,
            ..
        } => {
            if let Some(i) = input {
                walk_fn_expr(i, out);
            }
            for (cond, val) in alternatives {
                walk_fn_expr(cond, out);
                walk_fn_expr(val, out);
            }
            if let Some(e) = else_expr {
                walk_fn_expr(e, out);
            }
        }
        Expr::ListPredicate {
            list, predicate, ..
        } => {
            walk_fn_expr(list, out);
            walk_fn_expr(predicate, out);
        }
        Expr::ListComprehension {
            list,
            filter,
            map_expr,
            ..
        } => {
            walk_fn_expr(list, out);
            if let Some(f) = filter {
                walk_fn_expr(f, out);
            }
            if let Some(m) = map_expr {
                walk_fn_expr(m, out);
            }
        }
        Expr::Reduce {
            init, list, expr, ..
        } => {
            walk_fn_expr(init, out);
            walk_fn_expr(list, out);
            walk_fn_expr(expr, out);
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────
// Fold-range collection
// ─────────────────────────────────────────────────────────────────────

fn collect_fold_ranges(doc: &Document, _source: &str) -> Vec<FoldRange> {
    let mut out = Vec::new();
    walk_fold_statement(&doc.statement, &mut out);
    out.sort_by_key(|r| r.start);
    out
}

fn push_fold(out: &mut Vec<FoldRange>, span: lora_ast::Span, kind: &str) {
    if span.end > span.start {
        out.push(FoldRange {
            start: span.start,
            end: span.end,
            kind: kind.to_owned(),
        });
    }
}

fn walk_fold_statement(stmt: &Statement, out: &mut Vec<FoldRange>) {
    if let Statement::Query(Query::Regular(rq)) = stmt {
        walk_fold_single(&rq.head, out);
        for u in &rq.unions {
            walk_fold_single(&u.query, out);
        }
    }
}

fn walk_fold_single(q: &SingleQuery, out: &mut Vec<FoldRange>) {
    match q {
        SingleQuery::SinglePart(part) => walk_fold_single_part(part, out),
        SingleQuery::MultiPart(mp) => {
            for p in &mp.parts {
                for rc in &p.reading_clauses {
                    walk_fold_reading(rc, out);
                }
                for uc in &p.updating_clauses {
                    walk_fold_updating(uc, out);
                }
                push_fold(out, p.with_clause.body.span, "projection");
            }
            walk_fold_single_part(&mp.tail, out);
        }
    }
}

fn walk_fold_single_part(part: &SinglePartQuery, out: &mut Vec<FoldRange>) {
    for rc in &part.reading_clauses {
        walk_fold_reading(rc, out);
    }
    for uc in &part.updating_clauses {
        walk_fold_updating(uc, out);
    }
    if let Some(ret) = &part.return_clause {
        push_fold(out, ret.body.span, "projection");
    }
}

fn walk_fold_reading(rc: &ReadingClause, out: &mut Vec<FoldRange>) {
    match rc {
        ReadingClause::Match(m) => {
            push_fold(out, m.pattern.span, "pattern");
        }
        ReadingClause::Unwind(u) => {
            push_fold(out, u.expr.span(), "expression");
        }
        ReadingClause::CallSubquery(c) => {
            // The whole `CALL { ... }` block is collapsible, and we
            // recurse so any folds inside the inner query (its own
            // patterns, projections, etc.) are surfaced too.
            push_fold(out, c.span, "subquery");
            walk_fold_single(&c.body.head, out);
            for u in &c.body.unions {
                walk_fold_single(&u.query, out);
            }
        }
        _ => {}
    }
}

fn walk_fold_updating(uc: &UpdatingClause, out: &mut Vec<FoldRange>) {
    if let UpdatingClause::Create(c) = uc {
        push_fold(out, c.pattern.span, "pattern");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettify_uppercases_keywords_and_normalises_commas() {
        let input = "match (a,b)\nwhere a.name = 'lower'\nreturn a,b\n";
        let out = prettify(input);
        // continuation clauses (WHERE) get indented two spaces
        assert_eq!(out, "MATCH (a, b)\n  WHERE a.name = 'lower'\nRETURN a, b\n");
    }

    #[test]
    fn prettify_breaks_clauses_onto_their_own_lines() {
        let input = "match (n) where n.age > 18 return n order by n.name limit 5";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (n)\n  WHERE n.age > 18\nRETURN n\n  ORDER BY n.name\n  LIMIT 5\n"
        );
    }

    #[test]
    fn prettify_keeps_clauses_inside_strings_alone() {
        let input = "RETURN 'match where return as one string'";
        let out = prettify(input);
        assert_eq!(out, "RETURN 'match where return as one string'\n");
    }

    #[test]
    fn prettify_is_idempotent() {
        let input = "match (n) where n.age > 18 return n order by n.name limit 5";
        let once = prettify(input);
        let twice = prettify(&once);
        assert_eq!(once, twice, "second prettify() should not change output");
    }

    #[test]
    fn prettify_splits_long_projections() {
        let input = "MATCH (n) RETURN n.name, n.age, n.email, n.role";
        let out = prettify(input);
        assert!(out.contains("RETURN\n  n.name,\n  n.age,\n  n.email,\n  n.role\n"));
    }

    #[test]
    fn prettify_keeps_short_projections_inline() {
        // Two simple references, no AS, no call — stays inline.
        let input = "MATCH (n) RETURN n.name, n.age";
        let out = prettify(input);
        assert_eq!(out, "MATCH (n)\nRETURN n.name, n.age\n");
    }

    #[test]
    fn prettify_splits_two_item_projection_with_alias() {
        // Two items but each has an `AS` alias — split for clarity.
        let input = "MATCH (n) RETURN n.name AS name, n.age AS age";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (n)\nRETURN\n  n.name AS name,\n  n.age AS age\n"
        );
    }

    #[test]
    fn prettify_statushook_storybook_example() {
        // The StatusHook story input that the user reported as not
        // pretty enough — RETURN with two aliased items now splits.
        let input = "MATCH (alice:Person {name: 'Alice'})-[r:KNOWS]->(friend)\nWHERE friend.age > $minAge AND friend.name <> $excluded\nRETURN friend.name AS name, count(r) AS hops";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (alice:Person {name: 'Alice'})-[r:KNOWS]->(friend)\n  WHERE friend.age > $minAge\n    AND friend.name <> $excluded\nRETURN\n  friend.name AS name,\n  count(r) AS hops\n"
        );
        // Idempotent.
        let twice = prettify(&out);
        assert_eq!(out, twice);
    }

    #[test]
    fn prettify_splits_two_item_projection_with_call() {
        // Function call in an item triggers the split too.
        let input = "MATCH (n)-[r]->() RETURN n.name, count(r)";
        let out = prettify(input);
        assert_eq!(out, "MATCH (n)-[r]->()\nRETURN\n  n.name,\n  count(r)\n");
    }

    #[test]
    fn split_top_level_commas_respects_brackets() {
        let parts = split_top_level_commas("a, [b, c], d");
        assert_eq!(parts, vec!["a", " [b, c]", " d"]);
    }

    #[test]
    fn prettify_splits_long_where_on_and() {
        let input = "MATCH (n) WHERE n.active = TRUE AND n.role = 'admin' RETURN n";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (n)\n  WHERE n.active = TRUE\n    AND n.role = 'admin'\nRETURN n\n"
        );
    }

    #[test]
    fn prettify_splits_long_where_on_or() {
        let input = "MATCH (n) WHERE n.role = 'admin' OR n.role = 'editor' RETURN n";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (n)\n  WHERE n.role = 'admin'\n    OR n.role = 'editor'\nRETURN n\n"
        );
    }

    #[test]
    fn prettify_where_split_respects_parens() {
        let input = "MATCH (n) WHERE (a AND b) OR c RETURN n";
        let out = prettify(input);
        // The inner AND is inside parens, so it must not be split.
        assert_eq!(out, "MATCH (n)\n  WHERE (a AND b)\n    OR c\nRETURN n\n");
    }

    #[test]
    fn prettify_where_split_keeps_single_predicate_inline() {
        let input = "MATCH (n) WHERE n.active = TRUE RETURN n";
        let out = prettify(input);
        assert_eq!(out, "MATCH (n)\n  WHERE n.active = TRUE\nRETURN n\n");
    }

    #[test]
    fn prettify_where_split_is_idempotent() {
        let input = "MATCH (n) WHERE a AND b AND c RETURN n";
        let once = prettify(input);
        let twice = prettify(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn prettify_where_split_ignores_and_inside_string() {
        // `AND` lives inside a string literal — must not become a split.
        let input = "MATCH (n) WHERE n.name = 'A AND B' AND n.active RETURN n";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (n)\n  WHERE n.name = 'A AND B'\n    AND n.active\nRETURN n\n"
        );
    }

    #[test]
    fn prettify_indents_call_subquery_body() {
        let input = "MATCH (u) CALL { WITH u MATCH (u)-[:R]->(m) RETURN m } RETURN u";
        let out = prettify(input);
        // CALL { ... } body lines are indented one extra level (2 spaces).
        assert!(out.contains("CALL {"), "got: {out:?}");
        assert!(out.contains("\n  WITH u"), "got: {out:?}");
        assert!(out.contains("\n  MATCH (u)-[:R]->(m)"), "got: {out:?}");
        assert!(out.contains("\n  RETURN m"), "got: {out:?}");
        // Closing `}` returns to the outer indent.
        assert!(out.contains("\n}\n"), "got: {out:?}");
    }

    #[test]
    fn prettify_subquery_indent_is_idempotent() {
        let input = "MATCH (u) CALL { WITH u MATCH (u)-[:R]->(m) RETURN m } RETURN u";
        let once = prettify(input);
        let twice = prettify(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn prettify_keeps_list_comprehension_where_inline() {
        // `WHERE` inside a `[ ... ]` list comprehension is a comprehension
        // filter, not a clause — the prettifier must leave it inline so
        // we don't break the comprehension across lines.
        let input =
            "MATCH (n) WITH [g IN n.tags WHERE g IN ['a','b'] | g] AS overlap RETURN overlap";
        let out = prettify(input);
        assert!(
            out.contains("[g IN n.tags WHERE g IN ['a', 'b'] | g] AS overlap"),
            "got: {out:?}"
        );
        // And the WHERE token must not start a new line anywhere in the
        // output (the only WHERE is the comprehension one).
        for line in out.lines() {
            assert!(!line.trim_start().starts_with("WHERE "), "got: {out:?}");
        }
    }

    #[test]
    fn prettify_keeps_list_predicate_where_inline() {
        // Same rule for list predicates: `ANY(x IN xs WHERE pred(x))`
        // and friends are expression-level — the inner `WHERE` is a
        // predicate marker, not a clause.
        let input = "MATCH (n) WHERE ANY(t IN n.tags WHERE t STARTS WITH 'a') RETURN n";
        let out = prettify(input);
        // Only one WHERE line — the top-level one.
        let where_lines = out
            .lines()
            .filter(|l| l.trim_start().starts_with("WHERE "))
            .count();
        assert_eq!(where_lines, 1, "got: {out:?}");
    }

    #[test]
    fn prettify_reflows_multi_line_projection_with_trailing_comma() {
        // Continuation line on a comma — used to leave a blank line +
        // mis-indented continuation in the output.
        let input =
            "MATCH (n) WITH n, a, b,\n     c * 1.0 / d AS score\nRETURN n.name AS name, score";
        let out = prettify(input);
        // 4 items must split, no blank line, no extra indent.
        assert!(
            out.contains("WITH\n  n,\n  a,\n  b,\n  c * 1.0 / d AS score\n"),
            "got: {out:?}"
        );
        // 2-item RETURN with an `AS` alias now splits too.
        assert!(
            out.contains("RETURN\n  n.name AS name,\n  score\n"),
            "got: {out:?}"
        );
        // No blank line snuck in.
        assert!(!out.contains("\n\n"), "got: {out:?}");
    }

    #[test]
    fn prettify_normalises_existing_and_continuation_indent() {
        // The user's bug: AND continuation kept the 2-space indent
        // matching WHERE, when it should be WHERE + 2 = 4.
        let input = "MATCH (n) WHERE n.active = TRUE\n  AND n.role = 'admin' RETURN n";
        let out = prettify(input);
        assert_eq!(
            out,
            "MATCH (n)\n  WHERE n.active = TRUE\n    AND n.role = 'admin'\nRETURN n\n"
        );
    }

    #[test]
    fn prettify_keeps_on_create_set_together() {
        // ON CREATE SET / ON MATCH SET must stay on a single line even
        // though SET is independently a clause starter.
        let input = "MERGE (n:Person {email: $email}) ON CREATE SET n.name = $name ON MATCH SET n.lastSeen = timestamp() RETURN n";
        let out = prettify(input);
        assert_eq!(
            out,
            "MERGE (n:Person {email: $email})\n  ON CREATE SET n.name = $name\n  ON MATCH SET n.lastSeen = timestamp()\nRETURN n\n"
        );
    }

    #[test]
    fn prettify_normalises_double_space_in_clause_starter() {
        // `ON MATCH  SET` (two spaces, used for visual alignment) must
        // still match the multi-word ON-MATCH-SET starter.
        let input = "MERGE (n) ON CREATE SET n.x = 1\nON MATCH  SET n.y = 2\nRETURN n";
        let out = prettify(input);
        assert!(out.contains("\n  ON MATCH SET n.y = 2"), "got: {out:?}");
        assert!(
            !out.contains("\nSET n.y"),
            "ON MATCH must not be split from SET; got: {out:?}"
        );
    }

    #[test]
    fn prettify_collapses_multi_space_outside_strings() {
        let input = "MATCH  (n)   RETURN  n";
        let out = prettify(input);
        assert_eq!(out, "MATCH (n)\nRETURN n\n");
    }

    #[test]
    fn prettify_preserves_unicode_in_comments() {
        // The em-dash `—` (U+2014, 3 UTF-8 bytes) used to be mangled
        // into mojibake because byte-level passes were re-emitting it
        // via `bytes[i] as char`.
        let input = "// Three queries — fold each one from the gutter.\nMATCH (n) RETURN n";
        let out = prettify(input);
        assert!(
            out.contains("// Three queries — fold each one from the gutter."),
            "got: {out:?}"
        );
    }

    #[test]
    fn prettify_preserves_unicode_in_strings() {
        // Non-ASCII inside string literals must round-trip unchanged.
        let input = "MATCH (n) WHERE n.name = 'Café — résumé' RETURN n";
        let out = prettify(input);
        assert!(out.contains("'Café — résumé'"), "got: {out:?}");
    }

    #[test]
    fn prettify_preserves_unicode_in_block_comments() {
        let input = "/* µ ≈ π — really */ MATCH (n) RETURN n";
        let out = prettify(input);
        assert!(out.contains("/* µ ≈ π — really */"), "got: {out:?}");
    }

    #[test]
    fn prettify_handles_four_byte_emoji_in_comment() {
        // 🌟 is a 4-byte UTF-8 char (F0 9F 8C 9F). Must round-trip.
        let input = "// star ⭐ rocket 🚀 done\nMATCH (n) RETURN n";
        let out = prettify(input);
        assert!(out.contains("// star ⭐ rocket 🚀 done"), "got: {out:?}");
    }

    #[test]
    fn prettify_handles_four_byte_emoji_in_string() {
        let input = "MATCH (n) WHERE n.tag = '🚀 boom' RETURN n";
        let out = prettify(input);
        assert!(out.contains("'🚀 boom'"), "got: {out:?}");
    }

    #[test]
    fn prettify_handles_nbsp_in_comment() {
        // NBSP (U+00A0) — two bytes (C2 A0). The continuation byte A0
        // used to be reported as "whitespace" by the byte-cast trim,
        // which would advance into the middle of the char.
        let input = "// a\u{A0}b\nMATCH (n) RETURN n";
        let out = prettify(input);
        assert!(out.contains("a\u{A0}b"), "got: {out:?}");
    }

    #[test]
    fn prettify_idempotent_on_unicode_heavy_query() {
        // Stress test: multi-byte chars in every kind of position.
        let input = "// résumé é\nMATCH (n) WHERE n.tag = '日本語' AND n.x = 1 RETURN n";
        let once = prettify(input);
        let twice = prettify(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn prettify_does_not_panic_on_storybook_inputs() {
        // Exercises every story's example query end-to-end. The goal
        // is a panic check, not a content check — any panic here means
        // the editor would crash when the user clicks Prettify.
        let cases: &[&str] = &[
            "MATCH (n) RETURN n",
            "MATCH (a:Person)-[:KNOWS]->(b)\nWHERE a.name = 'Alice'\nRETURN a, b\nORDER BY b.name\nLIMIT 10",
            "MATCH (alice:Person {name: 'Alice'})-[r:KNOWS*1..3]->(friend)\nWHERE friend.age > 21 AND NOT friend.archived\nWITH alice, friend, count(r) AS hops\nRETURN friend.name AS name, hops\nORDER BY hops ASC\nLIMIT 5",
            "MERGE (n:Person {email: $email})\nON CREATE SET n.name = $name, n.createdAt = timestamp()\nON MATCH  SET n.lastSeen = timestamp()\nRETURN n",
            "MATCH (n:Person)\nRETURN n.name AS name,\n       CASE\n         WHEN n.age < 18 THEN 'minor'\n         WHEN n.age < 65 THEN 'adult'\n         ELSE 'senior'\n       END AS bracket",
            "MATCH (user:Person {name: $user})\nCALL {\n  WITH user\n  MATCH (user)-[:RATED]->(m:Movie)\n  RETURN collect(m.genre) AS preferredGenres\n}\nRETURN preferredGenres",
            "// Three queries — fold each one from the gutter.\nMATCH (a) RETURN a;\nMATCH (b) RETURN b;",
            // Unicode-heavy:
            "// 日本語 — テスト\nMATCH (n) WHERE n.name = 'café' RETURN n",
            // Pathological input — the format() entry skips these,
            // but the inner passes are still exercised by paste/format
            // round-trips, so they must not panic either.
            "",
            ";",
            ";;;",
            "MATCH",
            "//",
            "/*",
            "'unterminated",
        ];
        for case in cases {
            let _ = format(case);
        }
    }

    #[test]
    fn prettify_preserves_whitespace_inside_strings() {
        let input = "RETURN 'a   b'";
        let out = prettify(input);
        assert_eq!(out, "RETURN 'a   b'\n");
    }

    #[test]
    fn prettify_splits_inline_case_expression() {
        let input =
            "MATCH (n) RETURN n.id, CASE WHEN n.age < 18 THEN 'minor' WHEN n.age < 65 THEN 'adult' ELSE 'senior' END AS bracket";
        let out = prettify(input);
        // CASE keyword stays on its own line; WHEN / ELSE indented +2;
        // END returns to CASE indent with the trailing alias preserved.
        assert!(
            out.contains("  CASE\n    WHEN n.age < 18 THEN 'minor'\n    WHEN n.age < 65 THEN 'adult'\n    ELSE 'senior'\n  END AS bracket"),
            "got: {out:?}"
        );
    }

    #[test]
    fn prettify_splits_case_with_scrutinee() {
        let input =
            "MATCH (n) RETURN CASE n.role WHEN 'admin' THEN 'red' WHEN 'editor' THEN 'orange' ELSE 'gray' END AS roleColor";
        let out = prettify(input);
        // CASE n.role on one line; each branch on its own indented line.
        assert!(out.contains("CASE n.role\n"), "got: {out:?}");
        assert!(out.contains("WHEN 'admin' THEN 'red'\n"), "got: {out:?}");
        assert!(out.contains("ELSE 'gray'\n"), "got: {out:?}");
        assert!(out.contains("END AS roleColor"), "got: {out:?}");
    }

    #[test]
    fn prettify_case_split_is_idempotent() {
        let input = "MATCH (n) RETURN CASE WHEN n.x > 0 THEN 'pos' ELSE 'non-pos' END AS sign";
        let once = prettify(input);
        let twice = prettify(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn prettify_case_storybook_example() {
        // The CaseExpression story's exact source — verifies both the
        // CASE-split and idempotency on the user-reported input.
        let input = "MATCH (n:Person)
RETURN n.name AS name,
       CASE
         WHEN n.age < 18 THEN 'minor'
         WHEN n.age < 65 THEN 'adult'
         ELSE 'senior'
       END AS bracket,
       CASE n.role
         WHEN 'admin'   THEN 'red'
         WHEN 'editor'  THEN 'orange'
         ELSE                'gray'
       END AS roleColor";
        let out = prettify(input);
        assert!(out.contains("RETURN\n  n.name AS name,\n"), "got: {out:?}");
        assert!(
            out.contains("CASE\n    WHEN n.age < 18 THEN 'minor'"),
            "got: {out:?}"
        );
        assert!(
            out.contains("CASE n.role\n    WHEN 'admin' THEN 'red'"),
            "got: {out:?}"
        );
        let twice = prettify(&out);
        assert_eq!(out, twice);
    }

    #[test]
    fn prettify_writeside_storybook_example() {
        // Reproduces the user's reported broken output for the
        // `WriteSideQueries` story.
        let input = "MERGE (n:Person {email: $email})
ON CREATE SET n.name = $name, n.createdAt = timestamp()
ON MATCH SET n.lastSeen = timestamp()
RETURN n";
        let out = prettify(input);
        assert_eq!(
            out,
            "MERGE (n:Person {email: $email})\n  ON CREATE SET n.name = $name, n.createdAt = timestamp()\n  ON MATCH SET n.lastSeen = timestamp()\nRETURN n\n"
        );
        // Idempotent.
        let twice = prettify(&out);
        assert_eq!(out, twice);
    }

    #[test]
    fn prettify_handles_complex_example_from_storybook() {
        // Reproduces the exact "ComplexExample" story input the user
        // reported as not properly prettified.
        let input = "// Find the top 5 most-influential users who follow Alice transitively,
// scored by an aggregate over their second-degree connections.
MATCH (alice:Person {name: 'Alice'})
MATCH path = (alice)-[:FOLLOWS*1..3]->(follower:Person)
WHERE follower.active = TRUE
  AND follower.createdAt > datetime('2024-01-01T00:00:00Z')

WITH alice, follower, length(path) AS distance

OPTIONAL MATCH (follower)-[:FOLLOWS]->(fof:Person)
WHERE fof <> alice

WITH alice, follower, distance, count(DISTINCT fof) AS reach

WITH follower, reach, distance,
     reach * 1.0 / (distance * distance) AS influenceScore
ORDER BY influenceScore DESC
LIMIT 5

RETURN follower.name AS user,
       reach,
       distance,
       influenceScore";
        let out = prettify(input);
        // WHERE chain at the correct (+2) continuation indent.
        assert!(
            out.contains("  WHERE follower.active = TRUE\n    AND follower.createdAt > datetime('2024-01-01T00:00:00Z')\n"),
            "got: {out:?}"
        );
        // WITH with 4 items emitted as one-per-line, no blank line, no stray indent.
        assert!(
            out.contains(
                "WITH\n  follower,\n  reach,\n  distance,\n  reach * 1.0 / (distance * distance) AS influenceScore\n"
            ),
            "got: {out:?}"
        );
        // The 4-item RETURN block is also normalised.
        assert!(
            out.contains(
                "RETURN\n  follower.name AS user,\n  reach,\n  distance,\n  influenceScore\n"
            ),
            "got: {out:?}"
        );
        // No double blank lines in the body.
        assert!(!out.contains("\n\n\n"), "got: {out:?}");
        // And it's idempotent.
        let twice = prettify(&out);
        assert_eq!(out, twice);
    }

    #[test]
    fn prettify_does_not_touch_string_contents() {
        let input = "RETURN 'match where return'";
        let out = uppercase_keywords(input);
        assert_eq!(out, "RETURN 'match where return'");
    }

    #[test]
    fn format_returns_input_on_parse_error() {
        let bad = "MATCH (";
        assert_eq!(format(bad), bad);
    }

    #[test]
    fn line_col_to_span_first_line() {
        let src = "MATCH (";
        let (s, _e) = line_col_to_span(src, 1, 8);
        assert_eq!(s, 7);
    }

    #[test]
    fn line_col_to_span_later_lines() {
        let src = "MATCH (n)\nWHERE a.name = 'Joos";
        let (s, _e) = line_col_to_span(src, 2, 16);
        assert_eq!(&src[..s], "MATCH (n)\nWHERE a.name = ");
    }

    #[test]
    fn parse_expected_extracts_rule_names() {
        let msg = "--> 1:8\n  = expected node_pattern, properties, or symbolic_name";
        let rules = parse_expected(msg);
        assert_eq!(rules, vec!["node_pattern", "properties", "symbolic_name"]);
    }

    #[test]
    fn mark_labels_in_node_pattern() {
        let slice = "(n:Person:Employee)";
        let mut spans: Vec<(usize, usize)> = Vec::new();
        mark_labels(slice, 100, &mut |s, e| spans.push((s, e)));
        // Person at offset 3 (100+3..100+9), Employee at offset 10 (100+10..100+18)
        assert_eq!(spans, vec![(103, 109), (110, 118)]);
    }

    #[test]
    fn mark_labels_skips_inside_property_map() {
        // Regression: `mark_labels` used to flag the identifier after
        // any `:` even inside a `{ … }` map literal, so the variable
        // reference in `{email: row.email}` got coloured as a Label
        // (`row` would show as a green label-style word on top of its
        // variable colour).
        let slice = "(p:Person {email: row.email})";
        let mut spans: Vec<(usize, usize)> = Vec::new();
        mark_labels(slice, 0, &mut |s, e| spans.push((s, e)));
        // Only `Person` at offset 3..9 should be marked.
        assert_eq!(spans, vec![(3, 9)]);
    }

    #[test]
    fn mark_labels_skips_colons_inside_strings() {
        let slice = "(n:Person {email: ':NotALabel'})";
        let mut spans: Vec<(usize, usize)> = Vec::new();
        mark_labels(slice, 0, &mut |s, e| spans.push((s, e)));
        // Only `Person` at offset 3..9.
        assert_eq!(spans, vec![(3, 9)]);
    }

    fn assert_has_span(spans: &[HighlightSpan], src: &str, expect_kind: HighlightKind, text: &str) {
        let hit = spans
            .iter()
            .any(|s| s.kind == expect_kind && &src[s.start..s.end] == text);
        assert!(
            hit,
            "expected a {expect_kind:?} span for {text:?}, got: {:?}",
            spans
                .iter()
                .map(|s| (s.kind, &src[s.start..s.end]))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn lexical_fallback_colours_labels_props_params_and_literals() {
        // A doc with three queries separated by blank lines (no `;`) —
        // the AST parse fails on the whole slice, so without the
        // fallback the editor would lose all semantic colours.
        let src = "\
MERGE (n:Person {email: $email})
ON CREATE SET n.name = $name, n.createdAt = timestamp()
RETURN n

UNWIND $rows AS row
MERGE (p:Person {email: row.email})
RETURN count(p) AS created";
        let spans = collect_highlights_lexical(src);
        assert_has_span(&spans, src, HighlightKind::Label, "Person");
        assert_has_span(&spans, src, HighlightKind::PropertyKey, "email");
        assert_has_span(&spans, src, HighlightKind::PropertyKey, "name");
        assert_has_span(&spans, src, HighlightKind::Parameter, "$email");
        assert_has_span(&spans, src, HighlightKind::Parameter, "$name");
        assert_has_span(&spans, src, HighlightKind::Parameter, "$rows");
        assert_has_span(&spans, src, HighlightKind::FunctionName, "timestamp");
        assert_has_span(&spans, src, HighlightKind::FunctionName, "count");
    }

    #[test]
    fn lexical_fallback_colours_rel_types_inside_brackets() {
        let src = "(a)-[r:KNOWS]->(b)";
        let spans = collect_highlights_lexical(src);
        assert_has_span(&spans, src, HighlightKind::RelType, "KNOWS");
        assert_has_span(&spans, src, HighlightKind::Variable, "a");
        assert_has_span(&spans, src, HighlightKind::Variable, "b");
        assert_has_span(&spans, src, HighlightKind::Variable, "r");
    }

    #[test]
    fn lexical_fallback_recognises_string_and_number_literals() {
        let src = "RETURN 'Alice', 42, 3.14, TRUE, NULL";
        let spans = collect_highlights_lexical(src);
        assert_has_span(&spans, src, HighlightKind::StringLiteral, "'Alice'");
        assert_has_span(&spans, src, HighlightKind::NumberLiteral, "42");
        assert_has_span(&spans, src, HighlightKind::NumberLiteral, "3.14");
        assert_has_span(&spans, src, HighlightKind::BoolLiteral, "TRUE");
        assert_has_span(&spans, src, HighlightKind::NullLiteral, "NULL");
    }

    #[test]
    fn lexical_fallback_distinguishes_namespace_call_from_property_access() {
        // `math.abs(x)` — `math` is a namespace, `abs` is a function.
        // `n.name` (no parens after) — `name` is a property key.
        let src = "RETURN math.abs(x), n.name";
        let spans = collect_highlights_lexical(src);
        assert_has_span(&spans, src, HighlightKind::Namespace, "math");
        assert_has_span(&spans, src, HighlightKind::FunctionName, "abs");
        assert_has_span(&spans, src, HighlightKind::PropertyKey, "name");
    }

    // ── property-map expansion ────────────────────────────────────────

    #[test]
    fn prettify_keeps_short_create_property_map_inline() {
        let input = "CREATE (a:Person {name: 'Alice'})";
        let out = prettify(input);
        assert_eq!(out, "CREATE (a:Person {name: 'Alice'})\n");
    }

    #[test]
    fn prettify_keeps_two_key_create_map_inline() {
        // Two keys, short body — stays inline.
        let input = "CREATE (a:Person {name: 'Alice', age: 30})";
        let out = prettify(input);
        assert_eq!(out, "CREATE (a:Person {name: 'Alice', age: 30})\n");
    }

    #[test]
    fn prettify_expands_create_property_map_with_many_keys() {
        let input = "CREATE (n:User {id: 1, name: 'Alice', email: 'a@b.c'})";
        let out = prettify(input);
        assert_eq!(
            out,
            "CREATE (n:User {\n  id: 1,\n  name: 'Alice',\n  email: 'a@b.c'\n})\n"
        );
    }

    #[test]
    fn prettify_expands_merge_property_map() {
        let input = "MERGE (n:User {id: 1, name: 'Alice', email: 'a@b.c'})";
        let out = prettify(input);
        assert_eq!(
            out,
            "MERGE (n:User {\n  id: 1,\n  name: 'Alice',\n  email: 'a@b.c'\n})\n"
        );
    }

    #[test]
    fn prettify_expands_set_map_merge() {
        let input = "MATCH (n) SET n += {a: 1, b: 2, c: 3, d: 4}";
        let out = prettify(input);
        assert!(
            out.contains("SET n += {\n  a: 1,\n  b: 2,\n  c: 3,\n  d: 4\n}"),
            "got: {out:?}"
        );
    }

    #[test]
    fn prettify_expands_relationship_property_map() {
        let input = "CREATE (a)-[r:R {since: 2024, weight: 0.5, source: 'x'}]->(b)";
        let out = prettify(input);
        assert_eq!(
            out,
            "CREATE (a)-[r:R {\n  since: 2024,\n  weight: 0.5,\n  source: 'x'\n}]->(b)\n"
        );
    }

    #[test]
    fn prettify_forces_split_when_map_contains_case() {
        // Two keys would normally stay inline, but a CASE on the RHS
        // forces a multi-line layout for readability.
        let input = "CREATE (n:Foo {id: 1, status: CASE WHEN x > 0 THEN 'A' ELSE 'B' END})";
        let out = prettify(input);
        assert!(out.contains("CREATE (n:Foo {\n"), "got: {out:?}");
        assert!(out.contains("\n  id: 1,\n"), "got: {out:?}");
        assert!(out.contains("\n  status: CASE\n"), "got: {out:?}");
    }

    #[test]
    fn prettify_does_not_split_map_inside_function_arg() {
        // `apoc.foo(...)` is inside a RETURN line; map literal should
        // stay inline — only CREATE/MERGE/SET lines get the split.
        let input = "MATCH (n) RETURN apoc.foo({a: 1, b: 2, c: 3, d: 4})";
        let out = prettify(input);
        assert!(
            out.contains("apoc.foo({a: 1, b: 2, c: 3, d: 4})"),
            "got: {out:?}"
        );
    }

    // ── multi-line CASE collapse + re-split ───────────────────────────

    #[test]
    fn prettify_splits_hand_wrapped_multiline_case() {
        // CASE / END span multiple lines in the input. Today's pipeline
        // collapses them via `collapse_multiline_blocks` then re-splits.
        let input = "WITH x, CASE\n  WHEN x > 0 THEN 'pos'\n  ELSE 'neg'\nEND AS sign\nRETURN sign";
        let out = prettify(input);
        assert!(out.contains("CASE\n"), "got: {out:?}");
        assert!(out.contains("WHEN x > 0 THEN 'pos'"), "got: {out:?}");
        assert!(out.contains("ELSE 'neg'"), "got: {out:?}");
        assert!(out.contains("END AS sign"), "got: {out:?}");
        // Idempotent.
        let twice = prettify(&out);
        assert_eq!(out, twice, "format(format(x)) should equal format(x)");
    }

    #[test]
    fn prettify_splits_back_to_back_when_on_same_line() {
        // Two WHEN keywords on the same line should each get their own
        // line after prettify — already handled by `split_case_body`,
        // but worth a regression test now that more inputs reach it.
        let input = "RETURN CASE WHEN x = 1 THEN 'a' WHEN x = 2 THEN 'b' ELSE 'c' END";
        let out = prettify(input);
        assert!(out.contains("CASE\n"), "got: {out:?}");
        assert!(out.contains("\n  WHEN x = 1 THEN 'a'\n"), "got: {out:?}");
        assert!(out.contains("\n  WHEN x = 2 THEN 'b'\n"), "got: {out:?}");
        assert!(out.contains("\n  ELSE 'c'\n"), "got: {out:?}");
    }

    // ── user-reported example ─────────────────────────────────────────

    #[test]
    fn prettify_create_with_multiline_map_and_case() {
        let input = concat!(
            "WITH range(1, 1000) AS ids\n",
            "UNWIND ids AS id\n",
            "\n",
            "CREATE (n:TestRecord { id: id,\n",
            "      name: 'Record ' + toString(id),\n",
            "      createdAt: datetime(),\n",
            "      randomValue: rand(),\n",
            "      status: CASE\n",
            "          WHEN id % 3 = 0 THEN 'ACTIVE' WHEN id % 3 = 1 THEN 'PENDING'\n",
            "          ELSE 'ARCHIVED'\n",
            "      END\n",
            "  })\n",
            "\n",
            "RETURN n",
        );
        let out = prettify(input);
        let expected = concat!(
            "WITH range(1, 1000) AS ids\n",
            "UNWIND ids AS id\n",
            "\n",
            "CREATE (n:TestRecord {\n",
            "  id: id,\n",
            "  name: 'Record ' + toString(id),\n",
            "  createdAt: datetime(),\n",
            "  randomValue: rand(),\n",
            "  status: CASE\n",
            "    WHEN id % 3 = 0 THEN 'ACTIVE'\n",
            "    WHEN id % 3 = 1 THEN 'PENDING'\n",
            "    ELSE 'ARCHIVED'\n",
            "  END\n",
            "})\n",
            "\n",
            "RETURN n\n",
        );
        assert_eq!(out, expected);
        // Idempotent.
        let twice = prettify(&out);
        assert_eq!(out, twice, "format(format(x)) should equal format(x)");
    }

    // ── builtin registry surface ──────────────────────────────────────

    #[test]
    fn builtins_table_covers_every_namespace() {
        // Spot-check: every namespace the editor advertises should
        // actually have at least one spec exposed. If a namespace
        // appears in `data.ts` but has zero members here, autocomplete
        // for `<ns>.|` will return nothing.
        let names: std::collections::HashSet<&'static str> = lora_builtins_meta::BUILTIN_SPECS
            .iter()
            .map(|s| s.name)
            .collect();
        for ns in [
            "math.", "string.", "list.", "map.", "temporal.", "bytes.", "crypto.", "uuid.",
            "json.", "geo.", "vector.", "node.", "edge.", "path.", "value.", "type.", "cast.",
            "text.", "number.", "bits.",
        ] {
            assert!(
                names.iter().any(|n| n.starts_with(ns)),
                "no `{ns}` builtins exposed — `data.ts` would list an empty namespace",
            );
        }
    }

    #[test]
    fn builtins_table_resolves_temporal_aliases() {
        // The `date("2026-11-01")` family routes through the alias
        // table back to `temporal.now`. Regression test for the alias
        // additions in 0.11.x.
        for alias in ["date", "time", "localdatetime", "localtime", "duration"] {
            let canonical = lora_builtins_meta::canonical_builtin_name(alias);
            assert_eq!(
                canonical,
                Some("temporal.now"),
                "`{alias}` should alias to `temporal.now`",
            );
        }
    }

    // ── unknown-function diagnostic ───────────────────────────────────

    fn unknown_function_diags(source: &str) -> Vec<Diagnostic> {
        let doc = lora_parser::parse_query(source).expect("parses");
        let mut diags = Vec::new();
        check_unknown_functions(&doc, source, &mut diags);
        diags
    }

    fn diag_messages(diags: &[Diagnostic]) -> String {
        diags
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn unknown_function_flags_typo() {
        let diags = unknown_function_diags("RETURN strng.uppr('x')");
        assert_eq!(diags.len(), 1, "messages: {}", diag_messages(&diags));
        assert!(
            diags[0].message.contains("strng.uppr"),
            "got: {}",
            diags[0].message,
        );
    }

    #[test]
    fn unknown_function_allows_canonical_namespaced_call() {
        let diags = unknown_function_diags("RETURN string.upper('alice')");
        assert!(diags.is_empty(), "messages: {}", diag_messages(&diags));
    }

    #[test]
    fn unknown_function_allows_aggregate() {
        let diags = unknown_function_diags("MATCH (n) RETURN count(n)");
        assert!(diags.is_empty(), "messages: {}", diag_messages(&diags));
    }

    #[test]
    fn unknown_function_allows_date_alias() {
        // Regression: the `date("2026-11-01")` alias must round-trip
        // through resolve_function without firing the warning.
        let diags = unknown_function_diags("RETURN date('2026-11-01')");
        assert!(diags.is_empty(), "messages: {}", diag_messages(&diags));
    }
}
