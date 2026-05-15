import { StreamLanguage, type StreamParser } from "@codemirror/language";

// Every word the editor recognises as a Cypher keyword. Adding to this
// set is the easiest way to surface a new clause / modifier in the
// synchronous highlighter; the WASM parser is authoritative for
// validity. Keep the entries upper-case — the lexer uppercases each
// matched identifier before looking it up.
const KEYWORDS = new Set([
  // Clauses
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
  "FOREACH",
  "ORDER",
  "BY",
  "ASC",
  "ASCENDING",
  "DESC",
  "DESCENDING",
  "LIMIT",
  "SKIP",
  "USING",
  "UNION",
  "ALL",
  "LOAD",
  "CSV",
  "FROM",
  "FIELDTERMINATOR",
  "PROFILE",
  "EXPLAIN",
  // Sub-query / call
  "CALL",
  "YIELD",
  // Flow keywords
  "AS",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
  "DISTINCT",
  "EXISTS",
  // Schema DDL
  "CONSTRAINT",
  "INDEX",
  "TEXT",
  "FULLTEXT",
  "POINT",
  "RANGE",
  "LOOKUP",
  "SHOW",
  "DROP",
  "UNIQUE",
  "FOR",
  "ON",
  "REQUIRE",
  "ASSERT",
  // MERGE actions
  "MATCH",
  // Boolean / comparison operators expressed as words
  "AND",
  "OR",
  "XOR",
  "NOT",
  "IN",
  "IS",
  "STARTS",
  "ENDS",
  "CONTAINS",
]);

// Atoms are technically keywords too but get a distinct CSS class
// (cm-lora-bool / cm-lora-null) so themes can lean on them.
const ATOMS = new Set(["TRUE", "FALSE", "NULL"]);

interface State {
  inString: "'" | '"' | null;
}

const parser: StreamParser<State> = {
  startState: () => ({ inString: null }),
  token(stream, state) {
    if (state.inString) {
      while (!stream.eol()) {
        const ch = stream.next();
        if (ch === "\\") {
          stream.next();
        } else if (ch === state.inString) {
          state.inString = null;
          return "string";
        }
      }
      return "string";
    }

    if (stream.eatSpace()) return null;

    const ch = stream.peek();
    if (ch === "'" || ch === '"') {
      state.inString = stream.next() as "'" | '"';
      return "string";
    }

    if (stream.match(/\/\/.*/)) return "comment";
    if (stream.match(/\/\*[\s\S]*?\*\//)) return "comment";

    // `$param` references — single-pass; the `$` itself is
    // operator-typed but the trailing identifier is the parameter.
    if (stream.match(/\$[A-Za-z_][A-Za-z0-9_]*/)) {
      return "atom";
    }

    if (stream.match(/-?\d+(\.\d+)?([eE][-+]?\d+)?/)) return "number";

    if (stream.match(/[A-Za-z_][A-Za-z0-9_]*/)) {
      const word = stream.current() ?? "";
      const upper = word.toUpperCase();
      if (ATOMS.has(upper)) return "atom";
      if (KEYWORDS.has(upper)) return "keyword";
      // Function call: identifier immediately followed by `(` (after
      // optional whitespace). The StreamLanguage runs on every
      // keystroke so this is what gives `count(`, `timestamp(`,
      // `date(` their colour even when the AST parse is still
      // in-flight or has failed.
      const rest = stream.string.slice(stream.pos);
      if (/^\s*\(/.test(rest)) return "function";
      return "variableName";
    }

    if (stream.match(/[-+*/%=<>!]+/)) return "operator";
    if (stream.match(/[()\[\]{},.;:|]/)) return "punctuation";

    stream.next();
    return null;
  },
};

/**
 * Minimal CodeMirror language for Cypher highlighting. The WASM parser is
 * authoritative for validity — this only colours the editor while the
 * user types.
 */
export const loraQueryLanguage = StreamLanguage.define(parser);
