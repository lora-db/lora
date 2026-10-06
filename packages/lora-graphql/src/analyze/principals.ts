// The callers the access matrix and the authorization lints evaluate as:
// anonymous, authenticated, `viewer`, and one per claim value the rules test,
// each with the token (and, for `viewer`, the model) its verdicts are read off.

import type {
  AuthorizationWhere,
  Field,
  GraphModel,
  NodeType,
  RelationshipPropertiesType,
} from "../model/types.js";

export interface Principal {
  name: string;
  jwt: Record<string, unknown> | undefined;
  /** The model this principal's verdicts are read off, when not the schema's. */
  model?: GraphModel;
}

/** `node` (or its field, or a property type's field) as `p` sees the model. */
export const nodeAs = (p: Principal, node: NodeType): NodeType =>
  p.model?.nodes.get(node.name) ?? node;
export const fieldAs = <F extends Field>(
  p: Principal,
  node: NodeType,
  f: F,
): F => (nodeAs(p, node).fields.get(f.name) as F | undefined) ?? f;
export const propertyAs = <F extends { name: string }>(
  p: Principal,
  props: RelationshipPropertiesType,
  f: F,
): F =>
  (p.model?.relationshipProperties.get(props.name)?.fields.get(f.name) as
    | F
    | undefined) ?? f;

/**
 * Anonymous, authenticated (a token with no roles), `viewer` when rules
 * name the caller's node, and one principal per claim value the rules
 * test with `includes`, `eq` or `in` (`roles:admin`).
 */
export function principalsOf(model: GraphModel): Principal[] {
  const subject = model.viewer?.claim ?? "sub";
  const base = token(model, { [subject]: "someone" });
  const tested = new Map<
    string,
    { claim: string; value: unknown; list: boolean }
  >();
  const visit = (where: unknown) => {
    if (Array.isArray(where)) return where.forEach(visit);
    if (where === null || typeof where !== "object") return;
    for (const [key, value] of Object.entries(where)) {
      if (key === "jwt" && value && typeof value === "object") {
        for (const [claim, ops] of Object.entries(value)) {
          for (const [op, operand] of Object.entries(
            (ops ?? {}) as Record<string, unknown>,
          )) {
            const values =
              op === "in" && Array.isArray(operand)
                ? operand
                : op === "includes" || op === "eq"
                  ? [operand]
                  : [];
            for (const v of values) {
              if (typeof v !== "string" || claim === subject) continue;
              tested.set(`${claim}:${v}`, {
                claim,
                value: v,
                list: op === "includes",
              });
            }
          }
        }
      } else visit(value);
    }
  };
  // `@authentication(jwt:)` holds the claims without the `jwt` key a
  // rule writes them under.
  const claims = (where: unknown) => (where ? { jwt: where } : undefined);
  const rules = (a: NodeType["authorization"]) => [
    ...(a?.filter ?? []).map((r) => r.where),
    ...(a?.validate ?? []).map((r) => r.where),
    ...(a?.mask ?? []).map((m) => m.unless),
  ];
  visit(model.bypass);
  for (const node of model.nodes.values()) {
    visit(rules(node.authorization));
    visit(claims(node.authenticationJwt));
    for (const f of node.fields.values()) {
      visit(rules(f.authorization));
      visit(claims(f.authenticationJwt));
    }
  }
  for (const props of model.relationshipProperties.values()) {
    for (const f of props.fields.values()) visit(rules(f.authorization));
  }
  for (const f of [...model.queries, ...model.mutations]) {
    visit(rules(f.authorization));
    visit(claims(f.authenticationJwt));
  }
  const related = viewerModel(model);
  return [
    { name: "anonymous", jwt: undefined },
    { name: "authenticated", jwt: base },
    ...(related ? [{ name: "viewer", jwt: base, model: related }] : []),
    ...[...tested.entries()]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([name, t]) => ({
        name,
        jwt: token(model, {
          [subject]: "someone",
          [t.claim]: t.list ? [t.value] : t.value,
        }),
      })),
  ];
}

/**
 * The model as the `viewer` principal meets it: an authenticated caller
 * on nodes that are theirs, so every part of a rule that only names the
 * caller's node (`isViewer`, alone or through relationships) passes. It
 * shows the most a related caller gets, where `authenticated` shows what
 * every signed-in caller gets. Undefined when no rule has such a part.
 *
 * A part under `NOT` is left as written. "Not following themselves" does
 * not fail for the viewer because the viewer is who it names: it depends
 * on the rows, as it does for any signed-in caller, and passing the part
 * and then negating it would read the viewer below `authenticated`.
 */
