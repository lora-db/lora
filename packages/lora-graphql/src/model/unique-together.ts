// Which `@uniqueTogether` types a hand-written statement may write. The
// write-set of a `@cypher` mutation is unknown, so this reads it off the
// text, generously: a type is touched when the statement names one of its
// labels, the relationship type of a constrained relationship, or a
// constrained scalar's property (`.prop`). A superset, never a guess that
// leaves a write out — short of a statement that builds labels or
// property names dynamically, which LoraDB's Cypher cannot.

import type { GraphModel, NodeType } from "./types.js";

const NAME = "[A-Za-z_][A-Za-z0-9_]*|`(?:[^`]|``)+`";
const unquote = (n: string) =>
  n.startsWith("`") ? n.slice(1, -1).replace(/``/g, "`") : n;

/** The `@uniqueTogether` types `statement` may write. */
export function uniqueTogetherTouched(
  model: GraphModel,
  statement: string,
): NodeType[] {
  const colon = new Set(
    [...statement.matchAll(new RegExp(`:\\s*(${NAME})`, "g"))].map((m) =>
      unquote(m[1]!),
    ),
  );
  const dot = new Set(
    [...statement.matchAll(new RegExp(`\\.\\s*(${NAME})`, "g"))].map((m) =>
      unquote(m[1]!),
    ),
  );
  return [...model.nodes.values()].filter(
    (node) =>
      node.uniqueTogether.length > 0 &&
      (node.labels.some((l) => colon.has(l)) ||
        node.uniqueTogether.some(
          (u) =>
            u.scalars.some((f) => dot.has(f.property)) ||
            [...u.singles, ...(u.set ? [u.set] : [])].some((f) =>
              colon.has(f.type),
            ),
        )),
  );
}
