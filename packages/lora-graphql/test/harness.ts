import { createDatabase } from "@loradb/lora-node";
import { graphql, type ExecutionResult } from "graphql";
import { LoraGraphQL, loraDriver, type StatementEvent } from "../src/index.js";
import { festivalTypeDefs, seedStatements } from "./fixtures.js";

export interface Harness {
  db: Awaited<ReturnType<typeof createDatabase>>;
  lora: LoraGraphQL;
  statements: StatementEvent[];
  run: (
    source: string,
    variables?: Record<string, unknown>,
    context?: Record<string, unknown>,
  ) => Promise<ExecutionResult>;
  data: <T = Record<string, unknown>>(
    source: string,
    variables?: Record<string, unknown>,
    context?: Record<string, unknown>,
  ) => Promise<T>;
}

export async function festivalHarness(
  options: {
    assert?: boolean;
    typeDefs?: string;
    seed?: string[];
    maxCost?: number;
  } = {},
): Promise<Harness> {
  const db = await createDatabase();
  const statements: StatementEvent[] = [];
  const lora = new LoraGraphQL({
    typeDefs: options.typeDefs ?? festivalTypeDefs,
    driver: loraDriver(db),
    onStatement: (e) => statements.push(e),
    ...(options.maxCost !== undefined ? { maxCost: options.maxCost } : {}),
  });
  if (options.assert !== false) await lora.assertSchema({ create: true });
  for (const s of options.seed ?? seedStatements) await db.execute(s);
  const schema = lora.getSchema();
  const run = (
    source: string,
    variables: Record<string, unknown> = {},
    contextValue: Record<string, unknown> = {},
  ) => graphql({ schema, source, variableValues: variables, contextValue });
  const data = async <T>(
    source: string,
    variables?: Record<string, unknown>,
    context?: Record<string, unknown>,
  ) => {
    const r = await run(source, variables, context);
    if (r.errors) throw r.errors[0];
    return r.data as T;
  };
  return { db, lora, statements, run, data };
}
