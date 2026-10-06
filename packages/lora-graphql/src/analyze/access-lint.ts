// Authorization lint for `check`: rules that are valid but probably not what
// was meant. Warnings, not failures, decided with the access matrix's
// verdicts for an authenticated caller without roles.

import type { GraphQLSchema } from "graphql";
import { authFilter, maskSettled } from "../compile/auth.js";
import { newContext, type CompileContext } from "../compile/context.js";
import type {
  AuthorizationWhere,
  Field,
  GraphModel,
  ModelWarning,
  NestedOperation,
  NodeType,
} from "../model/types.js";
import { PLACEHOLDER } from "../model/types.js";
import { isRecord, token } from "./principals.js";
import { typeVerdict } from "./verdicts.js";

/**
 * Authorization lints for `check`: valid rules that are probably not what
 * was meant.
 */
export function accessLints(
  model: GraphModel,
  schema: GraphQLSchema,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  const subject = model.viewer?.claim ?? "sub";
  const authenticated = newContext(
    { schema, fragments: {}, variables: {} },
    model,
    { jwt: token(model, { [subject]: "someone" }) },
  );
  for (const node of model.nodes.values()) {
    const filters = node.authorization?.filter ?? [];
    filters.forEach((rule, i) => {
      // A filter any signed-in caller passes filters nothing for them.
      const probe = { ...authenticated, params: {}, vars: new Set(["n"]) };
      const alone = {
        ...node,
        authorization: { filter: [rule], validate: [] },
      };
      if (
        authFilter(probe, alone, "n", [...rule.operations][0]!) === undefined
      ) {
        out.push({
          type: node.name,
          message: `filter[${i}] holds for every authenticated caller: it only keeps out anonymous ones (use @authentication for that)`,
        });
      }
    });
    const rules = [
      ...filters.map((r, i) => ({ r, label: `filter[${i}]` })),
      ...(node.authorization?.validate ?? []).map((r, i) => ({
        r,
        label: `validate[${i}]`,
      })),
    ];
    for (const { r, label } of rules) {
      if (!r.requireAuthenticationDefaulted) continue;
      const branch = claimFreeBranch(r.where);
      if (branch !== undefined) {
        out.push({
          type: node.name,
          message: `${label} has a branch that needs no claims (${branch}), but requireAuthentication defaults to true, so anonymous callers are refused by the whole rule; set @authorizationDefaults(requireAuthentication: false) if anonymous callers should get such branches, or true to say they should not`,
        });
      }
    }
    // `bypass: true` (or `false`) on the type says it was decided.
    if (model.bypass && node.authorization?.bypass === undefined) {
      for (const f of node.fields.values()) {
        if (
          f.kind !== "custom" &&
          ((f.authorization?.validate.length ?? 0) > 0 ||
            (f.authorization?.mask?.length ?? 0) > 0)
        ) {
          out.push({
            type: node.name,
            field: f.name,
            message: `the schema's bypass skips the rules of ${node.name}.${f.name} for callers passing it; add @authorization(bypass: false) to ${node.name} if they must hold for everyone, or bypass: true to acknowledge it`,
          });
        }
      }
    }
  }
  out.push(
    ...bypassReach(model),
    ...rewritableRuleFields(model, authenticated),
    ...maskOracles(model, authenticated),
    ...refusedNestedWrites(model, () =>
      newContext({ schema, fragments: {}, variables: {} }, model, {
        jwt: token(model, { [subject]: "someone" }),
      }),
    ),
  );
  return out;
}

/**
 * Every type whose rules the schema's bypass skips, in one line: the
 * bypass is written once and reaches each type that has rules, so a new
 * type joins it without a word. Types that say `bypass: true` or `false`
 * have decided and are left out.
 */
