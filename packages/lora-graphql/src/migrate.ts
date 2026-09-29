// `lora-graphql migrate neo4j`: rewrite an @neo4j/graphql SDL into this
// package's vocabulary. What maps is rewritten; what does not is removed
// and listed as a TODO. With the client's operations, the opt-in surface
// (@mutation, @filterable, @sortable) follows observed usage; without
// them, mutations stay on for every type (as in @neo4j/graphql).

import {
  Kind,
  parse,
  print,
  visit,
  type ArgumentNode,
  type ConstArgumentNode,
  type ConstDirectiveNode,
  type ConstValueNode,
  type DocumentNode,
  type FieldDefinitionNode,
  type ObjectTypeDefinitionNode,
  type SelectionSetNode,
  type ValueNode,
} from "graphql";
import { defaultPlural } from "./model/build.js";

export interface MigrationResult {
  /** The rewritten SDL. */
  typeDefs: string;
  /** What has no equivalent or needs a decision, by type and field. */
  todos: string[];
}

const ROOT = new Set(["Query", "Mutation", "Subscription"]);

/** Removed with a TODO: no equivalent here. */
const UNSUPPORTED: Record<string, string> = {
  coalesce:
    "@coalesce is not supported: it hides predicates from indexes; store a value instead",
  subscriptionsAuthorization:
    "@subscriptionsAuthorization is not supported: SUBSCRIBE rules go in @authorization",
  shareable: "federation (@shareable) is not supported",
  external: "federation (@external) is not supported",
  requires: "federation (@requires) is not supported",
  provides: "federation (@provides) is not supported",
  exclude: "@exclude: use @query / @mutation to choose operations",
  vector:
    "@vector on types (with a provider) is not supported: put @vector(dimensions:, similarity:) on a [Float!] field",
};

/** neo4j 5 filter suffixes → @filterable operators. */
const SUFFIX_OPS: Array<[string, string]> = [
  ["_NOT_IN", "IN"],
  ["_STARTS_WITH", "STARTS_WITH"],
  ["_ENDS_WITH", "ENDS_WITH"],
  ["_CONTAINS", "CONTAINS"],
  ["_INCLUDES", "INCLUDES"],
  ["_GTE", "GTE"],
  ["_LTE", "LTE"],
  ["_GT", "GT"],
  ["_LT", "LT"],
  ["_IN", "IN"],
  ["_EQ", "EQ"],
];

/** neo4j 6 nested filter operators → @filterable operators. */
const NESTED_OPS: Record<string, string> = {
  eq: "EQ",
  in: "IN",
  lt: "LT",
  lte: "LTE",
  gt: "GT",
  gte: "GTE",
  contains: "CONTAINS",
  startsWith: "STARTS_WITH",
  endsWith: "ENDS_WITH",
  includes: "INCLUDES",
  caseInsensitive: "CASE_INSENSITIVE",
};

interface Usage {
  filters: Map<string, Map<string, Set<string>>>;
  sorts: Map<string, Set<string>>;
  mutations: Map<string, Set<string>>;
}

export function migrateNeo4j(
  sdl: string,
  operations: DocumentNode[] = [],
): MigrationResult {
  const doc = parse(sdl, { noLocation: true });
  const todos: string[] = [];
  const nodeTypes = doc.definitions.filter(
    (d): d is ObjectTypeDefinitionNode =>
      d.kind === Kind.OBJECT_TYPE_DEFINITION &&
      !ROOT.has(d.name.value) &&
      !has(d.directives, "relationshipProperties") &&
      !has(d.directives, "jwt"),
  );
  const nodeNames = new Set(nodeTypes.map((t) => t.name.value));
  const usage =
    operations.length > 0 ? observe(operations, nodeTypes) : undefined;

  const out = visit(doc, {
    ObjectTypeDefinition(t) {
      if (!nodeNames.has(t.name.value)) return undefined;
      const type = t.name.value;
      let directives = migrateDirectives(
        t.directives ?? [],
        type,
        undefined,
        todos,
      );
      if (!has(directives, "node"))
        directives = [...directives, directive("node")];
      const ops = usage?.mutations.get(type);
      if (!has(directives, "mutation")) {
        if (!usage) {
          directives = [...directives, directive("mutation")];
          todos.push(
            `${type}: @mutation added (@neo4j/graphql generates mutations for every type); remove it or narrow its operations`,
          );
        } else if (ops && ops.size > 0) {
          directives = [
            ...directives,
            directive("mutation", {
              operations: enumList([...ops].sort()),
            }),
          ];
        }
      }
      const idFields = (t.fields ?? []).filter((f) => has(f.directives, "id"));
      if (idFields.length === 0) {
        todos.push(
          `${type}: no @id field; add a required @key field (the sort tie-breaker and mutation address)`,
        );
      } else if (idFields.length > 1) {
        todos.push(`${type}: several @id fields; keep one as the @key`);
      }
      const fields = (t.fields ?? []).map((f) =>
        migrateField(f, type, usage, todos),
      );
      return { ...t, directives, fields };
    },
  });
  return { typeDefs: print(out), todos };
}

