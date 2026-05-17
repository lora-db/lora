/**
 * Thin, typed facade over the WASM pest parser.
 *
 * The Rust crate exposes `parse`, `validate`, `format`, `highlight`,
 * `outline`, and `analyse` via wasm-bindgen. Each takes a single source
 * string (plus an optional config object for `analyse`) and returns a
 * JS value. We normalise the shapes here so callers do not need to
 * know about the wasm-bindgen serialization edges.
 *
 * The Rust crate is built with `wasm-pack build --target bundler`,
 * which emits a JS shim that imports the `.wasm` file directly. The
 * bundler (Vite + vite-plugin-wasm) instantiates it — there is no
 * `init()` to call, and the wasm-bindgen `#[wasm_bindgen(start)]` hook
 * fires automatically on import.
 */

import {
  parse as wasmParse,
  validate as wasmValidate,
  format as wasmFormat,
  highlight as wasmHighlight,
  outline as wasmOutline,
  analyse as wasmAnalyse,
} from "../wasm/lora_query_wasm.js";

export interface Span {
  start: number;
  end: number;
}

export type DiagnosticSeverity = "error" | "warning" | "info";

export interface ParseError {
  /** Severity of the diagnostic. Syntax errors are `error`. */
  severity: DiagnosticSeverity;
  /** Short human summary suitable for an inline tooltip headline. */
  message: string;
  /** The full pest report — caret + `= expected …` footer. */
  details: string;
  /** 1-based line number of the failure (0 for non-positional diagnostics). */
  line: number;
  /** 1-based column number of the failure (0 for non-positional diagnostics). */
  column: number;
  /** Rule names pest was hoping to see (empty for semantic diagnostics). */
  expected: string[];
  /** Short concrete code snippets valid at the failure site. */
  examples: string[];
  /** Byte offsets into the original source. */
  span: Span;
}

export interface ParseResult {
  ok: boolean;
  /** Loosely-typed AST. Shape mirrors the Rust `lora_ast::Document`. */
  ast: unknown | null;
  errors: ParseError[];
}

export type HighlightKind =
  | "variable"
  | "parameter"
  | "label"
  | "relType"
  | "propertyKey"
  | "functionName"
  | "namespace"
  | "stringLiteral"
  | "numberLiteral"
  | "boolLiteral"
  | "nullLiteral"
  | "keyword";

export interface HighlightSpan {
  start: number;
  end: number;
  kind: HighlightKind;
}

export type VariableKind = "node" | "relationship" | "scalar" | "pattern";

export interface OutlineVariable {
  name: string;
  declStart: number;
  declEnd: number;
  /** First label observed on this binding, if any. */
  label: string | null;
  /** Which kind of binding introduced this variable. */
  kind: VariableKind;
  /**
   * When introduced via `… AS x`, the source variable name (if the
   * projection was a bare variable reference). Lets completion follow
   * aliases when resolving `x.|`.
   */
  aliasOf: string | null;
}

export interface Outline {
  variables: OutlineVariable[];
  parameters: string[];
  labels: string[];
  relTypes: string[];
}

export interface FoldRange {
  start: number;
  end: number;
  kind: string;
}

export interface AnalyseConfig {
  labels?: readonly string[];
  relTypes?: readonly string[];
  /** When true, warn for labels not in the provided list. */
  strictLabels?: boolean;
  /** When true, warn for rel-types not in the provided list. */
  strictRelTypes?: boolean;
}

export interface Analysis {
  /** Semantic diagnostics — warnings/info only. Syntax errors come from {@link validate}. */
  diagnostics: ParseError[];
  /** Suggested fold ranges, sorted by `start`. */
  foldRanges: FoldRange[];
}

/**
 * No-op under the bundler target — the wasm module is instantiated
 * eagerly by the bundler. Kept as a stable async hook so consumers can
 * `await initParser()` without caring which wasm-pack target shipped.
 */
export function initParser(): Promise<void> {
  return Promise.resolve();
}

// ─── Single-flight cache per WASM export ─────────────────────────────
//
// Five subsystems (decoration, scope, folding, linter, host-callbacks
// effect) each watch the same document and each call the WASM parser
// independently. Without coordination a single keystroke triggers the
// Rust pipeline 5–7 times against an identical source.
//
// `singleFlight` wraps an underlying WASM call with:
//   - an in-flight map keyed by source, so concurrent callers for the
//     same source share one promise (the one *currently* asked for);
//   - a tiny last-result cache so a caller arriving right after a
//     completion gets the cached value synchronously (still wrapped in
//     a Promise to preserve the existing async contract).
//
// The cache is intentionally minimal — one entry — because the editor
// only ever asks for the "current document" and stale entries are dead
// weight. When the document changes, the new source replaces the
// previous entry on first access.
function singleFlight<R>(
  fn: (source: string) => R | Promise<R>,
): (source: string) => Promise<R> {
  const inflight = new Map<string, Promise<R>>();
  let cached: { source: string; value: R } | null = null;

  return (source: string): Promise<R> => {
    if (cached && cached.source === source) {
      return Promise.resolve(cached.value);
    }
    const existing = inflight.get(source);
    if (existing) return existing;
    const promise = Promise.resolve()
      .then(() => fn(source))
      .then((value) => {
        cached = { source, value };
        return value;
      })
      .finally(() => {
        inflight.delete(source);
      });
    inflight.set(source, promise);
    return promise;
  };
}

