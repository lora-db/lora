// Test helpers: an in-memory LoraDB with the schema asserted, and a plan
// assertion. Import from `@loradb/lora-graphql/testing`; needs
// `@loradb/lora-node`. Framework-agnostic: failures throw plain errors.

import {
  graphql,
  type DocumentNode,
  type ExecutionResult,
  type GraphQLSchema,
} from "graphql";
import { loraDriver, type LoraDatabaseLike } from "./driver.js";
import { LoraGraphQL } from "./lora-graphql.js";
import { type LoraGraphQLOptions, type StatementEvent } from "./options.js";

/** The node binding's database, as far as tests use it. */
export type TestDatabase = LoraDatabaseLike & {
  execute(query: string, params?: never): Promise<unknown>;
  dispose?(): void;
};

export interface TestLoraGraphQLOptions extends Omit<
  LoraGraphQLOptions,
  "driver"
> {
  /** Cypher to run after the schema is asserted, or a function given the database. */
  seed?: string | string[] | ((db: TestDatabase) => Promise<void>);
}

export interface TestLoraGraphQL {
  lora: LoraGraphQL;
  db: TestDatabase;
  schema: GraphQLSchema;
  /** Every statement run so far (clear with `statements.length = 0`). */
  statements: StatementEvent[];
  /** Execute a document; errors stay in the result. */
  run: (
    source: string,
    variables?: Record<string, unknown>,
    context?: Record<string, unknown>,
  ) => Promise<ExecutionResult>;
  /** Execute a document and return its data; throws on the first error. */
  data: <T = Record<string, unknown>>(
    source: string,
    variables?: Record<string, unknown>,
    context?: Record<string, unknown>,
  ) => Promise<T>;
  /** Release the database. */
  close: () => void;
}

/** A LoraGraphQL over a fresh in-memory database, schema asserted and seeded. */
export async function createTestLoraGraphQL(
  options: TestLoraGraphQLOptions,
): Promise<TestLoraGraphQL> {
  let createDatabase: () => Promise<TestDatabase>;
  try {
    ({ createDatabase } = (await import("@loradb/lora-node")) as unknown as {
      createDatabase: typeof createDatabase;
    });
  } catch {
    throw new Error(
      "createTestLoraGraphQL needs @loradb/lora-node: npm install --save-dev @loradb/lora-node",
    );
  }
  const db = await createDatabase();
  const statements: StatementEvent[] = [];
  const { seed, onStatement, ...rest } = options;
  const lora = new LoraGraphQL({
    ...rest,
    driver: loraDriver(db),
    onStatement: (event) => {
      statements.push(event);
      onStatement?.(event);
    },
  });
  await lora.assertSchema({ create: true });
  if (typeof seed === "function") await seed(db);
  else {
    for (const text of typeof seed === "string" ? [seed] : (seed ?? [])) {
      await db.execute(text);
    }
  }
  const schema = lora.getSchema();
  const run = (
    source: string,
    variables: Record<string, unknown> = {},
    contextValue: Record<string, unknown> = {},
  ) => graphql({ schema, source, variableValues: variables, contextValue });
  return {
    lora,
    db,
    schema,
    statements,
    run,
    data: async <T>(
      source: string,
      variables?: Record<string, unknown>,
      context?: Record<string, unknown>,
    ) => {
      const r = await run(source, variables, context);
      if (r.errors) throw r.errors[0];
      return r.data as T;
    },
    close: () => db.dispose?.(),
  };
}

/**
 * Assert that every statement of a query's root fields uses the index
 * access it was compiled for (no label scans where a seek was expected,
 * no mutating plans, no row-budget excess). Throws listing each finding.
 */
export async function expectSeeks(
  lora: LoraGraphQL,
  document: string | DocumentNode,
  variables: Record<string, unknown> = {},
  options: { rowBudget?: number; context?: unknown } = {},
): Promise<void> {
  const fields = await lora.explain(document, variables, options);
  const findings = fields.flatMap((f) =>
    f.reports.flatMap((r) =>
      r.findings.map((x) => `${f.field}: ${x.message}\n  ${x.statement}`),
    ),
  );
  if (findings.length > 0) {
    throw new Error(
      `expected every root field to seek:\n${findings.join("\n")}`,
    );
  }
}