function migrateField(
  f: FieldDefinitionNode,
  type: string,
  usage: Usage | undefined,
  todos: string[],
): FieldDefinitionNode {
  let directives = migrateDirectives(
    f.directives ?? [],
    type,
    f.name.value,
    todos,
  );
  const id = directives.find((d) => d.name.value === "id");
  if (id) {
    directives = directives.filter(
      (d) => d.name.value !== "id" && d.name.value !== "unique",
    );
    directives.push(directive("key", { generate: bool(true) }));
  }
  const used = usage?.filters.get(type)?.get(f.name.value);
  if (used && used.size > 0 && !has(directives, "filterable")) {
    directives.push(
      has(directives, "relationship")
        ? directive("filterable")
        : directive("filterable", { byValue: enumList([...used].sort()) }),
    );
  }
  if (
    usage?.sorts.get(type)?.has(f.name.value) &&
    !has(directives, "sortable")
  ) {
    directives.push(directive("sortable"));
  }
  return { ...f, directives };
}

function migrateDirectives(
  directives: readonly ConstDirectiveNode[],
  type: string,
  field: string | undefined,
  todos: string[],
): ConstDirectiveNode[] {
  const at = field ? `${type}.${field}` : type;
  const out: ConstDirectiveNode[] = [];
  for (const d of directives) {
    const name = d.name.value;
    if (name in UNSUPPORTED) {
      todos.push(`${at}: ${UNSUPPORTED[name]}`);
      continue;
    }
    if (name === "key" && !field) {
      todos.push(`${at}: federation @key(fields:) is not supported`);
      continue;
    }
    if (name === "unique") {
      out.push(directive("unique"));
      continue;
    }
    if (name === "relationship") {
      out.push(migrateRelationship(d, at, todos));
      continue;
    }
    if (name === "fulltext") {
      out.push(renameInList(d, "indexes", "indexName", "name"));
      continue;
    }
    if (name === "subscription") {
      out.push(migrateSubscription(d, at, todos));
      continue;
    }
    if (name === "authorization" || name === "authentication") {
      todos.push(
        `${at}: check the @${name} rule's filters: operators are written { field: { eq: ... } } here`,
      );
    }
    out.push(d);
  }
  return out;
}

function migrateRelationship(
  d: ConstDirectiveNode,
  at: string,
  todos: string[],
): ConstDirectiveNode {
  const args: ConstArgumentNode[] = [];
  for (const a of d.arguments ?? []) {
    const name = a.name.value;
    if (name === "queryDirection" && a.value.kind === Kind.ENUM) {
      if (a.value.value.includes("UNDIRECTED")) {
        args.push(arg("queryDirection", enumValue("UNDIRECTED")));
      }
      continue;
    }
    if (name === "nestedOperations" && a.value.kind === Kind.LIST) {
      const kept = a.value.values.filter(
        (v) => v.kind === Kind.ENUM && v.value !== "CONNECT_OR_CREATE",
      );
      if (kept.length < a.value.values.length) {
        todos.push(`${at}: connectOrCreate is not supported; use upsert`);
      }
      args.push(arg("nestedOperations", { kind: Kind.LIST, values: kept }));
      continue;
    }
    args.push(a);
  }
  return { ...d, arguments: args };
}

