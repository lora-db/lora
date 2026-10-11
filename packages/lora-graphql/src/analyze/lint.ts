// Schema lint for `check`: choices that are valid but costly or risky.
// Warnings, not failures: each may be intended.

import { codeOnly } from "../model/cypher-lexer.js";
import type { GraphModel, ModelWarning } from "../model/types.js";

export interface LintOptions {
  /** Statistics are loaded: relationship degrees are known. */
  statistics?: boolean;
}

const settableOn = (
  f: { settableOn: { create: boolean; update: boolean } },
  op: "create" | "update",
) => f.settableOn[op];

/** Whether a field has a rule that judges its value (not only @normalize). */
const hasRules = (f: { constraints?: object | undefined }) =>
  f.constraints !== undefined &&
  Object.keys(f.constraints).some((rule) => rule !== "normalize");

export function lintModel(
  model: GraphModel,
  options: LintOptions = {},
): ModelWarning[] {
  const out: ModelWarning[] = [];
  for (const node of model.nodes.values()) {
    for (const [i, trigger] of node.triggers.entries()) {
      // What a statement wrote is known only from what it returns.
      if (
        trigger.writes &&
        !/\bAS\s+`?(created|updated)`?\b/i.test(codeOnly(trigger.statement))
      ) {
        out.push({
          type: node.name,
          field: `@trigger ${i + 1}`,
          message:
            "the statement writes but returns neither `created` nor `updated`: every mutation it fires in is reported as broad, so listeners and subscriptions cannot tell what changed. Return the nodes it wrote, e.g. RETURN n AS created",
        });
      }
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
        // LoraDB compares strings exactly: without @normalize, two
        // spellings of one value are two values to a unique constraint.
        if (
          f.unique &&
          !f.list &&
          (f.type === "String" || f.type === "ID") &&
          !f.constraints?.normalize &&
          (settableOn(f, "create") || settableOn(f, "update"))
        ) {
          out.push({
            type: node.name,
            field: f.name,
            message:
              "@unique on text a client supplies, without @normalize: 'Ada@x.com', 'ada@x.com' and 'ada@x.com ' are three values. Add @normalize(trim: true, case: LOWER) if they should be one",
          });
        }
        // A rule on a generated key never runs: the key is not an input.
        if (f.key && f.generate && hasRules(f)) {
          out.push({
            type: node.name,
            field: f.name,
            message:
              "validation rules on a @key(generate: true) apply only to a key the client supplies; a generated UUID is stored as it is",
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
    // A filter rule never guards CREATE: there is no node to filter yet.
    const rules = (op: string) => [
      ...(op === "CREATE" ? [] : (node.authorization?.filter ?? [])),
      ...(node.authorization?.validate ?? []),
    ];
    const open = [...node.mutations].filter(
      (op) =>
        !node.authentication?.has(op) &&
        !rules(op).some((r) => r.operations.has(op)) &&
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
