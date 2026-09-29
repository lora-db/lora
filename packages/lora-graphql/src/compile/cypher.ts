// A small typed Cypher AST and its printer. The compiler never
// concatenates Cypher text: it builds these nodes and prints them, so
// identifiers are always escaped and values are always parameters.

export type BinaryOp =
  | "="
  | "<>"
  | "<"
  | "<="
  | ">"
  | ">="
  | "AND"
  | "OR"
  | "IN"
  | "CONTAINS"
  | "STARTS WITH"
  | "ENDS WITH";

export type Expr =
  | { kind: "var"; name: string }
  | { kind: "param"; name: string }
  | { kind: "literal"; value: null | boolean | number | string }
  | { kind: "prop"; target: Expr; key: string }
  | { kind: "binary"; op: BinaryOp; left: Expr; right: Expr }
  | { kind: "not"; expr: Expr }
  | { kind: "isNull"; expr: Expr; negated: boolean }
  | { kind: "call"; fn: string; args: Expr[] }
  | { kind: "list"; items: Expr[] }
  | { kind: "map"; entries: MapEntry[] }
  | { kind: "mapProjection"; variable: string; entries: ProjectionEntry[] }
  | {
      kind: "comprehension";
      pattern: Pattern;
      where: Expr | undefined;
      projection: Expr;
    }
  | {
      kind: "slice";
      target: Expr;
      from: Expr | undefined;
      to: Expr | undefined;
    }
  | { kind: "case"; when: Expr; then: Expr; else: Expr };

export interface MapEntry {
  key: string;
  value: Expr;
}

export type ProjectionEntry =
  | { kind: "property"; key: string }
  | { kind: "entry"; key: string; value: Expr };

export interface NodePattern {
  variable?: string;
  labels: string[];
}

export interface RelPattern {
  variable?: string;
  type: string;
  direction: "OUT" | "IN";
}

/** A linear pattern: node (rel node)*. */
export interface Pattern {
  start: NodePattern;
  hops: Array<{ rel: RelPattern; node: NodePattern }>;
}

export interface SortItem {
  expr: Expr;
  direction: "ASC" | "DESC";
}

export interface ReturnItem {
  expr: Expr;
  alias?: string;
}

export type Clause =
  | { kind: "match"; pattern: Pattern; where: Expr | undefined }
  | { kind: "unwind"; expr: Expr; alias: string }
  /** `CALL proc(args) YIELD item AS alias, … [WHERE …]` */
  | {
      kind: "procedure";
      procedure: string;
      args: Expr[];
      yields: Array<{ item: string; alias?: string }>;
      where?: Expr | undefined;
    }
  /** A hand-written statement (`@cypher`), parameters already renamed. */
  | { kind: "raw"; text: string }
  | {
      kind: "with";
      distinct?: boolean;
      items: ReturnItem[];
      where?: Expr | undefined;
      orderBy?: SortItem[] | undefined;
      limit?: Expr | undefined;
    }
  | { kind: "call"; imports: string[]; body: Clause[] }
  | {
      kind: "return";
      items: ReturnItem[];
      orderBy?: SortItem[] | undefined;
      limit?: Expr | undefined;
    };

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

export const v = (name: string): Expr => ({ kind: "var", name });
export const param = (name: string): Expr => ({ kind: "param", name });
export const lit = (value: null | boolean | number | string): Expr => ({
  kind: "literal",
  value,
});
export const prop = (target: Expr, key: string): Expr => ({
  kind: "prop",
  target,
  key,
});
export const bin = (op: BinaryOp, left: Expr, right: Expr): Expr => ({
  kind: "binary",
  op,
  left,
  right,
});
export const not = (expr: Expr): Expr => ({ kind: "not", expr });
export const isNull = (expr: Expr, negated = false): Expr => ({
  kind: "isNull",
  expr,
  negated,
});
export const fn = (name: string, ...args: Expr[]): Expr => ({
  kind: "call",
  fn: name,
  args,
});

/** AND of every defined operand; undefined when there are none. */
export function and(...operands: Array<Expr | undefined>): Expr | undefined {
  return fold("AND", operands);
}

