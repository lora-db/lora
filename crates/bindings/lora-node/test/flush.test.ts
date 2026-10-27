import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import { createDatabase } from "../ts/index.js";

const dirs: string[] = [];
afterEach(async () => {
  for (const dir of dirs.splice(0))
    await rm(dir, { recursive: true, force: true });
});

async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "lora-flush-"));
  dirs.push(dir);
  return dir;
}

/** Write `n` batches in a child process, flush after each, then SIGKILL it. */
function writeAndKill(dir: string, n: number): Promise<number> {
  const entry = join(process.cwd(), "dist", "index.js");
  const script = `
    const { createDatabase } = await import(${JSON.stringify(entry)});
    const db = await createDatabase("app", {
      databaseDir: ${JSON.stringify(dir)},
      groupSyncIntervalMs: 3600000,
    });
    for (let i = 0; i < ${n}; i++) {
      await db.execute("CREATE (:Batch {id: $i})", { i });
      await db.flush();
    }
    process.stdout.write("flushed");
    process.kill(process.pid, "SIGKILL");
  `;
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      ["--input-type=module", "-e", script],
      {
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    let out = "";
    let err = "";
    child.stdout.on("data", (c) => (out += c));
    child.stderr.on("data", (c) => (err += c));
    child.on("error", reject);
    child.on("exit", (_code, signal) => {
      if (out !== "flushed" || signal !== "SIGKILL") {
        reject(new Error(`child: ${out} ${signal} ${err}`));
      } else resolve(n);
    });
  });
}

describe("flush() (G-74)", () => {
  it("resolves with a durable LSN that grows, and null in memory", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: dir });
    await db.execute("CREATE (:A)");
    const first = await db.flush();
    await db.execute("CREATE (:A)");
    const second = await db.flush();
    expect(first.lsn).toBeGreaterThan(0);
    expect(second.lsn).toBeGreaterThan(first.lsn!);
    await db.close();

    const memory = await createDatabase();
    expect(await memory.flush()).toEqual({ lsn: null });
    await memory.close();
  });

  it("keeps every flushed write across a kill -9", async () => {
    const dir = await tempDir();
    const n = await writeAndKill(dir, 50);
    const db = await createDatabase("app", { databaseDir: dir });
    const { rows } = await db.execute("MATCH (b:Batch) RETURN count(b) AS c");
    expect(rows[0]!.c).toBe(n);
    await db.close();
  }, 60_000);
});
