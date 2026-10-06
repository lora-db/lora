// A `@cypher` field: its statement, the column it returns, its arguments
// and parameters, and the checks on what the statement may do where.

import {
  getNamedType,
  isInterfaceType,
  isUnionType,
  valueFromAST,
  type ConstValueNode,
  type GraphQLArgument,
  type GraphQLDirective,
  type GraphQLField,
  type GraphQLObjectType,
  type GraphQLSchema,
} from "graphql";
import type { ModelProblem } from "../../errors.js";
import { codeOnly, maskLiterals, scanParams } from "../cypher-lexer.js";
import type {
  AuthorizationWhere,
  CypherArgument,
  CypherField,
  ModelWarning,
  ScalarField,
} from "../types.js";
import { outOfRange } from "../inputs.js";
import { unwrap, isScalarLike, shapeOf, unwrapInput } from "./shapes.js";
import { directive } from "./directive-args.js";
import { buildScalarField } from "./scalar-field.js";
import { authOps, readOnlyRules, rootFieldRules } from "./authorization.js";

const WRITE_CLAUSES = /\b(CREATE|MERGE|SET|DELETE|REMOVE|DETACH)\b/;

export function buildCypherField(
  owner: string,
  f: GraphQLField<unknown, unknown>,
  d: (n: string) => GraphQLDirective,
  problems: ModelProblem[],
  warnings: ModelWarning[],
  ctx: {
    nodeNames: Set<string>;
    plainNames: Set<string>;
    schema: GraphQLSchema;
    root: "Query" | "Mutation" | undefined;
    /** Whether the @jwt type has a @viewer claim (enables `$viewer`). */
    viewer: boolean;
  },
): CypherField | undefined {
  const at = (message: string) =>
    problems.push({ type: owner, field: f.name, message });
  const warn = (message: string) =>
    warnings.push({ type: owner, field: f.name, message });
  const args = directive(d("cypher"), f, at);
  if (!args) return undefined;
  const authorization = ctx.root
    ? rootFieldRules(directive(d("authorization"), f, at), at)
    : readOnlyRules(directive(d("authorization"), f, at), at);
  const statement = args["statement"] as string;
  for (const other of [
    "relationship",
    "key",
    "unique",
    "alias",
    "index",
    "default",
    "timestamp",
    "groupBy",
  ]) {
    if (directive(d(other), f, at))
      at(`@${other} cannot be combined with @cypher`);
  }

  const shape = unwrap(f.type);
  const named = getNamedType(f.type).name;
  const node = ctx.nodeNames.has(named) ? named : undefined;
  const namedType = ctx.schema.getType(named);
  const abstract =
    namedType && (isInterfaceType(namedType) || isUnionType(namedType))
      ? named
      : undefined;
  const object = ctx.plainNames.has(named) ? named : undefined;
  if (!node && !abstract && !object && !isScalarLike(named, ctx.schema)) {
    at(
      "@cypher fields return scalars, enums, @node types, interfaces or unions over them, or object types without @node",
    );
    return undefined;
  }
  if (shape.depth > 1) at("nested lists are not supported");
  if (shape.list && !shape.itemRequired) {
    at("list items must be non-null, e.g. [Festival!]");
  }

  const columnName =
    (args["columnName"] as string | undefined) ?? inferColumn(statement);
  if (!columnName) {
    at("cannot tell which column holds the value; set @cypher(columnName:)");
    return undefined;
  }

  const cypherArgs: CypherArgument[] = [];
  for (const a of f.args) {
    const argNamed = getNamedType(a.type).name;
    if (!isScalarLike(argNamed, ctx.schema)) {
      at(`argument ${a.name}: @cypher arguments must be scalars or enums`);
      continue;
    }
    if (a.name === "jwt")
      at("argument `jwt` is reserved for the request's claims");
    if (a.name === "viewer")
      at("argument `viewer` is reserved for the caller's @viewer key");
    const argShape = unwrapInput(a.type);
    const size = directive(d("size"), a, at)?.["max"] as number | undefined;
    if (size !== undefined && !argShape.list) {
      at(`argument ${a.name}: @size applies to list arguments`);
    } else if (size !== undefined && size < 1) {
      at(`argument ${a.name}: @size(max:) must be at least 1`);
    }
    const range = cypherArgumentRange(a, argNamed, d("range"), at);
    const defaultValue = argumentDefault(a);
    if (range && outOfRange(defaultValue, range) !== undefined) {
      at(`argument ${a.name}: its default is outside @range`);
    }
    cypherArgs.push({
      name: a.name,
      type: { named: argNamed, ...argShape },
      defaultValue,
      description: a.description ?? undefined,
      ...(size !== undefined ? { maxItems: size } : {}),
      ...(range ? { range } : {}),
    });
  }

  const params = [...new Set(scanParams(statement).map((p) => p.name))];
  const known = new Set([...cypherArgs.map((a) => a.name), "jwt", "viewer"]);
  for (const p of params) {
    if (p === "viewer" && !ctx.viewer) {
      at(
        "the statement uses $viewer, which needs a @viewer claim on the @jwt type",
      );
    } else if (!known.has(p)) {
      at(`the statement uses $${p}, which is neither an argument nor $jwt`);
    }
  }
  for (const a of cypherArgs) {
    if (!params.includes(a.name))
      warn(`argument ${a.name} is never used by the statement`);
  }

  const code = codeOnly(statement);
  if (ctx.root !== "Mutation" && WRITE_CLAUSES.test(code)) {
    at(
      ctx.root === "Query"
        ? "a Query field must not write; declare it on Mutation"
        : "an object field must not write",
    );
  }
  if (ctx.root === undefined && !/\bTHIS\b/.test(code)) {
    warn(
      "the statement never uses `this`, so it returns the same value for every parent",
    );
  }
  if (code.includes("OPTIONAL MATCH")) {
    warn(
      "OPTIONAL MATCH is slow on LoraDB; a pattern comprehension is usually 17-130x faster",
    );
  }

  // Opt-in filters and sorts: computed per row, so never through an index.
  let computed: ScalarField | undefined;
  if (
    directive(d("filterable"), f, at) !== undefined ||
    directive(d("sortable"), f, at) !== undefined
  ) {
    if (ctx.root !== undefined) {
      at("@filterable and @sortable on @cypher apply to @node type fields");
    } else if (node || abstract || object || shape.list) {
      at(
        "only scalar, non-list @cypher fields can be @filterable or @sortable",
      );
    } else if (cypherArgs.some((a) => a.defaultValue === undefined)) {
      at(
        "a @filterable or @sortable @cypher field needs a default for every argument",
      );
    } else {
      computed = buildScalarField(
        { name: owner } as GraphQLObjectType,
        f,
        d,
        problems,
        { allowKey: false },
      );
      warn(
        `filtering or sorting by ${f.name} runs its statement for every ${owner} considered: no index applies`,
      );
    }
  }

  const field: CypherField = {
    kind: "cypher",
    authorization,
    name: f.name,
    owner,
    statement,
    columnName,
    type: { named, ...shapeOf(shape) },
    node,
    abstract,
    object,
    args: cypherArgs,
    params,
    computed,
    authentication: authOps(directive(d("authentication"), f, at)),
    authenticationJwt: directive(d("authentication"), f, at)?.["jwt"] as
      | AuthorizationWhere
      | undefined,
    description: f.description ?? undefined,
  };
  if (computed) computed.computedBy = field;
  return field;
}