/** OR of every defined operand; undefined when there are none. */
export function or(...operands: Array<Expr | undefined>): Expr | undefined {
  return fold("OR", operands);
}

function fold(
  op: "AND" | "OR",
  operands: Array<Expr | undefined>,
): Expr | undefined {
  const present = operands.filter((e): e is Expr => e !== undefined);
  if (present.length === 0) return undefined;
  return present.reduce((acc, e) => bin(op, acc, e));
}

// ---------------------------------------------------------------------------
// Printer
// ---------------------------------------------------------------------------

const SIMPLE_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

// Words that may not appear bare where the grammar expects a name.
const RESERVED = new Set(
  [
    "all",
    "and",
    "any",
    "as",
    "asc",
    "ascending",
    "by",
    "call",
    "case",
    "contains",
    "count",
    "create",
    "delete",
    "desc",
    "descending",
    "detach",
    "distinct",
    "else",
    "end",
    "ends",
    "exists",
    "false",
    "in",
    "is",
    "limit",
    "match",
    "merge",
    "none",
    "not",
    "null",
    "on",
    "optional",
    "or",
    "order",
    "remove",
    "return",
    "set",
    "single",
    "skip",
    "starts",
    "then",
    "true",
    "union",
    "unwind",
    "when",
    "where",
    "with",
    "xor",
    "yield",
  ].map((w) => w.toLowerCase()),
);

