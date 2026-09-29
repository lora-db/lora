// lora-graphql: the build-time half of the library.
//
//   lora-graphql print <schema.graphql>
//   lora-graphql directives
//   lora-graphql requirements <schema.graphql> [--ddl]
//   lora-graphql check <schema.graphql> [--operations <file|dir>]... [--json]
//   lora-graphql diff <old.graphql> <new.graphql> [--allow-breaking] [--json]

import { readdir, readFile, stat } from "node:fs/promises";
import { join } from "node:path";
import {
  getNamedType,
  isEnumType,
  isInputObjectType,
  isListType,
  isNonNullType,
  Kind,
  parse,
  typeFromAST,
  visit,
  type DefinitionNode,
  type FragmentDefinitionNode,
  type GraphQLInputType,
  type GraphQLSchema,
  type OperationDefinitionNode,
} from "graphql";
import { diffSchemas } from "./analyze/diff.js";
import { formatProblem, ModelError } from "./errors.js";
import { buildModel } from "./model/build.js";
import { directiveTypeDefs } from "./model/directives.js";
import { inferRequirements, requirementDdl } from "./analyze/indexes.js";
import { LoraGraphQL, type CheckOptions } from "./lora-graphql.js";
import { loraDriver, type LoraDriver } from "./driver.js";

const USAGE = `lora-graphql <command>

  print <schema.graphql>                    the public SDL clients see
  directives                                the directive definitions, for editors
  requirements <schema.graphql> [--ddl]     constraints and indexes the API needs
  check <schema.graphql> [--operations <file|dir>]... [--json]
                                            CI gate: model, @cypher statements and the
                                            plans of your operations, on an in-memory LoraDB
  diff <old.graphql> <new.graphql> [--allow-breaking] [--json]
                                            database statements and API changes`;

interface Io {
  out: (line: string) => void;
  err: (line: string) => void;
}

export async function main(
  argv: string[],
  io: Io = defaultIo,
): Promise<number> {
  const [command, ...rest] = argv;
  const flags = new Set(
    rest.filter((a) => a.startsWith("--") && a !== "--operations"),
  );
  const positional: string[] = [];
  const operations: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const a = rest[i]!;
    if (a === "--operations") operations.push(rest[++i] ?? "");
    else if (!a.startsWith("--")) positional.push(a);
  }
  try {
    switch (command) {
      case "print":
        return await print(need(positional, 1), io);
      case "directives":
        io.out(directiveTypeDefs.trim());
        return 0;
      case "requirements":
        return await requirements(need(positional, 1), flags.has("--ddl"), io);
      case "check":
        return await check(
          need(positional, 1),
          operations,
          flags.has("--json"),
          io,
        );
      case "diff": {
        const [before, after] = need(positional, 2);
        return await diff(before!, after!, flags, io);
      }
      default:
        io.err(USAGE);
        return 2;
    }
  } catch (err) {
    if (err instanceof UsageError) {
      io.err(`${err.message}\n\n${USAGE}`);
      return 2;
    }
    if (err instanceof ModelError) {
      io.err(err.message);
      return 1;
    }
    io.err(err instanceof Error ? err.message : String(err));
    return 1;
  }
}

class UsageError extends Error {}

function need(positional: string[], n: number): string[] {
  if (positional.length < n)
    throw new UsageError(`expected ${n} file argument(s)`);
  return positional;
}

const read = (path: string) => readFile(path, "utf8");

/** A LoraGraphQL over a driver that refuses to run anything. */
function offline(typeDefs: string): LoraGraphQL {
  const driver: LoraDriver = {
    run: () => Promise.reject(new Error("no database")),
  };
  return new LoraGraphQL({ typeDefs, driver });
}

async function print([file]: string[], io: Io): Promise<number> {
  io.out(offline(await read(file!)).printPublicSchema());
  return 0;
}

async function requirements(
  [file]: string[],
  ddl: boolean,
  io: Io,
): Promise<number> {
  const model = buildModel(await read(file!));
  for (const r of inferRequirements(model)) {
    io.out(
      ddl
        ? `${requirementDdl(r)};`
        : `${r.kind === "index" ? `${r.index} index` : `${r.constraint} constraint`} on :${r.label}(${r.property}), because ${r.reason}`,
    );
  }
  return 0;
}

async function check(
  [file]: string[],
  operationPaths: string[],
  json: boolean,
  io: Io,
): Promise<number> {
  const typeDefs = await read(file!);
  let createDatabase: (() => Promise<unknown>) | undefined;
  try {
    ({ createDatabase } = (await import("@loradb/lora-node")) as unknown as {
      createDatabase: () => Promise<unknown>;
    });
  } catch {
    io.err(
      "check needs @loradb/lora-node: npm install --save-dev @loradb/lora-node",
    );
    return 1;
  }
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db as Parameters<typeof loraDriver>[0]),
  });
  await lora.assertSchema({ create: true });
  const schema = lora.getSchema();
  const operations: NonNullable<CheckOptions["operations"]> = [];
  for (const path of operationPaths) {
    for (const f of await graphqlFiles(path)) {
      operations.push(...operationsIn(schema, f.path, f.source));
    }
  }
  const report = await lora.check({ operations });
  if (json) {
    io.out(JSON.stringify(report, null, 2));
    return report.ok ? 0 : 1;
  }
  for (const w of report.warnings) io.out(`warning  ${formatProblem(w)}`);
  for (const c of report.cypher)
    io.out(`error    ${c.type}.${c.field}: ${c.message}`);
  for (const e of report.errors)
    io.out(`error    ${e.operation}: ${e.message}`);
  for (const p of report.plans) {
    for (const r of p.reports) {
      for (const f of r.findings) {
        io.out(`error    ${p.operation} › ${p.field}: ${f.message}`);
      }
    }
  }
  const checked = report.plans.length;
  io.out(
    report.ok
      ? `ok       ${checked} root field(s) in ${operations.length} operation(s) seek as expected`
      : "failed",
  );
  return report.ok ? 0 : 1;
}

