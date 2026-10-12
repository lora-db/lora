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
  const dir = await mkdtemp(join(tmpdir(), "lora-lock-"));
  dirs.push(dir);
  return dir;
}

/** Try to open `dir` from a separate Node process; resolve with its outcome. */
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

describe("database directory lock", () => {
  it("refuses a second process with a typed LORA_LOCKED error", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: dir });
    await db.execute("CREATE (:X {key: 'a'})");

    const child = await openInChild(dir);
    expect(child.ok).toBe(false);
    expect(child.message).toContain("LORA_LOCKED");
    expect(child.message).toContain("locked by another process");

    db.dispose();
  });

  it("releases the lock on dispose so another process can open", async () => {
    const dir = await tempDir();
    const db = await createDatabase("app", { databaseDir: dir });
    await db.execute("CREATE (:X {key: 'a'})");
    db.dispose();

    const child = await openInChild(dir);
    expect(child).toEqual({ ok: true, message: "" });

    const reopened = await createDatabase("app", { databaseDir: dir });
    const { rows } = await reopened.execute("MATCH (x:X) RETURN count(x) AS c");
    expect(rows[0].c).toBe(1);
    reopened.dispose();
  });

  it("gives a second in-process open the same engine, never a second writer", async () => {
    const dir = await tempDir();
    const a = await createDatabase("app", { databaseDir: dir });
    const b = await createDatabase("app", { databaseDir: dir });
    await a.execute("CREATE (:X {key: 'a'})");
    await b.execute("CREATE (:X {key: 'b'})");
    // Both handles see both writes: one engine, one WAL writer.
    const { rows } = await a.execute(
      "MATCH (x:X) RETURN x.key AS k ORDER BY k",
    );
    expect(rows.map((r) => r.k)).toEqual(["a", "b"]);
    a.dispose();
    b.dispose();

    // After both are disposed the directory is free again and the data
    // recovers intact.
    const c = await createDatabase("app", { databaseDir: dir });
    expect(await c.nodeCount()).toBe(2);
    c.dispose();
  });

  it("keeps the directory locked while any in-process handle is open", async () => {
    const dir = await tempDir();
    const a = await createDatabase("app", { databaseDir: dir });
    const b = await createDatabase("app", { databaseDir: dir });
    a.dispose();

    // `b` still owns the engine, so another process cannot become a
    // second writer on the same WAL.
    const child = await openInChild(dir);
    expect(child.ok).toBe(false);
    expect(child.message).toContain("LORA_LOCKED");
    await b.execute("CREATE (:X {key: 'b'})");
    expect(await b.nodeCount()).toBe(1);
    b.dispose();

    expect(await openInChild(dir)).toEqual({ ok: true, message: "" });
  });
});

describe("transaction() writes wait for the lock on their own thread", () => {
  it("queued write batches do not take the libuv pool from reads", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:Seed {id: 0})");
    const holder = await db.begin();
    // More than the default libuv pool of 4.
    const writes = Array.from({ length: 12 }, (_, i) =>
      db.transaction([{ query: "CREATE (:W {id: $i})", params: { i } }]),
    );
    // A read runs on a libuv worker: it must not queue behind the writes.
    const started = Date.now();
    const { rows } = await db.execute(
      "MATCH (s:Seed) RETURN count(s) AS c",
      {},
      { timeoutMs: 3000 },
    );
    expect(rows[0].c).toBe(1);
    expect(Date.now() - started).toBeLessThan(1500);
    await holder.commit();
    await Promise.all(writes);
    const after = await db.execute("MATCH (w:W) RETURN count(w) AS c");
    expect(after.rows[0].c).toBe(12);
  });

  it("commits every statement or none, naming the one that failed", async () => {
    const db = await createDatabase();
    const [created, counted] = await db.transaction([
      { query: "CREATE (:T {id: 1})" },
      { query: "MATCH (t:T) RETURN count(t) AS c" },
    ]);
    expect(created.rows).toEqual([]);
    expect(counted.rows[0].c).toBe(1);
    const err = await db
      .transaction([
        { query: "CREATE (:T {id: 2})" },
        { query: "RETURN assert(1 = 2, 'refused') AS ok" },
        { query: "CREATE (:T {id: 3})" },
      ])
      .then(
        () => undefined,
        (e: unknown) => e as Error,
      );
    expect(err?.message).toContain("assertion failed: refused");
    expect(err?.message).toContain("(statement 2 of 3)");
    const { rows } = await db.execute("MATCH (t:T) RETURN count(t) AS c");
    expect(rows[0].c).toBe(1);
  });

  it("gives up waiting for the lock at timeoutMs", async () => {
    const db = await createDatabase();
    const holder = await db.begin();
    const err = await db
      .transaction([{ query: "CREATE (:T)" }], "read_write", { timeoutMs: 50 })
      .then(
        () => undefined,
        (e: unknown) => e as { code?: string },
      );
    expect(err?.code).toBe("LORA_TIMEOUT");
    await holder.commit();
    const { rows } = await db.execute("MATCH (t:T) RETURN count(t) AS c");
    expect(rows[0].c).toBe(0);
  });
});
