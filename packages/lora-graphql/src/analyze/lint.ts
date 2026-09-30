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

/**
 * `@mutation` types with a generated write no rule guards: any caller
 * that reaches the API can make it. An error in `check()`, unless the type
 * declares the operation open with `@authorization(public: [...])`. A
 * write is guarded by `@authentication` for it, a filter or validate rule
 * for it (a `@authorizationDefaults(mutations:)` default included), or a
 * schema bypass alone does not count: it only lets some callers skip rules.
 */
export function unguardedMutations(model: GraphModel): ModelWarning[] {
  const out: ModelWarning[] = [];
  for (const node of model.nodes.values()) {
    const rules = [
      ...(node.authorization?.filter ?? []),
      ...(node.authorization?.validate ?? []),
    ];
    const open = [...node.mutations].filter(
      (op) =>
        !node.authentication?.has(op) &&
        !rules.some((r) => r.operations.has(op)) &&
        !node.authorization?.public?.has(op),
    );
    if (open.length > 0) {
      out.push({
        type: node.name,
        message: `${open.join(", ")} ${open.length === 1 ? "has" : "have"} no @authentication or @authorization rule: any caller that reaches the API can make ${open.length === 1 ? "it" : "them"}. Guard ${open.length === 1 ? "it" : "them"}, or declare @authorization(public: [${open.join(", ")}])`,
      });
    }
  }
  return out;
}
