// lora-graphql: the build-time half of the library.
//
//   lora-graphql print <schema.graphql>
//   lora-graphql directives
//   lora-graphql requirements <schema.graphql> [--ddl]
//   lora-graphql check <schema.graphql> [--operations <file|dir>]... [--variables <file>] [--context <file>]
//                      [--baseline <file> [--update-baseline]] [--row-budget <n>]
//                      [--database <dir> [--name <db>]] [--json]
//   lora-graphql compile <schema.graphql> --operations <file|dir>... [--out <dir>]
//   lora-graphql analyze <schema.graphql> --database <dir> [--name <db>] [--sample <n>]
//   lora-graphql migrate neo4j <schema.graphql> [--operations <file|dir>]...
//   lora-graphql diff <old.graphql> <new.graphql> [--allow-breaking] [--json]
//   lora-graphql diff --base <git-ref> <file|dir>... [--allow-breaking] [--json]

import { execFileSync } from "node:child_process";
import { mkdir, readdir, readFile, stat, writeFile } from "node:fs/promises";
import { basename, dirname, join, relative } from "node:path";
import {
  getNamedType,
  isEnumType,
  isInputObjectType,
  isListType,
  isNonNullType,
  Kind,
  parse,
  print as printAst,
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
import {
  describeRequirement,
  inferRequirements,
  requirementDdl,
} from "./analyze/indexes.js";
import { LoraGraphQL, type CheckOptions } from "./lora-graphql.js";
import { loraDriver, type LoraDriver } from "./driver.js";
import { usedFragments } from "./codegen.js";
import { migrateNeo4j } from "./migrate.js";

const USAGE = `lora-graphql <command>

  print <schema.graphql>                    the public SDL clients see
  directives                                the directive definitions, for editors
  access <schema.graphql> [--json]          who may do what: each type and guarded field,
                                            operation and kind of caller
  requirements <schema.graphql> [--ddl]     constraints and indexes the API needs
  check <schema.graphql> [--operations <file|dir>]... [--json]
        [--variables <file>] [--context <file>]
        [--baseline <file> [--update-baseline]]
        [--row-budget <n>] [--database <dir> [--name <db>]]
                                            CI gate: model, lint, @cypher statements and
                                            the plans of your operations, on an in-memory
                                            LoraDB (or an existing one with --database)
  compile <schema.graphql> --operations <file|dir>... [--out <dir>]
                                            persisted-operation manifest.json and
                                            operations.d.ts types
  analyze <schema.graphql> --database <dir> [--name <db>] [--sample <n>]
                                            statistics JSON for useStatistics()
  migrate neo4j <schema.graphql> [--operations <file|dir>]...
                                            rewrite an @neo4j/graphql SDL; TODOs for
                                            what has no equivalent
  diff <old.graphql> <new.graphql> [--allow-breaking] [--json]
  diff --base <git-ref> <file|dir>... [--allow-breaking] [--json]
                                            database statements and API changes; exit 1
                                            on a breaking change or a destructive statement
                                            (--base: the SDL files at a git ref against the
                                            working tree, each side concatenated)`;

interface Io {
  out: (line: string) => void;
  err: (line: string) => void;
}

export async function main(
  argv: string[],
  io: Io = defaultIo,
): Promise<number> {
  const [command, ...rest] = argv;
  const flags = new Set<string>();
  const values = new Map<string, string>();
  const positional: string[] = [];
  const operations: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const a = rest[i]!;
    if (a === "--operations") operations.push(rest[++i] ?? "");
    else if (VALUE_FLAGS.has(a)) values.set(a, rest[++i] ?? "");
    else if (a.startsWith("--")) flags.add(a);
    else positional.push(a);
  }
  try {
    switch (command) {
      case "print":
        return await print(need(positional, 1), io);
      case "directives":
        io.out(directiveTypeDefs.trim());
        return 0;
      case "access":
        return await access(need(positional, 1), flags.has("--json"), io);
      case "requirements":
        return await requirements(need(positional, 1), flags.has("--ddl"), io);
      case "check":
        return await check(need(positional, 1), operations, flags, values, io);
      case "compile":
        return await compile(need(positional, 1), operations, values, io);
      case "analyze":
        return await analyzeCommand(need(positional, 1), values, io);
      case "migrate": {
        const [from, file] = need(positional, 2);
        if (from !== "neo4j") throw new UsageError("migrate supports neo4j");
        const docs = [];
        for (const path of operations) {
          for (const f of await graphqlFiles(path)) docs.push(parse(f.source));
        }
        const result = migrateNeo4j(await read(file!), docs);
        for (const t of result.todos) io.out(`# TODO(migrate): ${t}`);
        if (result.todos.length > 0) io.out("");
        io.out(result.typeDefs);
        return 0;
      }
      case "diff": {
        const base = values.get("--base");
        if (base !== undefined) {
          if (!base) throw new UsageError("--base takes a git ref");
          const paths = need(positional, 1);
          const before = await atGitRef(base, paths);
          if (before === undefined) {
            io.out(
              `${base} has none of ${paths.join(", ")}; nothing to compare`,
            );
            return 0;
          }
          return diff(before, await sdlFiles(paths), flags, io);
        }
        const [before, after] = need(positional, 2);
        return diff(await read(before!), await read(after!), flags, io);
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

const VALUE_FLAGS = new Set([
  "--base",
  "--out",
  "--variables",
  "--context",
  "--baseline",
  "--row-budget",
  "--database",
  "--name",
  "--sample",
]);

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

async function access(
  [file]: string[],
  json: boolean,
  io: Io,
): Promise<number> {
  const matrix = offline(await read(file!)).accessMatrix();
  if (json) {
    io.out(JSON.stringify(matrix, null, 2));
    return 0;
  }
  const rows = matrix.map((e) => [
    e.field ? `${e.type}.${e.field}` : e.type,
    e.operation,
    e.principal,
    e.verdict,
    e.by.join(", "),
  ]);
  const widths = [0, 1, 2, 3].map((i) =>
    Math.max(...rows.map((r) => r[i]!.length), 0),
  );
  for (const r of rows) {
    io.out(
      r
        .map((cell, i) => (i < 4 ? cell.padEnd(widths[i]!) : cell))
        .join("  ")
        .trimEnd(),
    );
  }
  return 0;
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
        : `${describeRequirement(r)}, because ${r.reason}`,
    );
  }
  return 0;
}

/** The node binding's createDatabase, or a message saying how to get it. */
async function nodeBinding(
  io: Io,
  command: string,
): Promise<
  | ((name?: string, options?: { databaseDir?: string }) => Promise<unknown>)
  | undefined
> {
  try {
    const { createDatabase } =
      (await import("@loradb/lora-node")) as unknown as {
        createDatabase: (
          name?: string,
          options?: { databaseDir?: string },
        ) => Promise<unknown>;
      };
    return createDatabase;
  } catch {
    io.err(
      `${command} needs @loradb/lora-node: npm install --save-dev @loradb/lora-node`,
    );
    return undefined;
  }
}

async function check(
  [file]: string[],
  operationPaths: string[],
  flags: Set<string>,
  values: Map<string, string>,
  io: Io,
): Promise<number> {
  const json = flags.has("--json");
  const typeDefs = await read(file!);
  const createDatabase = await nodeBinding(io, "check");
  if (!createDatabase) return 1;
  // An existing database is checked as it is; an in-memory one gets what
  // the API needs, so only the plans are under test.
  const databaseDir = values.get("--database");
  const db = await createDatabase(
    databaseDir ? (values.get("--name") ?? "app") : undefined,
    databaseDir ? { databaseDir } : {},
  );
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db as Parameters<typeof loraDriver>[0]),
  });
  if (!databaseDir) await lora.assertSchema({ create: true });
  const schema = lora.getSchema();
  const fixtures = values.has("--variables")
    ? (JSON.parse(await read(values.get("--variables")!)) as Record<
        string,
        Record<string, unknown>
      >)
    : {};
  // GraphQL contexts per operation name, `*` for every other operation.
  const contexts = values.has("--context")
    ? (JSON.parse(await read(values.get("--context")!)) as Record<
        string,
        unknown
      >)
    : {};
  const operations: NonNullable<CheckOptions["operations"]> = [];
  for (const path of operationPaths) {
    for (const f of await graphqlFiles(path)) {
      for (const op of operationsIn(schema, f.path, f.source)) {
        const short = op.name!.split("#")[1] ?? "";
        const given = fixtures[op.name!] ?? fixtures[short];
        const context = contexts[op.name!] ?? contexts[short] ?? contexts["*"];
        operations.push({
          ...op,
          ...(given ? { variables: given } : {}),
          ...(context !== undefined ? { context } : {}),
        });
      }
    }
  }
  const rowBudget = values.has("--row-budget")
    ? Number(values.get("--row-budget"))
    : undefined;
  const report = await lora.check({
    operations,
    ...(rowBudget !== undefined ? { rowBudget } : {}),
  });

  // Plan baseline: the operators of every statement, so a plan change
  // shows up in review.
  const baselinePath = values.get("--baseline");
  const current: Record<string, string[][]> = {};
  for (const p of report.plans) {
    current[`${p.operation} › ${p.field}`] = p.reports.map((r) => r.operators);
  }
  const changed: string[] = [];
  if (baselinePath) {
    let before: Record<string, string[][]> | undefined;
    try {
      before = JSON.parse(await read(baselinePath)) as Record<
        string,
        string[][]
      >;
    } catch {
      before = undefined;
    }
    if (flags.has("--update-baseline") || !before) {
      await writeFile(baselinePath, JSON.stringify(current, null, 2) + "\n");
    } else {
      for (const key of new Set([
        ...Object.keys(before),
        ...Object.keys(current),
      ])) {
        if (JSON.stringify(before[key]) !== JSON.stringify(current[key])) {
          changed.push(key);
        }
      }
    }
  }
  const ok = report.ok && changed.length === 0;

  if (json) {
    io.out(JSON.stringify({ ...report, ok, planChanges: changed }, null, 2));
    return ok ? 0 : 1;
  }
  for (const w of report.warnings) io.out(`warning  ${formatProblem(w)}`);
  for (const w of report.lint) io.out(`lint     ${formatProblem(w)}`);
  for (const u of report.unused) {
    io.out(
      `unused   ${u.type} index ${u.name} on :${u.labels.join(":")}(${u.properties.join(", ")})`,
    );
  }
  for (const s of report.security) io.out(`error    ${formatProblem(s)}`);
  for (const m of report.missing)
    io.out(`error    missing ${describeRequirement(m)}`);
  for (const c of report.cypher)
    io.out(`error    ${c.type}.${c.field}: ${c.message}`);
  for (const e of report.errors)
    io.out(`error    ${e.operation}: ${e.message}`);
  for (const p of report.plans) {
    for (const r of p.reports) {
      for (const f of r.findings) {
        io.out(`error    ${p.operation} › ${p.field}: ${f.message}`);
      }
      for (const n of r.notes) {
        io.out(`lint     ${p.operation} › ${p.field}: ${n.message}`);
      }
    }
  }
  for (const key of changed) {
    io.out(
      `error    ${key}: the plan differs from the baseline (--update-baseline to accept)`,
    );
  }
  const checked = report.plans.length;
  io.out(
    ok
      ? `ok       ${checked} root field(s) in ${operations.length} operation(s) seek as expected`
      : "failed",
  );
  return ok ? 0 : 1;
}