async function graphqlFiles(
  path: string,
): Promise<Array<{ path: string; source: string }>> {
  const info = await stat(path);
  if (info.isFile()) return [{ path, source: await read(path) }];
  const out: Array<{ path: string; source: string }> = [];
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const full = join(path, entry.name);
    if (entry.isDirectory()) out.push(...(await graphqlFiles(full)));
    else if (/\.(graphql|gql)$/.test(entry.name)) {
      out.push({ path: full, source: await read(full) });
    }
  }
  return out;
}

/** Each query in a file, with example values for its required variables. */
function operationsIn(
  schema: GraphQLSchema,
  path: string,
  source: string,
): NonNullable<CheckOptions["operations"]> {
  const doc = parse(source);
  const fragments = new Map(
    doc.definitions
      .filter(
        (d): d is FragmentDefinitionNode => d.kind === Kind.FRAGMENT_DEFINITION,
      )
      .map((f) => [f.name.value, f]),
  );
  // The fragments an operation spreads, transitively.
  const used = (root: DefinitionNode): FragmentDefinitionNode[] => {
    const seen = new Map<string, FragmentDefinitionNode>();
    const walk = (node: DefinitionNode) =>
      visit(node, {
        FragmentSpread(spread) {
          const f = fragments.get(spread.name.value);
          if (f && !seen.has(f.name.value)) {
            seen.set(f.name.value, f);
            walk(f);
          }
        },
      });
    walk(root);
    return [...seen.values()];
  };
  return doc.definitions
    .filter(
      (d): d is OperationDefinitionNode =>
        d.kind === Kind.OPERATION_DEFINITION && d.operation === "query",
    )
    .map((op, i) => {
      const variables: Record<string, unknown> = {};
      for (const v of op.variableDefinitions ?? []) {
        if (v.defaultValue || v.type.kind !== Kind.NON_NULL_TYPE) continue;
        const type = typeFromAST(schema, v.type) as
          | GraphQLInputType
          | undefined;
        if (type) variables[v.variable.name.value] = sample(type);
      }
      return {
        name: `${path}${op.name ? `#${op.name.value}` : `#${i + 1}`}`,
        document: { kind: Kind.DOCUMENT, definitions: [op, ...used(op)] },
        variables,
      };
    });
}

const SAMPLES: Record<string, unknown> = {
  String: "x",
  ID: "x",
  Int: 1,
  Float: 1,
  Boolean: true,
  BigInt: "1",
  Date: "2026-01-01",
  Time: "12:00:00Z",
  LocalTime: "12:00:00",
  DateTime: "2026-01-01T12:00:00Z",
  LocalDateTime: "2026-01-01T12:00:00",
  Duration: "P1D",
};

function sample(type: GraphQLInputType): unknown {
  const inner = isNonNullType(type) ? type.ofType : type;
  if (isListType(inner)) return [sample(inner.ofType)];
  const named = getNamedType(inner);
  if (isEnumType(named)) return named.getValues()[0]?.value;
  if (isInputObjectType(named)) {
    const out: Record<string, unknown> = {};
    for (const f of Object.values(named.getFields())) {
      if (isNonNullType(f.type) && f.defaultValue === undefined) {
        out[f.name] = sample(f.type);
      }
    }
    return out;
  }
  return SAMPLES[named.name] ?? "x";
}

async function diff(
  before: string,
  after: string,
  flags: Set<string>,
  io: Io,
): Promise<number> {
  const result = diffSchemas(await read(before), await read(after));
  const breaking = result.api.breaking.length > 0;
  const code = breaking && !flags.has("--allow-breaking") ? 1 : 0;
  if (flags.has("--json")) {
    io.out(JSON.stringify(result, null, 2));
    return code;
  }
  io.out("# Database");
  if (result.database.statements.length === 0) io.out("no changes");
  for (const s of result.database.statements) {
    io.out(`${s.destructive ? "// destructive: " : "// "}${s.reason}`);
    io.out(`${s.text};`);
  }
  io.out("\n# API");
  if (!breaking && result.api.dangerous.length === 0) io.out("no changes");
  for (const c of result.api.breaking) io.out(`breaking   ${c.description}`);
  for (const c of result.api.dangerous) io.out(`dangerous  ${c.description}`);
  for (const n of result.notes) io.out(`note       ${n}`);
  return code;
}

const defaultIo: Io = {
  out: (line) => process.stdout.write(line + "\n"),
  err: (line) => process.stderr.write(line + "\n"),
};

const invokedDirectly =
  typeof process !== "undefined" &&
  process.argv[1] !== undefined &&
  /lora-graphql(\.js)?$|cli\.js$/.test(process.argv[1]);
if (invokedDirectly) {
  main(process.argv.slice(2)).then((code) => process.exit(code));
}
