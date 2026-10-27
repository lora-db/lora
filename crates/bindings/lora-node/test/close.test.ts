import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import { createDatabase, type Database } from "../ts/index.js";

const dirs: string[] = [];
afterEach(async () => {
  for (const dir of dirs.splice(0))
    await rm(dir, { recursive: true, force: true });
});

async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "lora-close-"));
  dirs.push(dir);
  return dir;
}

// Festimap G-71 ran 1,000; `LORA_CLOSE_ROUNDS=1000` does the same here.
const ROUNDS = Number(process.env.LORA_CLOSE_ROUNDS ?? 40);

/** Open `dir` from a separate Node process and dispose it at once. */
function openInChild(dir: string): Promise<{ ok: boolean; message: string }> {
  const nativePath = join(process.cwd(), "ts", "native.js");
  const script = `
    const native = require(${JSON.stringify(nativePath)});
    try {
      const db = new native.Database("app", ${JSON.stringify(dir)});
      db.dispose();
      process.stdout.write(JSON.stringify({ ok: true, message: "" }));
    } catch (err) {
      process.stdout.write(JSON.stringify({ ok: false, message: String(err && err.message) }));
    }
  `;
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["-e", script], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    child.stdout.on("data", (c) => (out += c));
    child.stderr.on("data", (c) => (err += c));
    child.on("error", reject);
    child.on("exit", () => {
      try {
        resolve(JSON.parse(out));
      } catch {
        reject(new Error(`child produced no result: ${err}`));
      }
    });
  });
}

/**
 * One round of Festimap's repro: a change feed being consumed, a write it
 * delivers, and a read still running when the handle goes away.
 */
async function busyHandle(dir: string): Promise<Database> {
  const db = await createDatabase("app", {
    databaseDir: dir,
    groupSyncIntervalMs: 1000,
  });
  const feed = db.changes();
  await feed.ready;
  void (async () => {
    for await (const _ of feed);
  })();
  await db.execute("UNWIND range(1, 2000) AS i CREATE (:N {i: i})");
  db.execute("MATCH (n:N) RETURN count(n)").catch(() => {});
  return db;
}

describe("closing a database (G-71)", () => {
  it(`reopens after dispose() with work in flight, ${ROUNDS} rounds`, async () => {
    const dir = await tempDir();
    for (let round = 0; round < ROUNDS; round++) {
      (await busyHandle(dir)).dispose();
    }
    const db = await createDatabase("app", { databaseDir: dir });
    const { rows } = await db.execute("MATCH (n:N) RETURN count(n) AS c");
    expect(rows[0]!.c).toBe(2000 * ROUNDS);
    await db.close();
  }, 120_000);

  it(`reopens after await close() with work in flight, ${ROUNDS} rounds`, async () => {
    const dir = await tempDir();
    for (let round = 0; round < ROUNDS; round++) {
      await (await busyHandle(dir)).close();
    }
    const db = await createDatabase("app", { databaseDir: dir });
    const { rows } = await db.execute("MATCH (n:N) RETURN count(n) AS c");
    expect(rows[0]!.c).toBe(2000 * ROUNDS);
    await db.close();
  }, 120_000);

  it("unlocks the directory for another process once close() resolves", async () => {
    const dir = await tempDir();
    for (let round = 0; round < 5; round++) {
      await (await busyHandle(dir)).close();
      const child = await openInChild(dir);
      expect(child).toEqual({ ok: true, message: "" });
    }
  }, 60_000);

  it("waits for a running query and lets it finish", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: dir });
    await db.execute("UNWIND range(1, 20000) AS i CREATE (:N {i: i})");
    const running = db.execute(
      "MATCH (a:N) WHERE a.i < 200 MATCH (b:N) RETURN count(*) AS c",
    );
    await db.close();
    const { rows } = await running;
    expect(rows[0]!.c).toBe(199 * 20000);
    await expect(db.execute("RETURN 1")).rejects.toThrow(/closed/);
  }, 60_000);

  it("keeps a database another handle shares open", async () => {
    const dir = await tempDir();
    const first = await createDatabase("app", { databaseDir: dir });
    const second = await createDatabase("app", { databaseDir: dir });
    await first.execute("CREATE (:N {i: 1})");
    await first.close();
    await expect(first.execute("RETURN 1")).rejects.toThrow(/closed/);
    const { rows } = await second.execute("MATCH (n:N) RETURN count(n) AS c");
    expect(rows[0]!.c).toBe(1);
    // Still held by `second`.
    expect((await openInChild(dir)).ok).toBe(false);
    await second.close();
    expect((await openInChild(dir)).ok).toBe(true);
  }, 60_000);

  it("is idempotent and backs `await using`", async () => {
    const dir = await tempDir();
    {
      await using db = await createDatabase("app", { databaseDir: dir });
      await db.execute("CREATE (:N)");
      await db.close();
    }
    const db = await createDatabase();
    await db.close();
    await db.close();
    db.dispose();
    expect((await openInChild(dir)).ok).toBe(true);
  });
});