function viewerModel(model: GraphModel): GraphModel | undefined {
  const viewer = model.viewer;
  if (!viewer) return undefined;
  let found = false;
  const always = { jwt: { [viewer.claim]: { exists: true } } };
  const namesViewer = (node: NodeType | undefined, where: unknown): boolean => {
    if (!node || !isRecord(where)) return false;
    const entries = Object.entries(where);
    if (entries.length !== 1) return false;
    const [key, value] = entries[0]!;
    if (key === "AND" || key === "OR") {
      if (!Array.isArray(value) || value.length === 0) return false;
      return key === "AND"
        ? value.every((w) => namesViewer(node, w))
        : value.some((w) => namesViewer(node, w));
    }
    if (node.name === viewer.type && key === viewer.field) {
      return (
        isRecord(value) &&
        Object.keys(value).length === 1 &&
        value["eq"] === `$jwt.${viewer.claim}`
      );
    }
    const f = node.fields.get(key);
    if (f?.kind !== "relationship") return false;
    const target = model.nodes.get(f.target);
    if (!f.list) return namesViewer(target, value);
    return (
      isRecord(value) &&
      Object.keys(value).length === 1 &&
      namesViewer(target, value["some"])
    );
  };
  type Ends = Partial<
    Record<"node" | "source" | "target", NodeType | undefined>
  >;
  const rewrite = (
    where: AuthorizationWhere,
    ends: Ends,
    negated = false,
  ): AuthorizationWhere => {
    if (!isRecord(where)) return where;
    const out: Record<string, unknown> = {};
    let dropped = false;
    for (const [key, value] of Object.entries(where)) {
      if (key === "AND" || key === "OR") {
        out[key] = Array.isArray(value)
          ? value.map((w) => rewrite(w as AuthorizationWhere, ends, negated))
          : value;
      } else if (key === "NOT") {
        out[key] = rewrite(value as AuthorizationWhere, ends, !negated);
      } else if (
        !negated &&
        (key === "node" || key === "source" || key === "target") &&
        namesViewer(ends[key], value)
      ) {
        dropped = true;
      } else out[key] = value;
    }
    if (!dropped) return out;
    found = true;
    // The part passes: it leaves the rule, or stands as a claim the
    // caller has when nothing else is tested beside it.
    return Object.keys(out).length === 0 ? always : out;
  };
  const authorization = <A extends NodeType["authorization"]>(
    a: A,
    ends: Ends,
  ): A =>
    a && {
      ...a,
      filter: a.filter.map((r) => ({ ...r, where: rewrite(r.where, ends) })),
      validate: a.validate.map((r) => ({
        ...r,
        where: rewrite(r.where, ends),
      })),
      ...(a.mask
        ? {
            mask: a.mask.map((m) => ({
              ...m,
              unless: rewrite(m.unless, ends),
            })),
          }
        : {}),
    };
  const nodes = new Map<string, NodeType>();
  for (const node of model.nodes.values()) {
    const fields = new Map<string, Field>();
    for (const f of node.fields.values()) {
      fields.set(
        f.name,
        f.authorization
          ? ({
              ...f,
              authorization: authorization(f.authorization, {
                node,
                ...(f.kind === "relationship"
                  ? { source: node, target: model.nodes.get(f.target) }
                  : {}),
              }),
            } as Field)
          : f,
      );
    }
    nodes.set(node.name, {
      ...node,
      fields,
      authorization: authorization(node.authorization, { node }),
    });
  }
  const relationshipProperties = new Map<string, RelationshipPropertiesType>();
  for (const props of model.relationshipProperties.values()) {
    // Property rules test the ends of the relationship fields using them.
    const user = [...model.nodes.values()]
      .flatMap((n) => [...n.fields.values()])
      .find((f) => f.kind === "relationship" && f.properties === props.name);
    const ends: Ends =
      user?.kind === "relationship"
        ? {
            source: model.nodes.get(user.owner),
            target: model.nodes.get(user.target),
          }
        : {};
    relationshipProperties.set(props.name, {
      ...props,
      fields: new Map(
        [...props.fields.values()].map((f) => [
          f.name,
          f.authorization
            ? { ...f, authorization: authorization(f.authorization, ends) }
            : f,
        ]),
      ),
    } as RelationshipPropertiesType);
  }
  return found ? { ...model, nodes, relationshipProperties } : undefined;
}

/**
 * A token carrying `claims` where the `@jwt` type says they live
 * (`@jwtClaim(path: "app_metadata.roles")`).
 */
export function token(
  model: GraphModel,
  claims: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [claim, value] of Object.entries(claims)) {
    const path = (model.jwt?.get(claim) ?? claim).split(".");
    let at = out;
    for (const part of path.slice(0, -1)) {
      at = (at[part] ??= {}) as Record<string, unknown>;
    }
    at[path[path.length - 1]!] = value;
  }
  return out;
}

export const isRecord = (x: unknown): x is Record<string, unknown> =>
  typeof x === "object" && x !== null && !Array.isArray(x);
