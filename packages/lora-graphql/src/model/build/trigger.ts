// Reading `@trigger` off a node type (repeatable), checked against the
// type: the operations it mutates, the fields it names, the parameters
// its statement may use.

import type { GraphQLDirective, GraphQLObjectType } from "graphql";
import type { ModelProblem } from "../../errors.js";
import { codeOnly, scanParams } from "../cypher-lexer.js";
import type { MutationOperation, NodeType, Trigger } from "../types.js";
import { directive, directiveNodes } from "./directive-args.js";

const WRITE_CLAUSES = /\b(CREATE|MERGE|SET|DELETE|REMOVE|DETACH)\b/i;

/** What a trigger statement may read as a `$parameter`. */
export const TRIGGER_PARAMS: readonly string[] = ["jwt", "viewer", "operation"];

/** Every `@trigger` of a node type, in the order written, checked. */
export function readTriggers(
  t: GraphQLObjectType,
  node: NodeType,
  d: (name: string) => GraphQLDirective,
  problems: ModelProblem[],
  /** Whether the @jwt type has a @viewer claim (enables `$viewer`). */
  viewer: boolean,
): Trigger[] {
  const out: Trigger[] = [];
  for (const { node: dir } of directiveNodes("trigger", t)) {
    let ok = true;
    const at = (message: string) => {
      ok = false;
      problems.push({ type: t.name, message: `@trigger: ${message}` });
    };
    const args = directive(d("trigger"), { astNode: { directives: [dir] } }, at);
    if (!args) continue;
    const operations = new Set(args["on"] as MutationOperation[]);
    const statement = (args["statement"] as string).trim();
    const fields = (args["fields"] ?? undefined) as string[] | undefined;
    if (operations.size === 0) at("on is empty");
    if (!statement) at("statement is empty");
    // A trigger fires for what generated mutations write.
    for (const op of operations) {
      if (!node.mutations.has(op)) {
        at(
          `on lists ${op}, but ${t.name} has no generated ${op} mutation (@mutation(operations:)) to fire it`,
        );
      }
    }
    if (fields !== undefined) {
      if (!operations.has("UPDATE")) {
        at("fields narrows UPDATE, which on does not list");
      }
      if (fields.length === 0) at("fields is empty");
      for (const name of fields) {
        const f = node.fields.get(name);
        if (!f) at(`${t.name} has no field ${name}`);
        else if (f.kind === "cypher" || f.kind === "custom") {
          at(`${name} is computed, not written: an update never sets it`);
        }
      }
    }
    const params = [...new Set(scanParams(statement).map((p) => p.name))];
    for (const p of params) {
      if (!TRIGGER_PARAMS.includes(p)) {
        at(
          `the statement uses $${p}; a trigger statement has $jwt, $viewer and $operation`,
        );
      } else if (p === "viewer" && !viewer) {
        at("the statement uses $viewer, but no @jwt claim has @viewer");
      }
    }
    if (!ok) continue;
    out.push({
      operations,
      statement,
      fields,
      params,
      writes: WRITE_CLAUSES.test(codeOnly(statement)),
    });
  }
  return out;
}