/** Persisted operations from .graphql files (one per operation) or JSON (id → source). */
async function persistedOperations(
  paths: string[],
): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (const path of paths) {
    const info = await stat(path);
    if (info.isFile() && path.endsWith(".json")) {
      Object.assign(
        out,
        JSON.parse(await read(path)) as Record<string, string>,
      );
      continue;
    }
    for (const f of await graphqlFiles(path)) {
      const doc = parse(f.source);
      const fragments = new Map(
        doc.definitions
          .filter(
            (d): d is FragmentDefinitionNode =>
              d.kind === Kind.FRAGMENT_DEFINITION,
          )
          .map((d) => [d.name.value, d]),
      );
      const rel = relative(process.cwd(), f.path);
      doc.definitions
        .filter(
          (d): d is OperationDefinitionNode =>
            d.kind === Kind.OPERATION_DEFINITION,
        )
        .forEach((op, i) => {
          const id = `${rel}#${op.name?.value ?? i + 1}`;
          out[id] = printAst({
            kind: Kind.DOCUMENT,
            definitions: [op, ...usedFragments(op, fragments)],
          });
        });
    }
  }
  return out;
}

async function compile(
  [file]: string[],
  operationPaths: string[],
  values: Map<string, string>,
  io: Io,
): Promise<number> {
  if (operationPaths.length === 0) {
    throw new UsageError("compile needs --operations <file|dir>");
  }
  const lora = offline(await read(file!));
  const manifest = lora.buildManifest(
    await persistedOperations(operationPaths),
  );
  const outDir = values.get("--out") ?? "lora-graphql";
  await mkdir(outDir, { recursive: true });
  await writeFile(
    join(outDir, "manifest.json"),
    JSON.stringify(manifest) + "\n",
  );
  await writeFile(
    join(outDir, "operations.d.ts"),
    lora.generateTypes(manifest),
  );
  io.out(
    `wrote ${Object.keys(manifest.operations).length} operation(s) to ${join(outDir, "manifest.json")} and ${join(outDir, "operations.d.ts")}`,
  );
  return 0;
}

