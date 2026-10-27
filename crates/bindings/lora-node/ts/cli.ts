#!/usr/bin/env node
/**
 * The `lora` command of `@loradb/lora-node`.
 *
 * ```text
 * lora snapshot <databaseDir> <targetDir> [--name <name>]
 * ```
 *
 * Copies a database that no process has open into `targetDir`, as
 * `db.snapshot(targetDir)` does for an open one: it opens the database,
 * which takes its directory lock, so a directory another process owns is
 * refused rather than copied mid-write.
 */

import { readdirSync } from "node:fs";
import { join, relative, sep } from "node:path";

import { createDatabase, LoraError } from "./index.js";

const USAGE = `usage: lora snapshot <databaseDir> <targetDir> [--name <name>]

Copy a closed database directory to <targetDir> (missing or empty) as a
self-contained snapshot. <name> picks the database when <databaseDir> holds
more than one. For a database a running process has open, call
\`await db.snapshot(targetDir)\` in that process instead.`;

class UsageError extends Error {}

/**
 * Names of the databases under `dir`, by path: each `<name>.loradb`
 * container, or the work directory or lock file of one not written yet.
 */
function databaseNames(dir: string): string[] {
  const names = new Set<string>();
  const walk = (at: string) => {
    for (const entry of readdirSync(at, { withFileTypes: true })) {
      const path = join(at, entry.name);
      const match = /^(.+\.loradb)(\.wal|\.lock)?$/.exec(entry.name);
      if (match && (entry.isFile() || match[2] === ".wal")) {
        const container = relative(dir, join(at, match[1]!));
        names.add(container.split(sep).join("/").slice(0, -".loradb".length));
      } else if (entry.isDirectory()) {
        walk(path);
      }
    }
  };
  walk(dir);
  return [...names].sort();
}

async function snapshot(args: string[]): Promise<void> {
  const positional: string[] = [];
  let name: string | undefined;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (arg === "--name") {
      name = args[++i];
      if (name === undefined) throw new UsageError("--name needs a value");
    } else if (arg.startsWith("--")) {
      throw new UsageError(`unknown option \`${arg}\``);
    } else {
      positional.push(arg);
    }
  }
  if (positional.length !== 2) {
    throw new UsageError("expected <databaseDir> and <targetDir>");
  }
  const [databaseDir, targetDir] = positional as [string, string];

  if (name === undefined) {
    let names: string[];
    try {
      names = databaseNames(databaseDir);
    } catch (err) {
      throw new Error(
        `cannot read database directory \`${databaseDir}\`: ${(err as Error).message}`,
      );
    }
    if (names.length === 0) {
      throw new Error(`no database (\`*.loradb\`) in \`${databaseDir}\``);
    }
    if (names.length > 1) {
      throw new UsageError(
        `\`${databaseDir}\` holds ${names.length} databases (${names
          .map((n) => `\`${n}\``)
          .join(", ")}); pick one with --name`,
      );
    }
    name = names[0]!;
  }

  let db;
  try {
    db = await createDatabase(name, { databaseDir });
  } catch (err) {
    if (err instanceof LoraError && err.code === "LORA_LOCKED") {
      throw new Error(
        `database \`${name}\` in \`${databaseDir}\` is open in another process; ` +
          "snapshot it from that process with `await db.snapshot(targetDir)`",
      );
    }
    throw err;
  }
  try {
    const result = await db.snapshot(targetDir);
    process.stdout.write(
      `${result.path}: ${result.bytes} bytes, ${result.nodeCount} nodes, ` +
        `${result.relationshipCount} relationships, at LSN ${result.lsn}\n`,
    );
  } finally {
    db.dispose();
  }
}

async function main(argv: string[]): Promise<number> {
  const [command, ...args] = argv;
  try {
    if (command === "snapshot") {
      await snapshot(args);
      return 0;
    }
    if (command === undefined || command === "--help" || command === "-h") {
      process.stdout.write(`${USAGE}\n`);
      return command === undefined ? 2 : 0;
    }
    throw new UsageError(`unknown command \`${command}\``);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    process.stderr.write(`lora: ${message}\n`);
    if (err instanceof UsageError) {
      process.stderr.write(`${USAGE}\n`);
      return 2;
    }
    return 1;
  }
}

void main(process.argv.slice(2)).then((code) => {
  process.exitCode = code;
});