function bypassReach(model: GraphModel): ModelWarning[] {
  if (!model.bypass) return [];
  const reached: string[] = [];
  for (const node of model.nodes.values()) {
    if (node.authorization?.bypass !== undefined) continue;
    const ruled =
      (node.authorization?.filter.length ?? 0) > 0 ||
      (node.authorization?.validate.length ?? 0) > 0 ||
      node.key.keyScope !== undefined ||
      [...node.fields.values()].some(
        (f) =>
          (f.authorization?.filter.length ?? 0) > 0 ||
          (f.authorization?.validate.length ?? 0) > 0 ||
          (f.authorization?.mask?.length ?? 0) > 0,
      );
    if (ruled) reached.push(node.name);
  }
  // Property rules have no type to say `bypass: false` on.
  const properties = [...model.relationshipProperties.values()]
    .filter((p) =>
      [...p.fields.values()].some(
        (f) => (f.authorization?.validate.length ?? 0) > 0,
      ),
    )
    .map((p) => p.name);
  if (reached.length + properties.length === 0) return [];
  const parts = [
    ...(reached.length > 0
      ? [
          `the rules of ${reached.length} ${reached.length === 1 ? "type" : "types"} that ${reached.length === 1 ? "says" : "say"} neither bypass: true nor bypass: false (${reached.join(", ")})`,
        ]
      : []),
    ...(properties.length > 0
      ? [`the property rules of ${properties.join(", ")}, which cannot opt out`]
      : []),
  ];
  return [
    {
      type: "schema",
      message: `@authorizationDefaults(bypass:) reaches ${parts.join(", and ")}; callers passing it skip them`,
    },
  ];
}

/**
 * Fields of `node` a rule tests at the top level of its `part` (`node`,
 * `source` or `target`), through AND, OR and NOT. `pinned`: every test of
 * the field names one node by a claim or context value (`author: {
 * isViewer: true }` once expanded), so it passes for one node only.
 */
function testedFields(
  where: unknown,
  part: "node" | "source" | "target",
  node: NodeType,
  model: GraphModel,
  out = new Map<string, { pinned: boolean }>(),
): Map<string, { pinned: boolean }> {
  if (Array.isArray(where)) {
    for (const w of where) testedFields(w, part, node, model, out);
    return out;
  }
  if (!isRecord(where)) return out;
  for (const [key, value] of Object.entries(where)) {
    if (key === "AND" || key === "OR" || key === "NOT") {
      testedFields(value, part, node, model, out);
    } else if (key === part) nodeTests(value, node, model, out);
    // `${node.trip.key}` anywhere in the rule reads the node's `trip` too.
    for (const name of referencedFields(value, part)) {
      if (node.fields.has(name)) out.set(name, { pinned: false });
    }
  }
  return out;
}

/** The first field of every `${<part>.path}` placeholder in a rule value. */
function referencedFields(value: unknown, part: string): string[] {
  if (typeof value === "string") {
    return [...value.matchAll(PLACEHOLDER)]
      .filter((m) => m[1] === part)
      .map((m) => m[2]!.split(".")[0]!);
  }
  if (Array.isArray(value)) {
    return value.flatMap((v) => referencedFields(v, part));
  }
  if (isRecord(value)) {
    return Object.values(value).flatMap((v) => referencedFields(v, part));
  }
  return [];
}

function nodeTests(
  filter: unknown,
  node: NodeType,
  model: GraphModel,
  out: Map<string, { pinned: boolean }>,
): void {
  if (Array.isArray(filter)) {
    for (const f of filter) nodeTests(f, node, model, out);
    return;
  }
  if (!isRecord(filter)) return;
  for (const [key, value] of Object.entries(filter)) {
    if (key === "AND" || key === "OR" || key === "NOT") {
      nodeTests(value, node, model, out);
      continue;
    }
    const name = node.fields.has(key)
      ? key
      : key.replace(/(Connection|Aggregate|Exists)$/, "");
    const f = node.fields.get(name);
    if (!f || (f.kind !== "scalar" && f.kind !== "relationship")) continue;
    const pinned =
      f.kind === "relationship" &&
      key === name &&
      pinsOne(value, model.nodes.get(f.target));
    out.set(name, { pinned: (out.get(name)?.pinned ?? true) && pinned });
  }
}

