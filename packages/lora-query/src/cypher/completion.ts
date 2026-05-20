import {
  snippet,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import {
  CYPHER_ALIASES,
  CYPHER_CLAUSES,
  CYPHER_CONSTANTS,
  CYPHER_KEYWORDS,
  CYPHER_NAMESPACES,
  CYPHER_TOP_LEVEL_FUNCTIONS,
  type CypherAlias,
  NAMESPACE_MEMBERS,
  type CypherToken,
} from "./data";
import type { EditorView } from "@codemirror/view";
import { findVariable, getOutline } from "./scope";
import { getProviders, type PropertyContext } from "./providers";
import type { Outline, OutlineVariable } from "../parser";

function aliasToCompletion(a: CypherAlias): Completion {
  return {
    label: a.alias,
    type: "function",
    detail: `→ ${a.canonical}`,
    info: `Compatibility alias for \`${a.canonical}\`.`,
    apply: snippet(`${a.alias}(\${1})`),
    boost: -1,
  };
}

function toCompletion(
  t: CypherToken,
  opts: { boost?: number; apply?: string | ((view: EditorView, completion: Completion, from: number, to: number) => void) } = {},
): Completion {
  const base: Completion = {
    label: t.label,
    type: t.kind,
    info: t.info,
  };
  if (t.detail !== undefined) base.detail = t.detail;
  if (opts.boost !== undefined) base.boost = opts.boost;
  if (opts.apply !== undefined) base.apply = opts.apply;
  // Functions get a snippet-style apply so the cursor lands inside
  // the parens, ready for the user to type the first arg.
  if (t.kind === "function" && opts.apply === undefined) {
    base.apply = snippet(`${t.label}(\${1})`);
  }
  return base;
}

/**
 * Walk through `AS` aliases to find the true source of a variable. If
 * `x` was introduced by `WITH n AS x`, this returns the OutlineVariable
 * for `n` (with its label / kind intact) instead of `x`. Caps recursion
 * to avoid pathological cycles.
 */
function resolveVariable(
  name: string,
  outline: Outline,
): OutlineVariable | null {
  const seen = new Set<string>();
  let current = findVariable(outline, name);
  while (current && current.aliasOf && !seen.has(current.aliasOf)) {
    seen.add(current.aliasOf);
    const next = findVariable(outline, current.aliasOf);
    if (!next) return current;
    current = next;
  }
  return current;
}

function variableCompletion(v: OutlineVariable): Completion {
  return {
    label: v.name,
    type: "variable",
    detail: v.label ? `:${v.label}` : "variable",
    info: v.label
      ? `Bound earlier in this query (label \`${v.label}\`).`
      : "Variable bound earlier in this query.",
    boost: 6,
  };
}

/** Common Cypher clauses promoted to snippet completions with tab stops. */
const SNIPPETS: Array<{ keyword: string; template: string; info: string }> = [
  {
    keyword: "MATCH",
    template: "MATCH (${1:n})\nRETURN ${1:n}",
    info: "Read a node from the graph and return it.",
  },
  {
    keyword: "MATCH ()-[]->()",
    template: "MATCH (${1:a})-[${2:r}:${3:KNOWS}]->(${4:b})\nRETURN ${1:a}, ${4:b}",
    info: "Read a relationship pattern.",
  },
  {
    keyword: "OPTIONAL MATCH",
    template: "OPTIONAL MATCH (${1:n})-[${2:r}]->(${3:m})",
    info: "Pattern that returns NULL when no match instead of dropping the row.",
  },
  {
    keyword: "MATCH path = (a)-[*]->(b)",
    template:
      "MATCH ${1:p} = (${2:a})-[${3:*1..3}]->(${4:b})\nRETURN ${1:p}",
    info: "Bind a whole path to a variable and return it.",
  },
  {
    keyword: "WHERE n.x IS NULL",
    template: "WHERE ${1:n}.${2:property} IS NULL",
    info: "Filter rows where a property is unset.",
  },
  {
    keyword: "WHERE n.x IS NOT NULL",
    template: "WHERE ${1:n}.${2:property} IS NOT NULL",
    info: "Filter rows where a property is present.",
  },
  {
    keyword: "WHERE n.x IN […]",
    template: "WHERE ${1:n}.${2:property} IN [${3:'a', 'b'}]",
    info: "Membership test against a list literal.",
  },
  {
    keyword: "WHERE NOT EXISTS pattern",
    template:
      "WHERE NOT EXISTS { (${1:n})-[:${2:KNOWS}]->(${3:m}) }",
    info: "Filter out rows where the inner pattern matches.",
  },
  {
    keyword: "CREATE",
    template: "CREATE (${1:n}:${2:Label} {${3:name}: ${4:'value'}})",
    info: "Create a single node.",
  },
  {
    keyword: "MERGE",
    template:
      "MERGE (${1:n}:${2:Label} {${3:key}: ${4:'value'}})\nON CREATE SET ${1:n}.createdAt = ${5:timestamp()}",
    info: "Upsert a node.",
  },
  {
    keyword: "WITH",
    template: "WITH ${1:n}, count(*) AS ${2:total}",
    info: "Pipe rows into the next clause with an aggregation.",
  },
  {
    keyword: "UNWIND",
    template: "UNWIND ${1:list} AS ${2:item}",
    info: "Turn a list into one row per item.",
  },
  {
    keyword: "CASE",
    template:
      "CASE ${1:expr}\n  WHEN ${2:value} THEN ${3:result}\n  ELSE ${4:other}\nEND",
    info: "Conditional expression.",
  },
  {
    keyword: "CALL { … }",
    template: "CALL {\n  ${1:MATCH (n) RETURN n LIMIT 10}\n}",
    info: "Subquery that returns rows.",
  },
  {
    keyword: "EXISTS { … }",
    template: "EXISTS { (${1:n})-[:${2:KNOWS}]->(${3:m}) }",
    info: "True if the inner pattern matches at least once.",
  },
  // Recipes — common multi-clause templates the user can drop in
  {
    keyword: "Recipe: friends of friends",
    template:
      "MATCH (${1:me}:Person {name: ${2:'Alice'}})-[:KNOWS]->(friend)-[:KNOWS]->(fof)\nWHERE fof <> ${1:me} AND NOT (${1:me})-[:KNOWS]->(fof)\nRETURN DISTINCT fof.name AS suggestion\nLIMIT 10",
    info: "Suggest second-degree connections via :KNOWS edges.",
  },
  {
    keyword: "Recipe: paginated list",
    template:
      "MATCH (${1:n}:${2:Label})\nWITH ${1:n}\nORDER BY ${1:n}.${3:createdAt} DESC\nSKIP ${4:0}\nLIMIT ${5:20}\nRETURN ${1:n}",
    info: "Paginated read.",
  },
  {
    keyword: "Recipe: top by aggregate",
    template:
      "MATCH (${1:n}:${2:Label})-[r:${3:LIKES}]->(${4:m})\nWITH ${4:m}, count(r) AS score\nORDER BY score DESC\nLIMIT ${5:10}\nRETURN ${4:m}, score",
    info: "Top results by aggregated relationship count.",
  },
  // Advanced expressions
  {
    keyword: "shortestPath((a)-[*]-(b))",
    template: "shortestPath((${1:a})-[${2:*}]-(${3:b}))",
    info: "Shortest path between two nodes.",
  },
  {
    keyword: "allShortestPaths((a)-[*]-(b))",
    template: "allShortestPaths((${1:a})-[${2:*}]-(${3:b}))",
    info: "All shortest paths between two nodes.",
  },
  {
    keyword: "List comprehension",
    template: "[${1:x} IN ${2:list} WHERE ${3:pred} | ${4:x.value}]",
    info: "Filter + project items of a list.",
  },
  {
    keyword: "Map projection",
    template: "${1:n} {.${2:name}, .${3:age}}",
    info: "Build a map from selected properties of a node / map.",
  },
  {
    keyword: "Reduce",
    template:
      "reduce(${1:acc} = ${2:0}, ${3:x} IN ${4:list} | ${1:acc} + ${3:x})",
    info: "Fold a list into a single value.",
  },
  // Temporal / spatial literals
  {
    keyword: "date('YYYY-MM-DD')",
    template: "date('${1:2026-01-01}')",
    info: "Date literal.",
  },
  {
    keyword: "datetime(...)",
    template: "datetime('${1:2026-01-01T00:00:00Z}')",
    info: "Datetime literal in ISO-8601.",
  },
  {
    keyword: "duration(P1D)",
    template: "duration('${1:P1D}')",
    info: "ISO-8601 duration literal.",
  },
  {
    keyword: "point({x, y})",
    template: "point({x: ${1:0}, y: ${2:0}})",
    info: "2D Cartesian point literal.",
  },
  {
    keyword: "point({lat, lon})",
    template: "point({latitude: ${1:0}, longitude: ${2:0}})",
    info: "Lat/lon geographic point literal.",
  },
  // Query analysis prefixes
  {
    keyword: "PROFILE",
    template: "PROFILE\n${1:MATCH (n) RETURN n}",
    info: "Run the query and return execution statistics.",
  },
  {
    keyword: "EXPLAIN",
    template: "EXPLAIN\n${1:MATCH (n) RETURN n}",
    info: "Show the query plan without running it.",
  },
  // DDL
  {
    keyword: "CREATE INDEX",
    template:
      "CREATE INDEX ${1:idx_name} FOR (${2:n}:${3:Label}) ON (${2:n}.${4:property})",
    info: "Add a single-property index.",
  },
  {
    keyword: "CREATE TEXT INDEX",
    template:
      "CREATE TEXT INDEX ${1:idx_name} FOR (${2:n}:${3:Label}) ON (${2:n}.${4:property})",
    info: "Add a text-search index.",
  },
  {
    keyword: "DROP INDEX",
    template: "DROP INDEX ${1:idx_name} IF EXISTS",
    info: "Drop an index by name.",
  },
  {
    keyword: "SHOW INDEXES",
    template: "SHOW INDEXES",
    info: "List every index in the database.",
  },
  {
    keyword: "CREATE CONSTRAINT (unique)",
    template:
      "CREATE CONSTRAINT ${1:c_name} FOR (${2:n}:${3:Label}) REQUIRE ${2:n}.${4:property} IS UNIQUE",
    info: "Uniqueness constraint on a property.",
  },
  {
    keyword: "CREATE CONSTRAINT (exists)",
    template:
      "CREATE CONSTRAINT ${1:c_name} FOR (${2:n}:${3:Label}) REQUIRE ${2:n}.${4:property} IS NOT NULL",
    info: "Existence constraint on a property.",
  },
  {
    keyword: "DROP CONSTRAINT",
    template: "DROP CONSTRAINT ${1:c_name} IF EXISTS",
    info: "Drop a constraint by name.",
  },
  {
    keyword: "SHOW CONSTRAINTS",
    template: "SHOW CONSTRAINTS",
    info: "List every constraint in the database.",
  },
];

const COMPARISON_OPS: Array<{ label: string; detail: string; info: string }> = [
  { label: "=", detail: "equal", info: "Equality." },
  { label: "<>", detail: "not equal", info: "Inequality." },
  { label: "<", detail: "less than", info: "Numeric / temporal less-than." },
  { label: "<=", detail: "less or equal", info: "Numeric / temporal ≤." },
  { label: ">", detail: "greater than", info: "Numeric / temporal greater-than." },
  { label: ">=", detail: "greater or equal", info: "Numeric / temporal ≥." },
  { label: "IN", detail: "membership", info: "True when the LHS is in the RHS list." },
  { label: "STARTS WITH", detail: "prefix match", info: "String prefix test." },
  { label: "ENDS WITH", detail: "suffix match", info: "String suffix test." },
  { label: "CONTAINS", detail: "substring", info: "Substring containment test." },
  { label: "=~", detail: "regex match", info: "Regex match (Java syntax)." },
];

const BOOLEAN_OPS: Array<{ label: string; info: string }> = [
  { label: "AND", info: "Boolean conjunction." },
  { label: "OR", info: "Boolean disjunction." },
  { label: "XOR", info: "Boolean exclusive-or." },
];

function booleanOpCompletions(): Completion[] {
  return BOOLEAN_OPS.map((b) => ({
    label: b.label,
    type: "operator",
    info: b.info,
    boost: 6,
    apply: `${b.label} `,
  }));
}

function comparisonOpCompletions(): Completion[] {
  return COMPARISON_OPS.map((op) => ({
    label: op.label,
    type: "operator",
    detail: op.detail,
    info: op.info,
    boost: 5,
    apply: `${op.label} `,
  }));
}

/**
 * Operator + value snippets the user can drop in right after typing a
 * property access in a filter position (`WHERE n.name|`). Each entry
 * inserts a complete `op rhs` fragment with the cursor landing on the
 * value placeholder. Choosing a snippet finishes the predicate in one
 * keystroke; the alternative is the bare-operator menu from
 * {@link comparisonOpCompletions} which only inserts the operator.
 *
 * Pass `paramHint` to seed the `= $param` entry with a value that
 * matches the property name (`WHERE n.email = $email`) — the
 * idiomatic Cypher pattern for parametrised filters.
 */
function operatorWithRhsCompletions(paramHint: string | null): Completion[] {
  const out: Completion[] = [];
  if (paramHint) {
    out.push({
      label: `= $${paramHint}`,
      type: "operator",
      detail: "equality vs parameter",
      info: `Compare to the \`$${paramHint}\` parameter.`,
      boost: 9,
      apply: snippet(` = \${1:$${paramHint}}`),
    });
  }
  out.push(
    {
      label: "= 'value'",
      type: "operator",
      detail: "equality vs string",
      info: "String equality test.",
      boost: 8,
      apply: snippet(" = '${1:value}'"),
    },
    {
      label: "= 0",
      type: "operator",
      detail: "equality vs number",
      info: "Numeric equality test.",
      boost: 7,
      apply: snippet(" = ${1:0}"),
    },
    {
      label: "IS NULL",
      type: "operator",
      detail: "is null",
      info: "True when the property is unset.",
      boost: 8,
      apply: " IS NULL",
    },
    {
      label: "IS NOT NULL",
      type: "operator",
      detail: "is not null",
      info: "True when the property has any value.",
      boost: 8,
      apply: " IS NOT NULL",
    },
    {
      label: "STARTS WITH '…'",
      type: "operator",
      detail: "prefix match",
      info: "String prefix test.",
      boost: 6,
      apply: snippet(" STARTS WITH '${1:prefix}'"),
    },
    {
      label: "ENDS WITH '…'",
      type: "operator",
      detail: "suffix match",
      info: "String suffix test.",
      boost: 5,
      apply: snippet(" ENDS WITH '${1:suffix}'"),
    },
    {
      label: "CONTAINS '…'",
      type: "operator",
      detail: "substring",
      info: "Substring containment test.",
      boost: 6,
      apply: snippet(" CONTAINS '${1:substring}'"),
    },
    {
      label: "IN […]",
      type: "operator",
      detail: "membership",
      info: "Membership test against a list literal.",
      boost: 5,
      apply: snippet(" IN [${1:'a', 'b'}]"),
    },
    {
      label: "> 0",
      type: "operator",
      detail: "greater than",
      info: "Numeric / temporal `>` test.",
      boost: 4,
      apply: snippet(" > ${1:0}"),
    },
    {
      label: "<= 0",
      type: "operator",
      detail: "less or equal",
      info: "Numeric / temporal `<=` test.",
      boost: 3,
      apply: snippet(" <= ${1:0}"),
    },
  );
  return out;
}

/**
 * `AS <alias>` suggestions for projections in RETURN / WITH after the
 * user has finished typing a value expression like `n.name`. We seed
 * a few common forms; the most useful is the auto-derived alias that
 * matches the property name.
 */
function asAliasCompletions(propertyHint: string | null): Completion[] {
  const out: Completion[] = [];
  if (propertyHint) {
    out.push({
      label: `AS ${propertyHint}`,
      type: "keyword",
      detail: "auto-alias",
      info: "Alias matching the property name.",
      boost: 9,
      apply: ` AS ${propertyHint}`,
    });
  }
  out.push(
    {
      label: "AS <alias>",
      type: "keyword",
      detail: "named projection",
      info: "Give this projection a custom name.",
      boost: 7,
      apply: snippet(" AS ${1:alias}"),
    },
  );
  return out;
}

function isNullCompletions(): Completion[] {
  return [
    {
      label: "NULL",
      type: "constant",
      detail: "IS NULL test",
      info: "Match the absence of value.",
      boost: 6,
    },
    {
      label: "NOT NULL",
      type: "constant",
      detail: "IS NOT NULL test",
      info: "Match the presence of any value.",
      boost: 6,
    },
  ];
}

function mergeActionCompletions(): Completion[] {
  const mk = (label: string, template: string, info: string): Completion => ({
    label,
    type: "keyword",
    info,
    apply: snippet(template),
    boost: 7,
  });
  return [
    mk(
      "ON CREATE SET",
      "ON CREATE SET ${1:n}.${2:createdAt} = ${3:timestamp()}",
      "Runs only when the MERGE actually created the row.",
    ),
    mk(
      "ON MATCH SET",
      "ON MATCH SET ${1:n}.${2:updatedAt} = ${3:timestamp()}",
      "Runs only when the MERGE matched an existing row.",
    ),
  ];
}

function caseFlowCompletions(): Completion[] {
  const mk = (label: string, template: string, info: string): Completion => ({
    label,
    type: "keyword",
    info,
    apply: snippet(template),
    boost: 7,
  });
  return [
    mk("WHEN", "WHEN ${1:value} THEN ${2:result}", "Add a branch."),
    mk("ELSE", "ELSE ${1:result}", "Fallback branch."),
    {
      label: "END",
      type: "keyword",
      info: "Close the CASE expression.",
      boost: 6,
    },
    {
      label: "THEN",
      type: "keyword",
      info: "Introduce the branch result.",
      boost: 5,
    },
  ];
}

function existsSnippets(): Completion[] {
  return [
    {
      label: "EXISTS { … }",
      type: "keyword",
      info: "True if the inner pattern matches at least once.",
      apply: snippet("EXISTS { (${1:n})-[:${2:KNOWS}]->(${3:m}) }"),
      boost: 5,
    },
    {
      label: "NOT EXISTS { … }",
      type: "keyword",
      info: "True if the inner pattern matches zero rows.",
      apply: snippet("NOT EXISTS { (${1:n})-[:${2:KNOWS}]->(${3:m}) }"),
      boost: 4,
    },
  ];
}

function rangeLiteralCompletions(): Completion[] {
  return [
    {
      label: "*1..3",
      type: "operator",
      detail: "variable length",
      info: "Variable-length path: 1 to 3 hops.",
      apply: snippet("*${1:1}..${2:3}"),
      boost: 6,
    },
    {
      label: "*",
      type: "operator",
      detail: "any length",
      info: "Variable-length path: 1 or more hops.",
      boost: 5,
    },
    {
      label: "*0..",
      type: "operator",
      detail: "open length",
      info: "Variable-length path: 0 or more hops.",
      apply: snippet("*${1:0}.."),
      boost: 4,
    },
  ];
}

/**
 * Used INSIDE a `{ … }` map literal — inserts a `key: value` pair so
 * the syntax stays valid. NEVER used after `var.` in expression
 * position; for that, see {@link genericPropertyNameSnippets}.
 */
function genericMapKeySnippets(): Completion[] {
  return [
    {
      label: "name",
      type: "property",
      info: "Generic property placeholder.",
      apply: snippet("name: ${1:'value'}"),
      boost: 4,
    },
    {
      label: "id",
      type: "property",
      info: "Generic identifier property.",
      apply: snippet("id: ${1:0}"),
      boost: 4,
    },
    {
      label: "<key>: <value>",
      type: "property",
      info: "Insert a key / value pair scaffold.",
      apply: snippet("${1:key}: ${2:'value'}"),
      boost: 3,
    },
  ];
}

/**
 * Bare property-name suggestions used AFTER `var.` in expression
 * position. Inserts just the property name — the user continues with
 * an operator (`=`, `IN`, ...) themselves. Never the `key: value`
 * map-entry form (which only parses inside `{ … }`).
 */
function genericPropertyNameSnippets(): Completion[] {
  return [
    {
      label: "name",
      type: "property",
      info: "Common property name.",
      boost: 4,
    },
    {
      label: "id",
      type: "property",
      info: "Common identifier property.",
      boost: 4,
    },
    {
      label: "createdAt",
      type: "property",
      info: "Common timestamp property.",
      boost: 3,
    },
    {
      label: "updatedAt",
      type: "property",
      info: "Common timestamp property.",
      boost: 3,
    },
  ];
}

function patternStarterCompletions(): Completion[] {
  const mk = (label: string, template: string, info: string): Completion => ({
    label,
    type: "keyword",
    info,
    apply: snippet(template),
    boost: 6,
  });
  return [
    mk("(n)", "(${1:n})", "Open a node pattern."),
    mk("(n:Label)", "(${1:n}:${2:Label})", "Node with a label."),
    mk("(n:Label {key: value})", "(${1:n}:${2:Label} {${3:key}: ${4:'value'}})", "Node with label + properties."),
    mk(
      "(a)-[r]->(b)",
      "(${1:a})-[${2:r}]->(${3:b})",
      "Pair of nodes joined by a relationship.",
    ),
    mk(
      "(a)-[r:TYPE]->(b)",
      "(${1:a})-[${2:r}:${3:KNOWS}]->(${4:b})",
      "Typed relationship.",
    ),
    mk(
      "(a)<-[:TYPE]-(b)",
      "(${1:a})<-[:${2:KNOWS}]-(${3:b})",
      "Incoming typed relationship.",
    ),
  ];
}

function setItemSnippets(outline: Outline, cursor: number): Completion[] {
  const inScope = outline.variables.filter((v) => v.declStart < cursor);
  if (inScope.length === 0) {
    return [
      {
        label: "<var>.<key> = <value>",
        type: "keyword",
        info: "Assign a property on a bound variable.",
        apply: snippet("${1:n}.${2:key} = ${3:value}"),
        boost: 6,
      },
    ];
  }
  return inScope.flatMap((v) => [
    {
      label: `${v.name}.<key> = <value>`,
      type: "variable",
      info: "Assign a property on this variable.",
      apply: snippet(`${v.name}.\${1:key} = \${2:value}`),
      boost: 6,
    } as Completion,
    {
      label: `${v.name} += { … }`,
      type: "variable",
      info: "Merge a properties map into this node/rel.",
      apply: snippet(`${v.name} += {\${1:key}: \${2:'value'}}`),
      boost: 4,
    } as Completion,
  ]);
}

function patternContinuationCompletions(): Completion[] {
  const mk = (label: string, template: string, info: string): Completion => ({
    label,
    type: "keyword",
    info,
    apply: snippet(template),
    boost: 7,
  });
  return [
    mk("-[:TYPE]->", "-[:${1:KNOWS}]->(${2:b})", "Outgoing relationship."),
    mk("-[r:TYPE]->", "-[${1:r}:${2:KNOWS}]->(${3:b})", "Outgoing relationship with variable."),
    mk("<-[:TYPE]-", "<-[:${1:KNOWS}]-(${2:b})", "Incoming relationship."),
    mk("-[:TYPE]-", "-[:${1:KNOWS}]-(${2:b})", "Undirected relationship."),
    mk("-->", "-->(${1:b})", "Anonymous outgoing relationship."),
    mk("<--", "<--(${1:b})", "Anonymous incoming relationship."),
    mk("--", "--(${1:b})", "Anonymous undirected relationship."),
  ];
}

function returnAutoFillCompletions(outline: Outline, cursor: number): Completion[] {
  const inScope = outline.variables.filter((v) => v.declStart < cursor);
  const out: Completion[] = [];
  if (inScope.length > 0) {
    out.push({
      label: inScope.map((v) => v.name).join(", "),
      type: "variable",
      detail: "all in-scope variables",
      info: "Return every variable currently bound.",
      boost: 8,
    });
    for (const v of inScope) {
      out.push({
        label: `${v.name}`,
        type: "variable",
        detail: v.label ? `:${v.label}` : "variable",
        info: "Return this variable.",
        boost: 6,
      });
      out.push({
        label: `${v.name}.name AS ${v.name}Name`,
        type: "property",
        detail: "named projection",
        info: "Project a property with an alias.",
        boost: 4,
        apply: `${v.name}.\${1:name} AS ${v.name}\${2:Name}`,
      } as Completion);
    }
  }
  out.push({
    label: "count(*) AS total",
    type: "function",
    detail: "row count",
    info: "Aggregate — total number of rows.",
    boost: 5,
    apply: "count(*) AS total",
  });
  return out;
}

function orderBySmartCompletions(
  outline: Outline,
  cursor: number,
  afterValue: boolean,
): Completion[] {
  // After a value (e.g. `ORDER BY n.age `) only direction keywords are
  // syntactically valid next. Don't list variables — that would suggest
  // a comma-less continuation which Cypher rejects.
  if (afterValue) {
    return [
      {
        label: "DESC",
        type: "keyword",
        info: "Descending order — largest first.",
        boost: 6,
      },
      {
        label: "ASC",
        type: "keyword",
        info: "Ascending order — smallest first.",
        boost: 5,
      },
    ];
  }

  const inScope = outline.variables.filter((v) => v.declStart < cursor);
  return inScope.flatMap((v) => [
    {
      label: `${v.name} DESC`,
      type: "variable",
      detail: "sort descending",
      info: "Sort by this variable, largest first.",
      boost: 5,
      apply: `${v.name} DESC`,
    } as Completion,
    {
      label: `${v.name} ASC`,
      type: "variable",
      detail: "sort ascending",
      info: "Sort by this variable, smallest first.",
      boost: 4,
      apply: `${v.name} ASC`,
    } as Completion,
  ]);
}

/**
 * Suggest values to put on the right-hand side of a comparison /
 * containment operator. Variables in scope come first, then parameters,
 * then literal constants and string snippets.
 *
 * When `paramHint` is set (typically the property name on the LHS of
 * the operator: `n.email = |` → "email"), the matching `$<hint>`
 * parameter is surfaced ahead of everything else — that's the
 * idiomatic Cypher parametrised-filter pattern.
 */
function rhsCompletions(
  outline: Outline,
  cursor: number,
  paramHint: string | null = null,
): Completion[] {
  const inScope = outline.variables.filter((v) => v.declStart < cursor);
  const known = new Set(outline.parameters);
  const out: Completion[] = [];
  if (paramHint) {
    out.push({
      label: `$${paramHint}`,
      type: "constant",
      detail: known.has(paramHint)
        ? "parameter"
        : "new parameter",
      info: known.has(paramHint)
        ? `Existing query parameter \`$${paramHint}\`.`
        : `Suggest a new \`$${paramHint}\` parameter matching the property name.`,
      boost: 11,
    });
  }
  out.push(...inScope.map(variableCompletion));
  for (const p of outline.parameters) {
    if (p === paramHint) continue; // already surfaced above
    out.push(parameterCompletion(p));
  }
  out.push(
    {
      label: "TRUE",
      type: "constant",
      info: "Boolean true.",
      boost: 4,
    },
    {
      label: "FALSE",
      type: "constant",
      info: "Boolean false.",
      boost: 4,
    },
    {
      label: "NULL",
      type: "constant",
      info: "Absence of value.",
      boost: 4,
    },
    {
      label: "''",
      type: "text",
      detail: "string literal",
      info: "Empty string literal.",
      apply: snippet("'${1}'"),
      boost: 3,
    },
    {
      label: "[]",
      type: "text",
      detail: "list literal",
      info: "List literal.",
      apply: snippet("[${1}]"),
      boost: 3,
    },
  );
  return out;
}

/**
 * Walk back from `pos` through the trailing whitespace + comparison
 * operator + more whitespace, and return the property name immediately
 * before that operator (`n.email = |` → "email"). Returns `null` when
 * the LHS isn't a simple `<var>.<prop>` access.
 */
function lhsPropertyName(source: string, pos: number): string | null {
  let i = pos;
  while (i > 0 && (source[i - 1] === " " || source[i - 1] === "\t")) i--;
  // Skip the operator. We support the same set as the RHS branch.
  const opPatterns = [
    "CONTAINS",
    "STARTS WITH",
    "ENDS WITH",
    "<=",
    ">=",
    "<>",
    "=~",
    "=",
    "<",
    ">",
    "IN",
  ];
  let consumed = false;
  for (const op of opPatterns) {
    const start = i - op.length;
    if (start < 0) continue;
    if (source.slice(start, i).toUpperCase() === op) {
      i = start;
      consumed = true;
      break;
    }
  }
  if (!consumed) return null;
  while (i > 0 && (source[i - 1] === " " || source[i - 1] === "\t")) i--;
  return tailPropertyName(source, i);
}

/** Fresh node-variable names that aren't already in scope. */
function freshNodeNameCompletions(outline: Outline): Completion[] {
  const used = new Set(outline.variables.map((v) => v.name));
  const choices = ["n", "m", "a", "b", "c", "x", "node", "item"];
  return choices
    .filter((c) => !used.has(c))
    .slice(0, 5)
    .map((c, idx) => ({
      label: c,
      type: "variable",
      detail: "fresh name",
      info: "Use this as the pattern variable.",
      boost: 5 - idx,
    }));
}

/** DISTINCT is invalid inside percentile* — the spec requires two args. */
const AGGREGATES_WITHOUT_DISTINCT = new Set(["percentilecont", "percentiledisc"]);
/** Only `count` accepts `*` as the argument. */
const AGGREGATES_WITH_STAR = new Set(["count"]);

function numericPaginationCompletions(clause: "LIMIT" | "SKIP"): Completion[] {
  const sizes = [10, 25, 50, 100, 500, 1000];
  return sizes.map((n, idx) => ({
    label: String(n),
    type: "constant",
    detail: clause === "LIMIT" ? "rows" : "rows to skip",
    info: clause === "LIMIT" ? "Cap the result size." : "Offset into the result set.",
    boost: 6 - idx,
  }));
}

function aggregateInsideCompletions(
  outline: Outline,
  cursor: number,
  name: string,
): Completion[] {
  const inScope = outline.variables.filter((v) => v.declStart < cursor);
  const lower = name.toLowerCase();
  const out: Completion[] = [];
  if (!AGGREGATES_WITHOUT_DISTINCT.has(lower)) {
    out.push({
      label: "DISTINCT",
      type: "keyword",
      detail: "drop duplicates",
      info: "Aggregate over distinct values only.",
      boost: 8,
      apply: "DISTINCT ",
    });
  }
  if (AGGREGATES_WITH_STAR.has(lower)) {
    out.push({
      label: "*",
      type: "constant",
      detail: "every row",
      info: "Counts every row, including those with NULL.",
      boost: 7,
    });
  }
  out.push(...inScope.map(variableCompletion));
  return out;
}

function snippetCompletion(s: (typeof SNIPPETS)[number]): Completion {
  return {
    label: s.keyword,
    type: "keyword",
    info: s.info,
    apply: snippet(s.template),
    boost: 7,
  };
}

function parameterCompletion(name: string): Completion {
  return {
    label: `$${name}`,
    type: "constant",
    detail: "parameter",
    info: "Query parameter.",
    boost: 4,
  };
}

/** Clauses we recognise as "current scope" anchors. */
type ClauseAnchor =
  | "MATCH"
  | "OPTIONAL_MATCH"
  | "WHERE"
  | "WITH"
  | "RETURN"
  | "CREATE"
  | "MERGE"
  | "DELETE"
  | "DETACH_DELETE"
  | "SET"
  | "REMOVE"
  | "UNWIND"
  | "CALL"
  | "YIELD"
  | "ORDER_BY"
  | "LIMIT"
  | "SKIP"
  | "CASE";

interface Context {
  inString: boolean;
  inComment: boolean;
  afterColon: boolean;
  /** Most recent enclosing delimiter, top of the stack (`(`, `[`, `{`, or null). */
  delimiterTop: "(" | "[" | "{" | null;
  /**
   * Character immediately preceding the most recent open delimiter,
   * trimmed of whitespace. Used to differentiate `MATCH (` (preceded
   * by whitespace) from `count(` (preceded by `t`).
   */
  delimiterOpenPrev: string;
  /** Cursor is inside an *unclosed* `( ... )` pair. */
  insideParens: boolean;
  /** Cursor is inside an *unclosed* `[ ... ]` pair. */
  insideBrackets: boolean;
  /** Cursor is inside an *unclosed* `{ ... }` pair (map literal / properties). */
  insideBraces: boolean;
  /** Cursor is inside *any* unclosed bracket — useful for "no clauses here". */
  insideAnyDelimiter: boolean;
  /** When inside `{...}`, the enclosing pattern kind. */
  enclosingPatternKind: "node" | "relationship" | "map";
  /** Text snippet of the immediately-enclosing delimiter so we can mine a label/variable. */
  enclosingSlice: string;
  clause: ClauseAnchor | null;
  /** Last non-whitespace, non-comment character before the current word. */
  prevChar: string;
  /** True when prev word is `IS` — surface NULL / NOT NULL completions. */
  afterIs: boolean;
  /** True when prev char is `)` and we're at clause-body level — suggest pattern continuation. */
  afterClosedNode: boolean;
  /** True when prev token is a value (variable, var.prop, parameter, `)` of a fn call) at expression position. */
  afterValue: boolean;
  /** True when cursor sits right after `aggregate(` — surface DISTINCT. */
  afterAggregateOpen: boolean;
  /** Lower-cased name of the aggregate, when {@link afterAggregateOpen} is true. */
  afterAggregateName: string;
  /** True when cursor sits at the start of a clause body (just whitespace after the keyword). */
  emptyClauseBody: boolean;
}

/**
 * Single combined regex scanning every clause keyword in one pass.
 * Longest alternates first so `OPTIONAL\s+MATCH` wins over `MATCH` at
 * the same position. The capture group is uppercased before lookup so
 * the per-keyword `gi` regexes collapse to a single engine instance.
 */
const CLAUSE_KEYWORD_RE =
  /\b(OPTIONAL\s+MATCH|DETACH\s+DELETE|ORDER\s+BY|MATCH|WHERE|WITH|RETURN|CREATE|MERGE|DELETE|SET|REMOVE|UNWIND|CALL|YIELD|LIMIT|SKIP|CASE)\b/gi;

/** Maps the canonical (uppercased, whitespace-collapsed) keyword to its anchor. */
const CLAUSE_ANCHOR_BY_KEYWORD: Record<string, ClauseAnchor> = {
  "OPTIONAL MATCH": "OPTIONAL_MATCH",
  "DETACH DELETE": "DETACH_DELETE",
  "ORDER BY": "ORDER_BY",
  MATCH: "MATCH",
  WHERE: "WHERE",
  WITH: "WITH",
  RETURN: "RETURN",
  CREATE: "CREATE",
  MERGE: "MERGE",
  DELETE: "DELETE",
  SET: "SET",
  REMOVE: "REMOVE",
  UNWIND: "UNWIND",
  CALL: "CALL",
  YIELD: "YIELD",
  LIMIT: "LIMIT",
  SKIP: "SKIP",
  CASE: "CASE",
};

/**
 * Quick lexical scan of `text[0..pos]` to determine where the cursor
 * is. Tracks string / comment state and the open-bracket depth so we
 * never recommend completions that would close the wrong delimiter or
 * insert a clause inside a map / list.
 */
function scanContext(text: string, pos: number): Context {
  const head = text.slice(0, pos);

  let i = 0;
  let stringQuote: "'" | '"' | "`" | null = null;
  let inLineComment = false;
  let inBlockComment = false;
  /** Stack of `{ delim, openAt }` so we can recover the enclosing slice. */
  const stack: Array<{ delim: "(" | "[" | "{"; openAt: number }> = [];
  while (i < head.length) {
    const c = head[i];
    if (inLineComment) {
      if (c === "\n") inLineComment = false;
      i++;
      continue;
    }
    if (inBlockComment) {
      if (c === "*" && head[i + 1] === "/") {
        inBlockComment = false;
        i += 2;
        continue;
      }
      i++;
      continue;
    }
    if (stringQuote) {
      if (c === "\\" && i + 1 < head.length) {
        i += 2;
        continue;
      }
      if (c === stringQuote) stringQuote = null;
      i++;
      continue;
    }
    if (c === "'" || c === '"' || c === "`") {
      stringQuote = c as "'" | '"' | "`";
      i++;
      continue;
    }
    if (c === "/" && head[i + 1] === "/") {
      inLineComment = true;
      i += 2;
      continue;
    }
    if (c === "/" && head[i + 1] === "*") {
      inBlockComment = true;
      i += 2;
      continue;
    }
    switch (c) {
      case "(":
      case "[":
      case "{":
        stack.push({ delim: c as "(" | "[" | "{", openAt: i });
        break;
      case ")":
      case "]":
      case "}":
        if (stack.length > 0) stack.pop();
        break;
      default:
        break;
    }
    i++;
  }
  const parenDepth = stack.filter((s) => s.delim === "(").length;
  const bracketDepth = stack.filter((s) => s.delim === "[").length;
  const braceDepth = stack.filter((s) => s.delim === "{").length;
  const top = stack.length > 0 ? stack[stack.length - 1] : null;

  if (stringQuote || inLineComment || inBlockComment) {
    return {
      inString: stringQuote !== null,
      inComment: inLineComment || inBlockComment,
      afterColon: false,
      delimiterTop: null,
      delimiterOpenPrev: "",
      insideParens: false,
      insideBrackets: false,
      insideBraces: false,
      insideAnyDelimiter: false,
      enclosingPatternKind: "map",
      enclosingSlice: "",
      clause: null,
      prevChar: "",
      afterIs: false,
      afterClosedNode: false,
      afterValue: false,
      afterAggregateOpen: false,
      afterAggregateName: "",
      emptyClauseBody: false,
    };
  }

  const beforeCursor = head.replace(/[A-Za-z_][\w]*$/, "");
  const trimmed = beforeCursor.trimEnd();
  const afterColon = trimmed.endsWith(":") && !trimmed.endsWith("::");

  // One forward pass over the head, tracking only the latest match.
  // The previous implementation ran 18 separate global regexes for the
  // same job — each one a full O(N) scan of the document.
  let last: { idx: number; clause: ClauseAnchor } | null = null;
  CLAUSE_KEYWORD_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = CLAUSE_KEYWORD_RE.exec(head)) !== null) {
    const key = m[1]!.toUpperCase().replace(/\s+/g, " ");
    const clause = CLAUSE_ANCHOR_BY_KEYWORD[key];
    if (clause) last = { idx: m.index, clause };
  }

  // When inside a `{`, climb the stack to find the enclosing `(` or `[`
  // so we can label this as a node-properties or rel-properties map.
  let enclosingPatternKind: "node" | "relationship" | "map" = "map";
  let enclosingSlice = "";
  if (top) {
    enclosingSlice = head.slice(top.openAt);
    if (top.delim === "{") {
      for (let s = stack.length - 2; s >= 0; s--) {
        if (stack[s]!.delim === "(") {
          enclosingPatternKind = "node";
          enclosingSlice = head.slice(stack[s]!.openAt);
          break;
        }
        if (stack[s]!.delim === "[") {
          enclosingPatternKind = "relationship";
          enclosingSlice = head.slice(stack[s]!.openAt);
          break;
        }
      }
    } else if (top.delim === "(") {
      enclosingPatternKind = "node";
    } else if (top.delim === "[") {
      enclosingPatternKind = "relationship";
    }
  }

  // Cheap heuristics about the token immediately preceding the cursor.
  const prevChar = trimmed.length > 0 ? trimmed[trimmed.length - 1]! : "";
  const lastIdent = /([A-Za-z_][\w]*)\s*$/.exec(beforeCursor)?.[1] ?? "";
  const afterIs = lastIdent.toUpperCase() === "IS";

  const afterClosedNode = prevChar === ")";

  // Variable / var.prop / parameter / numeric or string literal /
  // closing delim of a fn call or list all count as "value-position"
  // — operators are valid right after.
  const afterValue =
    /[A-Za-z_][\w]*(?:\.[A-Za-z_][\w]*)*\s*$/.test(beforeCursor) ||
    /\$[A-Za-z_][\w]*\s*$/.test(beforeCursor) ||
    /\)\s*$/.test(beforeCursor) ||
    /\]\s*$/.test(beforeCursor) ||
    /\d+(?:\.\d+)?\s*$/.test(beforeCursor) ||
    /['"`]\s*$/.test(beforeCursor);

  const aggregateRe =
    /\b(count|sum|avg|min|max|collect|stdev|stdevp|percentileCont|percentileDisc)\s*\(\s*$/i;
  const aggMatch = aggregateRe.exec(beforeCursor);
  const afterAggregateOpen = aggMatch !== null;
  const afterAggregateName = aggMatch ? aggMatch[1]!.toLowerCase() : "";

  // emptyClauseBody: the cursor sits right after the most recent clause
  // keyword (only whitespace between the keyword and the cursor). Used
  // to surface bigger suggestions (e.g. RETURN auto-fill).
  let emptyClauseBody = false;
  if (last) {
    const after = head.slice(last.idx);
    const keywordMatch = /^(?:OPTIONAL\s+MATCH|DETACH\s+DELETE|ORDER\s+BY|UNION\s+ALL|[A-Z]+)/i.exec(after);
    if (keywordMatch) {
      const rest = after.slice(keywordMatch[0].length);
      emptyClauseBody = /^\s*$/.test(rest);
    }
  }

  return {
    inString: false,
    inComment: false,
    afterColon,
    delimiterTop: top ? top.delim : null,
    // Character *immediately* preceding the open delimiter (no
    // whitespace skipping). `MATCH (` → " "; `count(` → "t".
    delimiterOpenPrev: top && top.openAt > 0 ? head[top.openAt - 1] ?? "" : "",
    insideParens: parenDepth > 0,
    insideBrackets: bracketDepth > 0,
    insideBraces: braceDepth > 0,
    insideAnyDelimiter: parenDepth + bracketDepth + braceDepth > 0,
    enclosingPatternKind,
    enclosingSlice,
    clause: last?.clause ?? null,
    prevChar,
    afterIs,
    afterClosedNode,
    afterValue,
    afterAggregateOpen,
    afterAggregateName,
    emptyClauseBody,
  };
}

/**
 * Pull the return-column names out of a procedure signature of the
 * shape `db.indexes() :: (name, state, type)`. Returns the columns in
 * order, or an empty array if the signature doesn't follow the
 * `:: (…)` convention.
 */
function parseYieldColumns(signature: string): string[] {
  const idx = signature.indexOf("::");
  if (idx < 0) return [];
  const rest = signature.slice(idx + 2).trim();
  if (!rest.startsWith("(")) return [];
  const close = rest.indexOf(")");
  if (close < 0) return [];
  return rest
    .slice(1, close)
    .split(",")
    .map((s) => s.trim().split(/[\s:]/)[0]!) // strip type annotations like `name: STRING`
    .filter(Boolean);
}

/**
 * If the cursor sits right after a `<var>.<prop>` property-access
 * expression (with any number of trailing spaces), return the property
 * name. Used to seed property-aware completions like
 * `AS <prop>` and `= $<prop>`.
 *
 * Returns `null` when the text immediately before the cursor isn't a
 * complete dotted access — including when the cursor is mid-typing
 * the property name, which is the case the dotted-property branch
 * above handles.
 */
function tailPropertyName(source: string, pos: number): string | null {
  // Walk back skipping trailing whitespace.
  let i = pos;
  while (i > 0 && (source[i - 1] === " " || source[i - 1] === "\t")) i--;
  // Read backwards while we're in an identifier.
  let j = i;
  while (j > 0 && /\w/.test(source[j - 1]!)) j--;
  if (j === i) return null;
  // Must be preceded by `.` (and a variable name before that).
  if (source[j - 1] !== ".") return null;
  let k = j - 1;
  while (k > 0 && /\w/.test(source[k - 1]!)) k--;
  if (k === j - 1) return null;
  return source.slice(j, i);
}

/**
 * Look forward from `pos`, skipping spaces and the current word, and
 * report whether the next non-whitespace character is a `:` that ISN'T
 * the start of `::`. Used by the inline-map completion path to decide
 * whether the user is mid-pair (key already followed by a colon) and
 * the apply text should therefore drop the `: ` scaffold.
 */
function followedByColon(source: string, pos: number): boolean {
  let i = pos;
  // Skip the rest of the word the user is currently typing.
  while (i < source.length && /\w/.test(source[i]!)) i++;
  while (i < source.length && (source[i] === " " || source[i] === "\t")) i++;
  if (source[i] !== ":") return false;
  // Cypher doesn't have `::` in this position, but guard against the
  // type-cast operator just in case.
  return source[i + 1] !== ":";
}

/** Best-effort dig of `name` and the first `:Label` from a slice like `(alice:Person {...`. */
function readPatternHead(slice: string): { variable: string | null; label: string | null } {
  // Strip the leading delimiter
  const inner = slice.startsWith("(") || slice.startsWith("[") ? slice.slice(1) : slice;
  const variableMatch = /^\s*([A-Za-z_][\w]*)/.exec(inner);
  const labelMatch = /:([A-Za-z_][\w]*)/.exec(inner);
  return {
    variable: variableMatch ? variableMatch[1]! : null,
    label: labelMatch ? labelMatch[1]! : null,
  };
}

/** Curated suggestion sets per clause. */
function suggestionsFor(
  ctx: Context,
  outline: Outline,
  cursor: number,
): Completion[] {
  // Scope-precise: only surface variables that were declared *before*
  // the cursor. RETURN n shouldn't auto-suggest a name introduced
  // further down the document on the next keystroke.
  const variables = outline.variables
    .filter((v) => v.declStart < cursor)
    .map(variableCompletion);
  const parameters = outline.parameters.map(parameterCompletion);

  const exprFunctions = [
    ...CYPHER_TOP_LEVEL_FUNCTIONS.map((f) => toCompletion(f)),
    ...CYPHER_NAMESPACES.map((n) =>
      toCompletion(n, { apply: `${n.label}.` }),
    ),
    // Compatibility aliases — `date`, `tolower`, `coalesce`, etc.
    // Surface at a lower boost than the canonical entries so the
    // namespaced form (`temporal.now`, `string.lower`) is suggested
    // first when both match the user's input.
    ...CYPHER_ALIASES.map(aliasToCompletion),
    ...CYPHER_CONSTANTS.map((c) => toCompletion(c, { boost: 1 })),
    // CASE and EXISTS subqueries are valid expressions and reachable
    // from every expression position (WHERE, RETURN, WITH, SET, …).
    ...CYPHER_KEYWORDS.filter((k) =>
      ["CASE", "EXISTS"].includes(k.label),
    ).map((k) =>
      toCompletion(k, {
        boost: 2,
        apply:
          k.label === "CASE"
            ? snippet(
                "CASE ${1:expr}\n  WHEN ${2:value} THEN ${3:result}\n  ELSE ${4:other}\nEND",
              )
            : snippet("EXISTS { ${1:(n)-[:KNOWS]->(m)} }"),
      }),
    ),
  ];
  const exprOperators = CYPHER_KEYWORDS.filter((k) =>
    ["AND", "OR", "NOT", "XOR", "IN", "IS NULL", "IS NOT NULL"].includes(
      k.label,
    ),
  ).map((k) => toCompletion(k, { boost: 2 }));

  const followClauses = (allowed: string[]): Completion[] =>
    CYPHER_CLAUSES.filter((c) => allowed.includes(c.label)).map((c) =>
      toCompletion(c, { boost: 3 }),
    );

  // ── Hard rule: inside any unclosed `(`, `[`, `{`, do NOT suggest
  // clauses. They'd inject text that breaks the surrounding delimiter.
  if (ctx.insideAnyDelimiter) {
    if (ctx.insideBraces) {
      // Inside `{ ... }`. Keys are user-defined; surface variables +
      // expression helpers for the value side after a colon.
      return [...variables, ...parameters, ...exprFunctions];
    }
    // `(` and `[` — pure expression position.
    return [
      ...variables,
      ...parameters,
      ...exprFunctions,
      ...exprOperators,
    ];
  }

  switch (ctx.clause) {
    case null:
      return [
        ...SNIPPETS.map(snippetCompletion),
        ...CYPHER_CLAUSES.filter((c) =>
          [
            "MATCH",
            "OPTIONAL MATCH",
            "CREATE",
            "MERGE",
            "UNWIND",
            "CALL",
            "WITH",
            "RETURN",
          ].includes(c.label),
        ).map((c) => toCompletion(c, { boost: 5 })),
      ];

    case "MATCH":
    case "OPTIONAL_MATCH":
    case "CREATE":
    case "MERGE":
      return [
        ...variables,
        ...followClauses([
          "WHERE",
          "RETURN",
          "WITH",
          "SET",
          "REMOVE",
          "DELETE",
          "ORDER BY",
          "LIMIT",
          "SKIP",
        ]),
      ];

    case "WHERE":
      return [
        ...variables,
        ...parameters,
        ...exprFunctions,
        ...exprOperators,
        ...followClauses(["RETURN", "WITH", "ORDER BY", "LIMIT", "SKIP"]),
      ];

    case "WITH":
    case "RETURN":
      return [
        ...variables,
        ...parameters,
        ...exprFunctions,
        ...CYPHER_KEYWORDS.filter((k) =>
          ["AS", "DISTINCT"].includes(k.label),
        ).map((k) => toCompletion(k, { boost: 3 })),
        ...followClauses(["ORDER BY", "LIMIT", "SKIP", "UNION", "UNION ALL"]),
      ];

    case "SET":
    case "REMOVE":
      return [
        ...variables,
        ...parameters,
        ...exprFunctions,
        ...followClauses([
          "WHERE",
          "RETURN",
          "WITH",
          "SET",
          "REMOVE",
          "DELETE",
        ]),
      ];

    case "DELETE":
    case "DETACH_DELETE":
      return [
        ...variables,
        ...parameters,
        ...exprFunctions,
        ...followClauses(["RETURN", "WITH"]),
      ];

    case "ORDER_BY":
      return [
        ...variables,
        ...exprFunctions,
        ...CYPHER_KEYWORDS.filter((k) => ["ASC", "DESC"].includes(k.label)).map(
          (k) => toCompletion(k, { boost: 4 }),
        ),
        ...followClauses(["LIMIT", "SKIP"]),
      ];

    case "LIMIT":
    case "SKIP":
      return numericPaginationCompletions(ctx.clause).concat(parameters);

    case "UNWIND":
      return [
        ...variables,
        ...parameters,
        ...exprFunctions,
        ...CYPHER_KEYWORDS.filter((k) => k.label === "AS").map((k) =>
          toCompletion(k, { boost: 4 }),
        ),
      ];

    case "CALL":
      return [...followClauses(["YIELD", "RETURN", "WITH", "WHERE"])];

    case "YIELD":
      return [
        ...CYPHER_KEYWORDS.filter((k) => k.label === "AS").map((k) =>
          toCompletion(k, { boost: 4 }),
        ),
        ...followClauses(["WHERE", "RETURN", "WITH"]),
      ];

    case "CASE":
      return [
        ...variables,
        ...exprFunctions,
        ...CYPHER_KEYWORDS.filter((k) =>
          ["WHEN", "THEN", "ELSE", "END"].includes(k.label),
        ).map((k) => toCompletion(k, { boost: 4 })),
      ];
  }
}

/**
 * Context-aware completion source for Cypher.
 *
 * Behaviour:
 *  - Inside strings or comments → no completions.
 *  - After a `:` (label / rel-type) → no completions (we don't know
 *    the schema, so suggesting keywords would be misleading).
 *  - After `namespace.` → only that namespace's members.
 *  - Inside `(`, `[`, `{` → expression-position completions only
 *    (variables, parameters, functions); never clauses.
 *  - Otherwise the option set is gated by the surrounding clause:
 *    after MATCH/WHERE/RETURN/SET/... we surface only completions
 *    that are legal in that position. Variables declared earlier in
 *    the same query are surfaced with high boost so the user can
 *    quickly chain references.
 */
export function cypherCompletions(
  context: CompletionContext,
): CompletionResult | Promise<CompletionResult | null> | null {
  const ctx = scanContext(context.state.doc.toString(), context.pos);
  if (ctx.inString || ctx.inComment) return null;

  const providers = getProviders(context.state);
  const outline = getOutline(context.state);

  // ── DISTINCT inside aggregate calls ──
  if (ctx.afterAggregateOpen) {
    const word = context.matchBefore(/[\w]*/);
    return {
      from: word ? word.from : context.pos,
      options: aggregateInsideCompletions(outline, context.pos, ctx.afterAggregateName),
      validFor: /^\w*$/,
    };
  }

  // ── NULL / NOT NULL after IS ──
  if (ctx.afterIs && !ctx.insideAnyDelimiter) {
    const word = context.matchBefore(/[\w]*/);
    return {
      from: word ? word.from : context.pos,
      options: isNullCompletions(),
      validFor: /^\w*$/,
    };
  }

  // ── Right-hand side of a comparison operator ──
  if (
    !ctx.insideAnyDelimiter &&
    /(?:=|<>|<|<=|>|>=|=~|\bIN|\bSTARTS WITH|\bENDS WITH|\bCONTAINS)\s+$/i.test(
      context.state.doc.sliceString(
        Math.max(0, context.pos - 24),
        context.pos,
      ),
    ) &&
    (ctx.clause === "WHERE" ||
      ctx.clause === "RETURN" ||
      ctx.clause === "WITH" ||
      ctx.clause === "SET" ||
      ctx.clause === "CASE")
  ) {
    const word = context.matchBefore(/[\w]*/);
    if (context.explicit || (word && word.from === word.to)) {
      // Look back across the operator to find the property name on
      // the left-hand side (`n.email = |` → "email"). Used to surface
      // `$email` ahead of every other RHS suggestion so the idiomatic
      // parameterised filter is one keystroke away.
      const lhsHint = lhsPropertyName(
        context.state.doc.toString(),
        context.pos,
      );
      return {
        from: word ? word.from : context.pos,
        options: rhsCompletions(outline, context.pos, lhsHint),
        validFor: /^[\w'"`$]*$/,
      };
    }
  }

  // ── Fresh variable-name suggestions inside an empty `(` of a real
  //    pattern (not a function call). ──
  if (
    ctx.insideParens &&
    ctx.delimiterTop === "(" &&
    /^\(\s*$/.test(ctx.enclosingSlice) &&
    !/[A-Za-z0-9_]/.test(ctx.delimiterOpenPrev) &&
    (ctx.clause === "MATCH" ||
      ctx.clause === "OPTIONAL_MATCH" ||
      ctx.clause === "CREATE" ||
      ctx.clause === "MERGE")
  ) {
    const word = context.matchBefore(/[\w]*/);
    if (context.explicit || (word && word.from === word.to)) {
      return {
        from: word ? word.from : context.pos,
        options: freshNodeNameCompletions(outline),
        validFor: /^\w*$/,
      };
    }
  }

  // ── After a relationship arrow (`->`, `<-`, `--`, `-->`, `<--`) the
  //    next token must be a node pattern. Surface only `(…)` starters.
  if (
    !ctx.insideAnyDelimiter &&
    (ctx.clause === "MATCH" ||
      ctx.clause === "OPTIONAL_MATCH" ||
      ctx.clause === "CREATE" ||
      ctx.clause === "MERGE")
  ) {
    const tail = context.state.doc.sliceString(
      Math.max(0, context.pos - 6),
      context.pos,
    );
    if (/(?:-->|<--|->|<-|--)\s*$/.test(tail)) {
      const word = context.matchBefore(/[\w]*/);
      const starters = [
        { label: "(n)", template: "(${1:n})", info: "Bare node pattern." },
        {
          label: "(n:Label)",
          template: "(${1:n}:${2:Label})",
          info: "Node pattern with a label.",
        },
        {
          label: "(n:Label {key: value})",
          template:
            "(${1:n}:${2:Label} {${3:key}: ${4:'value'}})",
          info: "Node pattern with label and properties.",
        },
        { label: "()", template: "()", info: "Anonymous node." },
      ];
      return {
        from: word ? word.from : context.pos,
        options: starters.map((s) => ({
          label: s.label,
          type: "keyword",
          info: s.info,
          apply: snippet(s.template),
          boost: 7,
        })),
        validFor: /^\w*$/,
      };
    }
  }

  // ── Pattern continuation after a closing `)` (in pattern clauses) ──
  // Fires both on explicit invoke and on the `)`-trigger from
  // triggers.ts so the user gets the rel snippets the moment they
  // close a node pattern.
  if (
    ctx.afterClosedNode &&
    !ctx.insideAnyDelimiter &&
    (ctx.clause === "MATCH" ||
      ctx.clause === "OPTIONAL_MATCH" ||
      ctx.clause === "CREATE" ||
      ctx.clause === "MERGE")
  ) {
    const word = context.matchBefore(/[\w]*/);
    const continuation = patternContinuationCompletions();
    const followClauses: Completion[] = CYPHER_CLAUSES.filter((c) =>
      [
        "WHERE",
        "RETURN",
        "WITH",
        "SET",
        "REMOVE",
        "DELETE",
        "ORDER BY",
        "LIMIT",
        "SKIP",
      ].includes(c.label),
    ).map((c) => toCompletion(c, { boost: 3 }));
    const options =
      ctx.clause === "MERGE"
        ? [...continuation, ...mergeActionCompletions(), ...followClauses]
        : [...continuation, ...followClauses];
    return {
      from: word ? word.from : context.pos,
      options,
      validFor: /^[\w-]*$/,
    };
  }

  // ── Variable-length range after `-[r:TYPE` (the `*1..3` part). ──
  // Only fire when the user is past the type name — i.e. the
  // enclosing slice already has a type and isn't sitting right after
  // a `:`. Otherwise the `afterColon` branch (below) wins.
  if (
    ctx.insideBrackets &&
    ctx.delimiterTop === "[" &&
    !ctx.afterColon &&
    /\[[\w:|`\s]*[\w`]\s*$/.test(ctx.enclosingSlice)
  ) {
    if (context.explicit) {
      const word = context.matchBefore(/\*[\w.]*/);
      return {
        from: word ? word.from : context.pos,
        options: rangeLiteralCompletions(),
        validFor: /^\*[\w.]*$/,
      };
    }
  }

  // ── Inside `{ ... }` of a node/rel pattern with no provider hits:
  //    surface a generic `<key>: <value>` snippet so the user still
  //    gets a useful scaffold — UNLESS the cursor sits right before
  //    an existing `:`, in which case the `key: value` snippets would
  //    produce a `name: : 'value'` glitch. Return null there so the
  //    autocomplete popup just closes (no useful generic suggestion
  //    we can offer for a key being edited mid-pair).
  if (
    ctx.insideBraces &&
    (ctx.enclosingPatternKind === "node" ||
      ctx.enclosingPatternKind === "relationship") &&
    !providers.getPropertyKeys
  ) {
    if (followedByColon(context.state.doc.toString(), context.pos)) {
      return null;
    }
    const word = context.matchBefore(/[\w]*/);
    return {
      from: word ? word.from : context.pos,
      options: genericMapKeySnippets(),
      validFor: /^\w*$/,
    };
  }

  // After `:` — surface labels inside `(...)`, rel types inside
  // `[...]`. Also handles label/type unions: `(n:Person|`.
  if (ctx.afterColon) {
    const word = context.matchBefore(/[\w]*/);
    const candidates =
      ctx.enclosingPatternKind === "relationship"
        ? providers.relTypes
        : providers.labels;
    if (!candidates.length) return null;
    const detail =
      ctx.enclosingPatternKind === "relationship" ? "rel type" : "label";
    return {
      from: word ? word.from : context.pos,
      options: candidates.map((name) => ({
        label: name,
        type: "type",
        detail,
        boost: 5,
        // Wrap labels containing characters outside [A-Za-z0-9_] in
        // backticks so they're a valid identifier.
        apply: /^[A-Za-z_][\w]*$/.test(name) ? name : `\`${name}\``,
      })),
      validFor: /^[\w`]*$/,
    };
  }

  // After `:Foo|`, `:Foo|Bar|`, ... — label / rel-type unions inside
  // the current pattern delimiter. Allow any number of pipes by
  // letting word/colon/whitespace/pipe characters appear between the
  // opening `(` or `[` and the trailing pipe.
  {
    const head = context.state.doc.sliceString(
      Math.max(0, context.pos - 96),
      context.pos,
    );
    const pipeMatch = /[([][\w\s:|]*\|\s*([\w]*)$/.exec(head);
    if (pipeMatch && !ctx.insideBraces) {
      const candidates =
        ctx.enclosingPatternKind === "relationship"
          ? providers.relTypes
          : providers.labels;
      if (candidates.length) {
        const word = context.matchBefore(/[\w]*/);
        return {
          from: word ? word.from : context.pos,
          options: candidates.map((name) => ({
            label: name,
            type: "type",
            detail:
              ctx.enclosingPatternKind === "relationship"
                ? "rel type"
                : "label",
            boost: 5,
            apply: /^[A-Za-z_][\w]*$/.test(name) ? name : `\`${name}\``,
          })),
          validFor: /^[\w`]*$/,
        };
      }
    }
  }

  // After `CALL ` — surface stored procedures by name.
  {
    const head = context.state.doc.sliceString(
      Math.max(0, context.pos - 40),
      context.pos,
    );
    if (/\bCALL\s+([\w.]*)$/i.test(head) && providers.procedures.length > 0) {
      const word = context.matchBefore(/[\w.]*/);
      return {
        from: word ? word.from : context.pos,
        options: providers.procedures.map((p) => ({
          label: p.name,
          type: "function",
          ...(p.signature !== undefined && { detail: p.signature }),
          ...(p.info !== undefined && { info: p.info }),
          apply: `${p.name}(`,
          boost: 6,
        })),
        validFor: /^[\w.]*$/,
      };
    }
  }

  // Inside `{ ... }` of a node / rel pattern → property-key completion.
  if (
    ctx.insideBraces &&
    providers.getPropertyKeys &&
    (ctx.enclosingPatternKind === "node" ||
      ctx.enclosingPatternKind === "relationship")
  ) {
    const word = context.matchBefore(/[\w]*/);
    const head = readPatternHead(ctx.enclosingSlice);
    const reqCtx: PropertyContext = {
      kind: ctx.enclosingPatternKind,
      label: head.label,
      variable: head.variable,
    };
    // When the text immediately following the word already starts with
    // `:` (and isn't `::`), the user is editing an existing
    // `key: value` pair — applying `key: ` would produce a stray
    // `name: : 'value'`. Skip the colon in that case and insert just
    // the bare key.
    const hasFollowingColon = followedByColon(
      context.state.doc.toString(),
      context.pos,
    );
    const keysResult = providers.getPropertyKeys(reqCtx, context.state);
    const toResult = (keys: readonly string[]): CompletionResult | null => {
      const fromPos = word ? word.from : context.pos;
      const concrete: Completion[] = keys.map((k) => ({
        label: k,
        type: "property",
        detail: head.label ? `:${head.label}` : "property",
        // Bare key when the cursor sits in front of an existing
        // colon, otherwise the full `key: ` scaffold so newly-typed
        // keys get the separator for free.
        apply: hasFollowingColon ? k : `${k}: `,
        boost: 5,
      }));
      return {
        from: fromPos,
        // Drop the `key: value` snippet when a colon already follows
        // — those snippets would all produce the same double-colon
        // glitch. Concrete keys cover the use case there.
        options: hasFollowingColon
          ? concrete
          : [...concrete, ...genericMapKeySnippets()],
        validFor: /^\w*$/,
      };
    };
    if (Array.isArray(keysResult)) return toResult(keysResult);
    return Promise.resolve(keysResult).then(toResult);
  }

  // `$param` style completion.
  const dollar = context.matchBefore(/\$\w*/);
  if (dollar) {
    if (outline.parameters.length === 0 && !context.explicit) return null;
    return {
      from: dollar.from,
      options: outline.parameters.map(parameterCompletion),
      validFor: /^\$\w*$/,
    };
  }

  // namespace.member OR variable.property completion.
  const dotted = context.matchBefore(/[A-Za-z_][\w]*\.[\w]*/);
  if (dotted) {
    // We only handle one level of dotted access. `n.foo.bar` would
    // require schema knowledge of `n.foo`'s shape, which we don't have
    // — silently surrender rather than offer the wrong suggestions.
    if ((dotted.text.match(/\./g) ?? []).length > 1) {
      return null;
    }
    const dotIdx = dotted.text.indexOf(".");
    const head = dotted.text.slice(0, dotIdx);

    // 1. namespace lookup (math, string, list, ...)
    const members = NAMESPACE_MEMBERS[head.toLowerCase()];
    if (members) {
      return {
        from: dotted.from + dotIdx + 1,
        options: members.map((m) => toCompletion(m)),
        validFor: /^\w*$/,
      };
    }

    // 2. variable.property — look the variable up in the outline,
    //    following `AS` aliases so `WITH n AS x` still propagates the
    //    label.
    const resolved = resolveVariable(head, outline);
    // Paths (`MATCH p = (n)-[r]->(m)`) don't expose a property map —
    // suggesting keys would be misleading.
    if (resolved && resolved.kind === "pattern") {
      return null;
    }
    if (resolved && providers.getPropertyKeys) {
      const propertyKind: PropertyContext["kind"] =
        resolved.kind === "relationship" ? "relationship" : "node";
      const reqCtx: PropertyContext = {
        kind: propertyKind,
        label: resolved.label ?? null,
        variable: resolved.name,
      };
      const keysResult = providers.getPropertyKeys(reqCtx, context.state);
      const fromPos = dotted.from + dotIdx + 1;
      const toRes = (keys: readonly string[]): CompletionResult | null => {
        const concrete: Completion[] = keys.map((k) => ({
          label: k,
          type: "property",
          detail: resolved.label
            ? `:${resolved.label}.${k}`
            : `property`,
          boost: 5,
        }));
        // Expression-position fallback: bare property names only.
        // `key: value` would be a parse error here (it's the map-entry
        // form, valid only inside `{ … }`).
        return {
          from: fromPos,
          options: [...concrete, ...genericPropertyNameSnippets()],
          validFor: /^\w*$/,
        };
      };
      if (Array.isArray(keysResult)) return toRes(keysResult);
      return Promise.resolve(keysResult).then(toRes);
    }

    // No schema callback — still offer bare property names. Avoid the
    // map-entry form (`name: 'value'`) which would not parse outside a
    // `{ … }` literal.
    if (resolved) {
      return {
        from: dotted.from + dotIdx + 1,
        options: genericPropertyNameSnippets(),
        validFor: /^\w*$/,
      };
    }
    return null;
  }

  const word = context.matchBefore(/[\w]+/);
  let options = suggestionsFor(ctx, outline, context.pos);

  // ── Comparison operators after a value at expression position ──
  // We only surface them where a boolean comparison is the *idiomatic*
  // next token. When the value happens to be a `<var>.<prop>` access,
  // also seed the prop-aware `op rhs` snippets (`= $prop`, `IS NULL`,
  // `STARTS WITH '…'`, …) so finishing a predicate is one keystroke
  // away.
  if (
    ctx.afterValue &&
    !ctx.insideAnyDelimiter &&
    ctx.clause === "WHERE"
  ) {
    const propHint = tailPropertyName(
      context.state.doc.toString(),
      context.pos,
    );
    options = [
      ...operatorWithRhsCompletions(propHint),
      ...comparisonOpCompletions(),
      ...booleanOpCompletions(),
      ...options,
    ];
  }

  // ── AS alias completions after `<expr>` in RETURN / WITH ──
  // When the cursor is right after a complete value expression that
  // ends in `.prop`, suggest `AS prop` as a high-boost alias so the
  // user can finish `RETURN n.name | → AS name` in one keystroke.
  if (
    ctx.afterValue &&
    !ctx.insideAnyDelimiter &&
    (ctx.clause === "RETURN" || ctx.clause === "WITH")
  ) {
    const propHint = tailPropertyName(
      context.state.doc.toString(),
      context.pos,
    );
    if (propHint) {
      options = [...asAliasCompletions(propHint), ...options];
    }
  }

  // ── EXISTS / NOT EXISTS surface in WHERE only. ──
  if (ctx.clause === "WHERE" && !ctx.insideAnyDelimiter) {
    options = [...existsSnippets(), ...options];
  }

  // ── CASE flow: WHEN / THEN / ELSE / END snippets when inside CASE. ──
  if (ctx.clause === "CASE" && !ctx.insideAnyDelimiter) {
    options = [...caseFlowCompletions(), ...options];
  }

  // ── RETURN auto-fill + `*` at empty projection body. ──
  if (ctx.clause === "RETURN" && ctx.emptyClauseBody && !ctx.insideAnyDelimiter) {
    options = [
      {
        label: "*",
        type: "constant",
        detail: "all in scope",
        info: "Return every currently-bound variable.",
        boost: 9,
      } as Completion,
      ...returnAutoFillCompletions(outline, context.pos),
      ...options,
    ];
  }
  if (ctx.clause === "WITH" && ctx.emptyClauseBody && !ctx.insideAnyDelimiter) {
    options = [
      {
        label: "*",
        type: "constant",
        detail: "pass everything through",
        info: "Forward every currently-bound variable to the next clause.",
        boost: 9,
      } as Completion,
      ...options,
    ];
  }
  if (ctx.clause === "YIELD" && !ctx.insideAnyDelimiter) {
    const upToCursor = context.state.doc.sliceString(0, context.pos);
    // Look for the most recent `CALL <proc>(…)` before this YIELD.
    // The provider's `signature` field holds the return columns as
    // `name() :: (col1, col2, …)`; we parse them out so the host
    // doesn't have to repeat the column list separately.
    const callMatch = /\bCALL\s+([\w.]+)\s*\([^)]*\)\s*YIELD\b/i.exec(upToCursor);
    if (callMatch) {
      const procName = callMatch[1]!;
      const proc = providers.procedures.find((p) => p.name === procName);
      const columns = proc?.signature ? parseYieldColumns(proc.signature) : [];
      const columnOpts: Completion[] = columns.map((col, idx) => ({
        label: col,
        type: "variable",
        detail: `column of ${procName}`,
        info: `Column produced by \`${procName}\`.`,
        boost: 8 - idx,
      }));
      const star: Completion = {
        label: "*",
        type: "constant",
        detail: "yield every column",
        info: "Yield every column produced by the CALL.",
        boost: 9,
      };
      // For empty-body YIELD: lead with `*` then concrete columns.
      // For mid-typing, drop `*` (it only makes sense as a prefix).
      const leading = ctx.emptyClauseBody ? [star, ...columnOpts] : columnOpts;
      options = [...leading, ...options];
    } else if (ctx.emptyClauseBody) {
      options = [
        {
          label: "*",
          type: "constant",
          detail: "yield every column",
          info: "Yield every column produced by the CALL.",
          boost: 9,
        } as Completion,
        ...options,
      ];
    }
  }

  // ── Pattern starter snippets right after `MATCH`/`CREATE`/`MERGE`. ──
  if (
    (ctx.clause === "MATCH" ||
      ctx.clause === "OPTIONAL_MATCH" ||
      ctx.clause === "CREATE" ||
      ctx.clause === "MERGE") &&
    ctx.emptyClauseBody &&
    !ctx.insideAnyDelimiter
  ) {
    options = [...patternStarterCompletions(), ...options];
  }

  // ── SET item snippets: `var.key = value`, `var += { … }`. ──
  if (ctx.clause === "SET" && ctx.emptyClauseBody && !ctx.insideAnyDelimiter) {
    options = [...setItemSnippets(outline, context.pos), ...options];
  }

  // ── ORDER BY smarts: variables at expression start, ASC/DESC after
  //    a value. Use `emptyClauseBody` rather than `afterValue` so the
  //    literal `BY` keyword (which looks like an identifier) doesn't
  //    flip the branch into post-expression mode prematurely.
  if (ctx.clause === "ORDER_BY" && !ctx.insideAnyDelimiter) {
    const postValue = !ctx.emptyClauseBody && ctx.afterValue;
    options = [
      ...orderBySmartCompletions(outline, context.pos, postValue),
      ...options,
    ];
  }

  if (options.length === 0) return null;

  if (!word || word.from === word.to) {
    if (!context.explicit) return null;
    return {
      from: context.pos,
      options,
      validFor: /^\w*$/,
    };
  }

  return {
    from: word.from,
    options,
    validFor: /^\w*$/,
  };
}