/** The access `expectAccess` expects: entries allowed, and entries denied. */
export interface AccessExpectations {
  /** The claims to act as; undefined acts anonymously. */
  as: Record<string, unknown> | undefined;
  /** Other GraphQL context values the rules read (`$context.*`). */
  context?: Record<string, unknown>;
  allowed?: readonly string[];
  denied?: readonly string[];
}

/**
 * Assert who may do what, against the database: each entry runs as the
 * caller in a transaction that is always rolled back, and every mismatch
 * is reported at once. Entries:
 *
 * - `read Trip lou:tomorrowland` (denied: not visible, or refused)
 * - `create Trip lou:x {"name": "X"}` (other input fields as JSON)
 * - `update Trip lou:x {"name": "Y"}` and `delete Trip lou:x`
 * - `connect Trip.members lou:tomorrowland → f1` (also `->`)
 * - `disconnect Trip.members lou:tomorrowland → f1`
 * - `update-edge Trip.members lou:tomorrowland → f1 {"rsvp": "GOING"}`
 *   (on a single relationship the edge is the current one, whichever
 *   node the entry names)
 *
 * Denied means `FORBIDDEN`, `UNAUTHENTICATED` or `NOT_FOUND` (a node the
 * caller cannot see); any other error is reported as a mismatch either way.
 */
export async function expectAccess(
  target: LoraGraphQL | { lora: LoraGraphQL },
  expectations: AccessExpectations,
): Promise<void> {
  const lora = target instanceof LoraGraphQL ? target : target.lora;
  const problems: string[] = [];
  const check = async (entry: string, allowed: boolean) => {
    let outcome: AccessOutcome;
    try {
      outcome = await tryAccess(lora, entry, expectations);
    } catch (err) {
      problems.push(
        `${entry}: ${err instanceof Error ? err.message : String(err)}`,
      );
      return;
    }
    if (outcome.kind === "error") {
      problems.push(`${entry}: failed with ${outcome.message}`);
    } else if (allowed && outcome.kind === "denied") {
      problems.push(
        `expected allowed, was denied: ${entry} (${outcome.message})`,
      );
    } else if (!allowed && outcome.kind === "allowed") {
      problems.push(`expected denied, was allowed: ${entry}`);
    }
  };
  for (const entry of expectations.allowed ?? []) await check(entry, true);
  for (const entry of expectations.denied ?? []) await check(entry, false);
  if (problems.length > 0) {
    const who = expectations.as ? JSON.stringify(expectations.as) : "anonymous";
    throw new Error(
      `access as ${who} differs in ${problems.length} ${problems.length === 1 ? "entry" : "entries"}:\n  ${problems.join("\n  ")}`,
    );
  }
}

type AccessOutcome =
  | { kind: "allowed" }
  | { kind: "denied"; message: string }
  | { kind: "error"; message: string };

const DENIED = new Set(["FORBIDDEN", "UNAUTHENTICATED", "NOT_FOUND"]);

async function tryAccess(
  lora: LoraGraphQL,
  entry: string,
  expectations: AccessExpectations,
): Promise<AccessOutcome> {
  const { source, variables, isRead, root, wrote } = accessOperation(
    lora,
    entry,
  );
  const tx = await lora.begin();
  try {
    const result = await lora.execute({
      source,
      variables,
      context: {
        ...expectations.context,
        ...(expectations.as ? { jwt: expectations.as } : {}),
        transaction: tx,
      },
    });
    const error = result.errors?.[0];
    if (error) {
      const code = (error.extensions as Record<string, unknown> | undefined)?.[
        "code"
      ];
      return typeof code === "string" && DENIED.has(code)
        ? { kind: "denied", message: `${code}: ${error.message}` }
        : {
            kind: "error",
            message: `${String(code ?? "error")}: ${error.message}`,
          };
    }
    const payload = (result.data as Record<string, unknown> | null)?.[root];
    if (
      wrote &&
      (payload == null || !wrote(payload as Record<string, unknown>))
    ) {
      return { kind: "denied", message: "not visible, nothing written" };
    }
    if (isRead) {
      const value = (result.data as Record<string, unknown> | null)?.[root];
      if (value == null) return { kind: "denied", message: "not visible" };
    }
    return { kind: "allowed" };
  } finally {
    await tx.rollback();
  }
}

