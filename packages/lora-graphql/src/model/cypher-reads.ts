// What a read-only `@cypher` statement can read, told from its text: the
// labels and relationship types its patterns name. A response cache keyed
// on read-sets then keeps a page whose `@cypher` field reads `:ImageAsset`
// across a write to `:Message`. When the text does not say (a procedure
// call, a node pattern with no label on a variable nothing bound, a
// relationship with no type), the statement stays opaque: every write may
// have changed it.
//
// The scan follows the engine's grammar (`lora-parser/src/cypher.pest`):
// node patterns are `(var? (:A|B)* props?)`, relationship details
// `[var? (:R|S)? range? props?]`, and the only `CALL` a read may hold
// besides procedures is `CALL { }`. It errs one way only: a construct it
// does not recognise makes the statement opaque, never reads less.

import { maskLiterals } from "./cypher-lexer.js";

/** Labels and relationship types a statement reads; undefined: opaque. */
export interface StatementReads {
  labels: string[];
  relationships: string[];
}

const NAME = "(?:[\\p{L}_][\\p{L}\\p{N}_]*|`_*`)";
const LABELS = String.raw`(?::\s*${NAME}\s*(?:\|\s*:?\s*${NAME}\s*)*)+`;
const NODE = new RegExp(
  String.raw`^\(\s*(${NAME})?\s*(${LABELS})?\s*(\{|\$|\))`,
  "u",
);
const REL = new RegExp(
  String.raw`^\[\s*(${NAME})?\s*(${LABELS})?\s*(\*\s*\d*\s*(?:\.\.\s*\d*)?)?\s*(\{|\$|\])`,
  "u",
);
const NAMES = new RegExp(NAME, "gu");
const WORD = /[\p{L}\p{N}_]/u;

/**
 * Words that may stand before a pattern's `(`. Any other word before a
 * `(` is a function name, and the parenthesis its arguments. Listing a
 * word here that is not one only checks more parentheses.
 */
const KEYWORDS = new Set(
  (
    "MATCH OPTIONAL MERGE CREATE WHERE AND OR XOR NOT RETURN WITH UNWIND " +
    "WHEN THEN ELSE CASE END DISTINCT IN AS IS NULL ON SET DELETE DETACH " +
    "REMOVE UNION ALL ORDER BY SKIP LIMIT ASC DESC YIELD CALL EXISTS COUNT " +
    "STARTS ENDS CONTAINS FOREACH SHORTESTPATH ALLSHORTESTPATHS"
  ).split(" "),
);

/** Clauses that end a WITH's projection items. */
const AFTER_WITH =
  /\b(WHERE|ORDER|SKIP|LIMIT|MATCH|OPTIONAL|UNWIND|WITH|RETURN|CALL|MERGE|CREATE|UNION|SET|DELETE|DETACH|REMOVE|FOREACH)\b/giu;

/**
 * The labels and relationship types `statement` reads, or undefined when
 * they cannot be told. `bound` are the variables in scope when it runs
 * (`this` for a field of a type).
 */
