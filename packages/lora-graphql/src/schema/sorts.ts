// The `sort` inputs: one per @node type and per interface with @sortable
// fields, by type name.

import {
  GraphQLInputObjectType,
  type GraphQLInputFieldConfigMap,
} from "graphql";
import type { GraphModel } from "../model/types.js";
import type { BaseTypes } from "./base.js";
import { names } from "./names.js";

export function buildSorts(model: GraphModel, base: BaseTypes) {
  const { sortDirection } = base;

  // --- Sorts -----------------------------------------------------------------

  const sorts = new Map<string, GraphQLInputObjectType>();
  for (const node of model.nodes.values()) {
    const fields: GraphQLInputFieldConfigMap = {};
    for (const f of node.fields.values()) {
      if (f.kind === "scalar" && (f.key || f.sortable)) {
        fields[f.name] = { type: sortDirection };
      } else if (f.kind === "cypher" && f.computed?.sortable) {
        fields[f.name] = {
          type: sortDirection,
          description: "Computed by @cypher per node: root fields only.",
        };
      }
    }
    sorts.set(
      node.name,
      new GraphQLInputObjectType({
        name: names.sort(node.name),
        description: `Sorts ${node.name} nodes. One field per item; the @key breaks ties.`,
        fields,
      }),
    );
  }
  for (const abstract of model.abstracts.values()) {
    const sortable = [...abstract.fields.values()].filter((f) => f.sortable);
    if (abstract.kind !== "interface" || sortable.length === 0) continue;
    sorts.set(
      abstract.name,
      new GraphQLInputObjectType({
        name: names.sort(abstract.name),
        description: `Sorts ${abstract.name} nodes of every implementation. Ties break by type name, then key.`,
        fields: Object.fromEntries(
          sortable.map((f) => [f.name, { type: sortDirection }]),
        ),
      }),
    );
  }

  return sorts;
}

export type Sorts = ReturnType<typeof buildSorts>;
