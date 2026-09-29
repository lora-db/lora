import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, describe, expect, it } from "vitest";
import {
  createDatabase,
  LoraError,
  openWalDatabase,
  type ChangeFeed,
  type LoraChange,
  type LoraChangeBatch,
} from "../ts/index.js";

const dirs: string[] = [];
async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "lora-node-changes-"));
  dirs.push(dir);
  return dir;
}
afterAll(async () => {
  await Promise.all(dirs.map((d) => rm(d, { recursive: true, force: true })));
});

async function take(feed: ChangeFeed, n: number): Promise<LoraChangeBatch[]> {
  const out: LoraChangeBatch[] = [];
  while (out.length < n) {
    const next = await feed.next();
    if (next.done) throw new Error(`feed ended after ${out.length} batches`);
    out.push(next.value);
  }
  return out;
}

function kinds(batch: LoraChangeBatch): LoraChange["kind"][] {
  return batch.changes.map((c) => c.kind);
}

describe("db.changes()", () => {
  it("reports created, updated and deleted entities with typed shapes", async () => {
    const db = await createDatabase();
    const feed = db.changes();
    await feed.ready;

    await db.execute("CREATE (:User {id: 'u1', name: 'Ada'})");
    await db.execute(
      "MATCH (u:User {id: 'u1'}) SET u.name = 'Ada L', u:Admin REMOVE u.id",
    );
    await db.execute(
      "MATCH (u:User) CREATE (u)-[:WROTE {at: 1}]->(:Post {slug: 'p'})",
    );
    await db.execute("MATCH (u:User) DETACH DELETE u");

    const [created, updated, wrote, deleted] = await take(feed, 4);
    expect(created.changes).toEqual([
      {
        kind: "nodeCreated",
        id: expect.any(Number),
        labels: ["User"],
        properties: { id: "u1", name: "Ada" },
      },
    ]);
    const userId = (created.changes[0] as { id: number }).id;
    expect(updated.changes).toEqual([
      {
        kind: "nodeUpdated",
        id: userId,
        labels: ["User", "Admin"],
        properties: { name: "Ada L" },
        setKeys: ["name"],
        removedKeys: ["id"],
        addedLabels: ["Admin"],
        removedLabels: [],
      },
    ]);
    expect(kinds(wrote).sort()).toEqual(["nodeCreated", "relationshipCreated"]);
    const rel = wrote.changes.find((c) => c.kind === "relationshipCreated");
    expect(rel).toMatchObject({
      type: "WROTE",
      startId: userId,
      properties: { at: 1 },
    });
    expect(deleted.changes).toContainEqual({
      kind: "nodeDeleted",
      id: userId,
      labels: ["User", "Admin"],
      properties: { name: "Ada L" },
    });
    expect(deleted.changes.some((c) => c.kind === "relationshipDeleted")).toBe(
      true,
    );

    const lsns = [created, updated, wrote, deleted].map((b) => b.lsn);
    expect([...lsns].sort((a, b) => a - b)).toEqual(lsns);
    expect(new Set(lsns).size).toBe(4);
    expect(feed.lastLsn).toBe(deleted.lsn);
    feed.close();
    db.dispose();
  });

  it("captures transactions, executeMany, streams and clear, but not rollbacks", async () => {
    const db = await createDatabase();
    const feed = db.changes();
    await feed.ready;

    await db.transaction([{ query: "CREATE (:A)" }, { query: "CREATE (:B)" }]);
    const tx = await db.begin();
    await tx.executeMany([{ query: "CREATE (:C)" }, { query: "CREATE (:D)" }]);
    await tx.commit();
    const rolled = await db.begin();
    await rolled.execute("CREATE (:Nope)");
    await rolled.rollback();
    await expect(
      db.execute("CREATE (a:Nope)-[:R]->(:Nope) WITH a DELETE a"),
    ).rejects.toBeInstanceOf(LoraError);
    for await (const _ of db.stream("CREATE (n:Streamed) RETURN n")) {
      // drain
    }
    await db.clear();

    const batches = await take(feed, 4);
    expect(batches.map((b) => b.changes.length)).toEqual([2, 2, 1, 1]);
    expect(batches[2].changes[0]).toMatchObject({ labels: ["Streamed"] });
    expect(batches[3].changes).toEqual([{ kind: "reset" }]);
    const labels = batches
      .flatMap((b) => b.changes)
      .flatMap((c) => ("labels" in c ? c.labels : []));
    expect(labels).not.toContain("Nope");
    feed.close();
    db.dispose();
  });

  it("resumes from an LSN inside the in-memory window", async () => {
    const db = await createDatabase();
    const first = db.changes();
    await first.ready;
    await db.execute("CREATE (:N {i: 1})");
    await db.execute("CREATE (:N {i: 2})");
    await db.execute("CREATE (:N {i: 3})");
    const [a, b, c] = await take(first, 3);
    first.close();

    const resumed = db.changes({ fromLsn: a.lsn });
    const [rb, rc] = await take(resumed, 2);
    expect([rb.lsn, rc.lsn]).toEqual([b.lsn, c.lsn]);
    resumed.close();

    const big = db.changes({ fromLsn: BigInt(c.lsn) });
    await big.ready;
    await db.execute("CREATE (:N {i: 4})");
    const [d] = await take(big, 1);
    expect(d.lsn).toBeGreaterThan(c.lsn);
    big.close();

    const ahead = db.changes({ fromLsn: 1_000_000 });
    await expect(ahead.ready).rejects.toMatchObject({
      code: "LORA_CHANGES_TRUNCATED",
    });
    await expect(ahead.next()).rejects.toMatchObject({
      code: "LORA_CHANGES_TRUNCATED",
    });
    db.dispose();
  });

  it("ends a slow consumer with LORA_CHANGES_LAGGED without blocking writers", async () => {
    const db = await createDatabase();
    const feed = db.changes({ bufferSize: 2 });
    await feed.ready;
    for (let i = 0; i < 10; i++) {
      await db.execute("CREATE (:N {i: $i})", { i });
    }
    const got: LoraChangeBatch[] = [];
    let error: unknown;
    try {
      for await (const batch of feed) got.push(batch);
    } catch (err) {
      error = err;
    }
    expect(got).toHaveLength(2);
    expect(error).toBeInstanceOf(LoraError);
    expect((error as LoraError).code).toBe("LORA_CHANGES_LAGGED");

    const resumed = db.changes({ fromLsn: feed.lastLsn });
    const rest = await take(resumed, 8);
    expect(rest[0].lsn).toBeGreaterThan(got[1].lsn);
    resumed.close();
    db.dispose();
  });

  it("stops on AbortSignal, return() and dispose()", async () => {
    const db = await createDatabase();

    const controller = new AbortController();
    const aborted = db.changes({ signal: controller.signal });
    await aborted.ready;
    const pending = aborted.next();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });

    const broken = db.changes();
    await broken.ready;
    await db.execute("CREATE (:X)");
    for await (const batch of broken) {
      expect(batch.changes).toHaveLength(1);
      break;
    }
    expect(await broken.next()).toEqual({ done: true, value: undefined });

    const waiting = db.changes();
    await waiting.ready;
    const next = waiting.next();
    db.dispose();
    expect(await next).toEqual({ done: true, value: undefined });
  });

  it("resumes from the WAL across a reopen", async () => {
    const walDir = await tempDir();
    let before: number;
    {
      const db = await openWalDatabase({ walDir });
      const feed = db.changes();
      await feed.ready;
      await db.execute("CREATE (:Doc {slug: 'a'}), (:Doc {slug: 'b'})");
      [{ lsn: before }] = await take(feed, 1);
      await db.execute("MATCH (d:Doc {slug: 'a'}) SET d.title = 'A'");
      db.dispose();
    }

    const db = await openWalDatabase({ walDir });
    await db.execute("MATCH (d:Doc {slug: 'b'}) DETACH DELETE d");
    const feed = db.changes({ fromLsn: before });
    const [update, removal] = await take(feed, 2);
    expect(update.lsn).toBeGreaterThan(before);
    expect(update.changes).toEqual([
      expect.objectContaining({
        kind: "nodeUpdated",
        labels: ["Doc"],
        setKeys: ["title"],
        properties: { slug: "a", title: "A" },
      }),
    ]);
    expect(removal.changes).toEqual([
      expect.objectContaining({
        kind: "nodeDeleted",
        properties: { slug: "b" },
      }),
    ]);
    feed.close();
    db.dispose();
  });
});
