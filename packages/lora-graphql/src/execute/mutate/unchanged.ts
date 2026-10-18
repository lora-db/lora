// An upsert's rows that would write nothing: every stated property
// already holds and every stated connect is already there, with its
// properties. They are left out of the update, so a seed re-run over a
// seeded graph costs a read per row (G-57). The comparison shares the
// read that tells existing keys from new ones.
//
// Only what the caller may read is compared, or the answer would tell a
// write-only value. A row the comparison cannot settle cheaply (a nested
// create, a polymorphic field, a relationship with rules, a `@populatedBy`
// value) is written as before, and so meets every check it did.

import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  fieldValidate,
  propertyAccess,
  relationshipRules,
} from "../../compile/auth.js";
import { bind, type CompileContext } from "../../compile/context.js";
import { and, name, printExpr, type Expr } from "../../compile/cypher.js";
import { settable } from "../../model/inputs.js";
import { keyOf } from "../../compile/read/by-keys.js";
import { toStored } from "../../model/points.js";
import type {
  AuthOperation,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import { andText, arrow, labelTest } from "./fragments.js";
import type { Input } from "./plan.js";
import type { Runner } from "./runner.js";

/** A row the comparison may settle: what it sets, removes and connects. */
interface Candidate {
  /** Per relationship field: the targets and their stored properties. */
  connects: Map<RelationshipField, Array<{ to: unknown; props: Input }>>;
  /** The statement's row: key, set, remove, and `connects` per field. */
  row: Input;
}

/**
 * Which of an upsert's keys name a node the caller may read and update
 * (`existing`), and which of those an update would change nothing on
 * (`same`): one read for both. `input` is what an update of the row
 * would write. A row that cannot be settled here is never `same`.
 */
export async function existingRows(
  runner: Runner,
  node: NodeType,
  rows: Array<{ key: unknown; input: Input }>,
): Promise<{ existing: Set<string>; same: Set<string> }> {
  const ctx = runner.ctx();
  // A callback's value is only known by running it.
  const computed = [...node.fields.values()].some(
    (f) => f.kind === "scalar" && f.populatedBy?.operations.has("UPDATE"),
  );
  // Per relationship field: whether its connects may be compared.
  const comparable = new Map<RelationshipField, boolean>();
  const candidates: Candidate[] = [];
  const statementRows: Input[] = [];
  const stated = new Set<ScalarField>();
  const rels = new Set<RelationshipField>();
  for (const { key, input } of rows) {
    const c = computed
      ? undefined
      : candidate(ctx, comparable, node, key, input);
    if (!c) {
      statementRows.push({ key });
      continue;
    }
    candidates.push(c);
    statementRows.push(c.row);
    for (const n of Object.keys(input)) {
      const f = node.fields.get(n);
      if (f?.kind === "scalar" && !f.key) stated.add(f);
    }
    for (const rel of c.connects.keys()) rels.add(rel);
  }

  // The rules the update would hold the node to, before and after: on an
  // unchanged node both read what is stored now. A rule the claims alone
  // refuse throws: the rows are written, and fail as they did.
  let rules: Expr | undefined;
  const relTests: string[] = [];
  try {
    if (candidates.length > 0) checkAuthentication(ctx, node, "UPDATE");
    rules = and(
      authValidate(ctx, node, "n", "UPDATE", "BEFORE"),
      authValidate(ctx, node, "n", "UPDATE", "AFTER"),
      ...[...stated].flatMap((f) => [
        fieldValidate(ctx, node, f, "n", "UPDATE", "BEFORE"),
        fieldValidate(ctx, node, f, "n", "UPDATE", "AFTER"),
      ]),
    );
    for (const [i, rel] of [...rels].entries()) {
      const target = runner.env.model.nodes.get(rel.target)!;
      const param = `c${i}`;
      // Without edge properties a connect is its target's key: a smaller
      // parameter, which is most of what the comparison costs.
      const keyed = candidates.every((c) =>
        (c.connects.get(rel) ?? []).every(
          (w) => Object.keys(w.props).length === 0,
        ),
      );
      for (const c of candidates) {
        const list = c.connects.get(rel);
        if (list) c.row[param] = keyed ? list.map((w) => w.to) : list;
      }
      // Only what a connect could see: a hidden target is not found.
      const visible =
        ` WHERE ${labelTest("m", target)}` +
        andText(authFilter(ctx, target, "m", "READ")) +
        andText(authFilter(ctx, target, "m", "CREATE_RELATIONSHIP"));
      const there =
        `[(n)${arrow(rel, "r", "m", undefined)}${visible}` +
        ` AND m.${name(target.key.property)} = ${keyed ? "want" : "want.to"} | properties(r)]`;
      const connected = keyed
        ? `all(want IN row.${param} WHERE size(${there}) > 0)`
        : `all(want IN row.${param} WHERE any(edge IN ${there}` +
          ` WHERE all(k IN keys(want.props) WHERE edge[k] = want.props[k])))`;
      // A single relationship is replaced: it holds no other target.
      const alone = rel.list
        ? ""
        : ` AND (row.${param} IS NULL OR size([(n)${arrow(rel, "r", "m", undefined)} WHERE ${labelTest("m", target)} | 1]) = 1)`;
      const owner = authFilter(ctx, node, "n", "CREATE_RELATIONSHIP");
      relTests.push(
        `(row.${param} IS NULL OR (${connected}${alone}` +
          `${owner ? ` AND ${printExpr(owner)}` : ""}))`,
      );
    }
  } catch {
    for (const c of candidates) {
      for (const k of Object.keys(c.row)) if (k !== "key") delete c.row[k];
    }
    rules = undefined;
    relTests.length = 0;
  }

  const rowsParam = printExpr(bind(ctx, statementRows));
  // A row that is not compared has no `set`, and stops there.
  const tests = [
    "row.set IS NOT NULL",
    "all(k IN keys(row.set) WHERE n[k] = row.set[k])",
    "all(k IN coalesce(row.remove, []) WHERE n[k] IS NULL)",
    ...relTests,
    ...(rules ? [printExpr(rules)] : []),
  ];
  const found = await runner.run(
    `UNWIND ${rowsParam} AS row\n` +
      `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = row.key` +
      andText(authFilter(ctx, node, "n", "READ")) +
      andText(authFilter(ctx, node, "n", "UPDATE")) +
      "\n" +
      `RETURN n.${name(node.key.property)} AS key, coalesce(${tests.join("\n  AND ")}, false) AS same`,
    ctx,
  );
  return {
    existing: new Set(found.map((r) => keyOf(r["key"]))),
    same: new Set(
      found.filter((r) => r["same"] === true).map((r) => keyOf(r["key"])),
    ),
  };
}

/** What the row writes, when the comparison can settle it; else nothing. */
function candidate(
  ctx: CompileContext,
  comparable: Map<RelationshipField, boolean>,
  node: NodeType,
  key: unknown,
  input: Input,
): Candidate | undefined {
  const set: Input = {};
  const remove: string[] = [];
  const connects: Candidate["connects"] = new Map();
  try {
    for (const [n, value] of Object.entries(input)) {
      const f = node.fields.get(n);
      if (f?.kind === "scalar") {
        if (f.key) continue;
        if (!readable(f)) return undefined;
        checkFieldAuthentication(ctx, node.name, f, "UPDATE");
        if (value === null) {
          // A required field cannot be removed: the update says so.
          if (f.required) return undefined;
          remove.push(f.property);
        } else set[f.property] = toStored(f, value);
        continue;
      }
      if (f?.kind !== "relationship") return undefined;
      if (value === null || value === undefined) continue;
      let ok = comparable.get(f);
      if (ok === undefined) {
        ok = connectable(ctx, node, f);
        comparable.set(f, ok);
      }
      if (!ok) return undefined;
      const list = connectsOf(ctx, f, value as Input);
      if (!list) return undefined;
      connects.set(f, list);
    }
  } catch {
    return undefined;
  }
  return {
    connects,
    row: remove.length > 0 ? { key, set, remove } : { key, set },
  };
}

/**
 * Whether a field's connects may be compared: to a concrete type, with no
 * rule on the relationship or its ends that the comparison would skip,
 * and none the claims alone refuse. The same for every row.
 */
function connectable(
  ctx: CompileContext,
  owner: NodeType,
  rel: RelationshipField,
): boolean {
  const target = ctx.model.nodes.get(rel.target);
  if (!target || rel.via) return false;
  const vars = { owner: "a", target: "b", rel: "r" };
  const ops: Array<"CONNECT" | "UPDATE_EDGE" | "DISCONNECT"> = [
    "CONNECT",
    "UPDATE_EDGE",
    "DISCONNECT",
  ];
  try {
    if (ops.some((op) => relationshipRules(ctx, rel, op, vars) !== undefined)) {
      return false;
    }
    const ends: AuthOperation[] = [
      "CREATE_RELATIONSHIP",
      "DELETE_RELATIONSHIP",
    ];
    for (const op of ends) {
      for (const when of ["BEFORE", "AFTER"] as const) {
        if (authValidate(ctx, owner, "a", op, when)) return false;
        if (authValidate(ctx, target, "b", op, when)) return false;
      }
    }
    checkFieldAuthentication(ctx, owner.name, rel, "UPDATE");
    checkFieldAuthentication(ctx, owner.name, rel, "CREATE_RELATIONSHIP");
    checkAuthentication(ctx, owner, "CREATE_RELATIONSHIP");
    checkAuthentication(ctx, target, "CREATE_RELATIONSHIP");
    checkAuthentication(ctx, owner, "DELETE_RELATIONSHIP");
  } catch {
    return false;
  }
  return true;
}

/** A relationship input's connects, when it holds nothing else. */
function connectsOf(
  ctx: CompileContext,
  rel: RelationshipField,
  value: Input,
): Array<{ to: unknown; props: Input }> | undefined {
  const model = ctx.model;
  const target = model.nodes.get(rel.target)!;
  for (const [op, v] of Object.entries(value)) {
    if (op !== "connect" && v !== null && v !== undefined) return undefined;
  }
  const connect = value["connect"];
  const list = (
    connect === null || connect === undefined
      ? []
      : Array.isArray(connect)
        ? connect
        : [connect]
  ) as Input[];
  if (list.length === 0 || (!rel.list && list.length > 1)) return undefined;
  const props = rel.properties
    ? model.relationshipProperties.get(rel.properties)
    : undefined;
  const out: Array<{ to: unknown; props: Input }> = [];
  const seen = new Set<string>();
  for (const c of list) {
    const to = c[target.key.name];
    if (seen.has(keyOf(to))) return undefined;
    seen.add(keyOf(to));
    const edge = (c["edge"] as Input | null | undefined) ?? {};
    const stored: Input = {};
    for (const [n, v] of Object.entries(edge)) {
      const f = props?.fields.get(n);
      if (v === null || v === undefined) continue;
      // A re-connect updates the properties it is given.
      if (
        !f ||
        !readable(f) ||
        !settable(f, "UPDATE") ||
        propertyAccess(ctx, f, "UPDATE") !== "allowed"
      ) {
        return undefined;
      }
      stored[f.property] = toStored(f, v);
    }
    out.push({ to, props: stored });
  }
  return out;
}

/** Readable by anyone who may read the node: no rule, mask or write-only. */
function readable(f: ScalarField): boolean {
  return (
    !f.private &&
    f.selectableOn.read &&
    !f.authentication?.has("READ") &&
    !(f.authorization?.validate ?? []).some((r) => r.operations.has("READ")) &&
    !f.authorization?.mask
  );
}