/** `{ <key or unique field>: { eq: "$jwt.…" } }`: one node, by a claim. */
function pinsOne(value: unknown, target: NodeType | undefined): boolean {
  if (!target || !isRecord(value)) return false;
  const keys = Object.keys(value);
  if (keys.length !== 1) return false;
  const f = target.fields.get(keys[0]!);
  const test = value[keys[0]!];
  return (
    f?.kind === "scalar" &&
    (f.key || f.unique) &&
    !f.list &&
    isRecord(test) &&
    Object.keys(test).length === 1 &&
    typeof test["eq"] === "string" &&
    test["eq"].startsWith("$")
  );
}

const REPOINT: readonly NestedOperation[] = ["CONNECT", "DISCONNECT", "CREATE"];

/**
 * The nested writes an update input offers that re-point a single
 * relationship (`connect`, `disconnect`, `create`), or none.
 */
function repointable(node: NodeType, f: Field): NestedOperation[] {
  if (
    f.kind !== "relationship" ||
    f.list ||
    !node.mutations.has("UPDATE") ||
    !f.settableOn.update
  ) {
    return [];
  }
  return REPOINT.filter((op) => f.nestedOperations.has(op));
}

/** Whether an update can change the field's value. */
function updatable(node: NodeType, f: Field): boolean {
  if (f.kind === "scalar") {
    return node.mutations.has("UPDATE") && f.settableOn.update && !f.readonly;
  }
  return repointable(node, f).length > 0;
}

/**
 * A rule over a field the guarded operation can rewrite:
 * - an UPDATE rule testing a single relationship the update input can
 *   re-point: the rule sees the node before (and, validated AFTER, after)
 *   the write, so a caller passing it on two targets moves the node;
 * - a CREATE validate rule testing a field UPDATE can change while no
 *   UPDATE rule tests it: created as the rule demands, then changed.
 * Tests that name one node by a claim (`isViewer`) can only re-point to
 * the caller's own node, so the first check skips them. A type no
 * signed-in caller without roles may update has nothing to re-point, and a
 * scalar a CREATE rule tests is usually state an update moves on (a
 * request's status), so the second check looks at relationships only.
 */