export function statementReads(
  statement: string,
  bound: readonly string[],
): StatementReads | undefined {
  const m = maskLiterals(statement);
  const name = (start: number, text: string) => {
    const raw = statement.slice(start, start + text.length);
    return raw.startsWith("`") ? raw.slice(1, -1) : raw;
  };
  if (/\bCALL\b(?!\s*\{)/iu.test(m)) return undefined;
  // `--`, `-->`, `<--`: a relationship of any type.
  if (/--/.test(m)) return undefined;

  const labels = new Set<string>();
  const relationships = new Set<string>();
  // Variables known to hold nodes whose labels are already counted, per
  // `{ }` scope. A `CALL { }` body sees only what it imports.
  const scopes: Set<string>[] = [new Set(bound)];
  const top = () => scopes[scopes.length - 1]!;
  // A WITH's projection replaces the scope where its items end.
  const resets: Array<{ at: number; depth: number; names: Set<string> }> = [];
  const isWord = (c: string | undefined) => c !== undefined && WORD.test(c);
  const wordBefore = (i: number): string | undefined => {
    let j = i - 1;
    while (j >= 0 && /\s/.test(m[j]!)) j--;
    if (j < 0 || !isWord(m[j])) return undefined;
    let k = j;
    while (k > 0 && isWord(m[k - 1])) k--;
    return m.slice(k, j + 1);
  };
  const charBefore = (i: number): string | undefined => {
    let j = i - 1;
    while (j >= 0 && /\s/.test(m[j]!)) j--;
    return j < 0 ? undefined : m[j];
  };
  const addNames = (text: string, offset: number, into: Set<string>) => {
    for (const n of text.matchAll(NAMES)) {
      into.add(name(offset + n.index!, n[0]));
    }
  };

  for (let i = 0; i < m.length; i++) {
    const c = m[i]!;
    while (resets.length > 0 && resets[0]!.at <= i) {
      const r = resets.shift()!;
      if (r.depth === scopes.length) scopes[scopes.length - 1] = r.names;
    }
    if (c === "{") {
      const before = wordBefore(i);
      if (before?.toUpperCase() === "CALL") {
        // `CALL { WITH a, b ... }` imports a and b; any other body none.
        const leadingWith = /^\{\s*WITH\b/iu.test(m.slice(i));
        scopes.push(leadingWith ? new Set(top()) : new Set());
      } else {
        scopes.push(new Set(top()));
      }
      continue;
    }
    if (c === "}") {
      if (scopes.length > 1) scopes.pop();
      continue;
    }
    if (isWord(c) && !isWord(m[i - 1])) {
      // A property key (`this.with`), label (`:Union`) or parameter.
      const lead = /[.:$]/.test(charBefore(i) ?? "");
      let j = i;
      while (isWord(m[j])) j++;
      const word = m.slice(i, j).toUpperCase();
      if (lead) {
        // Not a keyword.
      } else if (word === "UNION" && scopes.length === 1) {
        // Each branch imports for itself: assume it imported nothing.
        scopes[0] = new Set();
      } else if (word === "WITH") {
        const names = withProjection(m, j, top(), (s, t, into) =>
          addNames(t, s, into),
        );
        resets.push({ at: names.end, depth: scopes.length, names: names.set });
      } else if (word === "AS") {
        // `UNWIND list AS x`, `RETURN ... AS x`: values made of what is
        // already counted.
        // `CAST(x AS INTEGER)` names a type, not a variable.
        const rest = new RegExp(String.raw`^\s*(${NAME})(?!\s*[)<])`, "u").exec(
          m.slice(j),
        );
        if (rest) addNames(rest[1]!, j + rest[0].indexOf(rest[1]!), top());
      }
      i = j - 1;
      continue;
    }
    if (c === "(") {
      const node = NODE.exec(m.slice(i));
      if (!node) continue;
      const [, variable, labelText] = node;
      const before = wordBefore(i);
      const call = before !== undefined && !KEYWORDS.has(before.toUpperCase());
      if (labelText) {
        const start = i + node[0].indexOf(labelText, variable?.length ?? 0);
        addNames(labelText, start, labels);
        if (variable) top().add(name(i + node[0].indexOf(variable), variable));
        continue;
      }
      // `count(x)`, `timestamp()`: a function's arguments.
      if (call) continue;
      if (!variable) return undefined;
      const v = name(i + node[0].indexOf(variable), variable);
      if (!top().has(v)) return undefined;
      continue;
    }
    if (c === "[" && (charBefore(i) === "-" || charBefore(i) === "<")) {
      const rel = REL.exec(m.slice(i));
      if (!rel || !rel[2]) return undefined;
      const start = i + rel[0].indexOf(rel[2], rel[1]?.length ?? 0);
      addNames(rel[2], start, relationships);
      continue;
    }
  }
  return {
    labels: [...labels].sort(),
    relationships: [...relationships].sort(),
  };
}

/**
 * The variables a `WITH` starting at `from` projects (aliases, bare
 * variables already in scope, all of them for `*`) and where its items
 * end.
 */
function withProjection(
  m: string,
  from: number,
  scope: Set<string>,
  addNames: (start: number, text: string, into: Set<string>) => void,
): { set: Set<string>; end: number } {
  let depth = 0;
  let end = m.length;
  AFTER_WITH.lastIndex = from;
  const stops = new Set<number>();
  for (let match; (match = AFTER_WITH.exec(m)); ) stops.add(match.index);
  const items: Array<[number, number]> = [];
  let itemStart = from;
  for (let i = from; i < m.length; i++) {
    const c = m[i]!;
    if (c === "(" || c === "[" || c === "{") depth++;
    else if (c === ")" || c === "]" || c === "}") {
      if (depth === 0) {
        end = i;
        break;
      }
      depth--;
    } else if (depth === 0 && c === ",") {
      items.push([itemStart, i]);
      itemStart = i + 1;
    } else if (
      depth === 0 &&
      stops.has(i) &&
      !/[\p{L}\p{N}_]/u.test(m[i - 1] ?? "")
    ) {
      end = i;
      break;
    }
  }
  items.push([itemStart, end]);
  const set = new Set<string>();
  for (const [s, e] of items) {
    const text = m.slice(s, e).replace(/^\s*DISTINCT\b/iu, "");
    const offset = e - text.length;
    if (text.trim() === "*") {
      for (const v of scope) set.add(v);
      continue;
    }
    const alias = new RegExp(String.raw`\bAS\s+(${NAME})\s*$`, "iu").exec(text);
    if (alias) {
      addNames(
        offset + alias.index + alias[0].lastIndexOf(alias[1]!),
        alias[1]!,
        set,
      );
      continue;
    }
    const bare = new RegExp(String.raw`^\s*(${NAME})\s*$`, "u").exec(text);
    if (bare) {
      const only = new Set<string>();
      addNames(offset + bare[0].indexOf(bare[1]!), bare[1]!, only);
      for (const v of only) if (scope.has(v)) set.add(v);
    }
  }
  return { set, end };
}
