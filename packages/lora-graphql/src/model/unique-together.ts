// Which `@uniqueTogether` types a hand-written statement may write. The
// write-set of a `@cypher` mutation is unknown, so this reads it off the
// text of its write clauses (`CREATE`, `MERGE`, `SET`, `REMOVE`,
// `DELETE`), generously: a type is touched when a write clause names one
// of its labels, the relationship type of a constrained relationship, or
// a constrained (or `where`-filtered) property (`.prop`). A `MATCH`, a
// `WHERE` or a pattern comprehension that only reads a type does not
// touch it (G-55).
//
// A clause that writes a variable without naming what it is (`SET v =
// $map`, `SET v += $map`, `DELETE v`) touches the labels and types the
// variable is bound to anywhere in the statement; deleting a node also
// touches the types constrained on a relationship to it. A variable bound
// to nothing the text names (an alias, an `UNWIND`, an untyped pattern)
// makes the whole statement count, as before. A superset, never a guess
// that leaves a write out — short of a statement that builds labels or
// property names dynamically, which LoraDB's Cypher cannot.

import { maskLiterals } from "./cypher-lexer.js";
import type { GraphModel, NodeType } from "./types.js";

const NAME = "[A-Za-z_][A-Za-z0-9_]*|`(?:[^`]|``)+`";
const unquote = (n: string) =>
  n.startsWith("`") ? n.slice(1, -1).replace(/``/g, "`") : n;

const WRITES = new Set([
  "CREATE",
  "MERGE",
  "SET",
  "REMOVE",
  "DELETE",
  "DETACH",
]);
/** Keywords that end a clause at the same bracket depth. */
const CLAUSES = new Set([
  ...WRITES,
  "MATCH",
  "OPTIONAL",
  "WITH",
  "RETURN",
  "UNWIND",
  "CALL",
  "FOREACH",
  "UNION",
  "WHERE",
  "YIELD",
  "LOAD",
  "USE",
  "ORDER",
  "SKIP",
  "LIMIT",
]);

/** The `@uniqueTogether` types `statement` may write. */
export function uniqueTogetherTouched(
  model: GraphModel,
  statement: string,
): NodeType[] {
  const constrained = [...model.nodes.values()].filter(
    (node) => node.uniqueTogether.length > 0,
  );
  if (constrained.length === 0) return [];
  // Strings and comments blanked, quoted names kept, offsets unchanged.
  const mask = maskLiterals(statement);
  let plain = "";
  for (let i = 0; i < mask.length; i++) {
    plain += mask[i] === "_" ? statement[i] : mask[i];
  }
  const names = new Set<string>();
  const props = new Set<string>();
  const extra = new Set<NodeType>();
  for (const { keyword, text } of writeClauses(mask, plain)) {
    for (const n of colonNames(text)) names.add(n);
    for (const p of dotNames(text)) props.add(p);
    const vars =
      keyword === "SET"
        ? assigned(text)
        : keyword === "DELETE"
          ? deleted(text)
          : [];
    for (const v of vars) {
      const bound = boundNames(plain, v);
      // Written, but the text does not say what it is.
      if (bound.length === 0)
        return touchedBy(constrained, colonNames(plain), dotNames(plain));
      for (const l of bound) {
        names.add(l);
        if (keyword !== "DELETE") continue;
        // A deleted node leaves the relationships to it.
        for (const node of constrained) {
          const linked = node.uniqueTogether.some((u) =>
            [...u.singles, ...(u.set ? [u.set] : [])].some((f) =>
              model.nodes.get(f.target)?.labels.includes(l),
            ),
          );
          if (linked) extra.add(node);
        }
      }
    }
  }
  return constrained.filter(
    (node) => extra.has(node) || touchedBy([node], names, props).length > 0,
  );
}

function touchedBy(
  constrained: readonly NodeType[],
  names: ReadonlySet<string>,
  props: ReadonlySet<string>,
): NodeType[] {
  return constrained.filter(
    (node) =>
      node.labels.some((l) => names.has(l)) ||
      node.uniqueTogether.some((u) => {
        const filtered = whereFields(node, u.where);
        return (
          u.scalars.some((f) => props.has(f.property)) ||
          [...u.singles, ...(u.set ? [u.set] : [])].some((f) =>
            names.has(f.type),
          ) ||
          filtered.props.some((p) => props.has(p)) ||
          filtered.types.some((t) => names.has(t))
        );
      }),
  );
}

