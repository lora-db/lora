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
import {
  LoraGraphQL,
  type LoraGraphQLOptions,
  type StatementEvent,
} from "./lora-graphql.js";

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
