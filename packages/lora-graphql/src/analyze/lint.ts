// Schema lint for `check`: choices that are valid but costly or risky.
// Warnings, not failures: each may be intended.

import type { GraphModel, ModelWarning } from "../model/types.js";

export interface LintOptions {
  /** Statistics are loaded: relationship degrees are known. */
  statistics?: boolean;
}

export function lintModel(
  model: GraphModel,
  options: LintOptions = {},
): ModelWarning[] {
  const out: ModelWarning[] = [];
  for (const node of model.nodes.values()) {
    const writes = node.mutations.size > 0;
    const guarded =
      node.authentication !== undefined ||
      (node.authorization?.filter.length ?? 0) > 0 ||
      (node.authorization?.validate.length ?? 0) > 0;
    if (writes && !guarded) {
      out.push({
        type: node.name,
        message: `has mutations (${[...node.mutations].join(", ")}) but no @authentication or @authorization: any caller that reaches the API can write it`,
      });
    }
    for (const f of node.fields.values()) {
      if (f.kind === "scalar") {
        if (f.filters.has("CASE_INSENSITIVE")) {
          out.push({
            type: node.name,
            field: f.name,
            message:
              "CASE_INSENSITIVE compares lowercased values and cannot use an index: every such filter scans the label",
          });
        }
        if (f.filters.has("IS_NULL")) {
          out.push({
            type: node.name,
            field: f.name,
            message:
              "IS_NULL cannot use an index: a filter on absence scans the label",
          });
        }
      }
      if (
        f.kind === "relationship" &&
        f.list &&
        f.cardinality === undefined &&
        !options.statistics
      ) {
        out.push({
          type: node.name,
          field: f.name,
          message:
            "no @cardinality and no statistics: cost estimates assume each parent has a full page of these; run analyze() or declare @cardinality(max:)",
        });
      }
    }
  }
  return out;
}