/** The properties and relationship types a `where:` filter reads. */
function whereFields(
  node: NodeType,
  where: unknown,
): { props: string[]; types: string[] } {
  const out = { props: [] as string[], types: [] as string[] };
  const walk = (value: unknown) => {
    if (Array.isArray(value)) value.forEach(walk);
    else if (value && typeof value === "object") {
      for (const [key, inner] of Object.entries(value)) {
        const f = node.fields.get(key);
        if (f?.kind === "scalar") out.props.push(f.property);
        else if (f?.kind === "relationship") out.types.push(f.type);
        walk(inner);
      }
    }
  };
  walk(where);
  return out;
}

/**
 * The write clauses of a statement, each from its keyword to the next
 * clause keyword at the same bracket depth, or the bracket that closes
 * around it (`FOREACH (… | …)`, `CALL { … }`). `ON CREATE SET` reads as
 * write clauses of its own, which is what it is.
 */
function writeClauses(
  mask: string,
  plain: string,
): { keyword: string; text: string }[] {
  const out: { keyword: string; text: string }[] = [];
  const open: { keyword: string; start: number; depth: number }[] = [];
  const close = (end: number, depth: number) => {
    while (open.length > 0 && open[open.length - 1]!.depth >= depth) {
      const s = open.pop()!;
      out.push({ keyword: s.keyword, text: plain.slice(s.start, end) });
    }
  };
  const word = /[A-Za-z0-9_]+/y;
  let depth = 0;
  let before = " ";
  for (let i = 0; i < mask.length; ) {
    const c = mask[i]!;
    if (c === "`") {
      const end = mask.indexOf("`", i + 1);
      i = end < 0 ? mask.length : end + 1;
      before = "`";
    } else if (c === "(" || c === "[" || c === "{") {
      depth++;
      i++;
      before = c;
    } else if (c === ")" || c === "]" || c === "}") {
      close(i, depth);
      depth--;
      i++;
      before = c;
    } else if (/\s/.test(c)) {
      i++;
    } else {
      word.lastIndex = i;
      const m = word.exec(mask);
      if (!m) {
        i++;
        before = c;
        continue;
      }
      const upper = m[0].toUpperCase();
      const end = i + m[0].length;
      if (
        CLAUSES.has(upper) &&
        // Not a property, label, parameter or map key.
        !".:$".includes(before) &&
        !/^\s*:/.test(mask.slice(end, end + 64))
      ) {
        close(i, depth);
        if (WRITES.has(upper)) open.push({ keyword: upper, start: i, depth });
      }
      i = end;
      before = "a";
    }
  }
  close(mask.length, -Infinity);
  return out;
}

function colonNames(text: string): Set<string> {
  return new Set(
    [...text.matchAll(new RegExp(`:\\s*(${NAME})`, "g"))].map((m) =>
      unquote(m[1]!),
    ),
  );
}

function dotNames(text: string): Set<string> {
  return new Set(
    [...text.matchAll(new RegExp(`\\.\\s*(${NAME})`, "g"))].map((m) =>
      unquote(m[1]!),
    ),
  );
}

/** Variables a `SET` replaces or merges a map into (`v = …`, `v += …`). */
function assigned(text: string): string[] {
  return [
    ...text.matchAll(
      new RegExp(`(?<![.\\w\`$])(${NAME})\\s*\\+?=(?![=~])`, "g"),
    ),
  ].map((m) => m[1]!);
}

const NOT_VARIABLES = new Set([
  "DELETE",
  "NULL",
  "TRUE",
  "FALSE",
  "IN",
  "IS",
  "NOT",
  "AND",
  "OR",
  "XOR",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
]);

/** Variables a `DELETE` names: identifiers that are not functions. */
function deleted(text: string): string[] {
  return [
    ...text.matchAll(new RegExp(`(?<![.:\\w\`$])(${NAME})(?!\\s*[(.])`, "g")),
  ]
    .map((m) => m[1]!)
    .filter((v) => !NOT_VARIABLES.has(v.toUpperCase()));
}

/** The labels and relationship types `v` is bound to anywhere in `plain`. */
function boundNames(plain: string, v: string): string[] {
  const escaped = v.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const at = new RegExp(
    `[(\\[]\\s*${escaped}\\s*(:\\s*(?:${NAME})\\s*(?:[:&|]\\s*:?\\s*(?:${NAME})\\s*)*)`,
    "g",
  );
  return [...plain.matchAll(at)].flatMap((m) =>
    [...m[1]!.matchAll(new RegExp(NAME, "g"))].map((n) => unquote(n[0])),
  );
}