// Same as `singleFlight` but the cache key combines source + a config
// signature so callers can pass distinct configs without polluting each
// other's cache slot.
function singleFlightKeyed<R, C>(
  fn: (source: string, config: C) => R | Promise<R>,
  configKey: (config: C) => string,
): (source: string, config: C) => Promise<R> {
  const inflight = new Map<string, Promise<R>>();
  let cached: { key: string; value: R } | null = null;

  return (source: string, config: C): Promise<R> => {
    const key = `${configKey(config)}\x00${source}`;
    if (cached && cached.key === key) {
      return Promise.resolve(cached.value);
    }
    const existing = inflight.get(key);
    if (existing) return existing;
    const promise = Promise.resolve()
      .then(() => fn(source, config))
      .then((value) => {
        cached = { key, value };
        return value;
      })
      .finally(() => {
        inflight.delete(key);
      });
    inflight.set(key, promise);
    return promise;
  };
}

const cachedParse = singleFlight<ParseResult>(
  (s) => wasmParse(s) as ParseResult,
);
const cachedValidate = singleFlight<ParseError[]>(
  (s) => wasmValidate(s) as ParseError[],
);
const cachedHighlight = singleFlight<HighlightSpan[]>(
  (s) => wasmHighlight(s) as HighlightSpan[],
);
const cachedOutline = singleFlight<Outline>(
  (s) => wasmOutline(s) as Outline,
);
const cachedAnalyse = singleFlightKeyed<Analysis, AnalyseConfig>(
  (s, cfg) => wasmAnalyse(s, cfg) as Analysis,
  (cfg) => {
    // Order-stable signature. `labels`/`relTypes` arrays are converted
    // to a sorted-comma string so a fresh array with the same content
    // hits the same cache slot.
    const labels = cfg.labels ? [...cfg.labels].sort().join(",") : "";
    const rels = cfg.relTypes ? [...cfg.relTypes].sort().join(",") : "";
    const sl = cfg.strictLabels ? "1" : "0";
    const sr = cfg.strictRelTypes ? "1" : "0";
    return `${sl}|${sr}|${labels}|${rels}`;
  },
);

/** Parse a Cypher source string into an AST + diagnostics. */
export function parse(source: string): Promise<ParseResult> {
  return cachedParse(source);
}

/**
 * Validate a Cypher source string. Returns the empty array when the
 * input parses cleanly, otherwise a list of structured diagnostics
 * (severity `"error"`).
 */
export function validate(source: string): Promise<ParseError[]> {
  return cachedValidate(source);
}

/**
 * The Rust prettifier appends a trailing `\n` (Unix file convention).
 * When the formatted text is loaded into CodeMirror that newline shows
 * up as a visually empty last line — exactly the wrong shape for a
 * read-only doc block, which has no cursor to "park" on the blank
 * line. Strip a single trailing newline so the editor's doc length
 * matches the visible line count. Multiple trailing newlines are not
 * something `format` emits, so we don't bother collapsing them.
 */
function stripTrailingNewline(s: string): string {
  return s.endsWith("\n") ? s.slice(0, -1) : s;
}

/**
 * Reformat a Cypher source string. When the input does not parse, the
 * original source is returned unchanged so the editor never destroys
 * partial work. Not cached — format is called interactively (prettify
 * button / shortcut) and re-formatting the same source is harmless.
 */
export async function format(source: string): Promise<string> {
  return stripTrailingNewline(wasmFormat(source));
}

/**
 * Synchronous variant of {@link format}. The underlying WASM call is
 * already synchronous; the async wrapper above exists only to match
 * the contract of the other parser helpers. Use this when you need
 * the formatted text *before* React's first render — for example as
 * a `useState` lazy initializer, so a predefined query mounts
 * already-prettified instead of flashing from raw to formatted.
 *
 * Safe to call at module load — the bundler-target WASM is fully
 * instantiated by the time this module finishes importing.
 */
export function formatSync(source: string): string {
  return stripTrailingNewline(wasmFormat(source));
}

/**
 * Walk the parsed AST and return per-token highlight spans. The editor
 * uses these to drive semantic colouring (variables, labels, function
 * names, ...). Returns an empty array when the source does not parse.
 */
export function highlight(source: string): Promise<HighlightSpan[]> {
  return cachedHighlight(source);
}

/**
 * Lightweight scope summary used by the autocomplete popup: declared
 * variables (with first-binding spans), parameter names, distinct
 * labels, and distinct relationship types observed anywhere in the
 * document. Returns an empty outline when the source does not parse.
 */
export function outline(source: string): Promise<Outline> {
  return cachedOutline(source);
}

// ─── Multi-statement helpers ─────────────────────────────────────────
//
// The WASM layer is now multi-statement-native: `validate`, `analyse`,
// `outline`, `highlight`, `parse`, and `format` all split the input on
// top-level `;` internally and emit results with whole-doc offsets.
// The `*All` exports below remain for backward compatibility and read
// as straight aliases.

/**
 * Validate every top-level statement in a multi-statement script.
 *
 * @deprecated Identical to {@link validate} now that the WASM layer
 * handles multi-statement scripts natively.
 */
export function validateAll(source: string): Promise<ParseError[]> {
  return validate(source);
}

/**
 * Run semantic analysis on every top-level statement.
 *
 * @deprecated Identical to {@link analyse} now that the WASM layer
 * handles multi-statement scripts natively.
 */
export function analyseAll(
  source: string,
  config: AnalyseConfig = {},
): Promise<Analysis> {
  return analyse(source, config);
}

/**
 * Second-pass semantic analysis. Returns warnings (undeclared variable
 * uses, schema mismatches, unused bindings) plus fold ranges. Syntax
 * errors are *not* duplicated here — use {@link validate} for those
 * and merge in the editor.
 */
export function analyse(
  source: string,
  config: AnalyseConfig = {},
): Promise<Analysis> {
  return cachedAnalyse(source, config);
}