/** The column of a statement ending in `RETURN x` or `RETURN … AS x`. */
/**
 * The column a statement returns, when it returns exactly one: the alias
 * of the single item of its last top-level RETURN (or the item, when it
 * is a bare variable). Brackets, strings and comments are respected, so
 * `RETURN coalesce(a, b) AS n` and a RETURN inside `CALL { }` do not
 * confuse it.
 */
/**
 * An SDL argument's default, as a value. graphql 16 keeps it on
 * `defaultValue`; graphql 17 builds `default: { literal }` from SDL and
 * leaves `defaultValue` undefined, so reading only `defaultValue` drops
 * `peers(limit: Int = 2)` to `peers(limit: Int)` there.
 */
/** `@range(min:, max:)` of a @cypher argument, checked against its type. */
function cypherArgumentRange(
  a: GraphQLArgument,
  named: string,
  def: GraphQLDirective,
  at: (message: string) => void,
): { min?: number; max?: number } | undefined {
  const args = directive(def, a, at);
  if (!args) return undefined;
  const min = args["min"] as number | null | undefined;
  const max = args["max"] as number | null | undefined;
  if (named !== "Int" && named !== "Float") {
    at(`argument ${a.name}: @range applies to Int and Float arguments`);
    return undefined;
  }
  if (min == null && max == null) {
    at(`argument ${a.name}: @range needs min, max or both`);
    return undefined;
  }
  if (min != null && max != null && min > max) {
    at(`argument ${a.name}: @range(min:) is greater than max`);
    return undefined;
  }
  return {
    ...(min != null ? { min } : {}),
    ...(max != null ? { max } : {}),
  };
}

function argumentDefault(a: GraphQLArgument): unknown {
  if (a.defaultValue !== undefined) return a.defaultValue;
  const d = (a as { default?: { value?: unknown; literal?: ConstValueNode } })
    .default;
  if (!d) return undefined;
  if ("value" in d) return d.value;
  return d.literal ? valueFromAST(d.literal, a.type) : undefined;
}

function inferColumn(statement: string): string | undefined {
  const code = maskLiterals(statement);
  const depths: number[] = [];
  let depth = 0;
  for (const c of code) {
    if (c === ")" || c === "]" || c === "}") depth--;
    depths.push(depth);
    if (c === "(" || c === "[" || c === "{") depth++;
  }
  const topLevel = (re: RegExp, from = 0) =>
    [...code.slice(from).matchAll(re)]
      .map((m) => ({ index: m.index! + from, length: m[0].length }))
      .filter((m) => depths[m.index] === 0);
  const last = topLevel(/\bRETURN\b/gi).at(-1);
  if (!last) return undefined;
  const start = last.index + last.length;
  const stop =
    topLevel(/\b(ORDER\s+BY|SKIP|LIMIT|UNION)\b/gi, start)[0]?.index ??
    code.length;
  const commas = topLevel(/,/g, start).filter((m) => m.index < stop);
  if (commas.length > 0) return undefined;
  const item = code.slice(start, stop).replace(/^\s*DISTINCT\b/i, "");
  const offset = stop - item.length;
  const alias = /\bAS\s+(`[^`]*`|[A-Za-z_][A-Za-z0-9_]*)\s*$/i.exec(item);
  if (alias) {
    const at = offset + alias.index + alias[0].indexOf(alias[1]!);
    const name = statement.slice(at, at + alias[1]!.length);
    return name.startsWith("`") ? name.slice(1, -1) : name;
  }
  const bare = item.trim();
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(bare) ? bare : undefined;
}