function accessOperation(
  lora: LoraGraphQL,
  entry: string,
): {
  operation: string;
  source: string;
  variables: Record<string, unknown>;
  isRead: boolean;
  root: string;
  /** How the payload says nothing was written: a hidden target answers no error. */
  wrote?: (payload: Record<string, unknown>) => boolean;
} {
  const m =
    /^\s*(read|create|update|delete|connect|disconnect|update-edge)\s+([A-Za-z_][\w]*)(?:\.([A-Za-z_]\w*))?\s+(\S+)(?:\s*(?:→|->)\s*(\S+))?\s*(\{.*\})?\s*$/s.exec(
      entry,
    );
  if (!m) {
    throw new Error(
      "expected `<read|create|update|delete> Type key [json]` or `<connect|disconnect|update-edge> Type.field key → key [json]`",
    );
  }
  const [, operation, typeName, fieldName, key, other, json] = m as unknown as [
    string,
    string,
    string,
    string | undefined,
    string,
    string | undefined,
    string | undefined,
  ];
  const node = lora.model.nodes.get(typeName);
  if (!node) throw new Error(`${typeName} is not a @node type`);
  const input = json ? (JSON.parse(json) as Record<string, unknown>) : {};
  const keyType = `${node.key.customScalar ?? node.key.type}!`;
  const lower = typeName.charAt(0).toLowerCase() + typeName.slice(1);
  const upperPlural =
    node.plural.charAt(0).toUpperCase() + node.plural.slice(1);
  const relational = ["connect", "disconnect", "update-edge"].includes(
    operation,
  );
  if (relational !== (fieldName !== undefined && other !== undefined)) {
    throw new Error(
      relational
        ? `${operation} takes Type.field key → key`
        : `${operation} takes Type key`,
    );
  }
  switch (operation) {
    case "read":
      return {
        operation,
        source: `query($key: ${keyType}) { ${lower}(${node.key.name}: $key) { ${node.key.name} } }`,
        variables: { key },
        isRead: true,
        root: lower,
      };
    case "create":
      return {
        operation,
        source: `mutation($input: [${typeName}CreateInput!]!) { create${upperPlural}(input: $input) { __typename } }`,
        variables: { input: [{ ...input, [node.key.name]: key }] },
        isRead: false,
        root: `create${upperPlural}`,
      };
    case "update":
      return {
        operation,
        source: `mutation($key: ${keyType}, $update: ${typeName}UpdateInput!) { update${typeName}(${node.key.name}: $key, update: $update) { ${lower} { __typename } info { relationshipsDeleted } } }`,
        variables: { key, update: input },
        isRead: false,
        root: `update${typeName}`,
        wrote: (p) => p[lower] != null,
      };
    case "delete":
      return {
        operation,
        source: `mutation($key: ${keyType}) { delete${typeName}(${node.key.name}: $key) { nodesDeleted } }`,
        variables: { key },
        isRead: false,
        root: `delete${typeName}`,
        wrote: (p) => Number(p["nodesDeleted"] ?? 0) > 0,
      };
  }
  const field = node.fields.get(fieldName!);
  if (field?.kind !== "relationship") {
    throw new Error(`${typeName}.${fieldName} is not a relationship field`);
  }
  const targetKey = lora.model.nodes.get(field.target)?.key.name ?? "key";
  const change =
    operation === "connect"
      ? {
          connect: field.list
            ? [{ [targetKey]: other, ...(json ? { edge: input } : {}) }]
            : { [targetKey]: other, ...(json ? { edge: input } : {}) },
        }
      : operation === "disconnect"
        ? { disconnect: field.list ? [other] : true }
        : // A single relationship has one edge: the update names no key.
          {
            update: field.list
              ? [{ [targetKey]: other, edge: input }]
              : { edge: input },
          };
  return {
    operation,
    source: `mutation($key: ${keyType}, $update: ${typeName}UpdateInput!) { update${typeName}(${node.key.name}: $key, update: $update) { ${lower} { __typename } info { relationshipsDeleted } } }`,
    variables: { key, update: { [fieldName!]: change } },
    isRead: false,
    root: `update${typeName}`,
    // A hidden owner answers a null node; a disconnect of a pair the
    // caller may not touch removes nothing.
    wrote: (p) =>
      p[lower] != null &&
      (operation !== "disconnect" ||
        Number(
          (p["info"] as Record<string, unknown> | undefined)?.[
            "relationshipsDeleted"
          ] ?? 0,
        ) > 0),
  };
}
