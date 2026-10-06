// The strings of a rule: `$jwt.claim` references and `${source.path}`
// placeholders, checked for form and for what they may read where they
// stand.

import type { Field, NodeType, RelationshipPropertiesType } from "../types.js";
import { PLACEHOLDER, RULE_REFERENCES } from "../types.js";

/**
 * Claim names referenced anywhere in a value: as "$jwt.name…" strings and
 * as ${jwt.name…} placeholders.
 */
export function claimRefs(value: unknown): string[] {
  if (typeof value === "string") {
    if (value.includes("${")) {
      return [...value.matchAll(PLACEHOLDER)]
        .filter((m) => m[1] === "jwt")
        .map((m) => m[2]!.split(".")[0]!);
    }
    return value.startsWith("$jwt.") ? [value.slice(5).split(".")[0]!] : [];
  }
  if (Array.isArray(value)) return value.flatMap(claimRefs);
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap(claimRefs);
  }
  return [];
}

const REFERENCE_PATH = /^[A-Za-z0-9_]+(\.[A-Za-z0-9_]+)*$/;

/**
 * Malformed claim and context references in a rule's strings: a whole
 * string "$jwt.path" names a path and nothing else; any `${` starts a
 * `${jwt.path}` or `${context.path}` placeholder.
 */
/** `${viewer.field}` placeholders need @viewer, a scalar field, a string operand. */
export function viewerRefProblems(
  value: unknown,
  viewerNode: NodeType | undefined,
  inList = false,
): string[] {
  if (typeof value === "string") {
    const refs = [...value.matchAll(PLACEHOLDER)].filter(
      (m) => m[1] === "viewer",
    );
    if (refs.length === 0) return [];
    if (!viewerNode) {
      return [
        `"${value}": \${viewer.…} needs a @viewer claim on the @jwt type`,
      ];
    }
    const out: string[] = [];
    if (inList) {
      out.push(
        `"${value}": \${viewer.…} stands for one value, not inside a list`,
      );
    }
    for (const m of refs) {
      if (viewerNode.fields.get(m[2]!)?.kind !== "scalar") {
        out.push(`"${value}": ${viewerNode.name} has no scalar field ${m[2]}`);
      }
    }
    return out;
  }
  if (Array.isArray(value)) {
    return value.flatMap((v) => viewerRefProblems(v, viewerNode, true));
  }
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap((v) =>
      viewerRefProblems(v, viewerNode, inList),
    );
  }
  return [];
}

/** What a rule's `${node.…}`, `${source.…}`, `${target.…}`, `${edge.…}` may read. */
export interface RuleReferences {
  node?: NodeType;
  source?: NodeType;
  target?: NodeType;
  edge?: RelationshipPropertiesType | undefined;
}

/**
 * `${node.path}`-style placeholders: the source must be available where
 * the string stands (`node` in a type's rules, `source` / `target` /
 * `edge` in a relationship field's), the path must reach a scalar through
 * single relationships to node types, and it stands for one value.
 */
export function ruleReferenceProblems(
  value: unknown,
  refs: RuleReferences,
  nodes: ReadonlyMap<string, NodeType>,
  inList = false,
): string[] {
  if (typeof value === "string") {
    const out: string[] = [];
    for (const m of value.matchAll(PLACEHOLDER)) {
      const [ref, source, path] = m as unknown as [string, string, string];
      if (!RULE_REFERENCES.has(source)) continue;
      if (inList) {
        out.push(`"${value}": ${ref} stands for one value, not inside a list`);
        continue;
      }
      if (source === "edge") {
        if (!refs.edge) {
          out.push(
            `"${value}": \${edge.…} reads a relationship rule's properties; there are none here`,
          );
        } else if (!refs.edge.fields.has(path)) {
          out.push(`"${value}": ${refs.edge.name} has no property ${path}`);
        }
        continue;
      }
      const start = refs[source as "node" | "source" | "target"];
      if (!start) {
        out.push(
          source === "node" && refs.source
            ? `"${value}": a relationship rule reads its ends: \${source.…} or \${target.…}`
            : source === "node"
              ? `"${value}": viewer: { … } tests the caller's node alone; read the rule's node in a node part`
              : refs.node
                ? `"${value}": \${${source}.…} reads a relationship rule's ends; here the rule's node is \${node.…}`
                : `"${value}": viewer: { … } tests the caller's node alone; read the rule's node in a node part`,
        );
        continue;
      }
      let node: NodeType = start;
      const steps = path.split(".");
      for (const [i, step] of steps.entries()) {
        const f: Field | undefined = node.fields.get(step);
        if (i === steps.length - 1) {
          if (f?.kind !== "scalar") {
            out.push(
              `"${value}": ${node.name}.${step} is not a scalar field: a reference ends on one`,
            );
          }
          break;
        }
        const target: NodeType | undefined =
          f?.kind === "relationship" ? nodes.get(f.target) : undefined;
        if (f?.kind !== "relationship" || f.list || !target) {
          out.push(
            `"${value}": ${node.name}.${step} is not a single relationship to a node type: a reference steps through those only`,
          );
          break;
        }
        node = target;
      }
    }
    return out;
  }
  if (Array.isArray(value)) {
    return value.flatMap((v) => ruleReferenceProblems(v, refs, nodes, true));
  }
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap((v) =>
      ruleReferenceProblems(v, refs, nodes, inList),
    );
  }
  return [];
}

export function ruleStringProblems(value: unknown): string[] {
  if (typeof value === "string") {
    if (value.includes("${")) {
      const rest = value.replace(PLACEHOLDER, (m, _source, path: string) =>
        REFERENCE_PATH.test(path) ? "" : m,
      );
      return rest.includes("${")
        ? [
            `"${value}": a placeholder is \${jwt.<claim>}, \${context.<path>}, \${viewer.<field>} or \${node.<path>}`,
          ]
        : [];
    }
    for (const prefix of ["$jwt.", "$context."]) {
      if (
        value.startsWith(prefix) &&
        !REFERENCE_PATH.test(value.slice(prefix.length))
      ) {
        const source = prefix.slice(1, -1);
        const path =
          /^[A-Za-z0-9_.]*[A-Za-z0-9_]/.exec(value.slice(prefix.length))?.[0] ??
          "claim";
        return [
          `"${value}" is not a ${source} reference; to put one inside a string write "\${${source}.${path}}${value.slice(prefix.length + path.length)}"`,
        ];
      }
    }
    // A misspelt placeholder would be compared as a literal and silently
    // stop meaning what it says. A literal "$" is written "\\$".
    if (
      value.startsWith("$") &&
      !value.startsWith("$jwt.") &&
      !value.startsWith("$context.")
    ) {
      return [
        `"${value}" is not a placeholder: write $jwt.<claim> or $context.<path>, or "\\${value}" for the literal string`,
      ];
    }
    return [];
  }
  if (Array.isArray(value)) return value.flatMap(ruleStringProblems);
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap(ruleStringProblems);
  }
  return [];
}