/** Escape an identifier (variable, label, type, property or map key). */
export function name(id: string): string {
  if (SIMPLE_NAME.test(id) && !RESERVED.has(id.toLowerCase())) return id;
  return "`" + id.replace(/`/g, "``") + "`";
}

export function printExpr(e: Expr): string {
  switch (e.kind) {
    case "var":
      return name(e.name);
    case "param":
      return "$" + name(e.name);
    case "literal":
      if (e.value === null) return "null";
      if (typeof e.value === "string") return JSON.stringify(e.value);
      return String(e.value);
    case "prop":
      return `${printOperand(e.target)}.${name(e.key)}`;
    case "binary":
      return `${printOperand(e.left, e.op)} ${e.op} ${printOperand(e.right, e.op)}`;
    case "not":
      return `NOT ${precedence(e.expr) <= NOT_PRECEDENCE || e.expr.kind === "binary" ? `(${printExpr(e.expr)})` : printExpr(e.expr)}`;
    case "isNull":
      return `${printOperand(e.expr)} IS ${e.negated ? "NOT " : ""}NULL`;
    case "call":
      return `${e.fn}(${e.args.map(printExpr).join(", ")})`;
    case "list":
      return `[${e.items.map(printExpr).join(", ")}]`;
    case "map":
      return `{ ${e.entries.map((m) => `${name(m.key)}: ${printExpr(m.value)}`).join(", ")} }`;
    case "mapProjection": {
      const entries = e.entries.map((p) =>
        p.kind === "property"
          ? `.${name(p.key)}`
          : `${name(p.key)}: ${printExpr(p.value)}`,
      );
      return entries.length === 0
        ? `${name(e.variable)} {}`
        : `${name(e.variable)} { ${entries.join(", ")} }`;
    }
    case "comprehension": {
      const where = e.where ? ` WHERE ${printExpr(e.where)}` : "";
      return `[${printPattern(e.pattern)}${where} | ${printExpr(e.projection)}]`;
    }
    case "case":
      return `CASE WHEN ${printExpr(e.when)} THEN ${printExpr(e.then)} ELSE ${printExpr(e.else)} END`;
    case "slice":
      return `${printOperand(e.target)}[${e.from ? printExpr(e.from) : ""}..${e.to ? printExpr(e.to) : ""}]`;
  }
}

// Operator precedence, loosest first. An operand is parenthesised only
// when it binds looser than its parent, or equally loose under a
// non-associative parent (comparisons).
const PRECEDENCE: Record<BinaryOp, number> = {
  OR: 1,
  AND: 2,
  "=": 4,
  "<>": 4,
  "<": 4,
  "<=": 4,
  ">": 4,
  ">=": 4,
  IN: 4,
  CONTAINS: 4,
  "STARTS WITH": 4,
  "ENDS WITH": 4,
};
const NOT_PRECEDENCE = 3;

function precedence(e: Expr): number {
  if (e.kind === "binary") return PRECEDENCE[e.op];
  if (e.kind === "not") return NOT_PRECEDENCE;
  if (e.kind === "isNull") return 4;
  return 10;
}

function printOperand(e: Expr, parentOp?: BinaryOp): string {
  const s = printExpr(e);
  const parent = parentOp ? PRECEDENCE[parentOp] : 10;
  const own = precedence(e);
  // Bracket AND under OR anyway: correct either way, easier to read.
  if (parentOp === "OR" && e.kind === "binary" && e.op === "AND") {
    return `(${s})`;
  }
  if (own > parent || own === 10) return s;
  if (own === parent && parentOp && (parentOp === "AND" || parentOp === "OR")) {
    return s;
  }
  return `(${s})`;
}

function printNode(n: NodePattern): string {
  const labels = n.labels.map((l) => ":" + name(l)).join("");
  return `(${n.variable ? name(n.variable) : ""}${labels})`;
}

export function printPattern(p: Pattern): string {
  let s = printNode(p.start);
  for (const { rel, node } of p.hops) {
    const inner = `[${rel.variable ? name(rel.variable) : ""}:${name(rel.type)}]`;
    s += rel.direction === "OUT" ? `-${inner}->` : `<-${inner}-`;
    s += printNode(node);
  }
  return s;
}

function printItems(items: ReturnItem[]): string {
  return items
    .map((i) => {
      const expr = printExpr(i.expr);
      if (i.alias === undefined) return expr;
      if (i.expr.kind === "var" && i.expr.name === i.alias) return expr;
      return `${expr} AS ${name(i.alias)}`;
    })
    .join(", ");
}

function printTail(
  orderBy: SortItem[] | undefined,
  limit: Expr | undefined,
): string {
  let s = "";
  if (orderBy && orderBy.length > 0) {
    s +=
      " ORDER BY " +
      orderBy.map((o) => `${printExpr(o.expr)} ${o.direction}`).join(", ");
  }
  if (limit) s += ` LIMIT ${printExpr(limit)}`;
  return s;
}

export function printClauses(clauses: Clause[], indent = ""): string {
  return clauses.map((c) => indent + printClause(c, indent)).join("\n");
}

function printClause(c: Clause, indent: string): string {
  switch (c.kind) {
    case "match":
      return (
        `MATCH ${printPattern(c.pattern)}` +
        (c.where ? `\n${indent}WHERE ${printExpr(c.where)}` : "")
      );
    case "unwind":
      return `UNWIND ${printExpr(c.expr)} AS ${name(c.alias)}`;
    case "procedure": {
      const yields = c.yields
        .map((y) =>
          y.alias && y.alias !== y.item
            ? `${name(y.item)} AS ${name(y.alias)}`
            : name(y.item),
        )
        .join(", ");
      return (
        `CALL ${c.procedure}(${c.args.map(printExpr).join(", ")}) YIELD ${yields}` +
        (c.where ? `\n${indent}WHERE ${printExpr(c.where)}` : "")
      );
    }
    case "raw":
      return c.text
        .trim()
        .split("\n")
        .map((line, i) => (i === 0 ? line : indent + line.trimEnd()))
        .join("\n");
    case "with":
      return (
        `WITH ${c.distinct ? "DISTINCT " : ""}${printItems(c.items)}` +
        (c.where ? ` WHERE ${printExpr(c.where)}` : "") +
        printTail(c.orderBy, c.limit)
      );
    case "call": {
      const inner = indent + "  ";
      const imports =
        c.imports.length > 0
          ? `${inner}WITH ${c.imports.map(name).join(", ")}\n`
          : "";
      return `CALL {\n${imports}${printClauses(c.body, inner)}\n${indent}}`;
    }
    case "return":
      return `RETURN ${printItems(c.items)}` + printTail(c.orderBy, c.limit);
  }
}