function rewritableRuleFields(
  model: GraphModel,
  authenticated: CompileContext,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  for (const node of model.nodes.values()) {
    if (!node.mutations.has("UPDATE")) continue;
    const probe = { ...authenticated, params: {}, vars: new Set(["n"]) };
    const { verdict } = typeVerdict(probe, node, "UPDATE");
    if (verdict === "denied" || verdict === "unauthenticated") continue;
    const rules = [
      ...(node.authorization?.filter ?? []).map((r, i) => ({
        r,
        label: `filter[${i}]`,
        after: false,
      })),
      ...(node.authorization?.validate ?? []).map((r, i) => ({
        r,
        label: `validate[${i}]`,
        after: r.when.has("AFTER"),
      })),
    ];
    const updateRules = rules.filter(({ r }) => r.operations.has("UPDATE"));
    const updateTests = new Map<
      string,
      { labels: string[]; after: boolean; pinned: boolean }
    >();
    for (const { r, label, after } of updateRules) {
      for (const [name, t] of testedFields(r.where, "node", node, model)) {
        const seen = updateTests.get(name);
        updateTests.set(name, {
          labels: [...(seen?.labels ?? []), label],
          after: (seen?.after ?? false) || after,
          pinned: (seen?.pinned ?? true) && t.pinned,
        });
      }
    }
    for (const [name, t] of updateTests) {
      const f = node.fields.get(name)!;
      const ops = repointable(node, f);
      if (ops.length === 0 || t.pinned || f.kind !== "relationship") continue;
      out.push({
        type: node.name,
        field: name,
        message: `${t.labels.join(", ")} ${t.labels.length === 1 ? "guards" : "guard"} UPDATE by testing ${name}, which the update input can re-point (${ops.map((o) => o.toLowerCase()).join(", ")}): ${t.after ? `the rule runs before and after the write, so a caller who passes it on both the old and the new ${f.target} moves the ${node.name}` : `the rule only sees the ${node.name} before the write, so a caller who passes it can move it to any ${f.target}`}; declare ${name} @settable(onCreate: true, onUpdate: false) if it is fixed once created`,
      });
    }
    const createTested = new Map<string, string[]>();
    (node.authorization?.validate ?? []).forEach((r, i) => {
      if (!r.operations.has("CREATE")) return;
      for (const name of testedFields(r.where, "node", node, model).keys()) {
        createTested.set(name, [
          ...(createTested.get(name) ?? []),
          `validate[${i}]`,
        ]);
      }
    });
    for (const [name, labels] of createTested) {
      const f = node.fields.get(name)!;
      if (updateTests.has(name) || !updatable(node, f)) continue;
      // A field-level UPDATE rule guards writing the field itself.
      if (f.authorization?.validate.some((r) => r.operations.has("UPDATE"))) {
        continue;
      }
      if (f.kind !== "relationship" || f.list) continue;
      out.push({
        type: node.name,
        field: name,
        message: `${labels.join(", ")} ${labels.length === 1 ? "tests" : "test"} ${name} on CREATE, but UPDATE can re-point it and no UPDATE rule tests it: a caller can create the ${node.name} as the rule demands, then change ${name}; declare ${name} @settable(onCreate: true, onUpdate: false), or add an UPDATE rule that tests it`,
      });
    }
  }
  return out;
}

/**
 * A validate rule (other than READ) or relationship rule testing a masked
 * field: rules see the stored value, so success versus FORBIDDEN tells a
 * caller what the mask hides from them. Skipped when the claims of an
 * authenticated caller already settle the mask.
 */
function maskOracles(
  model: GraphModel,
  authenticated: CompileContext,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  const masked = (node: NodeType, name: string) => {
    const f = node.fields.get(name);
    return (
      f?.kind === "scalar" &&
      (f.authorization?.mask?.length ?? 0) > 0 &&
      !maskSettled(authenticated, node, f)
    );
  };
  const report = (owner: NodeType, name: string, rule: string, ops: string[]) =>
    out.push({
      type: owner.name,
      field: name,
      message: `${rule} (${ops.join(", ")}) tests ${owner.name}.${name}, which a mask hides from callers failing its unless; rules see the stored value, so whether the request succeeds or is FORBIDDEN tells such a caller the hidden value. Test what the caller can see, or make sure every caller the rule decides for passes the mask`,
    });
  for (const node of model.nodes.values()) {
    const seen = new Set<string>();
    const once = (...args: Parameters<typeof report>) => {
      const id = `${args[0].name}.${args[1]}:${args[2]}`;
      if (seen.has(id)) return;
      seen.add(id);
      report(...args);
    };
    // A create's rules test values the caller wrote: nothing hidden.
    const opsOf = (r: { operations: ReadonlySet<string> }) =>
      [...r.operations].filter((op) => op !== "READ" && op !== "CREATE");
    (node.authorization?.validate ?? []).forEach((r, i) => {
      const ops = opsOf(r);
      if (ops.length === 0) return;
      for (const name of testedFields(r.where, "node", node, model).keys()) {
        if (masked(node, name)) once(node, name, `validate[${i}]`, ops);
      }
    });
    for (const f of node.fields.values()) {
      (f.authorization?.validate ?? []).forEach((r, i) => {
        const ops = opsOf(r);
        if (ops.length === 0) return;
        const label = `${node.name}.${f.name} validate[${i}]`;
        for (const part of ["node", "source"] as const) {
          for (const name of testedFields(r.where, part, node, model).keys()) {
            if (masked(node, name)) once(node, name, label, ops);
          }
        }
        const target =
          f.kind === "relationship" ? model.nodes.get(f.target) : undefined;
        if (!target) return;
        for (const name of testedFields(
          r.where,
          "target",
          target,
          model,
        ).keys()) {
          if (masked(target, name)) once(target, name, label, ops);
        }
      });
    }
  }
  return out;
}