function migrateSubscription(
  d: ConstDirectiveNode,
  at: string,
  todos: string[],
): ConstDirectiveNode {
  const events = d.arguments?.find((a) => a.name.value === "events")?.value;
  if (!events || events.kind !== Kind.LIST) return d;
  const ops = new Set<string>();
  let relationships = false;
  for (const e of events.values) {
    if (e.kind !== Kind.ENUM) continue;
    if (e.value.startsWith("CREATED") && !e.value.includes("RELATIONSHIP"))
      ops.add("CREATE");
    else if (e.value.startsWith("UPDATED")) ops.add("UPDATE");
    else if (e.value.startsWith("DELETED") && !e.value.includes("RELATIONSHIP"))
      ops.add("DELETE");
    else if (e.value.includes("RELATIONSHIP")) relationships = true;
  }
  todos.push(
    `${at}: subscriptions see writes made through this library (or the engine change feed), not CDC`,
  );
  return directive("subscription", {
    operations: enumList([...ops].sort()),
    ...(relationships ? { relationships: bool(true) } : {}),
  });
}

/** What the client's operations use: filters, sorts and mutations per type. */
function observe(
  operations: DocumentNode[],
  types: ObjectTypeDefinitionNode[],
): Usage {
  const usage: Usage = {
    filters: new Map(),
    sorts: new Map(),
    mutations: new Map(),
  };
  // Root fields as either naming would call them: this package's plural
  // and @neo4j/graphql's (which knows irregular plurals, e.g. people).
  const byPlural = new Map<string, ObjectTypeDefinitionNode>();
  for (const t of types) {
    byPlural.set(defaultPlural(t.name.value), t);
    byPlural.set(neo4jPlural(t.name.value), t);
  }
  const byName = new Map(types.map((t) => [t.name.value, t]));
  const note = (type: string, field: string, op: string) => {
    const fields = usage.filters.get(type) ?? new Map<string, Set<string>>();
    const ops = fields.get(field) ?? new Set<string>();
    ops.add(op);
    fields.set(field, ops);
    usage.filters.set(type, fields);
  };
  const where = (type: ObjectTypeDefinitionNode, value: ValueNode) => {
    if (value.kind !== Kind.OBJECT) return;
    for (const f of value.fields) {
      const key = f.name.value;
      if (key === "AND" || key === "OR") {
        if (f.value.kind === Kind.LIST)
          for (const v of f.value.values) where(type, v);
        continue;
      }
      if (key === "NOT") {
        where(type, f.value);
        continue;
      }
      // neo4j 5 relationship quantifiers: `actors_SOME: { name_EQ: ... }`.
      const quantifier = /^(.+)_(SOME|ALL|NONE|SINGLE)$/.exec(key);
      if (quantifier) {
        const rel = type.fields?.find((x) => x.name.value === quantifier[1]);
        const target = rel && byName.get(namedTypeOf(rel));
        if (rel && target) {
          note(type.name.value, quantifier[1]!, "EQ");
          where(target, f.value);
        }
        continue;
      }
      const suffix = SUFFIX_OPS.find(([s]) => key.endsWith(s));
      const fieldName = suffix ? key.slice(0, -suffix[0].length) : key;
      const def = type.fields?.find((x) => x.name.value === fieldName);
      if (!def) continue;
      const target = byName.get(namedTypeOf(def));
      if (target) {
        note(type.name.value, fieldName, "EQ");
        // `some` / `all` / `none` / `single`, or a nested where.
        if (f.value.kind === Kind.OBJECT) {
          for (const inner of f.value.fields) where(target, inner.value);
          where(target, f.value);
        }
        continue;
      }
      if (suffix) note(type.name.value, fieldName, suffix[1]);
      else if (f.value.kind === Kind.OBJECT) {
        for (const op of f.value.fields) {
          const mapped = NESTED_OPS[op.name.value];
          if (mapped) note(type.name.value, fieldName, mapped);
        }
      } else note(type.name.value, fieldName, "EQ");
    }
  };
  const walk = (set: SelectionSetNode | undefined, root: boolean) => {
    for (const sel of set?.selections ?? []) {
      if (sel.kind !== Kind.FIELD) continue;
      const name = sel.name.value;
      if (root) {
        const mutation = /^(create|update|delete|upsert)(.+)$/.exec(name);
        const plural = name.replace(/(Connection|Aggregate)$/, "");
        const type =
          byPlural.get(plural) ??
          (mutation ? byPlural.get(lowerFirst(mutation[2]!)) : undefined);
        if (type && mutation) {
          const ops = usage.mutations.get(type.name.value) ?? new Set<string>();
          const op = mutation[1]!.toUpperCase();
          if (op === "UPSERT") {
            ops.add("CREATE");
            ops.add("UPDATE");
          } else ops.add(op);
          usage.mutations.set(type.name.value, ops);
        }
        if (type) {
          for (const a of sel.arguments ?? []) argumentUse(type, a);
        }
      }
      walk(sel.selectionSet, false);
    }
  };
  const argumentUse = (type: ObjectTypeDefinitionNode, a: ArgumentNode) => {
    if (a.name.value === "where") where(type, a.value);
    if (a.name.value === "sort" || a.name.value === "options") {
      const sorts = usage.sorts.get(type.name.value) ?? new Set<string>();
      const collect = (v: ValueNode) => {
        if (v.kind === Kind.LIST) v.values.forEach(collect);
        else if (v.kind === Kind.OBJECT) {
          for (const f of v.fields) {
            if (f.name.value === "sort") collect(f.value);
            else if (f.value.kind === Kind.ENUM) sorts.add(f.name.value);
          }
        }
      };
      collect(a.value);
      usage.sorts.set(type.name.value, sorts);
    }
  };
  for (const doc of operations) {
    for (const def of doc.definitions) {
      if (def.kind === Kind.OPERATION_DEFINITION) walk(def.selectionSet, true);
    }
  }
  return usage;
}

