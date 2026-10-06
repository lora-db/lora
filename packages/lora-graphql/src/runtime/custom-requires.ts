// The check of `@customResolver(requires:)` selections against the
// generated schema, made once when the schema is built.

import {
  Kind,
  parse,
  specifiedRules,
  validate,
  type DocumentNode,
  type GraphQLSchema,
  type OperationDefinitionNode,
} from "graphql";
import { ModelError } from "../errors.js";
import type { GraphModel, NodeType } from "../model/types.js";

/**
 * Every `@customResolver(requires:)` is a valid selection on its type and
 * does not require custom fields (they are computed after the read).
 */
export function checkCustomRequires(
  model: GraphModel,
  schema: GraphQLSchema,
): void {
  const problems: Array<{ type: string; field: string; message: string }> = [];
  for (const node of model.nodes.values()) {
    for (const f of node.fields.values()) {
      if (f.kind !== "custom" || !f.requires) continue;
      let doc: DocumentNode;
      try {
        doc = parse(`fragment R on ${node.name} { ${f.requires} }`);
      } catch (err) {
        problems.push({
          type: node.name,
          field: f.name,
          message: `requires: ${(err as Error).message}`,
        });
        continue;
      }
      const errors = validate(
        schema,
        doc,
        specifiedRules.filter((r) => r.name !== "NoUnusedFragmentsRule"),
      );
      for (const e of errors) {
        problems.push({
          type: node.name,
          field: f.name,
          message: `requires: ${e.message}`,
        });
      }
      for (const name of requiredCustomFields(node, f.requires)) {
        problems.push({
          type: node.name,
          field: f.name,
          message: `requires: ${name} is a @customResolver field`,
        });
      }
    }
  }
  if (problems.length > 0) throw new ModelError(problems);
}

function requiredCustomFields(node: NodeType, requires: string): string[] {
  const doc = parse(`{ ${requires} }`);
  const op = doc.definitions[0] as OperationDefinitionNode;
  return op.selectionSet.selections.flatMap((s) =>
    s.kind === Kind.FIELD && node.fields.get(s.name.value)?.kind === "custom"
      ? [s.name.value]
      : [],
  );
}