/**
 * Nested creates, updates and deletes an input offers into a type whose
 * rules for that write refuse every authenticated caller without a role
 * or claim beyond the subject: surface only an admin (or the bypass) can
 * use. Decided with the access matrix's verdicts; skipped on an input the
 * same caller cannot use at all (an admin-only type's links).
 */
function refusedNestedWrites(
  model: GraphModel,
  context: () => CompileContext,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  const subject = model.viewer?.claim ?? "sub";
  const refuses = (t: NodeType, op: "CREATE" | "UPDATE" | "DELETE") => {
    const { verdict } = typeVerdict(context(), t, op);
    return verdict === "denied" || verdict === "unauthenticated";
  };
  for (const node of model.nodes.values()) {
    // An input the same caller cannot use advertises nothing to them.
    const createOpen = node.mutations.has("CREATE") && !refuses(node, "CREATE");
    const updateOpen = node.mutations.has("UPDATE") && !refuses(node, "UPDATE");
    if (!createOpen && !updateOpen) continue;
    for (const f of node.fields.values()) {
      if (f.kind !== "relationship" || f.via) continue;
      const offered = new Set<"CREATE" | "UPDATE" | "DELETE">();
      if (createOpen && f.settableOn.create) {
        if (f.nestedOperations.has("CREATE")) offered.add("CREATE");
      }
      if (updateOpen && f.settableOn.update) {
        for (const op of ["CREATE", "UPDATE", "DELETE"] as const) {
          if (f.nestedOperations.has(op)) offered.add(op);
        }
      }
      const targets = f.members
        .map((m) => model.nodes.get(m))
        .filter((t): t is NodeType => t !== undefined);
      if (targets.length === 0) continue;
      const refused = [...offered].filter((op) =>
        targets.every((t) => refuses(t, op)),
      );
      if (refused.length === 0) continue;
      const kept = [...f.nestedOperations].filter(
        (op) => !refused.includes(op as "CREATE"),
      );
      // Edge properties stay writable without the node: UPDATE_EDGE.
      if (
        refused.includes("UPDATE") &&
        f.properties &&
        !kept.includes("UPDATE_EDGE")
      ) {
        kept.push("UPDATE_EDGE");
      }
      out.push({
        type: node.name,
        field: f.name,
        message: `nested ${refused.map((o) => o.toLowerCase()).join(", ")} into ${f.target}: its ${refused.join(", ")} rules refuse every signed-in caller without a role or claim beyond ${subject}, so the input advertises writes only an admin (or the bypass) can make; declare nestedOperations: [${kept.join(", ")}] on ${f.name}`,
      });
    }
  }
  return out;
}

/** The path of an OR branch that tests no claim, if the rule has one. */
function claimFreeBranch(where: AuthorizationWhere): string | undefined {
  const or = where["OR"];
  if (!Array.isArray(or)) return undefined;
  const i = or.findIndex((b) => !usesClaims(b));
  return i >= 0 ? `OR[${i}]` : undefined;
}

function usesClaims(where: unknown): boolean {
  if (typeof where === "string") {
    return where.startsWith("$jwt.") || where.includes("${jwt.");
  }
  if (Array.isArray(where)) return where.some(usesClaims);
  if (where === null || typeof where !== "object") return false;
  return Object.entries(where).some(
    ([k, v]) =>
      k === "jwt" || k === "viewer" || k === "isViewer" || usesClaims(v),
  );
}