function namedTypeOf(f: FieldDefinitionNode): string {
  let t = f.type;
  while (t.kind !== Kind.NAMED_TYPE) t = t.type;
  return t.name.value;
}

function has(
  directives: readonly ConstDirectiveNode[] | undefined,
  name: string,
): boolean {
  return (directives ?? []).some((d) => d.name.value === name);
}

function directive(
  name: string,
  args: Record<string, ConstValueNode> = {},
): ConstDirectiveNode {
  return {
    kind: Kind.DIRECTIVE,
    name: { kind: Kind.NAME, value: name },
    arguments: Object.entries(args).map(([k, v]) => arg(k, v)),
  };
}

function arg(name: string, value: ConstValueNode): ConstArgumentNode {
  return { kind: Kind.ARGUMENT, name: { kind: Kind.NAME, value: name }, value };
}

const enumValue = (value: string): ConstValueNode => ({
  kind: Kind.ENUM,
  value,
});
const bool = (value: boolean): ConstValueNode => ({
  kind: Kind.BOOLEAN,
  value,
});
const enumList = (values: string[]): ConstValueNode => ({
  kind: Kind.LIST,
  values: values.map(enumValue),
});

/** Rename a key inside each object of a list argument. */
function renameInList(
  d: ConstDirectiveNode,
  argName: string,
  from: string,
  to: string,
): ConstDirectiveNode {
  return {
    ...d,
    arguments: (d.arguments ?? []).map((a) =>
      a.name.value === argName && a.value.kind === Kind.LIST
        ? arg(argName, {
            kind: Kind.LIST,
            values: a.value.values.map((v) =>
              v.kind === Kind.OBJECT
                ? {
                    ...v,
                    fields: v.fields.map((f) =>
                      f.name.value === from
                        ? { ...f, name: { kind: Kind.NAME, value: to } }
                        : f,
                    ),
                  }
                : v,
            ),
          })
        : a,
    ),
  };
}

const IRREGULAR: Record<string, string> = {
  person: "people",
  man: "men",
  woman: "women",
  child: "children",
  mouse: "mice",
  goose: "geese",
  foot: "feet",
  tooth: "teeth",
};

function neo4jPlural(typeName: string): string {
  const base = lowerFirst(typeName);
  const lower = base.toLowerCase();
  for (const [one, many] of Object.entries(IRREGULAR)) {
    if (lower.endsWith(one))
      return base.slice(0, base.length - one.length) + many;
  }
  return defaultPlural(typeName);
}

function lowerFirst(s: string): string {
  return s.charAt(0).toLowerCase() + s.slice(1);
}
