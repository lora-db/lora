// Reading `@uniqueTogether` off a node type (repeatable), checked against
// its fields. Which types a hand-written statement may write is in
// `../unique-together.ts`.

import type { GraphQLDirective, GraphQLObjectType } from "graphql";
import type { ModelProblem } from "../../errors.js";
import type {
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  UniqueTogether,
} from "../types.js";
import { directive, directiveNodes } from "./directive-args.js";
import { checkNodeWhere } from "./rule-checks.js";

/** Every `@uniqueTogether` of a node type (repeatable), checked. */
export function readUniqueTogether(
  t: GraphQLObjectType,
  node: NodeType,
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  d: (name: string) => GraphQLDirective,
  problems: ModelProblem[],
): UniqueTogether[] {
  const out: UniqueTogether[] = [];
  for (const { node: dir } of directiveNodes("uniqueTogether", t)) {
    let ok = true;
    const at = (message: string) => {
      ok = false;
      problems.push({ type: t.name, message: `@uniqueTogether: ${message}` });
    };
    const args = directive(
      d("uniqueTogether"),
      { astNode: { directives: [dir] } },
      at,
    );
    if (!args) continue;
    const names = args["fields"] as string[];
    const where = (args["where"] ?? undefined) as
      | Record<string, unknown>
      | undefined;
    if (names.length === 0) at("fields is empty");
    if (new Set(names).size < names.length) at("fields names a field twice");
    const scalars: ScalarField[] = [];
    const singles: RelationshipField[] = [];
    const sets: RelationshipField[] = [];
    for (const n of names) {
      const f = node.fields.get(n);
      if (!f) at(`${t.name} has no field ${n}`);
      else if (f.kind === "cypher" || f.kind === "custom") {
        at(
          `${n} is ${f.kind === "cypher" ? "a @cypher" : "a @customResolver"} field; only stored fields and relationships can be unique`,
        );
      } else if (f.kind === "scalar") {
        if (f.list) at(`${n} is a list; list scalar fields cannot be unique`);
        else scalars.push(f);
      } else if (!nodes.has(f.target)) {
        at(
          `${n} reaches an interface or union; only relationships to a @node type can be unique`,
        );
      } else if (f.list) sets.push(f);
      else singles.push(f);
    }
    if (sets.length > 1) {
      at(
        `fields names ${sets.length} list relationships (${sets.map((f) => f.name).join(", ")}); at most one is compared as a set`,
      );
    }
    if (where !== undefined) {
      checkNodeWhere(nodes, props, node, where, "where", at);
    }
    if (!ok) continue;
    out.push({ fields: names, scalars, singles, set: sets[0], where });
  }
  return out;
}