async function analyzeCommand(
  [file]: string[],
  values: Map<string, string>,
  io: Io,
): Promise<number> {
  const databaseDir = values.get("--database");
  if (!databaseDir) throw new UsageError("analyze needs --database <dir>");
  const createDatabase = await nodeBinding(io, "analyze");
  if (!createDatabase) return 1;
  const db = await createDatabase(values.get("--name") ?? "app", {
    databaseDir,
  });
  const lora = new LoraGraphQL({
    typeDefs: await read(file!),
    driver: loraDriver(db as Parameters<typeof loraDriver>[0]),
  });
  const sample = values.has("--sample")
    ? Number(values.get("--sample"))
    : undefined;
  const stats = await lora.analyze(sample !== undefined ? { sample } : {});
  io.out(JSON.stringify(stats, null, 2));
  (db as { dispose?: () => void }).dispose?.();
  return 0;
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

const SDL_FILE = /\.(graphql|gql)$/;

/** The SDL in `paths` (files, or directories searched recursively), joined. */
async function sdlFiles(paths: string[]): Promise<string> {
  const parts: string[] = [];
  for (const path of paths) {
    const files = await graphqlFiles(path);
    files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    parts.push(...files.map((f) => f.source));
  }
  return parts.join("\n");
}

/**
 * The SDL in `paths` as of git `ref`, joined in the same order as
 * `sdlFiles`, or undefined when the ref has none of them (a base from
 * before the schema existed). A ref that is not a commit is an error: a
 * gate that silently compared nothing would pass without looking.
 */
async function atGitRef(
  ref: string,
  paths: string[],
): Promise<string | undefined> {
  const git = (cwd: string, ...args: string[]) =>
    execFileSync("git", args, {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      maxBuffer: 64 * 1024 * 1024,
    });
  const parts: string[] = [];
  let found = false;
  for (const path of paths) {
    // Run git next to the path and name it `./…`, so relative and
    // absolute paths both resolve, from any directory of the repository.
    const isDir = await stat(path).then(
      (s) => s.isDirectory(),
      () => !SDL_FILE.test(path),
    );
    const cwd = isDir ? path : dirname(path);
    try {
      git(cwd, "rev-parse", "--verify", "--quiet", `${ref}^{commit}`);
    } catch {
      throw new Error(
        `diff: ${ref} is not a commit (fetch it, or pass another ref)`,
      );
    }
    if (!isDir) {
      try {
        parts.push(git(cwd, "show", `${ref}:./${basename(path)}`));
        found = true;
      } catch {
        // Not in the base: a new file.
      }
      continue;
    }
    // Paths come back relative to `cwd`; none when the directory is not
    // in the base.
    const files = git(cwd, "ls-tree", "-r", "--name-only", ref, "--", ".")
      .split("\n")
      .filter((f) => SDL_FILE.test(f));
    // Same order as sdlFiles: by path.
    files.sort((a, b) => {
      const pa = join(path, a);
      const pb = join(path, b);
      return pa < pb ? -1 : pa > pb ? 1 : 0;
    });
    for (const f of files) {
      parts.push(git(cwd, "show", `${ref}:./${f}`));
      found = true;
    }
  }
  return found ? parts.join("\n") : undefined;
}

function diff(
  before: string,
  after: string,
  flags: Set<string>,
  io: Io,
): number {
  const result = diffSchemas(before, after);
  const breaking = result.api.breaking.length > 0;
  const destructive = result.database.statements.some((s) => s.destructive);
  const code =
    (breaking || destructive) && !flags.has("--allow-breaking") ? 1 : 0;
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
